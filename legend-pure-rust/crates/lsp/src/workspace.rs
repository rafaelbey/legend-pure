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

//! Workspace state: open buffers, base repos, last compiled model.
//!
//! [`Workspace::compile`] is the per-recompile entry point. It clones
//! the base [`Repo`] list, patches in-flight buffer content into any
//! matching [`Repo::Filesystem`] file (URL-tail match against canonical
//! paths), and calls [`legend_pure_core_platform::repo::load`] to
//! produce a fresh [`PureModel`] (or [`PartialPureModel`]).
//!
//! `.purem` repos are cloned by `Arc<[u8]>` reference and never
//! recompiled — a key cost-of-edit constraint that keeps tier-1 LSP
//! latency manageable for downstream projects depending on
//! `platform.purem`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use legend_pure_core_platform::repo::{self, Repo, RepoParseResult};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_store::compiler::RelationStoreExtension;
use legend_pure_ide::{ReferenceIndex, build_reference_index};
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_parser::SectionParser;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline;
use smol_str::SmolStr;
use tower_lsp_server::ls_types::Uri;

/// Tracked state for an LSP workspace.
pub struct Workspace {
    /// Repos as configured at startup. Compile-time clones receive
    /// open-buffer overlays; this baseline is preserved for re-runs.
    pub base_repos: Vec<Repo>,
    /// Open buffers keyed by client URL.
    pub open_buffers: HashMap<Uri, String>,
    /// Last compiled model, retained between requests so that hover /
    /// goto / outline can serve answers without recompiling on every
    /// keypress.
    pub model: Option<Arc<PureModel>>,
    /// Reference index built alongside `model` after each compile.
    /// Carries every clickable source span and its target — drives
    /// goto-def, find-references, hover, and the future
    /// semanticTokens response. Rebuilt per compile because source
    /// spans move when the user edits.
    pub references: Option<Arc<ReferenceIndex>>,
    /// Diagnostics from the most recent compile, keyed by canonical
    /// source path (as the compiler sees it). The handler turns these
    /// into per-URL `publishDiagnostics`.
    pub diagnostics: HashMap<String, Vec<CompilationError>>,
    /// Auto-import packages forwarded to every compile.
    pub auto_imports: Vec<SmolStr>,

    // ---- Incremental recompile cache (T-20260513-01 Phase 1) ----
    /// Parsed-AST cache keyed by canonical source path. Populated on
    /// every successful compile (full or incremental); read by the
    /// incremental compile path to skip re-parsing untouched files.
    parsed_files: HashMap<SmolStr, SourceFile>,
    /// Canonical source path → chunk id, derived from `model.chunks`
    /// post-compile. Used to map dirty files to the chunks they live in.
    path_to_chunk: HashMap<SmolStr, u16>,
    /// Inverse of `path_to_chunk`: chunk id → all canonical paths
    /// inside it. Used when rebuilding a chunk to assemble its full
    /// source-file list (dirty + clean cached entries).
    chunk_files: HashMap<u16, Vec<SmolStr>>,
    /// Chunk dependency graph: `chunk_dependents[T]` = chunks that
    /// reference elements in chunk `T`. Computed from
    /// `references.by_target` post-compile. Drives the rerun closure.
    chunk_dependents: HashMap<u16, HashSet<u16>>,
    /// Canonical paths marked dirty by `did_change` since the last
    /// compile. Cleared every time `compile()` returns.
    pub dirty_files: HashSet<SmolStr>,
    /// The set of chunk ids that re-ran on the last compile, sorted
    /// ascending. `None` for the first (cold) compile. Exposed for
    /// integration tests asserting chunk-scoped behaviour.
    last_rerun_chunks: Option<Vec<u16>>,
    /// Number of files freshly parsed during the most-recent compile
    /// call. Reset to 0 at the start of each `compile()`. Exposed for
    /// integration tests asserting parse-cache behaviour.
    parse_count_since_last_compile: usize,
}

impl Workspace {
    /// Construct a fresh workspace from the configured repos.
    #[must_use]
    pub fn new(base_repos: Vec<Repo>, auto_imports: Vec<SmolStr>) -> Self {
        Self {
            base_repos,
            open_buffers: HashMap::new(),
            model: None,
            references: None,
            diagnostics: HashMap::new(),
            auto_imports,
            parsed_files: HashMap::new(),
            path_to_chunk: HashMap::new(),
            chunk_files: HashMap::new(),
            chunk_dependents: HashMap::new(),
            dirty_files: HashSet::new(),
            last_rerun_chunks: None,
            parse_count_since_last_compile: 0,
        }
    }

    /// Chunk ids that re-ran on the last compile. `None` after a cold
    /// (full) compile; `Some(sorted_vec)` after an incremental compile.
    /// Exposed for integration tests asserting chunk-scoped behaviour.
    #[must_use]
    pub fn last_rerun_chunks(&self) -> Option<&[u16]> {
        self.last_rerun_chunks.as_deref()
    }

    /// Number of files freshly parsed during the most-recent compile
    /// call. Includes both full-compile parsing (all source files) and
    /// incremental parsing (just dirty files). Exposed for integration
    /// tests asserting parse-cache behaviour.
    #[must_use]
    pub fn parse_count_since_last_compile(&self) -> usize {
        self.parse_count_since_last_compile
    }

    /// Record an open or modified buffer.
    pub fn set_open_buffer(&mut self, uri: Uri, content: String) {
        self.open_buffers.insert(uri, content);
    }

    /// Drop a closed buffer. After this, the next compile reads the
    /// file from disk again (Filesystem repo) or from the embedded
    /// blob (Purem repo).
    pub fn close_buffer(&mut self, uri: &Uri) {
        self.open_buffers.remove(uri);
    }

    /// Build a snapshot of [`Repo`]s with open-buffer content patched
    /// into matching `Filesystem` files.
    ///
    /// Match strategy: for each open buffer URL, take its `to_file_path`,
    /// and for every Filesystem repo file, override the content if the
    /// URL's filesystem path ends with the canonical path's tail
    /// (without the leading `/`). This handles the typical single-project
    /// workspace where the canonical path `/myproj/foo.pure` is the
    /// suffix of `/abs/path/myproj/foo.pure`.
    #[must_use]
    pub fn snapshot_repos(&self) -> Vec<Repo> {
        let mut out = self.base_repos.clone();
        if self.open_buffers.is_empty() {
            return out;
        }
        for (uri, content) in &self.open_buffers {
            let Some(disk_path) = uri.to_file_path() else {
                continue;
            };
            for repo in &mut out {
                if let Repo::Filesystem { files, .. } = repo {
                    for f in files.iter_mut() {
                        let canonical_tail = f.path.trim_start_matches('/');
                        if disk_path.ends_with(canonical_tail) {
                            f.content.clone_from(content);
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Recompile the workspace. Returns the result so the handler can
    /// publish diagnostics; also stores a fresh `Arc<PureModel>` and a
    /// per-source diagnostic index on `self`.
    ///
    /// Dispatch (T-20260513-01 Phase 1):
    /// - First compile (`self.model.is_none()`) → full path via
    ///   [`repo::load_from_parsed`]. The parsed ASTs are captured into
    ///   `self.parsed_files` so subsequent incremental compiles can
    ///   reuse them.
    /// - Subsequent compile with `dirty_files` all mapped to known
    ///   chunks → incremental path: re-parse only dirty files, then
    ///   call [`pipeline::compile_chunks_incremental`] on the rerun
    ///   closure.
    /// - Any other case (no dirty files, unseen path, missing prior
    ///   model) → full recompile.
    pub fn compile(&mut self) -> CompileOutcome {
        self.parse_count_since_last_compile = 0;

        let can_incremental = self.model.is_some()
            && !self.dirty_files.is_empty()
            && self
                .dirty_files
                .iter()
                .all(|p| self.path_to_chunk.contains_key(p));

        if can_incremental {
            self.incremental_compile()
        } else {
            self.full_compile()
        }
    }

    /// Cold path: re-parse everything, run the full pipeline, seed the
    /// incremental caches from the result.
    fn full_compile(&mut self) -> CompileOutcome {
        let snapshot = self.snapshot_repos();
        // Fresh DSL extensions per compile — each carries `RefCell`
        // state populated during `declare`/`define_*` and walked by
        // `walk_references`. Reusing across compiles would accumulate
        // stale entries unless every extension's `declare` started
        // with a clear, which today's implementations don't do.
        let mapping = MappingExtension::new();
        let relational = RelationalExtension::new();
        let store = RelationStoreExtension::new();
        let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];

        // Two-stage parse → compile so we can seed `parsed_files` with
        // the exact ASTs the compiler saw, sidestepping a re-parse on
        // the next did_change.
        let parsed = repo::parse_repos(&snapshot, &mut || -> Vec<Box<dyn SectionParser>> {
            vec![
                Box::new(MappingSectionParser::new()),
                Box::new(RelationalSectionParser),
            ]
        });
        self.parse_count_since_last_compile += parsed
            .iter()
            .filter_map(|r| r.source_files.as_ref().map(Vec::len))
            .sum::<usize>();
        let parsed_for_cache = clone_parsed_for_cache(&parsed);
        let result = repo::load_from_parsed(&snapshot, parsed, &self.auto_imports, &extensions);

        let (model, errors) = match result {
            Ok(m) => (m, Vec::<CompilationError>::new()),
            Err(partial) => (partial.model, partial.errors),
        };
        let mut by_source: HashMap<String, Vec<CompilationError>> = HashMap::new();
        for err in &errors {
            by_source
                .entry(err.source_info.source.to_string())
                .or_default()
                .push(err.clone());
        }
        self.diagnostics = by_source;
        let model_arc = Arc::new(model);
        // Reference index now comes from `build_reference_index` —
        // the discovered IdeExtension slice handles Mapping +
        // Relational refs automatically (Phase 2-FULL extracted them
        // from CompilerExtension::walk_references; Phase 2.5 moved
        // the trait + helpers into the dedicated legend-pure-ide
        // crate). The explicit CompilerExtension list `extensions`
        // is still held by the workspace for compile-time wiring;
        // dead weight here but harmless.
        let _ = &extensions;
        self.references = Some(Arc::new(build_reference_index(&model_arc)));
        self.model = Some(model_arc);

        // Seed the incremental caches from the compiled model + parsed inputs.
        self.parsed_files = parsed_for_cache;
        self.rebuild_path_and_chunk_indexes();
        self.rebuild_chunk_dependents();
        self.dirty_files.clear();
        self.last_rerun_chunks = None;

        CompileOutcome {
            error_count: errors.len(),
        }
    }

    /// Hot path: re-parse only dirty files, then run
    /// [`pipeline::compile_chunks_incremental`] on the rerun closure.
    fn incremental_compile(&mut self) -> CompileOutcome {
        let mapping = MappingExtension::new();
        let relational = RelationalExtension::new();
        let store = RelationStoreExtension::new();
        let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];

        // 1. Re-parse dirty files. Failures are collected as
        //    parse-diagnostics and the chunk is dropped from the rerun
        //    set, preserving its prior model state (the "transient
        //    keystroke parse error" case).
        let snapshot = self.snapshot_repos();
        let mut parse_errors: HashMap<SmolStr, Vec<CompilationError>> = HashMap::new();
        let mut failed_paths: HashSet<SmolStr> = HashSet::new();
        let dirty: Vec<SmolStr> = self.dirty_files.iter().cloned().collect();
        for canonical in &dirty {
            let Some(content) = read_content_for(&snapshot, canonical) else {
                failed_paths.insert(canonical.clone());
                continue;
            };
            match parse_one_file(&content, canonical.as_str()) {
                Ok(sf) => {
                    self.parsed_files.insert(canonical.clone(), sf);
                    self.parse_count_since_last_compile += 1;
                }
                Err(errs) => {
                    parse_errors.insert(canonical.clone(), errs);
                    failed_paths.insert(canonical.clone());
                }
            }
        }

        // 2. Compute dirty_chunks (skipping chunks whose only dirty
        //    file failed to parse — keep prior model entries intact),
        //    then BFS closure over chunk_dependents.
        let dirty_chunks: HashSet<u16> = self
            .dirty_files
            .iter()
            .filter(|p| !failed_paths.contains(*p))
            .filter_map(|p| self.path_to_chunk.get(p).copied())
            .collect();
        let rerun_set = closure(&dirty_chunks, &self.chunk_dependents);
        let rerun_order = topo_order_for_rerun(&rerun_set, &self.base_repos, &self.path_to_chunk);

        // 3. Build chunk_inputs by cloning each chunk's cached ASTs.
        let mut chunk_inputs: HashMap<u16, Vec<SourceFile>> = HashMap::new();
        for &chunk_id in &rerun_order {
            let files = self.chunk_files.get(&chunk_id).cloned().unwrap_or_default();
            let sources: Vec<SourceFile> = files
                .iter()
                .filter_map(|p| self.parsed_files.get(p).cloned())
                .collect();
            chunk_inputs.insert(chunk_id, sources);
        }

        // 4. Take ownership of the prior model. `try_unwrap` should
        //    succeed because the workspace mutex is held across
        //    compile + reference snapshot; if it doesn't, a request
        //    handler is racing with the compile — log and fall back
        //    to a full recompile to keep correctness.
        let Some(prior_arc) = self.model.take() else {
            return self.full_compile();
        };
        let prior_model = match Arc::try_unwrap(prior_arc) {
            Ok(m) => m,
            Err(arc) => {
                tracing::warn!(
                    "Workspace::incremental_compile: Arc<PureModel> had external strong refs; \
                     falling back to full recompile (handler racing with compile)"
                );
                self.model = Some(arc);
                return self.full_compile();
            }
        };

        let outcome = pipeline::compile_chunks_incremental(
            prior_model,
            chunk_inputs,
            rerun_order.clone(),
            &self.auto_imports,
            &extensions,
            &[],
        );

        // 5. Merge diagnostics: replace entries for canonical paths in
        //    rerun chunks; keep entries for clean chunks intact; layer
        //    parse-error diagnostics on top for the dirty files whose
        //    parse failed.
        for &chunk_id in &outcome.rerun_chunks {
            let Some(paths) = self.chunk_files.get(&chunk_id) else {
                continue;
            };
            for p in paths {
                self.diagnostics.remove(p.as_str());
            }
        }
        for err in &outcome.errors {
            let key = err.source_info.source.to_string();
            self.diagnostics.entry(key).or_default().push(err.clone());
        }
        for (canonical, errs) in parse_errors {
            self.diagnostics.insert(canonical.to_string(), errs);
        }

        // 6. Re-Arc the model, rebuild reference index whole-model
        //    (cheap relative to lowering), refresh path/chunk maps and
        //    chunk_dependents from the new model.
        let model_arc = Arc::new(outcome.model);
        // Reference index now comes from `build_reference_index` —
        // the discovered IdeExtension slice handles Mapping +
        // Relational refs automatically (Phase 2-FULL extracted them
        // from CompilerExtension::walk_references; Phase 2.5 moved
        // the trait + helpers into the dedicated legend-pure-ide
        // crate). The explicit CompilerExtension list `extensions`
        // is still held by the workspace for compile-time wiring;
        // dead weight here but harmless.
        let _ = &extensions;
        self.references = Some(Arc::new(build_reference_index(&model_arc)));
        self.model = Some(model_arc);
        self.rebuild_path_and_chunk_indexes();
        self.rebuild_chunk_dependents();
        self.dirty_files.clear();
        self.last_rerun_chunks = Some(outcome.rerun_chunks);

        // Error count returned to the caller mirrors the existing
        // contract: total errors stored in `self.diagnostics`.
        let total_errors: usize = self.diagnostics.values().map(Vec::len).sum();
        CompileOutcome {
            error_count: total_errors,
        }
    }

    /// Refresh `path_to_chunk` / `chunk_files` by scanning every
    /// non-bootstrap chunk's element nodes for their canonical source
    /// path. Single source of truth lives on the model.
    fn rebuild_path_and_chunk_indexes(&mut self) {
        self.path_to_chunk.clear();
        self.chunk_files.clear();
        let Some(model) = self.model.as_ref() else {
            return;
        };
        for chunk in model.chunks.iter().skip(1) {
            let chunk_id = chunk.chunk_id;
            // Deduplicate paths within the chunk (a file may declare
            // many elements; we want each canonical path once).
            let mut seen: HashSet<SmolStr> = HashSet::new();
            let mut paths: Vec<SmolStr> = Vec::new();
            for (_local_idx, node) in chunk.nodes.iter() {
                let path = SmolStr::new(node.source_info.source.as_str());
                if seen.insert(path.clone()) {
                    self.path_to_chunk.insert(path.clone(), chunk_id);
                    paths.push(path);
                }
            }
            self.chunk_files.insert(chunk_id, paths);
        }
    }

    /// Refresh `chunk_dependents` from `references.by_target`. A chunk
    /// `from` depends on chunk `tgt` (i.e. `chunk_dependents[tgt]`
    /// contains `from`) iff `from` references an element in `tgt`.
    fn rebuild_chunk_dependents(&mut self) {
        self.chunk_dependents.clear();
        let Some(refs) = self.references.as_ref() else {
            return;
        };
        for (target, locations) in &refs.by_target {
            let ElementId::InstanceId {
                chunk_id: tgt_chunk,
                ..
            } = *target
            else {
                continue;
            };
            for loc in locations {
                let Some(&from_chunk) = self.path_to_chunk.get(loc.canonical_path.as_str()) else {
                    continue;
                };
                if from_chunk != tgt_chunk {
                    self.chunk_dependents
                        .entry(tgt_chunk)
                        .or_default()
                        .insert(from_chunk);
                }
            }
        }
    }

    /// Map an open-buffer URL to the canonical source path the
    /// compiler uses for it. Returns `None` if the URL doesn't
    /// resolve to any known source path.
    #[must_use]
    pub fn canonical_path_for(&self, uri: &Uri) -> Option<String> {
        let disk_path = uri.to_file_path()?;
        for repo in &self.base_repos {
            if let Repo::Filesystem { files, .. } = repo {
                for f in files {
                    let tail = f.path.trim_start_matches('/');
                    if disk_path.ends_with(tail) {
                        return Some(f.path.clone());
                    }
                }
            }
        }
        None
    }

    /// Inverse of [`Self::canonical_path_for`]: given a canonical
    /// source path the compiler emits (e.g. `/user_proj/foo/bar.pure`),
    /// produce a `file://` URL the IDE can navigate to.
    ///
    /// Resolution order:
    /// 1. If an open buffer's URL maps to this canonical (via
    ///    [`Self::canonical_path_for`]), reuse its URL.
    /// 2. Walk [`Self::base_repos`] for a [`Repo::Filesystem`] whose
    ///    `prefix` matches the canonical's leading segment AND whose
    ///    `source_root` was populated. Reconstruct the on-disk path
    ///    as `source_root.join(canonical[prefix.len()..])`.
    ///
    /// Returns `None` for canonicals served from [`Repo::Embedded`] /
    /// [`Repo::Purem`] (no on-disk source) or filesystem repos built
    /// without a `source_root` (synthetic test fixtures).
    #[must_use]
    pub fn file_uri_for_canonical(&self, canonical: &str) -> Option<Uri> {
        for uri in self.open_buffers.keys() {
            if self.canonical_path_for(uri).as_deref() == Some(canonical) {
                return Some(uri.clone());
            }
        }
        for repo in &self.base_repos {
            let Repo::Filesystem {
                prefix,
                source_root,
                ..
            } = repo
            else {
                continue;
            };
            let Some(root) = source_root else { continue };
            let Some(rel) = canonical.strip_prefix(prefix.as_str()) else {
                continue;
            };
            // Strip the leading '/' so `join` treats `rel` as relative
            // — otherwise `join("/abs")` discards the root.
            let rel = rel.trim_start_matches('/');
            let disk = root.join(rel);
            if let Some(uri) = Uri::from_file_path(&disk) {
                return Some(uri);
            }
        }
        None
    }
}

/// Summary of a compile pass; full per-source diagnostics live on
/// [`Workspace::diagnostics`].
#[derive(Debug, Clone, Copy)]
pub struct CompileOutcome {
    /// Total number of errors produced.
    pub error_count: usize,
}

/// Clone the parse result's source files into a `HashMap` keyed by
/// canonical source path, for seeding `Workspace.parsed_files` after a
/// full compile.
fn clone_parsed_for_cache(parsed: &[RepoParseResult]) -> HashMap<SmolStr, SourceFile> {
    let mut out: HashMap<SmolStr, SourceFile> = HashMap::new();
    for repo_result in parsed {
        let Some(files) = repo_result.source_files.as_ref() else {
            continue;
        };
        for sf in files {
            let path = SmolStr::new(sf.source_info.source.as_str());
            out.insert(path, sf.clone());
        }
    }
    out
}

/// Re-parse a single file's content using the same parser + section
/// parser set as the full-compile path. Returns either the parsed
/// `SourceFile` or a list of `CompilationError`s attributed to the file.
fn parse_one_file(content: &str, canonical: &str) -> Result<SourceFile, Vec<CompilationError>> {
    use legend_pure_core_platform::repo::default_island_parsers;
    let section_parsers: Vec<Box<dyn SectionParser>> = vec![
        Box::new(MappingSectionParser::new()),
        Box::new(RelationalSectionParser),
    ];
    match legend_pure_parser_parser::parse_with_sections(
        content,
        canonical,
        default_island_parsers(),
        section_parsers,
    ) {
        Ok(sf) => Ok(sf),
        Err(partial) => {
            let mut errs = Vec::new();
            for e in partial.errors {
                let source_info = e.source_info().cloned().unwrap_or_else(|| {
                    legend_pure_parser_ast::SourceInfo::new(canonical, 0, 0, 0, 0)
                });
                errs.push(CompilationError {
                    message: e.message(),
                    source_info,
                    kind: CompilationErrorKind::ParseFailure {
                        source: SmolStr::new(canonical),
                    },
                });
            }
            Err(errs)
        }
    }
}

/// Read the most up-to-date content for a canonical path. Open-buffer
/// overrides win; otherwise read from the matching filesystem repo.
/// Returns `None` if the path can't be located.
fn read_content_for(snapshot: &[Repo], canonical: &str) -> Option<String> {
    for repo in snapshot {
        let Repo::Filesystem { files, .. } = repo else {
            continue;
        };
        for f in files {
            if f.path == canonical {
                return Some(f.content.clone());
            }
        }
    }
    None
}

/// BFS over `chunk_dependents` seeded with `dirty`. Returns the
/// closure including `dirty` itself; terminates on cycles via the
/// visited set.
fn closure(dirty: &HashSet<u16>, deps: &HashMap<u16, HashSet<u16>>) -> HashSet<u16> {
    let mut out: HashSet<u16> = dirty.iter().copied().collect();
    let mut q: Vec<u16> = out.iter().copied().collect();
    while let Some(c) = q.pop() {
        if let Some(succs) = deps.get(&c) {
            for &s in succs {
                if out.insert(s) {
                    q.push(s);
                }
            }
        }
    }
    out
}

/// Produce a deterministic topological-ish order for the rerun set:
/// repo-by-repo in `base_repos` declaration order. Each repo's chunks
/// have all their dependencies in earlier repos already compiled
/// (clean) or earlier in the same iteration; same ordering used by
/// `repo::load_from_parsed`. Falls back to numeric chunk_id order
/// for chunks whose owning repo can't be identified.
fn topo_order_for_rerun(
    rerun: &HashSet<u16>,
    base_repos: &[Repo],
    path_to_chunk: &HashMap<SmolStr, u16>,
) -> Vec<u16> {
    let mut ordered: Vec<u16> = Vec::new();
    let mut seen: HashSet<u16> = HashSet::new();

    for repo in base_repos {
        let prefix = match repo {
            Repo::Embedded { prefix, .. } => *prefix,
            Repo::Filesystem { prefix, .. } => prefix.as_str(),
            _ => continue,
        };
        // Collect distinct chunk ids whose canonical paths start with
        // this repo's prefix; insert into the ordered list once each.
        let mut repo_chunks: Vec<u16> = path_to_chunk
            .iter()
            .filter(|(path, _)| path.as_str().starts_with(prefix))
            .map(|(_, &c)| c)
            .filter(|c| rerun.contains(c) && !seen.contains(c))
            .collect();
        repo_chunks.sort_unstable();
        repo_chunks.dedup();
        for c in repo_chunks {
            if seen.insert(c) {
                ordered.push(c);
            }
        }
    }

    // Any rerun chunk not matched to a repo (defensive): append in
    // chunk_id order.
    let mut leftover: Vec<u16> = rerun
        .iter()
        .copied()
        .filter(|c| !seen.contains(c))
        .collect();
    leftover.sort_unstable();
    ordered.extend(leftover);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_core_platform::repo::OwnedSourceFile;
    use std::path::PathBuf;

    fn fs_repo_with_root(prefix: &str, source_root: PathBuf, files: Vec<&str>) -> Repo {
        let owned = files
            .into_iter()
            .map(|p| OwnedSourceFile {
                path: p.to_string(),
                content: String::new(),
            })
            .collect();
        Repo::Filesystem {
            prefix: prefix.to_string(),
            files: owned,
            meta: None,
            source_root: Some(source_root),
        }
    }

    #[test]
    fn file_uri_for_canonical_resolves_via_source_root() {
        // /tmp/proj/lib.pure is the on-disk file; canonical is /proj/lib.pure
        let repo = fs_repo_with_root(
            "/proj",
            PathBuf::from("/tmp/proj"),
            vec!["/proj/lib.pure", "/proj/sub/foo.pure"],
        );
        let ws = Workspace::new(vec![repo], Vec::new());

        let uri = ws
            .file_uri_for_canonical("/proj/lib.pure")
            .expect("filesystem repo with source_root must resolve");
        // The ls-types `Uri` type wraps `fluent_uri::Uri` whose
        // `scheme()` returns a typed `Scheme` (not a `&str`) and
        // whose `path()` returns a percent-encoded `EStr<Path>`.
        // Round-trip through `as_str()` to test against plain
        // strings — that's the on-wire form clients see anyway.
        let url_str = uri.as_str();
        assert!(url_str.starts_with("file://"), "got: {url_str}");
        assert!(url_str.ends_with("/tmp/proj/lib.pure"), "got: {url_str}");

        let nested = ws
            .file_uri_for_canonical("/proj/sub/foo.pure")
            .expect("nested canonical must resolve");
        assert!(nested.as_str().ends_with("/tmp/proj/sub/foo.pure"));
    }

    #[test]
    fn file_uri_for_canonical_returns_none_without_source_root() {
        // Synthetic test fixture: source_root is None.
        let repo = Repo::Filesystem {
            prefix: "/proj".to_string(),
            files: vec![OwnedSourceFile {
                path: "/proj/x.pure".into(),
                content: String::new(),
            }],
            meta: None,
            source_root: None,
        };
        let ws = Workspace::new(vec![repo], Vec::new());
        assert!(ws.file_uri_for_canonical("/proj/x.pure").is_none());
    }

    #[test]
    fn file_uri_for_canonical_returns_none_for_unknown_canonical() {
        let repo = fs_repo_with_root("/proj", PathBuf::from("/tmp/proj"), vec!["/proj/lib.pure"]);
        let ws = Workspace::new(vec![repo], Vec::new());
        // Canonical's prefix doesn't match any repo.
        assert!(ws.file_uri_for_canonical("/other/lib.pure").is_none());
    }

    #[test]
    fn file_uri_for_canonical_prefers_open_buffer_url() {
        // When a buffer is open for the matching canonical, that URL
        // is returned in preference to the synthesised disk URL.
        let repo = fs_repo_with_root("/proj", PathBuf::from("/tmp/proj"), vec!["/proj/lib.pure"]);
        let mut ws = Workspace::new(vec![repo], Vec::new());
        let buffer_uri = "file:///tmp/proj/lib.pure".parse::<Uri>().unwrap();
        ws.set_open_buffer(buffer_uri.clone(), "...".into());
        let resolved = ws.file_uri_for_canonical("/proj/lib.pure").unwrap();
        assert_eq!(resolved, buffer_uri);
    }
}
