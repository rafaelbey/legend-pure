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

//! Parse `legend-pure-classpath.toml` into a `Vec<Repo>` ready for
//! [`legend_pure_core_platform::repo::load`].
//!
//! The classpath is the distribution primitive: it lists every repo a
//! runtime should compose, with its kind + location. End-users edit the
//! TOML to swap in newer `.purem` files without rebuilding the binary.
//!
//! # Schema (`legend-pure-classpath.toml`)
//!
//! ```toml
//! # Optional. Defaults to the directory containing this file.
//! root = "."
//!
//! # Optional. Auto-imports applied to every repo (extends the platform
//! # default list — these are added on top of it).
//! auto_imports = ["my::custom::package"]
//!
//! [[repo]]
//! name = "platform"
//! kind = "purem"
//! path = "snapshots/platform.purem"
//!
//! [[repo]]
//! name = "my_app"
//! kind = "filesystem"
//! descriptor = "my_app.definition.json"
//!
//! # Reserved for v2: kind = "maven", coordinates = "...".
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use legend_pure_core_platform::repo::{Repo, RepoError, RepoMeta};
use serde::Deserialize;
use smol_str::SmolStr;
use thiserror::Error;

/// Parsed classpath metadata.
#[derive(Debug)]
pub struct Classpath {
    /// Resolved root for relative paths.
    pub root: PathBuf,
    /// Additional auto-imports declared in the TOML (caller still
    /// applies platform defaults on top).
    pub extra_auto_imports: Vec<SmolStr>,
    /// Resolved repos, in the order declared in the TOML. Topo-sort is
    /// applied later by [`legend_pure_core_platform::repo::load`].
    pub repos: Vec<Repo>,
}

/// Errors raised by [`load_classpath`].
#[derive(Debug, Error)]
pub enum ClasspathError {
    /// Reading the TOML file failed.
    #[error("reading classpath {}: {source}", path.display())]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse failure.
    #[error("invalid TOML in {}: {source}", path.display())]
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying parser error.
        #[source]
        source: toml::de::Error,
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
}

#[derive(Debug, Deserialize)]
struct ClasspathToml {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    auto_imports: Vec<String>,
    #[serde(default, rename = "repo")]
    repos: Vec<RepoEntryToml>,
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

/// Parse a classpath TOML file into resolved [`Repo`]s.
///
/// `toml_path` is read as text, deserialized, then each `[[repo]]` is
/// turned into a real [`Repo`] (`Purem`, `Filesystem`, etc.) with
/// relative paths resolved against the TOML's `root` (or the TOML's
/// parent directory if `root` is unset).
///
/// # Errors
///
/// Returns [`ClasspathError`] for any read / parse / build failure.
pub fn load_classpath(toml_path: &Path) -> Result<Classpath, ClasspathError> {
    let text = std::fs::read_to_string(toml_path).map_err(|source| ClasspathError::Io {
        path: toml_path.to_path_buf(),
        source,
    })?;
    let parsed: ClasspathToml = toml::from_str(&text).map_err(|source| ClasspathError::Parse {
        path: toml_path.to_path_buf(),
        source,
    })?;

    // Resolve root.
    let parent = toml_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let root = match &parsed.root {
        None => parent.clone(),
        Some(r) => {
            let p = PathBuf::from(r);
            if p.is_absolute() { p } else { parent.join(p) }
        }
    };

    // Build repos.
    let mut repos: Vec<Repo> = Vec::with_capacity(parsed.repos.len());
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in &parsed.repos {
        if !seen_names.insert(entry.name.clone()) {
            return Err(ClasspathError::DuplicateName {
                name: entry.name.clone(),
            });
        }
        let repo = build_repo(&root, entry)?;
        repos.push(repo);
    }

    Ok(Classpath {
        root,
        extra_auto_imports: parsed.auto_imports.into_iter().map(SmolStr::new).collect(),
        repos,
    })
}

fn build_repo(root: &Path, entry: &RepoEntryToml) -> Result<Repo, ClasspathError> {
    match entry.kind.as_str() {
        "purem" => build_purem(root, entry),
        "filesystem" => build_filesystem(root, entry),
        // Kinds reserved for v2:
        // "embedded" | "maven" => not yet supported
        other => Err(ClasspathError::UnsupportedKind {
            name: entry.name.clone(),
            kind: other.to_string(),
        }),
    }
}

fn build_purem(root: &Path, entry: &RepoEntryToml) -> Result<Repo, ClasspathError> {
    let path_str = entry.path.as_deref().ok_or_else(|| {
        // Surface a missing-`path` for purem as RepoBuild for one place
        // to handle, with a clear message.
        ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source: RepoError::DescriptorParse(format!(
                "[[repo]] '{}' kind=purem requires a `path = ...` field",
                entry.name
            )),
        }
    })?;
    let path = resolve_relative(root, path_str);
    if !path.is_file() {
        return Err(ClasspathError::PathMissing {
            name: entry.name.clone(),
            path,
        });
    }
    // Build a leaked-static RepoMeta from the entry. The `name` field
    // comes from the TOML; pattern/dependencies fall back to defaults
    // when absent.
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
    } else if let Some(p) = &entry.path {
        let dir = resolve_relative(root, p);
        if !dir.is_dir() {
            return Err(ClasspathError::PathMissing {
                name: entry.name.clone(),
                path: dir,
            });
        }
        // Note: from_filesystem doesn't carry RepoMeta. Topo-sort
        // currently rejects anonymous repos, so kind=filesystem in a
        // classpath should always go through descriptor=. Document and
        // surface a build error.
        let _ = dir;
        Err(ClasspathError::RepoBuild {
            name: entry.name.clone(),
            source: RepoError::DescriptorParse(format!(
                "kind=filesystem requires `descriptor = ...` (a Java repo descriptor JSON); the bare `path =` form is reserved",
            )),
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

/// Default classpath filename — looked up alongside the CLI binary if no
/// `--classpath` flag is passed.
pub const DEFAULT_FILENAME: &str = "legend-pure-classpath.toml";

/// Walk up from `start_dir` looking for a [`DEFAULT_FILENAME`] (Cargo
/// style). Returns the first match, or `None`.
#[must_use]
pub fn discover_classpath(start_dir: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start_dir);
    while let Some(d) = cur {
        let candidate = d.join(DEFAULT_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = d.parent();
    }
    None
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

    #[test]
    fn load_classpath_with_one_purem() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("snapshots").join("a.purem");

        // Write a minimal valid purem blob: just the header + 0-len payload.
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        write_file(&purem_path, &blob);

        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "a"
kind = "purem"
path = "snapshots/a.purem"
"#,
        );

        // Postcard'll fail to deserialize the empty payload as a slice,
        // which is fine — `from_purem_file` only validates the header.
        let cp = load_classpath(&toml_path).expect("load");
        assert_eq!(cp.repos.len(), 1);
        let meta = cp.repos[0].meta().expect("meta");
        assert_eq!(meta.name, "a");
    }

    #[test]
    fn load_classpath_rejects_duplicate_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("a.purem");
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        write_file(&purem_path, &blob);

        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "a"
kind = "purem"
path = "a.purem"

[[repo]]
name = "a"
kind = "purem"
path = "a.purem"
"#,
        );

        match load_classpath(&toml_path) {
            Err(ClasspathError::DuplicateName { name }) => assert_eq!(name, "a"),
            other => panic!("expected DuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn load_classpath_rejects_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "ghost"
kind = "purem"
path = "no-such.purem"
"#,
        );

        match load_classpath(&toml_path) {
            Err(ClasspathError::PathMissing { name, .. }) => assert_eq!(name, "ghost"),
            other => panic!("expected PathMissing, got {other:?}"),
        }
    }

    #[test]
    fn discover_classpath_walks_up() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let cp = tmp.path().join("legend-pure-classpath.toml");
        write_file(&cp, b"");

        let found = discover_classpath(&nested).expect("found");
        assert_eq!(found, cp);
    }
}
