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

//! Build-time helper that compiles one Pure repository — together with
//! every repo it depends on — and writes the resulting per-repo
//! [`PureModelSlice`][legend_pure_parser_pure::purem::slice::PureModelSlice]
//! to a single `.purem` blob.
//!
//! Run from a `build.rs` script for a crate that owns Pure repositories
//! and wants to ship them as binary artifacts:
//!
//! ```ignore
//! legend_pure_snapshot_builder::compile_to_purem(
//!     legend_pure_snapshot_builder::CompileRequest {
//!         descriptors: &descriptor_paths,
//!         target: "platform_dsl_mapping",
//!         output: &out_dir.join("platform_dsl_mapping.purem"),
//!         auto_imports: legend_pure_snapshot_builder::DEFAULT_PLATFORM_AUTO_IMPORTS,
//!     },
//! )?;
//! ```
//!
//! The output is byte-deterministic given identical inputs (relies on
//! `purem::write_repo`'s determinism contract).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};
use legend_pure_parser_pure::purem::{
    assert_no_dangling_refs, collect_test_partition, slice_by_repo_with_filter, write_repo,
};
use legend_pure_parser_pure::visibility::{RepoPattern, compile_repo_pattern};
use serde::Deserialize;
use smol_str::SmolStr;
use thiserror::Error;

/// Same set of auto-imports the platform's runtime loader uses (mirrors
/// `legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS`). Kept in
/// sync by hand — both consumers ship the same `.pure` repos, so they
/// must agree.
pub const DEFAULT_PLATFORM_AUTO_IMPORTS: &[&str] = &[
    "meta::pure::functions::lang",
    "meta::pure::functions::boolean",
    "meta::pure::functions::collection",
    "meta::pure::functions::math",
    "meta::pure::functions::string",
    "meta::pure::functions::date",
    "meta::pure::functions::meta",
    "meta::pure::functions::multiplicity",
    "meta::pure::functions::relation",
    "meta::pure::functions::asserts",
    "meta::pure::functions::io",
    "meta::pure::functions::tools",
    "meta::pure::profiles",
    "meta::pure::test::pct",
    "meta::pure::test::surveyor",
    "meta::pure::tools",
];

/// Inputs to a single snapshot-build invocation.
#[derive(Debug)]
pub struct CompileRequest<'a> {
    /// Descriptor JSON paths to load. Must include the target repo's
    /// descriptor and every transitive dependency. Order is irrelevant —
    /// the builder topologically sorts internally.
    pub descriptors: &'a [PathBuf],
    /// Name of the repo whose chunks should be sliced into the output
    /// blob. Must match the `name` field of one of the descriptors.
    pub target: &'a str,
    /// Path to write the production `.purem` blob (test elements
    /// stripped). Parent directory must exist. The companion tests blob
    /// is always written alongside as `<output>.tests.purem`; see
    /// [`tests_output_path`] for the exact derivation.
    pub output: &'a Path,
    /// Auto-imports applied to every repo's compile. Pass
    /// [`DEFAULT_PLATFORM_AUTO_IMPORTS`] unless you have reason to
    /// diverge.
    pub auto_imports: &'a [&'a str],
}

/// Failure modes for [`compile_to_purem`].
#[derive(Debug, Error)]
pub enum BuildError {
    /// Failed to read or parse a descriptor JSON.
    #[error("descriptor {path}: {source}")]
    Descriptor {
        /// Path to the offending descriptor.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Source root directory derived from the descriptor doesn't exist.
    #[error("source root missing: {0}")]
    SourceRootMissing(PathBuf),
    /// Filesystem traversal or read failed.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// One or more compilation or parse errors. Aggregated.
    #[error("compile failed with {} error(s); first: {first}", count)]
    Compile {
        /// Total error count.
        count: usize,
        /// First error's stringified diagnostic.
        first: String,
    },
    /// Wire-format serialization failed.
    #[error("write_repo failed: {0}")]
    Write(String),
    /// `target` doesn't match any descriptor's `name`.
    #[error("target repo {0} not found in supplied descriptors")]
    UnknownTarget(String),
    /// Cycle detected in the descriptor dependency graph.
    #[error("dependency cycle: {0}")]
    Cycle(String),
    /// The production slice references an element that the test
    /// partition placed in the tests slice. Indicates a bug in the
    /// partition or the slicer; never expected from a well-formed model.
    #[error("dangling reference in production slice: {0}")]
    DanglingReference(String),
}

/// Compute the path of the companion tests blob alongside `output`.
///
/// Inserts a `.tests` segment before the final extension:
///   `foo.purem` → `foo.tests.purem`
///   `foo`       → `foo.tests`
///
/// Used internally by [`compile_to_purem`] and exposed so callers
/// (Embedder, CLI) can predict the artifact path without re-deriving the
/// rule.
#[must_use]
pub fn tests_output_path(output: &Path) -> PathBuf {
    let stem = output.file_stem().map(|s| s.to_os_string());
    let ext = output.extension().map(|s| s.to_os_string());
    let parent = output.parent();
    let new_name = match (stem, ext) {
        (Some(stem), Some(ext)) => {
            let mut name = stem;
            name.push(".tests.");
            name.push(&ext);
            name
        }
        (Some(stem), None) => {
            let mut name = stem;
            name.push(".tests");
            name
        }
        _ => return output.to_path_buf(),
    };
    match parent {
        Some(p) if !p.as_os_str().is_empty() => p.join(new_name),
        _ => PathBuf::from(new_name),
    }
}

/// Run a snapshot-build: load every descriptor, compile in dependency
/// order, partition the target repo into prod / tests slices, and write
/// both blobs to disk.
///
/// Two files are written every time:
/// - `<req.output>` — production slice (test code stripped).
/// - `tests_output_path(req.output)` — tests slice (only test code).
///
/// Together they cover every non-bootstrap element of the target repo
/// exactly once. Production binaries embed only the production blob;
/// development workflows load both via the snapshots dir.
pub fn compile_to_purem(req: CompileRequest<'_>) -> Result<(), BuildError> {
    let all_descriptors = load_descriptors(req.descriptors)?;
    if !all_descriptors.iter().any(|d| d.name == req.target) {
        return Err(BuildError::UnknownTarget(req.target.to_string()));
    }
    // Restrict to the target + its transitive deps so unrelated repos
    // (which may be passed in for build-time convenience) don't get
    // compiled. This makes a build of `platform.purem` independent of
    // a broken DSL repo's compile errors.
    let descriptors = transitively_reachable(&all_descriptors, req.target)?;
    let order = topo_sort(&descriptors)?;

    let auto_imports: Vec<SmolStr> = req.auto_imports.iter().copied().map(SmolStr::new).collect();

    let mut model = init_bootstrap_model();
    populate_repo_visibility(&mut model, &descriptors);
    populate_repo_patterns(&mut model, &descriptors);

    let mut target_range: Option<Range<u16>> = None;
    let mut errors: Vec<CompilationError> = Vec::new();

    for idx in order {
        let desc = &descriptors[idx];
        let source_files = parse_repo_sources(desc)?;
        let chunks_before = model.chunks.len();
        let (_range, slice_errs) =
            compile_repo_slice(&mut model, &source_files, &auto_imports, &[]);
        errors.extend(slice_errs);
        if desc.name == req.target {
            #[allow(clippy::cast_possible_truncation)]
            let start = chunks_before as u16;
            #[allow(clippy::cast_possible_truncation)]
            let end = model.chunks.len() as u16;
            target_range = Some(start..end);
        }
    }

    errors.extend(finalize_model(&mut model, &auto_imports, &[]));

    if !errors.is_empty() {
        return Err(BuildError::Compile {
            count: errors.len(),
            first: format!("{}", errors[0]),
        });
    }

    let range = target_range.expect("target was validated above");
    let partition = collect_test_partition(&model);

    let prod_slice = slice_by_repo_with_filter(&model, range.clone(), Some(&partition.prod));
    let test_slice = slice_by_repo_with_filter(&model, range, Some(&partition.test));

    assert_no_dangling_refs(&model, &prod_slice, &partition.test)
        .map_err(|e| BuildError::DanglingReference(e.to_string()))?;

    let prod_bytes = write_repo(&prod_slice).map_err(|e| BuildError::Write(format!("{e}")))?;
    let test_bytes = write_repo(&test_slice).map_err(|e| BuildError::Write(format!("{e}")))?;

    if let Some(parent) = req.output.parent() {
        fs::create_dir_all(parent).map_err(|source| BuildError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(req.output, &prod_bytes).map_err(|source| BuildError::Io {
        path: req.output.to_path_buf(),
        source,
    })?;
    let tests_path = tests_output_path(req.output);
    fs::write(&tests_path, &test_bytes).map_err(|source| BuildError::Io {
        path: tests_path,
        source,
    })?;

    Ok(())
}

#[derive(Debug, Deserialize)]
struct DescriptorJson {
    name: String,
    pattern: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
struct LoadedDescriptor {
    name: String,
    pattern: String,
    compiled_pattern: regex::Regex,
    dependencies: Vec<String>,
    source_root: PathBuf,
}

const M3_BOOTSTRAP_CANONICAL: &str = "/platform/pure/grammar/m3.pure";

fn load_descriptors(paths: &[PathBuf]) -> Result<Vec<LoadedDescriptor>, BuildError> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let bytes = fs::read(p).map_err(|source| BuildError::Io {
            path: p.clone(),
            source,
        })?;
        let parsed: DescriptorJson =
            serde_json::from_slice(&bytes).map_err(|e| BuildError::Descriptor {
                path: p.clone(),
                source: Box::new(e),
            })?;
        let parent = p.parent().ok_or_else(|| BuildError::Descriptor {
            path: p.clone(),
            source: "descriptor has no parent directory".into(),
        })?;
        let source_root = parent.join(&parsed.name);
        if !source_root.is_dir() {
            return Err(BuildError::SourceRootMissing(source_root));
        }
        let compiled_pattern =
            compile_repo_pattern(&parsed.pattern).map_err(|e| BuildError::Descriptor {
                path: p.clone(),
                source: format!("invalid pattern regex {:?}: {e}", parsed.pattern).into(),
            })?;
        out.push(LoadedDescriptor {
            name: parsed.name,
            pattern: parsed.pattern,
            compiled_pattern,
            dependencies: parsed.dependencies,
            source_root,
        });
    }
    Ok(out)
}

/// Return the subset of `all` that's transitively reachable from
/// `target` via the `dependencies` edges, including `target` itself.
/// Any dep name that isn't in `all` is silently ignored — Java's
/// resolver does the same and falls back to "external/unknown."
fn transitively_reachable(
    all: &[LoadedDescriptor],
    target: &str,
) -> Result<Vec<LoadedDescriptor>, BuildError> {
    let by_name: HashMap<&str, &LoadedDescriptor> =
        all.iter().map(|d| (d.name.as_str(), d)).collect();
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut stack = vec![target.to_string()];
    while let Some(name) = stack.pop() {
        if !keep.insert(name.clone()) {
            continue;
        }
        if let Some(d) = by_name.get(name.as_str()) {
            for dep in &d.dependencies {
                if !keep.contains(dep) {
                    stack.push(dep.clone());
                }
            }
        }
    }
    let mut out: Vec<LoadedDescriptor> = all
        .iter()
        .filter(|d| keep.contains(&d.name))
        .cloned()
        .collect();
    // Stable order by name for reproducibility — topo sort runs again
    // before compile and gives the actual load order.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn topo_sort(descs: &[LoadedDescriptor]) -> Result<Vec<usize>, BuildError> {
    let name_to_idx: HashMap<&str, usize> = descs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.name.as_str(), i))
        .collect();

    // Kahn's algorithm with stable tie-breaking by name.
    let mut indegree = vec![0usize; descs.len()];
    for d in descs {
        for dep in &d.dependencies {
            if let Some(&dep_idx) = name_to_idx.get(dep.as_str()) {
                let _ = dep_idx; // dep is upstream of d
                indegree[name_to_idx[d.name.as_str()]] += 1;
            }
        }
    }

    let mut ready: BTreeSet<&str> = descs
        .iter()
        .filter(|d| {
            d.dependencies
                .iter()
                .all(|dep| !name_to_idx.contains_key(dep.as_str()))
        })
        .map(|d| d.name.as_str())
        .collect();

    let mut order = Vec::with_capacity(descs.len());
    while let Some(name) = ready.iter().next().copied() {
        ready.remove(name);
        let idx = name_to_idx[name];
        order.push(idx);
        for d in descs {
            if d.dependencies.iter().any(|dep| dep == &descs[idx].name) {
                let d_idx = name_to_idx[d.name.as_str()];
                indegree[d_idx] -= 1;
                if indegree[d_idx] == 0 {
                    ready.insert(d.name.as_str());
                }
            }
        }
    }

    if order.len() != descs.len() {
        let unresolved: Vec<&str> = descs
            .iter()
            .enumerate()
            .filter(|(i, _)| !order.contains(i))
            .map(|(_, d)| d.name.as_str())
            .collect();
        return Err(BuildError::Cycle(unresolved.join(", ")));
    }

    Ok(order)
}

fn populate_repo_visibility(model: &mut PureModel, descs: &[LoadedDescriptor]) {
    for d in descs {
        let mut visible: BTreeSet<SmolStr> = BTreeSet::new();
        visible.insert(SmolStr::new(&d.name));
        for dep in &d.dependencies {
            visible.insert(SmolStr::new(dep));
        }
        model.repo_visibility.insert(SmolStr::new(&d.name), visible);
    }
}

fn populate_repo_patterns(model: &mut PureModel, descs: &[LoadedDescriptor]) {
    for d in descs {
        model.repo_patterns.insert(
            SmolStr::new(&d.name),
            RepoPattern {
                source: SmolStr::new(&d.pattern),
                compiled: d.compiled_pattern.clone(),
            },
        );
    }
}

fn parse_repo_sources(desc: &LoadedDescriptor) -> Result<Vec<SourceFile>, BuildError> {
    let prefix = format!("/{}", desc.name);
    let m3_skip = desc.name == "platform";
    let mut parsed = Vec::new();
    for entry in walkdir::WalkDir::new(&desc.source_root).sort_by_file_name() {
        let entry = entry.map_err(|e| BuildError::Io {
            path: desc.source_root.clone(),
            source: e.into(),
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "pure" {
            continue;
        }
        let rel = path
            .strip_prefix(&desc.source_root)
            .map_err(|e| BuildError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::other(e.to_string()),
            })?;
        let rel_str = rel
            .to_str()
            .ok_or_else(|| BuildError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::other("non-UTF-8 path"),
            })?
            .replace('\\', "/");
        let canonical = format!("{prefix}/{rel_str}");
        if m3_skip && canonical == M3_BOOTSTRAP_CANONICAL {
            continue;
        }
        let content = fs::read_to_string(path).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let canonical_smol = SmolStr::new(&canonical);
        match legend_pure_parser_parser::parse_with_islands(
            &content,
            canonical_smol.as_str(),
            legend_pure_dsl_graph::parser::default_island_parsers(),
        ) {
            Ok(sf) => parsed.push(sf),
            Err(partial) => {
                // Surface parse errors as compile errors so the caller's
                // outer aggregator catches them. We prepend a synthetic
                // error so the diagnostic path uniformly funnels through
                // BuildError::Compile.
                parsed.push(partial.source_file);
                if let Some(first) = partial.errors.first() {
                    return Err(BuildError::Compile {
                        count: partial.errors.len(),
                        first: format!(
                            "{}: {}",
                            first
                                .source_info()
                                .map(|si| format!(
                                    "{}:{}:{}",
                                    si.source, si.start_line, si.start_column
                                ))
                                .unwrap_or_else(|| canonical.clone()),
                            first.message()
                        ),
                    });
                }
            }
        }
    }
    Ok(parsed)
}
