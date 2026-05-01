// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Polymorphic Pure-repository abstraction.
//!
//! A [`Repo`] is a self-describing collection of `.pure` source files and
//! `.json` manifests, all sharing a canonical-URL prefix (e.g.
//! `/platform`, `/platform_dsl_store`). Each repo carries a [`RepoMeta`]
//! deserialized from the existing Java descriptor JSON
//! (`platform.json`, `*.definition.json`).
//!
//! Three shapes are supported:
//!
//! - **Embedded** — files baked into the binary at build time via
//!   `include_str!`. Driven by `[[package.metadata.legend-pure.repos]]`
//!   in this crate's `Cargo.toml`.
//! - **Filesystem** — files read from disk at runtime. Use
//!   [`Repo::from_descriptor`] (preferred — pulls metadata from the Java
//!   JSON) or [`Repo::from_filesystem`] (escape hatch for ad-hoc test
//!   fixtures with no descriptor).
//! - **`.purem` snapshot** — Stage 2; not yet implemented. Variant slot
//!   reserved by the enum's exhaustiveness contract; see
//!   `docs/PUREM_FORMAT.md`.

use std::path::Path;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::PartialPureModel;
use serde::Deserialize;
use smol_str::SmolStr;
use thiserror::Error;
use walkdir::WalkDir;

use crate::sources;

/// Bootstrap M3 metamodel file. Parsed by the special-purpose
/// `m3_parser` (see `crates/pure/src/bootstrap.rs`), never by the
/// regular pipeline. Skip this canonical URL during embedding and
/// during runtime filesystem walks.
pub const M3_BOOTSTRAP_CANONICAL: &str = "/platform/pure/grammar/m3.pure";

/// Descriptor metadata read from the existing Java repo JSON
/// (`platform.json`, `*.definition.json`).
///
/// The Java schema is `{name, pattern, dependencies}` and is the single
/// source of truth for these fields — no Rust-specific descriptor format
/// is invented.
///
/// Stored with `&'static str` slices so build-time-emitted constants and
/// runtime-leaked metadata share the same shape. Runtime constructors
/// (`Repo::from_descriptor`) leak the parsed `String` fields once at
/// startup to obtain `&'static str` — acceptable since repo metadata
/// lives for the life of the process.
#[derive(Debug, Clone, Copy)]
pub struct RepoMeta {
    /// Repo name (e.g. `"platform"`, `"platform_dsl_store"`). Identifier
    /// used by `dependencies` references and as the canonical URL prefix
    /// segment.
    pub name: &'static str,
    /// Regex (Java syntax) over qualified package paths the repo is
    /// allowed to define. Read but unused in Stage 1 — Stage 2 wires it
    /// up as a per-repo validator.
    pub pattern: &'static str,
    /// Repo names this repo depends on. Read but unused in Stage 1 —
    /// Stage 2 uses it for topological repo-load ordering.
    pub dependencies: &'static [&'static str],
}

/// A `.pure` or `.json` file loaded from disk at runtime.
#[derive(Debug, Clone)]
pub struct OwnedSourceFile {
    /// Canonical URL of the form `/{repo_name}/{rel_path}`.
    pub path: String,
    /// File contents.
    pub content: String,
}

/// A Pure repository — a set of `.pure` sources and `.json` manifests
/// sharing a canonical-URL prefix.
#[non_exhaustive]
pub enum Repo {
    /// Files baked into the binary at build time.
    Embedded {
        /// Canonical URL prefix, of the form `/{name}`.
        prefix: &'static str,
        /// Mixed `.pure` + `.json` files, each with a canonical URL
        /// path. Use [`Repo::sources`] / [`Repo::manifests`] to filter
        /// by extension.
        files: &'static [sources::PureSourceFile],
        /// Descriptor metadata from the Java JSON.
        meta: &'static RepoMeta,
    },
    /// Files read from disk at runtime.
    Filesystem {
        /// Canonical URL prefix, of the form `/{name}`.
        prefix: String,
        /// Mixed `.pure` + `.json` files, each with a canonical URL
        /// path. Eagerly slurped on construction.
        files: Vec<OwnedSourceFile>,
        /// Descriptor metadata, populated when the repo was built from
        /// a descriptor JSON; `None` when built via [`Repo::from_filesystem`].
        meta: Option<RepoMeta>,
    },
}

impl Repo {
    /// The default embedded platform repo (from build.rs).
    #[must_use]
    pub fn embedded_platform() -> Self {
        Self::Embedded {
            prefix: "/platform",
            files: sources::REPO_PLATFORM_FILES,
            meta: &sources::REPO_PLATFORM_META,
        }
    }

    /// The default embedded `platform_dsl_store` repo.
    #[must_use]
    pub fn embedded_platform_dsl_store() -> Self {
        Self::Embedded {
            prefix: "/platform_dsl_store",
            files: sources::REPO_PLATFORM_DSL_STORE_FILES,
            meta: &sources::REPO_PLATFORM_DSL_STORE_META,
        }
    }

    /// The default embedded `platform_dsl_diagram` repo.
    #[must_use]
    pub fn embedded_platform_dsl_diagram() -> Self {
        Self::Embedded {
            prefix: "/platform_dsl_diagram",
            files: sources::REPO_PLATFORM_DSL_DIAGRAM_FILES,
            meta: &sources::REPO_PLATFORM_DSL_DIAGRAM_META,
        }
    }

    /// The default embedded `platform_dsl_tds` repo.
    #[must_use]
    pub fn embedded_platform_dsl_tds() -> Self {
        Self::Embedded {
            prefix: "/platform_dsl_tds",
            files: sources::REPO_PLATFORM_DSL_TDS_FILES,
            meta: &sources::REPO_PLATFORM_DSL_TDS_META,
        }
    }

    /// All embedded repos in Cargo.toml declaration order.
    #[must_use]
    pub fn default_embedded() -> Vec<Repo> {
        sources::default_embedded_repos()
    }

    /// Build a Filesystem repo from an existing Java descriptor JSON
    /// (e.g. `platform.json`, `*.definition.json`). Reads `name` from
    /// the JSON; `source_root` is `<dir_of_descriptor>/<name>/`;
    /// canonical URL prefix is `/<name>`.
    ///
    /// # Errors
    ///
    /// Returns [`RepoError`] if the descriptor or source root is
    /// missing, the JSON is malformed, or the walk fails.
    pub fn from_descriptor(descriptor_path: &Path) -> Result<Self, RepoError> {
        let descriptor_str =
            std::fs::read_to_string(descriptor_path).map_err(|source| RepoError::Io {
                path: descriptor_path.to_path_buf(),
                source,
            })?;
        let parsed: DescriptorJson = serde_json::from_str(&descriptor_str)
            .map_err(|e| RepoError::DescriptorParse(e.to_string()))?;

        let parent = descriptor_path
            .parent()
            .ok_or_else(|| RepoError::DescriptorParse("descriptor has no parent dir".into()))?;
        let source_root = parent.join(&parsed.name);
        if !source_root.is_dir() {
            return Err(RepoError::SourceRootMissing(source_root));
        }

        let prefix = format!("/{}", parsed.name);
        let files = walk_files(&source_root, &prefix, &parsed.name)?;
        let meta = RepoMeta::from_owned(parsed);

        Ok(Self::Filesystem {
            prefix,
            files,
            meta: Some(meta),
        })
    }

    /// Walk `root` for `.pure` and `.json` files and stamp `prefix` onto
    /// every canonical URL. Skips [`M3_BOOTSTRAP_CANONICAL`].
    ///
    /// Use this only for ad-hoc test fixtures that have no descriptor
    /// JSON — production code should prefer [`Repo::from_descriptor`],
    /// which carries [`RepoMeta`] forward.
    ///
    /// # Errors
    ///
    /// Returns [`RepoError`] if `root` is missing or the walk fails.
    pub fn from_filesystem(root: &Path, prefix: impl Into<String>) -> Result<Self, RepoError> {
        let prefix = prefix.into();
        if !root.is_dir() {
            return Err(RepoError::SourceRootMissing(root.to_path_buf()));
        }
        // The bare `from_filesystem` constructor doesn't know which repo
        // name is canonical for `M3_BOOTSTRAP_CANONICAL`, so the skip
        // rule keys off the canonical URL only — works regardless of
        // chosen prefix.
        let files = walk_files_with_skip(root, &prefix, |canonical| {
            canonical == M3_BOOTSTRAP_CANONICAL
        })?;
        Ok(Self::Filesystem {
            prefix,
            files,
            meta: None,
        })
    }

    /// Canonical URL prefix (e.g. `/platform`).
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::Embedded { prefix, .. } => prefix,
            Self::Filesystem { prefix, .. } => prefix.as_str(),
        }
    }

    /// Descriptor metadata if available — always `Some` for embedded
    /// repos and for filesystem repos built via [`Repo::from_descriptor`].
    #[must_use]
    pub fn meta(&self) -> Option<&RepoMeta> {
        match self {
            Self::Embedded { meta, .. } => Some(*meta),
            Self::Filesystem { meta, .. } => meta.as_ref(),
        }
    }

    /// All files in the repo (mixed `.pure` + `.json`), each as
    /// `(content, canonical_url)`.
    pub fn files(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        match self {
            Self::Embedded { files, .. } => Box::new(files.iter().map(|f| (f.content, f.path))),
            Self::Filesystem { files, .. } => {
                Box::new(files.iter().map(|f| (f.content.as_str(), f.path.as_str())))
            }
        }
    }

    /// `.pure` files only — feeds [`crate::platform::parse_and_compile`].
    pub fn sources(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        Box::new(self.files().filter(|(_, path)| path.ends_with(".pure")))
    }

    /// `.json` manifests only.
    pub fn manifests(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        Box::new(self.files().filter(|(_, path)| path.ends_with(".json")))
    }
}

#[derive(Deserialize)]
struct DescriptorJson {
    name: String,
    pattern: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

impl RepoMeta {
    /// Convert an owned `(name, pattern, dependencies)` triple into a
    /// `RepoMeta` whose `&'static str` fields point at leaked strings.
    /// Called once per runtime-loaded repo at startup.
    fn from_owned(d: DescriptorJson) -> Self {
        let name: &'static str = Box::leak(d.name.into_boxed_str());
        let pattern: &'static str = Box::leak(d.pattern.into_boxed_str());
        let deps: Vec<&'static str> = d
            .dependencies
            .into_iter()
            .map(|s| Box::leak(s.into_boxed_str()) as &'static str)
            .collect();
        let dependencies: &'static [&'static str] = Box::leak(deps.into_boxed_slice());
        Self {
            name,
            pattern,
            dependencies,
        }
    }
}

/// Errors raised when constructing a [`Repo`] from disk.
#[derive(Debug, Error)]
pub enum RepoError {
    /// The expected source-root directory does not exist.
    #[error("repo source root missing: {0}")]
    SourceRootMissing(std::path::PathBuf),
    /// I/O failure reading a file.
    #[error("io error reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Descriptor JSON failed to parse.
    #[error("invalid descriptor JSON: {0}")]
    DescriptorParse(String),
    /// `walkdir` traversal failure.
    #[error("walk error: {0}")]
    Walk(String),
    /// Encountered a file path that isn't valid UTF-8.
    #[error("non-UTF-8 path: {0}")]
    NonUtf8Path(std::path::PathBuf),
}

/// Walk `root`, skipping `<repo_name>/pure/grammar/m3.pure` (the
/// bootstrap M3 file) when `repo_name == "platform"`.
fn walk_files(
    root: &Path,
    prefix: &str,
    repo_name: &str,
) -> Result<Vec<OwnedSourceFile>, RepoError> {
    let m3_skip = repo_name == "platform";
    walk_files_with_skip(root, prefix, |canonical| {
        m3_skip && canonical == M3_BOOTSTRAP_CANONICAL
    })
}

fn walk_files_with_skip(
    root: &Path,
    prefix: &str,
    mut skip: impl FnMut(&str) -> bool,
) -> Result<Vec<OwnedSourceFile>, RepoError> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(|e| RepoError::Walk(e.to_string()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "pure" && ext != "json" {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .map_err(|e| RepoError::Walk(e.to_string()))?;
        let rel_str = rel
            .to_str()
            .ok_or_else(|| RepoError::NonUtf8Path(rel.to_path_buf()))?
            .replace('\\', "/");
        let canonical = format!("{prefix}/{rel_str}");

        if skip(&canonical) {
            continue;
        }

        let content = std::fs::read_to_string(path).map_err(|source| RepoError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        out.push(OwnedSourceFile {
            path: canonical,
            content,
        });
    }
    Ok(out)
}

/// Parse and compile every repo's `.pure` sources into a single
/// [`PureModel`].
///
/// Repos contribute their `.pure` files in declaration order; manifests
/// are not parsed and are accessed separately via [`find_manifest`].
///
/// Delegates to [`crate::platform::parse_and_compile`].
///
/// # Errors
///
/// Returns [`PartialPureModel`] if any parse or compilation errors
/// occur. The partial model still contains all successfully resolved
/// elements.
#[allow(clippy::result_large_err)]
pub fn load(repos: &[Repo], auto_imports: &[SmolStr]) -> Result<PureModel, PartialPureModel> {
    let pairs: Vec<(&str, &str)> = repos.iter().flat_map(|r| r.sources()).collect();
    crate::platform::parse_and_compile(pairs.into_iter(), auto_imports)
}

/// Suffix-match a manifest path across every repo's `.json` files.
/// Returns the first matching file's contents, or `None` if no match.
///
/// Match is anchored on a path-segment boundary — `"essential.json"`
/// does not match `"pct_essential_native.json"` because the character
/// preceding the match must be `/` (or the canonical URL must equal
/// `suffix` exactly).
#[must_use]
pub fn find_manifest<'a>(repos: &'a [Repo], suffix: &str) -> Option<&'a str> {
    for repo in repos {
        for (content, path) in repo.manifests() {
            if path == suffix {
                return Some(content);
            }
            if !path.ends_with(suffix) {
                continue;
            }
            let prefix_len = path.len() - suffix.len();
            let on_boundary = prefix_len == 0
                || path[..prefix_len]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c == '/' || c == '\\');
            if on_boundary {
                return Some(content);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_platform_prefix_is_slash_name() {
        let repo = Repo::embedded_platform();
        assert_eq!(repo.prefix(), "/platform");
    }

    #[test]
    fn embedded_platform_meta_from_java_json() {
        let repo = Repo::embedded_platform();
        let meta = repo.meta().expect("embedded repo carries meta");
        assert_eq!(meta.name, "platform");
        assert!(
            meta.pattern.contains("meta") && meta.pattern.contains("system"),
            "pattern should include the regex from platform.json, got: {}",
            meta.pattern
        );
        assert!(meta.dependencies.is_empty());
    }

    #[test]
    fn embedded_platform_dsl_store_dependencies() {
        let repo = Repo::embedded_platform_dsl_store();
        let meta = repo.meta().expect("embedded repo carries meta");
        assert_eq!(meta.name, "platform_dsl_store");
        assert_eq!(meta.dependencies, &["platform"]);
    }

    #[test]
    fn embedded_platform_files_carry_canonical_urls() {
        let repo = Repo::embedded_platform();
        let any_grammar = repo
            .files()
            .any(|(_, path)| path.starts_with("/platform/pure/"));
        assert!(
            any_grammar,
            "expected at least one /platform/pure/... file in embedded repo"
        );
    }

    #[test]
    fn embedded_skips_m3_pure() {
        let repo = Repo::embedded_platform();
        let has_m3 = repo.files().any(|(_, path)| path == M3_BOOTSTRAP_CANONICAL);
        assert!(
            !has_m3,
            "m3.pure must remain embedded only via bootstrap.rs"
        );
    }

    #[test]
    fn default_embedded_includes_all_four_repos() {
        let repos = Repo::default_embedded();
        let names: Vec<_> = repos
            .iter()
            .filter_map(|r| r.meta().map(|m| m.name))
            .collect();
        assert!(names.contains(&"platform"));
        assert!(names.contains(&"platform_dsl_store"));
        assert!(names.contains(&"platform_dsl_diagram"));
        assert!(names.contains(&"platform_dsl_tds"));
    }

    #[test]
    fn find_manifest_suffix_match_across_repos() {
        let repos = Repo::default_embedded();
        // The platform repo embeds `pct_essential_native.json` somewhere
        // under `/platform/pure/...`. A bare-suffix lookup should hit it.
        let hit = find_manifest(&repos, "pct_essential_native.json");
        assert!(hit.is_some(), "expected to find pct_essential_native.json");
    }

    #[test]
    fn find_manifest_requires_segment_boundary() {
        let repos = Repo::default_embedded();
        // "essential.json" should NOT match "pct_essential_native.json"
        // because the match must start at a path-segment boundary.
        // (No real manifest is named just "essential.json" — verifies
        // the boundary logic, not a positive hit.)
        let canonical_paths: Vec<_> = repos
            .iter()
            .flat_map(|r| r.manifests())
            .map(|(_, p)| p.to_string())
            .collect();
        for path in &canonical_paths {
            assert!(
                !path.ends_with("/essential.json"),
                "test invariant violated: an actual /essential.json exists; update the test"
            );
        }
        assert!(find_manifest(&repos, "essential.json").is_none());
    }

    #[test]
    fn from_filesystem_root_missing_errors() {
        match Repo::from_filesystem(Path::new("/no/such/dir"), "/x") {
            Err(RepoError::SourceRootMissing(_)) => {}
            other => panic!(
                "expected SourceRootMissing, got: {}",
                other
                    .err()
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_else(|| "Ok(_)".into())
            ),
        }
    }
}
