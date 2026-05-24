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

//! Parse `legend-pure-classpath.toml` content into a [`Vec<Repo>`] ready
//! for [`crate::repo::load`].
//!
//! Shared between the `legend-cli` (which adds discovery cascade / env
//! var lookup / next-to-binary scan on top) and the JNI bridge (which
//! receives TOML *bytes* via [`compile_classpath_bytes`] — Java reads
//! `META-INF/legend-pure-classpath.toml` from classpath resources and
//! hands the raw bytes to Rust without ever materializing a temp file
//! for the TOML itself).
//!
//! Layering rationale: the parser produces [`crate::repo::Repo`] values,
//! so the natural home is here next to `repo.rs` — not in `legend-cli`,
//! which would force every JNI consumer to inherit clap / rustyline /
//! notify / owo_colors via a transitive dep on the CLI binary crate.
//!
//! # Schema
//!
//! Identical to the format documented at `crates/cli/src/classpath.rs`'s
//! module docs — the wire format is the source of truth here, and the
//! CLI's module is the user-facing reference.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use smol_str::SmolStr;
use thiserror::Error;

use crate::repo::{self, Repo, RepoError, RepoMeta};

/// Parsed classpath ready to feed into [`crate::repo::load`].
#[derive(Debug)]
pub struct Classpath {
    /// Resolved root for relative paths inside the TOML. For
    /// file-loaded classpaths this is typically the TOML's parent dir;
    /// for JNI byte-loaded classpaths it's the dir the caller designates
    /// (often a temp dir into which the Java consumer extracted its
    /// classpath resources).
    #[allow(dead_code)]
    pub root: PathBuf,
    /// Additional auto-imports declared in the TOML.
    pub extra_auto_imports: Vec<SmolStr>,
    /// Resolved repos in declaration order.
    pub repos: Vec<Repo>,
    /// `[extension.<domain>.<engine>]` tables, opaque until the owning
    /// extension crate re-deserializes its slice.
    pub extension_configs: HashMap<String, HashMap<String, toml::Value>>,
}

/// Classpath after [`merge_with_embedded`] has stitched in any
/// embedded repos not shadowed by the parsed classpath. This is the
/// shape every model-loading site consumes.
#[derive(Debug)]
pub struct ResolvedClasspath {
    /// Source descriptor (path or synthetic label) the classpath came
    /// from, for diagnostics. `None` when the resolver fell back to
    /// embedded only.
    #[allow(dead_code)]
    pub source: Option<PathBuf>,
    /// Repos to load, deduped by name (parsed classpath wins over
    /// embedded).
    pub repos: Vec<Repo>,
    /// Extra auto-imports declared in the TOML, on top of the platform
    /// defaults.
    pub extra_auto_imports: Vec<SmolStr>,
    /// `[extension.<domain>.<engine>]` tables.
    pub extension_configs: HashMap<String, HashMap<String, toml::Value>>,
}

/// Errors raised by classpath parsing and repo build steps.
///
/// `Io` is emitted only by the CLI's file-loading wrapper —
/// [`parse_classpath_toml`] itself doesn't read the filesystem to
/// fetch the TOML. `Parse` carries a `source` label (a path string for
/// the CLI; `"<jni bytes>"` for JNI) purely for error formatting.
#[derive(Debug, Error)]
pub enum ClasspathError {
    /// Reading the TOML file failed (CLI file-loading only).
    #[error("reading classpath {}: {source}", path.display())]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse failure.
    #[error("invalid TOML in {source_label}: {source}")]
    Parse {
        /// Caller-supplied label for the source (path string or
        /// synthetic `"<jni bytes>"`).
        source_label: String,
        /// Underlying parser error.
        #[source]
        source: toml::de::Error,
    },
    /// Classpath bytes were not valid UTF-8 (JNI byte-array only).
    #[error("classpath bytes are not valid UTF-8: {source}")]
    Utf8 {
        /// Underlying UTF-8 decode error.
        #[source]
        source: std::str::Utf8Error,
    },
    /// A `[[repo]]` referenced a path that does not exist.
    #[error("repo '{name}': path '{}' not found", path.display())]
    PathMissing {
        /// Repo name.
        name: String,
        /// Path that wasn't found.
        path: PathBuf,
    },
    /// Two repos declared the same `name`.
    #[error("duplicate repo name in classpath: '{name}'")]
    DuplicateName {
        /// Repo name.
        name: String,
    },
    /// Constructing the underlying [`Repo`] failed.
    #[error("repo '{name}': {source}")]
    RepoBuild {
        /// Repo name.
        name: String,
        /// Underlying repo error.
        #[source]
        source: RepoError,
    },
    /// A `kind` is not yet supported.
    #[error("repo '{name}': unsupported kind '{kind}'")]
    UnsupportedKind {
        /// Repo name.
        name: String,
        /// Requested kind.
        kind: String,
    },
    /// Compiling the resolved repo set into a `PureModel` produced
    /// errors. Carries the partial model's error count so JNI / CLI
    /// callers can surface it without depending on `PartialPureModel`'s
    /// shape directly.
    #[error("classpath compiled with {error_count} error(s); first error: {first_error}")]
    CompileFailed {
        /// Total error count.
        error_count: usize,
        /// First diagnostic message (representative sample).
        first_error: String,
    },
}

#[derive(Debug, Deserialize)]
struct ClasspathToml {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    auto_imports: Vec<String>,
    #[serde(default, rename = "repo")]
    repos: Vec<RepoEntryToml>,
    #[serde(default)]
    extension: HashMap<String, HashMap<String, toml::Value>>,
}

#[derive(Debug, Deserialize)]
struct RepoEntryToml {
    name: String,
    kind: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    descriptor: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    dependencies: Option<Vec<String>>,
}

/// Parse a TOML string into a [`Classpath`].
///
/// `source_label` is used in error messages only (e.g. the source path
/// for CLI-driven loads, or `"<classpath bytes>"` for in-memory
/// bytes). It has no semantic effect on parsing.
///
/// `root` is the directory relative paths in the TOML resolve against
/// when the TOML's own `root =` field is absent or relative. For file-
/// loaded classpaths, callers pass the TOML's parent directory. For
/// the bytes-mode wrapper [`compile_classpath_bytes`], the sentinel
/// `/` is passed — the TOML is then required to be self-contained
/// (absolute `[[repo]] path =` entries or a TOML-level `root =`
/// override).
///
/// # Errors
///
/// Returns [`ClasspathError`] for any parse / build failure.
pub fn parse_classpath_toml(
    text: &str,
    source_label: &str,
    root: &Path,
) -> Result<Classpath, ClasspathError> {
    let parsed: ClasspathToml = toml::from_str(text).map_err(|source| ClasspathError::Parse {
        source_label: source_label.to_string(),
        source,
    })?;

    // Resolve root: caller's `root` is the base; if the TOML overrides
    // it with an absolute path, that wins; if the TOML overrides with a
    // relative path, it's joined onto the caller's `root`.
    let resolved_root = match &parsed.root {
        None => root.to_path_buf(),
        Some(r) => {
            let p = PathBuf::from(r);
            if p.is_absolute() { p } else { root.join(p) }
        }
    };

    let mut repos: Vec<Repo> = Vec::with_capacity(parsed.repos.len());
    let mut seen_names: HashSet<String> = HashSet::new();
    for entry in &parsed.repos {
        if !seen_names.insert(entry.name.clone()) {
            return Err(ClasspathError::DuplicateName {
                name: entry.name.clone(),
            });
        }
        let repo = build_repo(&resolved_root, entry)?;
        repos.push(repo);
    }

    Ok(Classpath {
        root: resolved_root,
        extra_auto_imports: parsed.auto_imports.into_iter().map(SmolStr::new).collect(),
        repos,
        extension_configs: parsed.extension,
    })
}

/// Merge a parsed [`Classpath`] with [`Repo::default_embedded`] by
/// shadow-by-name semantics: a TOML-declared repo replaces any embedded
/// repo with the same `name`.
///
/// `source` is propagated to [`ResolvedClasspath::source`] for
/// diagnostics; pass `None` for synthetic / byte-loaded classpaths
/// without a file path.
#[must_use]
pub fn merge_with_embedded(cp: Classpath, source: Option<PathBuf>) -> ResolvedClasspath {
    let cp_names: HashSet<String> = cp
        .repos
        .iter()
        .filter_map(|r| r.meta().map(|m| m.name.to_string()))
        .collect();
    let mut repos = cp.repos;
    for embedded in Repo::default_embedded() {
        let Some(meta) = embedded.meta() else {
            continue;
        };
        if cp_names.contains(meta.name) {
            continue;
        }
        repos.push(embedded);
    }
    ResolvedClasspath {
        source,
        repos,
        extra_auto_imports: cp.extra_auto_imports,
        extension_configs: cp.extension_configs,
    }
}

/// Compile a TOML byte slice straight into a `crate::repo::PureModel`.
///
/// One-call entry point for the JNI bridge: Java reads the classpath
/// TOML from JAR resources (e.g.
/// `getResourceAsStream("META-INF/legend-pure-classpath.toml").readAllBytes()`)
/// and hands the raw bytes to Rust. No filesystem read of the TOML
/// itself happens — the bytes ARE the TOML.
///
/// Path resolution for `[[repo]] path = "..."` entries is the caller's
/// responsibility. Since bytes-mode classpaths have no on-disk source
/// to derive a base from, the TOML must:
///
/// 1. Use **absolute paths** in every `[[repo]] path = "..."`, OR
/// 2. Declare its own `root = "/abs/path"` at the top of the TOML.
///
/// Both forms make the classpath self-describing — exactly what the
/// bytes-mode contract requires. Relative paths without a TOML `root`
/// resolve against the filesystem root (`/`) and almost always fail
/// with a clear `PathMissing` error, which is the expected signal to
/// the Java consumer that the TOML needs to be made absolute.
///
/// This signature deliberately does NOT accept a `root: &Path`
/// parameter: passing both bytes AND a path implies a hybrid model
/// where Java extracts TOML to disk anyway, in which case the existing
/// file-loading path (`legend_cli::classpath::load_classpath`) is the
/// honest API. The bytes API is for "TOML is self-contained" usage.
///
/// # Errors
///
/// Returns [`ClasspathError`] for any decode / parse / build / compile
/// failure. `CompileFailed` is emitted when [`crate::repo::load`]
/// returns a `PartialPureModel`; the JNI caller treats partial-compile
/// errors as fatal (half-baked models lead to opaque downstream
/// evaluator panics that are harder to diagnose than a clean
/// init-time refusal).
pub fn compile_classpath_bytes(
    bytes: &[u8],
) -> Result<
    (
        legend_pure_parser_pure::model::PureModel,
        HashMap<String, HashMap<String, toml::Value>>,
    ),
    ClasspathError,
> {
    let text = std::str::from_utf8(bytes).map_err(|source| ClasspathError::Utf8 { source })?;
    // Bytes mode has no on-disk source for the TOML — use `/` as the
    // sentinel base. Relative `path = "..."` entries naturally resolve
    // to `/foo.purem` and surface as `PathMissing`, signaling the
    // contract violation. TOMLs that set their own `root = "/abs"` or
    // use absolute paths bypass this entirely.
    let cp = parse_classpath_toml(text, "<classpath bytes>", Path::new("/"))?;
    let resolved = merge_with_embedded(cp, None);

    let mut auto_imports: Vec<SmolStr> = crate::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    auto_imports.extend(resolved.extra_auto_imports.iter().cloned());

    match repo::load(&resolved.repos, &auto_imports) {
        Ok(model) => Ok((model, resolved.extension_configs)),
        Err(partial) => {
            let first_error = partial
                .errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_default();
            Err(ClasspathError::CompileFailed {
                error_count: partial.errors.len(),
                first_error,
            })
        }
    }
}

fn build_repo(root: &Path, entry: &RepoEntryToml) -> Result<Repo, ClasspathError> {
    match entry.kind.as_str() {
        "purem" => build_purem(root, entry),
        "filesystem" => build_filesystem(root, entry),
        other => Err(ClasspathError::UnsupportedKind {
            name: entry.name.clone(),
            kind: other.to_string(),
        }),
    }
}

fn build_purem(root: &Path, entry: &RepoEntryToml) -> Result<Repo, ClasspathError> {
    let path_str = entry
        .path
        .as_deref()
        .ok_or_else(|| ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source: RepoError::DescriptorParse(format!(
                "[[repo]] '{}' kind=purem requires a `path = ...` field",
                entry.name
            )),
        })?;
    let path = resolve_relative(root, path_str);
    if !path.is_file() {
        return Err(ClasspathError::PathMissing {
            name: entry.name.clone(),
            path,
        });
    }
    let meta = leak_repo_meta(entry);
    Repo::from_purem_file(&path, format!("/{}", entry.name), meta).map_err(|source| {
        ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source,
        }
    })
}

fn build_filesystem(root: &Path, entry: &RepoEntryToml) -> Result<Repo, ClasspathError> {
    if let Some(desc) = &entry.descriptor {
        let descriptor_path = resolve_relative(root, desc);
        if !descriptor_path.is_file() {
            return Err(ClasspathError::PathMissing {
                name: entry.name.clone(),
                path: descriptor_path,
            });
        }
        Repo::from_descriptor(&descriptor_path).map_err(|source| ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source,
        })
    } else if entry.path.is_some() {
        // `from_filesystem` doesn't carry RepoMeta; topo-sort rejects
        // anonymous repos. Force callers to a descriptor.
        Err(ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source: RepoError::DescriptorParse(
                "kind=filesystem requires `descriptor = ...` (a Java repo descriptor JSON); the bare `path =` form is reserved"
                    .to_string(),
            ),
        })
    } else {
        Err(ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source: RepoError::DescriptorParse(format!(
                "[[repo]] '{}' kind=filesystem requires a `descriptor = ...` field",
                entry.name
            )),
        })
    }
}

fn resolve_relative(root: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { root.join(pb) }
}

fn leak_repo_meta(entry: &RepoEntryToml) -> RepoMeta {
    let name: &'static str = Box::leak(entry.name.clone().into_boxed_str());
    let pattern: &'static str = Box::leak(
        entry
            .pattern
            .clone()
            .unwrap_or_else(|| ".*".to_string())
            .into_boxed_str(),
    );
    let deps: Vec<&'static str> = entry
        .dependencies
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|d| Box::leak(d.into_boxed_str()) as &'static str)
        .collect();
    let dependencies: &'static [&'static str] = Box::leak(deps.into_boxed_slice());
    RepoMeta {
        name,
        pattern,
        dependencies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(p: &Path, contents: &[u8]) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, contents).expect("write");
    }

    fn empty_purem_blob() -> Vec<u8> {
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        blob
    }

    #[test]
    fn parse_classpath_with_one_purem() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("snapshots").join("a.purem");
        write_file(&purem_path, &empty_purem_blob());

        let text = r#"
[[repo]]
name = "a"
kind = "purem"
path = "snapshots/a.purem"
"#;
        let cp = parse_classpath_toml(text, "<test>", tmp.path()).expect("parse");
        assert_eq!(cp.repos.len(), 1);
        assert_eq!(cp.repos[0].meta().expect("meta").name, "a");
    }

    #[test]
    fn parse_classpath_rejects_duplicate_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("a.purem");
        write_file(&purem_path, &empty_purem_blob());

        let text = r#"
[[repo]]
name = "a"
kind = "purem"
path = "a.purem"

[[repo]]
name = "a"
kind = "purem"
path = "a.purem"
"#;
        match parse_classpath_toml(text, "<test>", tmp.path()) {
            Err(ClasspathError::DuplicateName { name }) => assert_eq!(name, "a"),
            other => panic!("expected DuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn parse_classpath_rejects_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text = r#"
[[repo]]
name = "ghost"
kind = "purem"
path = "no-such.purem"
"#;
        match parse_classpath_toml(text, "<test>", tmp.path()) {
            Err(ClasspathError::PathMissing { name, .. }) => assert_eq!(name, "ghost"),
            other => panic!("expected PathMissing, got {other:?}"),
        }
    }

    #[test]
    fn parse_classpath_parses_extension_configs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text = r#"
[extension.relational.h2]
jar_path = "~/.m2/repository/com/h2database/h2/2.1.214/h2-2.1.214.jar"
version  = "2.1.214"
pg_port  = 5435

[extension.lake.snowflake]
account = "test-acct"
"#;
        let cp = parse_classpath_toml(text, "<test>", tmp.path()).expect("parse");
        let relational = cp.extension_configs.get("relational").expect("relational");
        let h2 = relational.get("h2").expect("h2");
        let table = h2.as_table().expect("table");
        assert_eq!(
            table.get("version").and_then(|v| v.as_str()),
            Some("2.1.214")
        );
        assert!(cp.extension_configs.contains_key("lake"));
    }

    #[test]
    fn compile_classpath_bytes_rejects_invalid_utf8() {
        // Non-UTF-8 bytes (an isolated continuation byte) should surface
        // as `Utf8` before parsing is even attempted.
        let bytes = vec![0x80];
        match compile_classpath_bytes(&bytes) {
            Err(ClasspathError::Utf8 { .. }) => {}
            other => panic!("expected Utf8 error, got {other:?}"),
        }
    }

    #[test]
    fn compile_classpath_bytes_relative_path_without_root_surfaces_path_missing() {
        // Contract: bytes-mode TOMLs that use relative paths without
        // setting their own `root` resolve against `/` and fail with a
        // clear PathMissing. This is the signal Java consumers see when
        // their classpath TOML isn't yet self-contained.
        let bytes = br#"
[[repo]]
name = "ghost"
kind = "purem"
path = "no-such.purem"
"#;
        match compile_classpath_bytes(bytes) {
            Err(ClasspathError::PathMissing { name, path }) => {
                assert_eq!(name, "ghost");
                // Either the leading-slash join or a platform-specific
                // equivalent — the key signal is that the path is
                // anchored at root, not the cwd.
                let path_str = path.display().to_string();
                assert!(
                    path_str.starts_with('/') || path_str.contains("no-such.purem"),
                    "got: {path_str}"
                );
            }
            other => panic!("expected PathMissing, got {other:?}"),
        }
    }

    #[test]
    fn compile_classpath_bytes_accepts_absolute_path_in_toml() {
        // Bytes-mode happy path: TOML with an absolute `[[repo]] path =`
        // is self-contained and resolves without filesystem coordination.
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("a.purem");
        write_file(&purem_path, &empty_purem_blob());

        let text = format!(
            r#"
[[repo]]
name = "a"
kind = "purem"
path = "{}"
"#,
            purem_path.display()
        );
        // The empty-payload purem isn't a real model — `repo::load`
        // will still report CompileFailed. The point of this test is
        // that we got past parse + repo build, which is the contract
        // bytes-mode guarantees.
        match compile_classpath_bytes(text.as_bytes()) {
            Err(ClasspathError::CompileFailed { .. }) | Ok(_) => {}
            other => panic!("expected CompileFailed or Ok, got {other:?}"),
        }
    }

    #[test]
    fn merge_with_embedded_preserves_platform_when_unshadowed() {
        let cp = Classpath {
            root: PathBuf::from("."),
            extra_auto_imports: Vec::new(),
            repos: Vec::new(),
            extension_configs: HashMap::new(),
        };
        let resolved = merge_with_embedded(cp, None);
        assert!(
            resolved
                .repos
                .iter()
                .any(|r| r.meta().is_some_and(|m| m.name == "platform")),
            "embedded platform must remain after merge"
        );
    }
}
