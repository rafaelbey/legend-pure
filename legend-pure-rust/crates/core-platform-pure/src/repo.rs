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
use std::sync::Arc;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{self, PartialPureModel, init_bootstrap_model};
use legend_pure_parser_pure::purem::{ReadError, header::read_header, merge_slice, read_repo};
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

/// Composes the standard inline-island parsers shipped by legend-pure-rust.
///
/// Today's set covers `#{ … }#` (graph fetch from `dsl-graph`),
/// `#>{ … }#` (relation-store accessor from `dsl-store`), and
/// `#TDS\n cols\n rows\n#` (tabular fixtures from `dsl-tds`). All three
/// are needed so a parser invocation through [`load`] can handle any
/// `.pure` source that uses these grammars — the platform / engine repos
/// rely on TDS islands extensively in their relational tests.
///
/// New DSL crates that ship their own `default_island_parsers()` should
/// be chained here so [`load`] sees them automatically.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn legend_pure_parser_parser::IslandParser>> {
    let mut parsers = legend_pure_dsl_graph::parser::default_island_parsers();
    parsers.extend(legend_pure_dsl_store::parser::default_island_parsers());
    parsers.extend(legend_pure_dsl_tds::parser::default_island_parsers());
    parsers
}

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
#[derive(Debug, Clone)]
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
    /// Pre-compiled `.purem` snapshot. The blob is the output of
    /// [`legend_pure_parser_pure::purem::write_repo`]; loading is a
    /// `read_repo` + `merge_slice` pair that skips parse + compile
    /// entirely.
    ///
    /// Constructed by [`Repo::from_purem_bytes`] /
    /// [`Repo::from_purem_file`]. `meta` is required because the
    /// dependency-driven topo sort needs it.
    Purem {
        /// Canonical URL prefix, of the form `/{name}`.
        prefix: String,
        /// Descriptor metadata.
        meta: RepoMeta,
        /// The serialized [`PureModelSlice`] blob, including its 22-byte
        /// header. `Arc<[u8]>` so multiple consumers (e.g. JNI + CLI)
        /// can share without cloning.
        blob: Arc<[u8]>,
        /// Embedded `.json` manifests `(canonical_url, content)` pairs
        /// surfaced via [`Repo::manifests`]. Populated by the
        /// build-script-generated `purem-embedded` shape so
        /// [`find_manifest`] can find platform-side data files
        /// (`pct_essential_native.json` etc.) without dragging .pure
        /// sources into the binary. Empty for runtime-built `Purem`
        /// repos (`from_purem_bytes` / `from_purem_file`) since they
        /// load from external `.purem` blobs that don't carry
        /// manifests.
        manifests: &'static [(&'static str, &'static str)],
    },
}

impl Repo {
    /// The default embedded platform repo, materialised from the
    /// build-script-generated `.purem` blob.
    ///
    /// Equivalent to the first entry of [`Self::default_embedded`].
    /// Kept as a convenience for callers that want only the platform
    /// without iterating the default vec.
    #[must_use]
    pub fn embedded_platform() -> Self {
        Self::from_purem_static_with_manifests(
            "/platform",
            &sources::REPO_PLATFORM_META,
            sources::REPO_PLATFORM_PUREM,
            sources::REPO_PLATFORM_MANIFESTS,
        )
    }

    /// All embedded repos in Cargo.toml declaration order.
    ///
    /// After the binary trim, this returns just the `platform` purem
    /// blob — every other repo (precise primitives, all DSLs,
    /// store-relational) ships as a `.purem` artifact next to the
    /// binary and is loaded via classpath. See
    /// `crates/core-platform-pure/Cargo.toml`'s `[[package.metadata.legend-pure.repos]]`
    /// table for the full repo set.
    #[must_use]
    pub fn default_embedded() -> Vec<Repo> {
        sources::default_embedded_repos()
    }

    /// Test-time helper: [`Self::default_embedded`] plus every `.purem`
    /// artifact emitted by the build script
    /// (`crates/build`'s `purem-artifact` shape). Returns the embedded
    /// `platform` plus all DSL artifacts with their real dependency
    /// metadata so tests can run against a full platform without
    /// manually configuring a classpath.
    ///
    /// The snapshots-dir path is captured at compile time via
    /// `option_env!("LEGEND_PURE_BUILD_SNAPSHOTS_DIR")` (set by the
    /// build script's `cargo:rustc-env=` directive). At runtime,
    /// `LEGEND_PURE_BUILD_SNAPSHOTS_DIR` overrides it if set —
    /// useful for cargo-installed binaries that ship snapshots in a
    /// different location.
    ///
    /// Silently returns just the embedded set if neither path resolves
    /// to an existing directory.
    #[must_use]
    pub fn default_with_build_snapshots() -> Vec<Repo> {
        let mut repos = Self::default_embedded();
        // Runtime override wins.
        let dir_str: Option<String> = std::env::var("LEGEND_PURE_BUILD_SNAPSHOTS_DIR")
            .ok()
            .or_else(|| option_env!("LEGEND_PURE_BUILD_SNAPSHOTS_DIR").map(String::from));
        let Some(dir_str) = dir_str else {
            return repos;
        };
        let dir = std::path::PathBuf::from(dir_str);
        if !dir.is_dir() {
            return repos;
        }
        repos.extend(sources::default_artifact_repos(&dir));
        repos
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

    /// Construct a `Purem` repo from an in-memory `.purem` blob.
    ///
    /// Accepts the bytes produced by
    /// [`legend_pure_parser_pure::purem::write_repo`] (including the
    /// 22-byte magic header). The blob is stored as-is and read on
    /// demand by the merge-driven loader.
    #[must_use]
    pub fn from_purem_bytes(prefix: impl Into<String>, meta: RepoMeta, blob: Arc<[u8]>) -> Self {
        Self::Purem {
            prefix: prefix.into(),
            meta,
            blob,
            manifests: &[],
        }
    }

    /// Construct a `Purem` repo from a `&'static [u8]` blob — typically
    /// produced by `include_bytes!` from a build-script-generated
    /// `.purem` file. Used by `legend-pure-build`'s `purem-embedded`
    /// shape to generate `default_embedded_repos()` entries that wrap
    /// build-time-compiled blobs.
    ///
    /// The static slice is copied once into an `Arc<[u8]>` at
    /// construction; for production-scale repos this is a few-MB
    /// allocation paid once per process. A future `Cow`-based variant
    /// could eliminate the copy.
    #[must_use]
    pub fn from_purem_static(
        prefix: &'static str,
        meta: &'static RepoMeta,
        blob: &'static [u8],
    ) -> Self {
        Self::from_purem_static_with_manifests(prefix, meta, blob, &[])
    }

    /// Like [`Self::from_purem_static`] but also takes a static set of
    /// embedded `(canonical_url, content)` manifest pairs that
    /// [`Repo::manifests`] (and thus [`find_manifest`]) will surface.
    ///
    /// The build script's `purem-embedded` shape uses this to ship
    /// `.json` data files (PCT manifests, etc.) alongside the compiled
    /// `.purem` blob without dragging the `.pure` sources into the
    /// binary.
    #[must_use]
    pub fn from_purem_static_with_manifests(
        prefix: &'static str,
        meta: &'static RepoMeta,
        blob: &'static [u8],
        manifests: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self::Purem {
            prefix: prefix.to_string(),
            meta: *meta,
            blob: Arc::from(blob.to_vec().into_boxed_slice()),
            manifests,
        }
    }

    /// Construct a `Purem` repo by reading bytes from a file on disk.
    ///
    /// Validates the header eagerly (so a corrupt/wrong-version blob
    /// fails at construction, not at first use) but defers payload
    /// parsing to merge time.
    ///
    /// # Errors
    ///
    /// Returns [`RepoError`] for I/O failures or header validation
    /// failures (bad magic / wrong format_version / wrong schema_hash).
    pub fn from_purem_file(
        path: &Path,
        prefix: impl Into<String>,
        meta: RepoMeta,
    ) -> Result<Self, RepoError> {
        let bytes = std::fs::read(path).map_err(|source| RepoError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        // Header-only validation: catches magic / format_version /
        // schema_hash mismatches at construction without paying the
        // payload-deserialization cost. Full parse is deferred to
        // merge time.
        read_header(&bytes).map_err(|e| RepoError::PuremRead {
            path: path.to_path_buf(),
            source: ReadError::Header(e),
        })?;
        Ok(Self::Purem {
            prefix: prefix.into(),
            meta,
            blob: Arc::from(bytes.into_boxed_slice()),
            manifests: &[],
        })
    }

    /// Canonical URL prefix (e.g. `/platform`).
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::Embedded { prefix, .. } => prefix,
            Self::Filesystem { prefix, .. } => prefix.as_str(),
            Self::Purem { prefix, .. } => prefix.as_str(),
        }
    }

    /// Descriptor metadata if available — always `Some` for embedded,
    /// `Purem`, and filesystem repos built via [`Repo::from_descriptor`].
    #[must_use]
    pub fn meta(&self) -> Option<&RepoMeta> {
        match self {
            Self::Embedded { meta, .. } => Some(*meta),
            Self::Filesystem { meta, .. } => meta.as_ref(),
            Self::Purem { meta, .. } => Some(meta),
        }
    }

    /// `.purem` blob bytes for a [`Repo::Purem`] variant; `None` for
    /// source-based repos.
    #[must_use]
    pub fn purem_blob(&self) -> Option<&Arc<[u8]>> {
        match self {
            Self::Purem { blob, .. } => Some(blob),
            Self::Embedded { .. } | Self::Filesystem { .. } => None,
        }
    }

    /// All files in the repo (mixed `.pure` + `.json`), each as
    /// `(content, canonical_url)`. For [`Repo::Purem`] this surfaces
    /// only the embedded `manifests` — the compiled `.purem` blob is
    /// not iterated as source.
    pub fn files(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        match self {
            Self::Embedded { files, .. } => Box::new(files.iter().map(|f| (f.content, f.path))),
            Self::Filesystem { files, .. } => {
                Box::new(files.iter().map(|f| (f.content.as_str(), f.path.as_str())))
            }
            Self::Purem { manifests, .. } => {
                Box::new(manifests.iter().map(|(path, content)| (*content, *path)))
            }
        }
    }

    /// `.pure` files only — feeds [`crate::platform::parse_and_compile`].
    /// Empty for [`Repo::Purem`].
    pub fn sources(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        Box::new(self.files().filter(|(_, path)| path.ends_with(".pure")))
    }

    /// `.json` manifests only. For `Repo::Purem` this surfaces the
    /// static `manifests` field populated by the build script.
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
    /// `.purem` blob failed header validation.
    #[error("invalid .purem at {path}: {source}")]
    PuremRead {
        /// File path that failed.
        path: std::path::PathBuf,
        /// Underlying read error (header / payload / version mismatch).
        #[source]
        source: ReadError,
    },
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

/// Parse, compile, and merge every repo into a single [`PureModel`].
///
/// Composition contract:
/// 1. Topologically sort repos by their declared dependencies (see
///    [`crate::topo::topo_sort_repos`]).
/// 2. Bootstrap a fresh model.
/// 3. For each repo in dep order:
///    - [`Repo::Embedded`] / [`Repo::Filesystem`]: parse + compile this
///      repo's sources via [`pipeline::compile_repo_slice`]. Cross-repo
///      references resolve naturally against earlier repos.
///    - [`Repo::Purem`]: deserialize the blob and merge into the running
///      model — no parse, no compile.
/// 4. Finalize: rebuild derived indexes, run inference + validation.
///
/// Each repo lands in its own chunk(s), partitioning the model in a way
/// that downstream slice serialization (Phase A) can recover.
///
/// # Errors
///
/// Returns [`PartialPureModel`] if any parse / compile / topo / merge
/// errors occur. The partial model still contains all successfully
/// resolved elements.
#[allow(clippy::result_large_err)]
pub fn load(repos: &[Repo], auto_imports: &[SmolStr]) -> Result<PureModel, PartialPureModel> {
    let sorted = match crate::topo::topo_sort_repos(repos) {
        Ok(s) => s,
        Err(topo_err) => {
            // Surface the topo failure as a single PartialPureModel error
            // and bail before doing any compile work.
            let model = init_bootstrap_model();
            return Err(PartialPureModel {
                model,
                errors: vec![mk_synthetic_error("<repo-topo>", topo_err.to_string())],
            });
        }
    };

    let mut model = init_bootstrap_model();
    populate_repo_visibility(&mut model, repos);
    let mut errors: Vec<CompilationError> = Vec::new();

    for repo in sorted {
        match repo {
            Repo::Embedded { .. } | Repo::Filesystem { .. } => {
                let (parsed_files, parse_errs) = parse_repo_sources(repo);
                errors.extend(parse_errs);
                let (_range, slice_errs) =
                    pipeline::compile_repo_slice(&mut model, &parsed_files, auto_imports, &[]);
                errors.extend(slice_errs);
            }
            Repo::Purem { blob, meta, .. } => match read_repo(blob) {
                Ok(slice) => {
                    if let Err(e) = merge_slice(&mut model, slice) {
                        errors.push(mk_synthetic_error(
                            &format!("<purem:{}>", meta.name),
                            format!("merge failed: {e}"),
                        ));
                    }
                }
                Err(e) => {
                    errors.push(mk_synthetic_error(
                        &format!("<purem:{}>", meta.name),
                        format!("deserialize failed: {e}"),
                    ));
                }
            },
        }
    }

    errors.extend(pipeline::finalize_model(&mut model, auto_imports, &[]));

    if errors.is_empty() {
        Ok(model)
    } else {
        Err(PartialPureModel { model, errors })
    }
}

/// Parse one repo's `.pure` sources into [`SourceFile`]s.
///
/// Translates parse errors into [`CompilationError`]s so the merged
/// `load` accumulator handles every error class uniformly.
fn parse_repo_sources(
    repo: &Repo,
) -> (
    Vec<legend_pure_parser_ast::section::SourceFile>,
    Vec<CompilationError>,
) {
    let mut parsed_files = Vec::new();
    let mut errors = Vec::new();
    for (content, name) in repo.sources() {
        match legend_pure_parser_parser::parse_with_islands(content, name, default_island_parsers())
        {
            Ok(sf) => parsed_files.push(sf),
            Err(partial) => {
                parsed_files.push(partial.source_file);
                for e in partial.errors {
                    let source_info = e
                        .source_info()
                        .cloned()
                        .unwrap_or_else(|| SourceInfo::new(name, 0, 0, 0, 0));
                    errors.push(CompilationError {
                        message: e.message(),
                        source_info,
                        kind: CompilationErrorKind::ParseFailure {
                            source: SmolStr::new(name),
                        },
                    });
                }
            }
        }
    }
    (parsed_files, errors)
}

/// Populate `model.repo_visibility` from the repo list's descriptor
/// metadata. Each repo with a [`RepoMeta`] contributes one entry mapping
/// its name to `{ self } ∪ { each direct dep }`. Repos without metadata
/// (test fixtures via [`Repo::from_filesystem`]) are skipped — sources
/// in those repos won't have any rules to enforce, matching Java's
/// `getSourceRepoName` returning null for unrecognised repos.
fn populate_repo_visibility(model: &mut PureModel, repos: &[Repo]) {
    for repo in repos {
        let Some(meta) = repo.meta() else {
            continue;
        };
        let mut visible: std::collections::BTreeSet<SmolStr> = std::collections::BTreeSet::new();
        visible.insert(SmolStr::new(meta.name));
        for dep in meta.dependencies {
            visible.insert(SmolStr::new(*dep));
        }
        model
            .repo_visibility
            .insert(SmolStr::new(meta.name), visible);
    }
}

fn mk_synthetic_error(source: &str, message: String) -> CompilationError {
    CompilationError {
        message,
        source_info: SourceInfo::new(source, 0, 0, 0, 0),
        kind: CompilationErrorKind::ParseFailure {
            source: SmolStr::new(source),
        },
    }
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
    fn embedded_platform_carries_manifests() {
        // Phase 3b: source files are no longer embedded; only the
        // .purem blob + JSON manifests remain. Verify the manifest
        // surface has at least the PCT exclusion lists the runtime
        // depends on.
        let repo = Repo::embedded_platform();
        let manifest_paths: Vec<_> = repo.manifests().map(|(_, path)| path).collect();
        assert!(
            manifest_paths
                .iter()
                .any(|p| p.ends_with("/pct_essential_native.json")),
            "expected pct_essential_native.json among embedded manifests; got {manifest_paths:?}"
        );
    }

    #[test]
    fn default_embedded_returns_only_platform() {
        // Phase 3b: every non-platform repo ships as a .purem artifact
        // next to the binary, not in the binary itself. The only repo
        // baked into the binary is `platform`.
        let repos = Repo::default_embedded();
        let names: Vec<_> = repos
            .iter()
            .filter_map(|r| r.meta().map(|m| m.name))
            .collect();
        assert_eq!(names, vec!["platform"], "got names: {names:?}");
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
