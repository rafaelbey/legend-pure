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

#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Build-time utilities for embedding Pure repositories into a Rust crate.
//!
//! Any crate that owns one or more Pure repos — the platform crate, a
//! third-party DSL crate, a private user repo — can drop a single call
//! into its `build.rs` and have its `.pure` sources, JSON manifests,
//! and Java-style descriptor metadata embedded into a generated Rust
//! file:
//!
//! ```toml
//! # In Cargo.toml of the consuming crate:
//! [[package.metadata.legend-pure.repos]]
//! descriptor = "path/to/my_repo.definition.json"
//! shape = "embedded"
//!
//! [build-dependencies]
//! legend-pure-build = { path = "../build" }
//! ```
//!
//! ```rust,ignore
//! // build.rs
//! fn main() {
//!     legend_pure_build::Embedder::new()
//!         .run()
//!         .expect("legend-pure-build: failed to embed repos");
//! }
//! ```
//!
//! The generated file (default: `$OUT_DIR/legend_pure_repos.rs`) is
//! `include!`-able from any module in the consuming crate and emits:
//!
//! - `REPO_<NAME>_FILES: &[PureSourceFile]` — mixed `.pure` + `.json`
//! - `REPO_<NAME>_META: RepoMeta` — name / pattern / dependencies
//! - `default_embedded_repos() -> Vec<Repo>` — repos in declaration
//!   order
//!
//! Where `<NAME>` is the descriptor's `"name"` field upper-snake-cased.
//!
//! # Inference rules
//!
//! Each entry under `[[package.metadata.legend-pure.repos]]` needs only:
//!
//! - `descriptor` — path (relative to the crate root) of an existing
//!   Java repo descriptor JSON: `{name, pattern, dependencies}`.
//! - `shape` — `"embedded"` (only currently supported value;
//!   `"filesystem"` and `"purem"` reserved for future).
//!
//! From the descriptor's `name` and on-disk location:
//!
//! - **Source root** is `<dir_of_descriptor>/<name>/`.
//! - **Canonical URL** for each walked file is `/<name>/<rel_path>`.
//! - **Bootstrap skip** — files whose canonical URL appears in
//!   [`Embedder::skip_canonical`] are excluded from embedding. The
//!   default skip list is `[`[`M3_BOOTSTRAP_CANONICAL`]`]`.
//!
//! # `.purem` snapshot generation
//!
//! Two `shape` values produce binary `.purem` artifacts at build time
//! by invoking the [`legend_pure_snapshot_builder`] helper:
//!
//! - `shape = "purem-embedded"` — compiles the repo (with its declared
//!   transitive deps), writes `OUT_DIR/<name>.purem`, and emits an
//!   `include_bytes!` constant + a `Repo::from_purem_static` entry in
//!   `default_embedded_repos()`. Suitable for the bare-minimum bootstrap
//!   that every binary always carries (today: just `platform`).
//!
//! - `shape = "purem-artifact"` — compiles + writes the blob to
//!   `<target>/snapshots/<name>.purem` (sibling to the produced
//!   binaries) but does NOT include it in the binary. The CLI's
//!   classpath auto-discovery picks it up at runtime. Suitable for
//!   DSLs and any other repo that should ship as an external file.
//!
//! `shape = "embedded"` (legacy `include_str!`) is still supported for
//! ad-hoc test fixtures but is no longer the recommended path.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use walkdir::WalkDir;

/// The bootstrap M3 metamodel canonical URL. Always-skipped because
/// `m3.pure` is parsed by `m3_parser` (not the regular pipeline) and is
/// embedded directly in `legend-pure-pure::bootstrap`. Including it in
/// the regular embed would cause double-parsing and mismatched
/// metamodel state.
pub const M3_BOOTSTRAP_CANONICAL: &str = "/platform/pure/grammar/m3.pure";

/// Default name of the generated file written into `$OUT_DIR`.
pub const DEFAULT_OUTPUT_NAME: &str = "legend_pure_repos.rs";

/// Errors returned by the embedder.
#[derive(Debug, Error)]
pub enum BuildError {
    /// I/O failure reading a Cargo.toml, descriptor, or `.pure` file.
    #[error("io error reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `Cargo.toml` did not parse as TOML.
    #[error("parse {path} failed: {source}")]
    Toml {
        /// Cargo.toml that failed.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// A descriptor JSON did not parse.
    #[error("parse {path} failed: {source}")]
    Json {
        /// Descriptor that failed.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Required env var missing (build script context only).
    #[error("env var {0} not set (this should be invoked from a build.rs)")]
    Env(&'static str),
    /// File path that isn't valid UTF-8 — the generated `include_str!`
    /// macro requires a UTF-8 path literal.
    #[error("non-UTF-8 path: {0}")]
    NonUtf8Path(PathBuf),
    /// Descriptor JSON exists but the expected `<dir>/<name>/` source
    /// root does not.
    #[error("repo `{name}` source root does not exist: {path}")]
    SourceRootMissing {
        /// Repo name from the descriptor.
        name: String,
        /// Expected source-root path.
        path: PathBuf,
    },
    /// `Cargo.toml` had no `[package.metadata.legend-pure.repos]`
    /// table.
    #[error("no `[[package.metadata.legend-pure.repos]]` entries in {0}")]
    NoMetadata(PathBuf),
    /// Descriptor canonicalization succeeded but the path has no
    /// parent — should be impossible for a regular file.
    #[error("descriptor path has no parent: {0}")]
    DescriptorNoParent(PathBuf),
    /// Walk over a source root failed.
    #[error("walk failed: {0}")]
    Walk(String),
    /// `snapshot-builder` returned an error while compiling a repo.
    #[error("snapshot-builder failed for repo `{name}`: {source}")]
    Snapshot {
        /// Repo name being built.
        name: String,
        /// Underlying snapshot-builder error.
        #[source]
        source: legend_pure_snapshot_builder::BuildError,
    },
    /// `Cargo.toml` declared a repo with an unrecognized `shape`.
    #[error("unsupported shape `{shape}` for repo (descriptor: {descriptor})")]
    UnsupportedShape {
        /// The unrecognized shape string.
        shape: String,
        /// Descriptor path that referenced it.
        descriptor: String,
    },
    /// Could not resolve the workspace target dir from `OUT_DIR`.
    #[error("could not resolve target dir from OUT_DIR={0}")]
    NoTargetDir(PathBuf),
}

#[derive(Deserialize)]
struct CargoToml {
    package: CargoPackage,
}

#[derive(Deserialize)]
struct CargoPackage {
    metadata: CargoMetadata,
}

#[derive(Deserialize)]
struct CargoMetadata {
    #[serde(rename = "legend-pure")]
    legend_pure: LegendPureMetadata,
}

#[derive(Deserialize)]
struct LegendPureMetadata {
    repos: Vec<RepoEntry>,
}

#[derive(Deserialize, Debug)]
struct RepoEntry {
    descriptor: String,
    shape: String,
}

#[derive(Deserialize)]
struct DescriptorJson {
    name: String,
    pattern: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// Build-time embedder for Pure repositories.
///
/// Construct with [`Embedder::new`], optionally chain configuration
/// methods, then call [`Embedder::run`] from `build.rs`.
pub struct Embedder {
    cargo_toml: Option<PathBuf>,
    output: Option<PathBuf>,
    crate_path: String,
    emit_default_aggregator: bool,
    skip_canonical: Vec<String>,
}

impl Default for Embedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder {
    /// Construct an embedder with default configuration:
    /// - `cargo_toml`: `$CARGO_MANIFEST_DIR/Cargo.toml`
    /// - `output`: `$OUT_DIR/legend_pure_repos.rs`
    /// - `crate_path`: `"::legend_pure_core_platform"` (suitable for
    ///   downstream consumers)
    /// - `emit_default_aggregator`: `true`
    /// - `skip_canonical`: `[M3_BOOTSTRAP_CANONICAL]`
    #[must_use]
    pub fn new() -> Self {
        Self {
            cargo_toml: None,
            output: None,
            crate_path: "::legend_pure_core_platform".into(),
            emit_default_aggregator: true,
            skip_canonical: vec![M3_BOOTSTRAP_CANONICAL.into()],
        }
    }

    /// Override the path to the `Cargo.toml` whose
    /// `[[package.metadata.legend-pure.repos]]` table is read.
    #[must_use]
    pub fn cargo_toml(mut self, path: impl Into<PathBuf>) -> Self {
        self.cargo_toml = Some(path.into());
        self
    }

    /// Override the output file path. Default:
    /// `$OUT_DIR/legend_pure_repos.rs`.
    #[must_use]
    pub fn output(mut self, path: impl Into<PathBuf>) -> Self {
        self.output = Some(path.into());
        self
    }

    /// Set the module path prefix used by the generated code when
    /// referring to `Repo`, `RepoMeta`, and `PureSourceFile`.
    ///
    /// Use `"crate"` when invoked from `legend-pure-core-platform`'s
    /// own `build.rs` (so generated paths resolve as
    /// `crate::repo::Repo`); use the default
    /// `"::legend_pure_core_platform"` for any other crate (so
    /// generated paths resolve via the absolute path
    /// `::legend_pure_core_platform::repo::Repo`).
    #[must_use]
    pub fn crate_path(mut self, path: impl Into<String>) -> Self {
        self.crate_path = path.into();
        self
    }

    /// Whether to emit the
    /// `pub fn default_embedded_repos() -> Vec<Repo>` aggregator
    /// function. Defaults to `true`. Disable when the consumer crate
    /// hand-writes its own aggregator over the per-repo constants.
    #[must_use]
    pub fn emit_default_aggregator(mut self, on: bool) -> Self {
        self.emit_default_aggregator = on;
        self
    }

    /// Add a canonical URL to the always-skip list. The default skip
    /// list contains [`M3_BOOTSTRAP_CANONICAL`]; call this if your
    /// repo has additional bootstrap-only files.
    #[must_use]
    pub fn skip_canonical(mut self, url: impl Into<String>) -> Self {
        self.skip_canonical.push(url.into());
        self
    }

    /// Run the embedder, writing the generated file.
    ///
    /// Emits `cargo:rerun-if-changed=` directives for the Cargo.toml,
    /// each descriptor JSON, each source root, and each embedded
    /// source file — so any change triggers a re-run.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError`] on any I/O, parse, or path failure.
    pub fn run(self) -> Result<(), BuildError> {
        let manifest_dir = manifest_dir()?;
        let cargo_toml_path = self
            .cargo_toml
            .clone()
            .unwrap_or_else(|| manifest_dir.join("Cargo.toml"));
        let cargo_toml_str = fs::read_to_string(&cargo_toml_path).map_err(|e| BuildError::Io {
            path: cargo_toml_path.clone(),
            source: e,
        })?;
        let cargo_toml: CargoToml =
            toml::from_str(&cargo_toml_str).map_err(|source| BuildError::Toml {
                path: cargo_toml_path.clone(),
                source,
            })?;

        // The cargo_toml-as-rerun-if-changed signal — relative to the
        // manifest dir is portable across machines.
        if cargo_toml_path == manifest_dir.join("Cargo.toml") {
            println!("cargo:rerun-if-changed=Cargo.toml");
        } else {
            println!("cargo:rerun-if-changed={}", cargo_toml_path.display());
        }

        let output = match self.output.clone() {
            Some(p) => p,
            None => {
                let out_dir = env::var_os("OUT_DIR").ok_or(BuildError::Env("OUT_DIR"))?;
                PathBuf::from(out_dir).join(DEFAULT_OUTPUT_NAME)
            }
        };

        let repos = &cargo_toml.package.metadata.legend_pure.repos;
        if repos.is_empty() {
            return Err(BuildError::NoMetadata(cargo_toml_path));
        }

        let mut out = String::new();
        let mut default_repo_calls = Vec::<EmittedRepo>::new();
        let mut artifact_emitted = false;

        // Pre-resolve every repo's descriptor path so purem shapes can
        // pass the full descriptor list to snapshot-builder for topo-sort
        // + dependency resolution.
        let all_descriptors: Vec<PathBuf> = repos
            .iter()
            .map(|e| {
                fs::canonicalize(manifest_dir.join(&e.descriptor)).map_err(|source| {
                    BuildError::Io {
                        path: manifest_dir.join(&e.descriptor),
                        source,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        for entry in repos {
            match entry.shape.as_str() {
                "embedded" => {
                    self.emit_repo(entry, &manifest_dir, &mut out, &mut default_repo_calls)?;
                    // Side-effect: also emit a `.purem` artifact next to
                    // the produced binaries so classpath auto-discovery
                    // can pick it up. Failures are advisory (warning
                    // only) — the embedded source path is the
                    // authoritative source-of-truth, and a broken DSL
                    // compile shouldn't block the binary build.
                    self.emit_artifact_side_effect(
                        entry,
                        &manifest_dir,
                        &all_descriptors,
                        &mut artifact_emitted,
                    )?;
                }
                "purem-embedded" => {
                    self.emit_purem_embedded_repo(
                        entry,
                        &manifest_dir,
                        &all_descriptors,
                        &mut out,
                        &mut default_repo_calls,
                    )?;
                }
                "purem-artifact" => {
                    self.emit_purem_artifact_repo(
                        entry,
                        &manifest_dir,
                        &all_descriptors,
                        &mut out,
                        &mut artifact_emitted,
                    )?;
                }
                other => {
                    return Err(BuildError::UnsupportedShape {
                        shape: other.to_string(),
                        descriptor: entry.descriptor.clone(),
                    });
                }
            }
        }

        if self.emit_default_aggregator {
            self.emit_default_aggregator_fn(&default_repo_calls, &mut out)?;
        }

        fs::write(&output, out).map_err(|source| BuildError::Io {
            path: output,
            source,
        })?;
        Ok(())
    }

    fn emit_repo(
        &self,
        entry: &RepoEntry,
        manifest_dir: &Path,
        out: &mut String,
        default_repo_calls: &mut Vec<EmittedRepo>,
    ) -> Result<(), BuildError> {
        let descriptor_path = manifest_dir.join(&entry.descriptor);
        let descriptor_canonical =
            fs::canonicalize(&descriptor_path).map_err(|source| BuildError::Io {
                path: descriptor_path.clone(),
                source,
            })?;
        let descriptor_str =
            fs::read_to_string(&descriptor_canonical).map_err(|source| BuildError::Io {
                path: descriptor_canonical.clone(),
                source,
            })?;
        let descriptor: DescriptorJson =
            serde_json::from_str(&descriptor_str).map_err(|source| BuildError::Json {
                path: descriptor_canonical.clone(),
                source,
            })?;

        let descriptor_str_path = path_to_str(&descriptor_canonical)?;
        println!("cargo:rerun-if-changed={descriptor_str_path}");

        let descriptor_dir = descriptor_canonical
            .parent()
            .ok_or_else(|| BuildError::DescriptorNoParent(descriptor_canonical.clone()))?;
        let source_root = descriptor_dir.join(&descriptor.name);
        if !source_root.exists() {
            return Err(BuildError::SourceRootMissing {
                name: descriptor.name.clone(),
                path: source_root,
            });
        }

        let source_root_str = path_to_str(&source_root)?;
        println!("cargo:rerun-if-changed={source_root_str}");

        let const_suffix = sanitize_const(&descriptor.name);
        let cp = &self.crate_path;

        // --- RepoMeta const ----------------------------------------------
        writeln!(
            out,
            "/// Descriptor metadata read from the Java JSON for the `{name}` repo.",
            name = descriptor.name
        )
        .ok();
        writeln!(
            out,
            "pub const REPO_{const_suffix}_META: {cp}::repo::RepoMeta = {cp}::repo::RepoMeta {{",
        )
        .ok();
        writeln!(out, "    name: \"{}\",", descriptor.name).ok();
        writeln!(out, "    pattern: r#\"{}\"#,", descriptor.pattern).ok();
        write!(out, "    dependencies: &[").ok();
        for dep in &descriptor.dependencies {
            write!(out, "\"{dep}\", ").ok();
        }
        writeln!(out, "],").ok();
        writeln!(out, "}};\n").ok();

        // --- Files array (mixed .pure + .json) ---------------------------
        writeln!(
            out,
            "/// Embedded files (.pure + .json) for the `{name}` repo.",
            name = descriptor.name
        )
        .ok();
        writeln!(
            out,
            "pub const REPO_{const_suffix}_FILES: &[{cp}::sources::PureSourceFile] = &[",
        )
        .ok();

        let mut prefix = String::with_capacity(descriptor.name.len() + 1);
        prefix.push('/');
        prefix.push_str(&descriptor.name);

        for walk_entry in WalkDir::new(&source_root).sort_by_file_name() {
            let walk_entry = walk_entry.map_err(|e| BuildError::Walk(e.to_string()))?;
            let path = walk_entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext != "pure" && ext != "json" {
                continue;
            }

            let relative = path
                .strip_prefix(&source_root)
                .map_err(|e| BuildError::Walk(e.to_string()))?;
            let rel_str = path_to_str(relative)?;

            let canonical_url = format!("{prefix}/{rel_str}");
            if self
                .skip_canonical
                .iter()
                .any(|s| s.as_str() == canonical_url)
            {
                continue;
            }

            let abs = fs::canonicalize(path).map_err(|source| BuildError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let abs_str = path_to_str(&abs)?;

            writeln!(
                out,
                "    {cp}::sources::PureSourceFile {{ path: \"{canonical_url}\", \
                 content: include_str!(\"{abs_str}\") }},",
            )
            .ok();

            println!("cargo:rerun-if-changed={abs_str}");
        }
        out.push_str("];\n\n");

        default_repo_calls.push(EmittedRepo {
            const_suffix,
            prefix,
            shape: EmittedShape::Embedded,
        });
        Ok(())
    }

    /// Side-effect emission for the legacy `shape = "embedded"` path:
    /// also writes a `.purem` blob to `target/<profile>/snapshots/` so
    /// classpath-driven consumers can pick it up. Compile failures are
    /// degraded to `cargo:warning=` because the source `include_str!`
    /// path is the authoritative load path for embedded repos and
    /// shouldn't be blocked by a downstream compile bug.
    fn emit_artifact_side_effect(
        &self,
        entry: &RepoEntry,
        manifest_dir: &Path,
        all_descriptors: &[PathBuf],
        artifact_emitted: &mut bool,
    ) -> Result<(), BuildError> {
        let (_descriptor_canonical, descriptor) = self.read_descriptor(entry, manifest_dir)?;

        let target_snapshots_dir = resolve_target_snapshots_dir()?;
        fs::create_dir_all(&target_snapshots_dir).map_err(|source| BuildError::Io {
            path: target_snapshots_dir.clone(),
            source,
        })?;
        let purem_path = target_snapshots_dir.join(format!("{}.purem", descriptor.name));

        if let Err(source) =
            self.run_snapshot_builder(&descriptor.name, all_descriptors, &purem_path)
        {
            println!(
                "cargo:warning=snapshot-builder failed for `{name}`: {source}. \
                 The .purem artifact was not produced; embedded source path \
                 is unaffected.",
                name = descriptor.name,
            );
            let _ = fs::remove_file(&purem_path);
        }

        if !*artifact_emitted {
            let dir_str = path_to_str(&target_snapshots_dir)?;
            println!("cargo:rustc-env=LEGEND_PURE_BUILD_SNAPSHOTS_DIR={dir_str}");
            *artifact_emitted = true;
        }
        Ok(())
    }

    fn emit_purem_embedded_repo(
        &self,
        entry: &RepoEntry,
        manifest_dir: &Path,
        all_descriptors: &[PathBuf],
        out: &mut String,
        default_repo_calls: &mut Vec<EmittedRepo>,
    ) -> Result<(), BuildError> {
        let (descriptor_canonical, descriptor) = self.read_descriptor(entry, manifest_dir)?;
        self.emit_rerun_directives(&descriptor_canonical, &descriptor)?;

        let out_dir = env::var_os("OUT_DIR").ok_or(BuildError::Env("OUT_DIR"))?;
        let purem_path = PathBuf::from(&out_dir).join(format!("{}.purem", descriptor.name));
        self.run_snapshot_builder(&descriptor.name, all_descriptors, &purem_path)?;

        let const_suffix = sanitize_const(&descriptor.name);
        self.emit_meta_const(&descriptor, &const_suffix, out);

        let cp = &self.crate_path;
        let purem_path_str = path_to_str(&purem_path)?;
        writeln!(
            out,
            "/// Embedded `.purem` blob for the `{name}` repo (compiled at build time).",
            name = descriptor.name
        )
        .ok();
        writeln!(
            out,
            "pub static REPO_{const_suffix}_PUREM: &[u8] = include_bytes!(\"{purem_path_str}\");\n"
        )
        .ok();

        let prefix = format!("/{}", descriptor.name);
        default_repo_calls.push(EmittedRepo {
            const_suffix,
            prefix,
            shape: EmittedShape::Purem,
        });

        // Hint to dependent crates that they can find the snapshots
        // directory via this env var. Even though purem-embedded repos
        // are baked in, the side-band signal is harmless and allows
        // mixed pipelines to discover artifacts.
        let _ = cp;
        Ok(())
    }

    fn emit_purem_artifact_repo(
        &self,
        entry: &RepoEntry,
        manifest_dir: &Path,
        all_descriptors: &[PathBuf],
        out: &mut String,
        artifact_emitted: &mut bool,
    ) -> Result<(), BuildError> {
        let (descriptor_canonical, descriptor) = self.read_descriptor(entry, manifest_dir)?;
        self.emit_rerun_directives(&descriptor_canonical, &descriptor)?;

        let target_snapshots_dir = resolve_target_snapshots_dir()?;
        fs::create_dir_all(&target_snapshots_dir).map_err(|source| BuildError::Io {
            path: target_snapshots_dir.clone(),
            source,
        })?;
        let purem_path = target_snapshots_dir.join(format!("{}.purem", descriptor.name));
        // Artifacts are advisory: a compile failure for one DSL doesn't
        // block the build, since users can still point a classpath at
        // the live source. Emit a cargo warning and remove any stale
        // file so callers don't pick up a previous build's blob.
        if let Err(source) = self.run_snapshot_builder(&descriptor.name, all_descriptors, &purem_path) {
            println!(
                "cargo:warning=snapshot-builder failed for `{name}`: {source}. \
                 The .purem artifact was not produced; load from sources via \
                 --classpath kind=filesystem until the underlying compile error is fixed.",
                name = descriptor.name,
            );
            // Best-effort cleanup of any prior stale blob.
            let _ = fs::remove_file(&purem_path);
        }

        let const_suffix = sanitize_const(&descriptor.name);
        self.emit_meta_const(&descriptor, &const_suffix, out);

        // Emit the side-band env var the consumer's runtime can read to
        // discover the build-time-emitted snapshots dir. Only once.
        if !*artifact_emitted {
            let dir_str = path_to_str(&target_snapshots_dir)?;
            println!("cargo:rustc-env=LEGEND_PURE_BUILD_SNAPSHOTS_DIR={dir_str}");
            *artifact_emitted = true;
        }
        Ok(())
    }

    fn read_descriptor(
        &self,
        entry: &RepoEntry,
        manifest_dir: &Path,
    ) -> Result<(PathBuf, DescriptorJson), BuildError> {
        let descriptor_path = manifest_dir.join(&entry.descriptor);
        let descriptor_canonical =
            fs::canonicalize(&descriptor_path).map_err(|source| BuildError::Io {
                path: descriptor_path.clone(),
                source,
            })?;
        let descriptor_str =
            fs::read_to_string(&descriptor_canonical).map_err(|source| BuildError::Io {
                path: descriptor_canonical.clone(),
                source,
            })?;
        let descriptor: DescriptorJson =
            serde_json::from_str(&descriptor_str).map_err(|source| BuildError::Json {
                path: descriptor_canonical.clone(),
                source,
            })?;
        Ok((descriptor_canonical, descriptor))
    }

    fn emit_rerun_directives(
        &self,
        descriptor_canonical: &Path,
        descriptor: &DescriptorJson,
    ) -> Result<(), BuildError> {
        let descriptor_str_path = path_to_str(descriptor_canonical)?;
        println!("cargo:rerun-if-changed={descriptor_str_path}");

        let descriptor_dir = descriptor_canonical
            .parent()
            .ok_or_else(|| BuildError::DescriptorNoParent(descriptor_canonical.to_path_buf()))?;
        let source_root = descriptor_dir.join(&descriptor.name);
        if !source_root.exists() {
            return Err(BuildError::SourceRootMissing {
                name: descriptor.name.clone(),
                path: source_root,
            });
        }
        let source_root_str = path_to_str(&source_root)?;
        println!("cargo:rerun-if-changed={source_root_str}");

        // Also emit per-file rerun directives so any source change
        // re-triggers the build script. Reuses the same WalkDir pass as
        // the legacy embed path.
        for walk_entry in WalkDir::new(&source_root).sort_by_file_name() {
            let walk_entry = walk_entry.map_err(|e| BuildError::Walk(e.to_string()))?;
            let path = walk_entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext != "pure" && ext != "json" {
                continue;
            }
            let abs = fs::canonicalize(path).map_err(|source| BuildError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let abs_str = path_to_str(&abs)?;
            println!("cargo:rerun-if-changed={abs_str}");
        }
        Ok(())
    }

    fn run_snapshot_builder(
        &self,
        target: &str,
        all_descriptors: &[PathBuf],
        output: &Path,
    ) -> Result<(), BuildError> {
        legend_pure_snapshot_builder::compile_to_purem(
            legend_pure_snapshot_builder::CompileRequest {
                descriptors: all_descriptors,
                target,
                output,
                auto_imports: legend_pure_snapshot_builder::DEFAULT_PLATFORM_AUTO_IMPORTS,
            },
        )
        .map_err(|source| BuildError::Snapshot {
            name: target.to_string(),
            source,
        })
    }

    fn emit_meta_const(&self, descriptor: &DescriptorJson, const_suffix: &str, out: &mut String) {
        let cp = &self.crate_path;
        writeln!(
            out,
            "/// Descriptor metadata read from the Java JSON for the `{name}` repo.",
            name = descriptor.name
        )
        .ok();
        writeln!(
            out,
            "pub const REPO_{const_suffix}_META: {cp}::repo::RepoMeta = {cp}::repo::RepoMeta {{",
        )
        .ok();
        writeln!(out, "    name: \"{}\",", descriptor.name).ok();
        writeln!(out, "    pattern: r#\"{}\"#,", descriptor.pattern).ok();
        write!(out, "    dependencies: &[").ok();
        for dep in &descriptor.dependencies {
            write!(out, "\"{dep}\", ").ok();
        }
        writeln!(out, "],").ok();
        writeln!(out, "}};\n").ok();
    }

    fn emit_default_aggregator_fn(
        &self,
        emitted: &[EmittedRepo],
        out: &mut String,
    ) -> Result<(), BuildError> {
        let cp = &self.crate_path;
        out.push_str("/// Default embedded repos in Cargo.toml declaration order.\n");
        out.push_str("/// Generated from `[[package.metadata.legend-pure.repos]]`.\n");
        out.push_str("#[must_use]\n");
        writeln!(
            out,
            "pub fn default_embedded_repos() -> Vec<{cp}::repo::Repo> {{"
        )
        .ok();
        out.push_str("    vec![\n");
        for repo in emitted {
            match repo.shape {
                EmittedShape::Embedded => {
                    writeln!(out, "        {cp}::repo::Repo::Embedded {{").ok();
                    writeln!(out, "            prefix: \"{p}\",", p = repo.prefix).ok();
                    writeln!(
                        out,
                        "            files: REPO_{s}_FILES,",
                        s = repo.const_suffix
                    )
                    .ok();
                    writeln!(
                        out,
                        "            meta: &REPO_{s}_META,",
                        s = repo.const_suffix
                    )
                    .ok();
                    out.push_str("        },\n");
                }
                EmittedShape::Purem => {
                    writeln!(
                        out,
                        "        {cp}::repo::Repo::from_purem_static(\"{p}\", \
                         &REPO_{s}_META, REPO_{s}_PUREM),",
                        p = repo.prefix,
                        s = repo.const_suffix
                    )
                    .ok();
                }
            }
        }
        out.push_str("    ]\n");
        out.push_str("}\n");
        Ok(())
    }
}

/// Convenience wrapper for the common case: default config + run.
///
/// Equivalent to `Embedder::new().run()`. Suitable for any external
/// crate that just wants to embed its declared repos.
///
/// # Errors
///
/// Forwards [`BuildError`] from [`Embedder::run`].
pub fn run() -> Result<(), BuildError> {
    Embedder::new().run()
}

struct EmittedRepo {
    const_suffix: String,
    prefix: String,
    shape: EmittedShape,
}

/// Whether an emitted repo is source-embedded (legacy `include_str!`)
/// or `.purem`-embedded (`include_bytes!`).
#[derive(Debug, Clone, Copy)]
enum EmittedShape {
    /// Legacy `include_str!` source embedding.
    Embedded,
    /// `.purem` blob via `include_bytes!`.
    Purem,
}

fn resolve_target_snapshots_dir() -> Result<PathBuf, BuildError> {
    let out_dir = env::var_os("OUT_DIR").ok_or(BuildError::Env("OUT_DIR"))?;
    let out_dir = PathBuf::from(out_dir);
    // OUT_DIR == target/<profile>/build/<crate>-<hash>/out
    // Walk three ancestors up to reach target/<profile>/.
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .ok_or_else(|| BuildError::NoTargetDir(out_dir.clone()))?;
    Ok(target_dir.join("snapshots"))
}

fn manifest_dir() -> Result<PathBuf, BuildError> {
    env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or(BuildError::Env("CARGO_MANIFEST_DIR"))
}

fn path_to_str(path: &Path) -> Result<String, BuildError> {
    let s = path
        .to_str()
        .ok_or_else(|| BuildError::NonUtf8Path(path.to_path_buf()))?;
    Ok(s.replace('\\', "/"))
}

/// Sanitize a repo name (e.g. `platform_dsl_store`) into an UPPER_SNAKE
/// const-name fragment (e.g. `PLATFORM_DSL_STORE`). ASCII alphanumerics
/// and `_` survive; anything else becomes `_`.
fn sanitize_const(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            s.push(c.to_ascii_uppercase());
        } else {
            s.push('_');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_const_uppers_and_filters() {
        assert_eq!(sanitize_const("platform"), "PLATFORM");
        assert_eq!(sanitize_const("platform_dsl_store"), "PLATFORM_DSL_STORE");
        assert_eq!(sanitize_const("foo-bar"), "FOO_BAR");
        assert_eq!(sanitize_const("a.b"), "A_B");
    }

    #[test]
    fn embedder_defaults() {
        let e = Embedder::new();
        assert_eq!(e.crate_path, "::legend_pure_core_platform");
        assert!(e.emit_default_aggregator);
        assert_eq!(e.skip_canonical, vec![M3_BOOTSTRAP_CANONICAL.to_string()]);
    }

    #[test]
    fn embedder_chaining() {
        let e = Embedder::new()
            .crate_path("crate")
            .emit_default_aggregator(false)
            .skip_canonical("/extra/skip.pure");
        assert_eq!(e.crate_path, "crate");
        assert!(!e.emit_default_aggregator);
        assert_eq!(e.skip_canonical.len(), 2);
        assert!(e.skip_canonical.iter().any(|s| s == "/extra/skip.pure"));
    }
}
