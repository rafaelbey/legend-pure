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

//! Compiler pipeline: AST → `PureModel`.
//!
//! Orchestrates the multi-pass compilation of parsed AST source files into
//! a fully resolved [`PureModel`].
//!
//! # Pipeline Phases
//!
//! 1. **Declaration (Pass 1)** — Iterate all AST elements, assign `ElementId`s,
//!    allocate shells, build the global package tree.
//! 2. **Topological Sort (Pass 1.5)** — Build DAG from hard dependencies
//!    (supertypes), topologically sort via Kahn's algorithm.
//!    Cyclic inheritance = compilation error.
//! 3. **Definition (Pass 2)** — Hydrate shells in topological order,
//!    resolving soft dependencies to existing shells.
//! 4. **Type Inference (Pass 2.5)** — Bottom-up type inference for all
//!    function and qualified property bodies.
//! 5. **Freeze** — Call `rebuild_derived_indexes()`.
//! 6. **Validation (Pass 3)** — Read-only pass on the frozen model.

use std::collections::{HashMap, HashSet, VecDeque};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::element as ast;
use legend_pure_parser_ast::element::PackageableElement as _;
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_ast::source_info::Spanned;

use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::extension::{DeclareCtx, DefineCtx, ValidateCtx};
use crate::ids::ElementId;
use crate::model::{Element, ElementNode, ModelChunk, PureModel};
use crate::nodes::association::Association;
use crate::nodes::class::{self, Class};
use crate::nodes::enumeration::{EnumValue, Enumeration};
use crate::nodes::function::Function;
use crate::nodes::measure::Measure;
use crate::nodes::profile::Profile;
use crate::nodes::unit::Unit;
use crate::resolve::{self, ResolutionContext};
use crate::types::{Multiplicity, Parameter, PrimitiveType, TypeExpr};

/// A partial compilation result: the best-effort model plus accumulated errors.
///
/// Returned in the `Err` variant of [`compile()`] when errors occur. Unlike
/// discarding the model on failure, this allows callers to:
/// - **LSP**: show diagnostics alongside partial type info and navigation
/// - **Batch**: report all errors at once, not one at a time
/// - **Autofix**: inspect the partial model to suggest code fixes
#[derive(Debug)]
pub struct PartialPureModel {
    /// The compiled model (may be incomplete due to errors).
    pub model: PureModel,
    /// Compilation errors (guaranteed non-empty).
    pub errors: Vec<CompilationError>,
}

/// Compiles a list of parsed source files into a `PureModel`.
///
/// This is the main entry point for the compiler pipeline.
///
/// `auto_imports` provides the list of packages that are implicitly imported
/// in every section (e.g., `meta::pure::metamodel`, `meta::pure::profiles`).
/// The caller controls which packages are auto-imported.
///
/// **Extension discovery**: every [`crate::extension::CompilerExtension`]
/// registered via the [`crate::extension::COMPILER_EXTENSIONS`]
/// distributed slice is folded in automatically, topologically sorted
/// by [`crate::extension::CompilerExtension::depends_on`] declarations.
/// Tests that need to compose extensions by hand should call
/// [`compile_with_extensions`] instead.
///
/// # Errors
///
/// - `Ok(PureModel)` — compilation succeeded with zero errors
/// - `Err(PartialPureModel)` — errors occurred, but the model is still
///   available via [`PartialPureModel::model`] for diagnostics / LSP
///
/// # Panics
///
/// Panics at extension discovery if two `CompilerExtension`s share a
/// `name()`, an extension declares an unknown dependency, or the
/// dependency graph contains a cycle. See
/// [`crate::extension::discovered_compiler_extensions`].
#[allow(clippy::result_large_err)] // Ok(PureModel) is equally large — intentional API
pub fn compile(
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
) -> Result<PureModel, PartialPureModel> {
    let discovered = crate::extension::discovered_compiler_extensions();
    compile_with_extensions(source_files, auto_imports, &discovered)
}

/// Compiles with a slice of [`CompilerExtension`]s. Extensions plug
/// into each pipeline phase after the M3 work for that phase is done.
///
/// The 0-extension call is identical to [`compile`]. The intent of the
/// extension slot is M2 DSL support (Mapping, Diagram, Relational).
///
/// # Errors
/// Same `Result<PureModel, PartialPureModel>` shape as [`compile`].
#[allow(clippy::result_large_err)]
#[tracing::instrument(
    level = "info",
    name = "compile",
    skip_all,
    fields(n_source_files = source_files.len(), n_extensions = extensions.len()),
)]
pub fn compile_with_extensions(
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
) -> Result<PureModel, PartialPureModel> {
    compile_with_extensions_and_islands(source_files, auto_imports, extensions, &[])
}

/// Compiles with both [`CompilerExtension`]s and inline-island
/// lowerers ([`crate::island_lower::IslandLowerer`]).
///
/// Each DSL crate that owns an island grammar (today: `dsl-tds`)
/// supplies an `IslandLowerer` impl that runs at body-lowering time,
/// transforming the parsed island content into a synthetic AST
/// expression that the main lowerer recurses on.
///
/// # Errors
///
/// Same `Result<PureModel, PartialPureModel>` shape as [`compile`].
#[allow(clippy::result_large_err)]
#[tracing::instrument(
    level = "info",
    name = "compile_and_islands",
    skip_all,
    fields(
        n_source_files = source_files.len(),
        n_extensions = extensions.len(),
        n_island_lowerers = island_lowerers.len(),
    ),
)]
pub fn compile_with_extensions_and_islands(
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
) -> Result<PureModel, PartialPureModel> {
    let mut model = init_bootstrap_model();
    let mut errors = Vec::new();

    let (_chunks, slice_errors) = compile_repo_slice_with_islands(
        &mut model,
        source_files,
        auto_imports,
        extensions,
        island_lowerers,
    );
    errors.extend(slice_errors);

    errors.extend(finalize_model(&mut model, auto_imports, extensions));

    if errors.is_empty() {
        Ok(model)
    } else {
        Err(PartialPureModel { model, errors })
    }
}

/// Initialize a fresh `PureModel` with the bootstrap chunk + M3 metamodel.
///
/// This is the one-time setup step that must run before any
/// [`compile_repo_slice`] calls. Allocates Chunk 0 (primitives), registers
/// the M3 metamodel stubs, resolves M3 supertype strings to `ElementId`s,
/// and wires `Any`'s reflective properties.
///
/// Repo-driven loading should call this once, then call
/// [`compile_repo_slice`] for each repo's sources, then call
/// [`finalize_model`] once at the end.
#[must_use]
pub fn init_bootstrap_model() -> PureModel {
    let mut model = PureModel::new();

    // Chunk 0 — bootstrap primitives
    let (bootstrap_chunk, m3_registrations) = bootstrap::create_bootstrap_chunk(model.root_package);
    model.chunks.push(bootstrap_chunk);

    // Register bootstrap elements in the root package
    for local_idx in 0..model.chunks[0].nodes.len() {
        let eid = ElementId::InstanceId {
            chunk_id: 0,
            local_idx,
        };
        model.register_element(model.root_package, eid);
    }

    // Register M3 metamodel stubs in their canonical packages
    // (e.g., Function → meta::pure::metamodel::function::Function)
    bootstrap::register_m3_packages(&mut model, &m3_registrations);

    // Resolve M3 supertype strings to actual ElementIds.
    // After M3 parsing, supertypes are stored as TypeExpr::Generic("ClassName").
    // Now that all M3 elements are registered in their packages, we can resolve
    // them to TypeExpr::Named { element } for subtype checking.
    resolve_m3_supertypes(&mut model);

    // Inject `Any.classifierGenericType` and `Any.elementOverride`
    // properties — m3.pure declares them on `Any` but our `m3_parser`
    // intentionally defers parsing the dense slot expression into
    // typed properties. Adding the bare names + resolved types here
    // unblocks the platform `properties()` accumulator (concatenates
    // declared + association + inherited recursively up to Any), which
    // surveyor's `testProperties` asserts as size 7 — without these
    // two slots the count ran short.
    wire_any_reflective_properties(&mut model);

    model
}

/// Compile one repo's sources against an existing `PureModel` (which must
/// already contain the bootstrap chunk and any prior repos' chunks).
///
/// Runs Pass 1 (declaration) through Pass 2b' (class/association bodies) plus
/// every registered extension hook for those phases. Inference (Pass 2.5)
/// and validation (Pass 3) are deferred to [`finalize_model`] — they walk
/// the whole model and only need to run once after all repos have loaded.
///
/// Returns the range of newly-allocated chunk indices (`chunk_id` values
/// from `before` to `after`) plus any compilation errors. The chunk range
/// identifies "this repo's chunks" for later slice serialization.
///
/// Cross-repo references are resolved naturally: the resolver consults the
/// model's package tree, which already contains earlier repos' elements.
/// Such references are stored as ordinary `ElementId`s in memory; encoding
/// to FQN strings happens only at slice-serialization time (see Phase A's
/// `slice_by_repo`).
#[must_use]
#[allow(clippy::result_large_err)]
pub fn compile_repo_slice(
    model: &mut PureModel,
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
) -> (std::ops::Range<usize>, Vec<CompilationError>) {
    compile_repo_slice_with_islands(model, source_files, auto_imports, extensions, &[])
}

/// Like [`compile_repo_slice`], plus a slice of inline-island
/// lowerers ([`crate::island_lower::IslandLowerer`]) that body-pass
/// dispatch consults on `Expression::Island(_)`.
#[must_use]
#[allow(clippy::result_large_err)]
pub fn compile_repo_slice_with_islands(
    model: &mut PureModel,
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
) -> (std::ops::Range<usize>, Vec<CompilationError>) {
    let chunks_before = model.chunks.len();
    let mut errors = Vec::new();
    // Take the scope out of model so we can hold &mut model and
    // &mut scope simultaneously in ctx constructors. Restored at
    // function return so subsequent compile_repo_slice + finalize_model
    // calls on the same model see the same scope (declare → validate
    // flow across functions).
    let mut scope = std::mem::take(&mut model.compile_scope);

    // ---- Pass 1: Declaration ----
    let (declarations, unit_mappings) = pass_declare(source_files, model, &mut errors);

    // ---- Pass 1: Extension declare hooks ----
    // Extensions allocate shells for any DSL-specific element variants
    // they own. Runs after M3 declarations so extension hooks see a
    // fully-populated M3 package tree to anchor against.
    for ext in extensions {
        let mut ctx = DeclareCtx {
            source_files,
            model,
            auto_imports,
            errors: &mut errors,
            scope: Some(&mut scope),
        };
        ext.declare(&mut ctx);
    }

    // ---- Pass 1.5: Topological Sort ----
    let sorted = pass_topo_sort(&declarations, source_files, model, &mut errors);

    // ---- Pass 2a: Signatures & Non-Function Elements ----
    // Resolve everything EXCEPT function expression bodies.
    // After this pass, all function signatures (params, return types) are
    // available for type-based dispatch during body compilation.
    let (id_to_decl, mut import_scope_cache) = pass_define_signatures(
        &sorted,
        source_files,
        &declarations,
        &unit_mappings,
        auto_imports,
        model,
        &mut errors,
    );

    // ---- Pass 2a: Extension define_signatures hooks ----
    for ext in extensions {
        let mut ctx = DefineCtx {
            source_files,
            model,
            auto_imports,
            errors: &mut errors,
            scope: Some(&mut scope),
        };
        ext.define_signatures(&mut ctx);
    }

    // ---- Pass 2a': Milestoning Synthesis ----
    // After Pass 2a every property's `type_expr` is resolved to a class
    // `ElementId`, so we can detect milestoned target classes by stereotype.
    // Synthesize the date properties, `milestoning` slot, edge-point
    // properties, and qualified-property signatures here — *before* Pass 2b
    // body lowering — so dispatch and type inference see the augmented
    // property/QP surface. Bodies of the synthesized QPs are intentionally
    // empty in Phase A; populating them requires the date-context propagation
    // pass (Phase B) and the runtime `getAll(...)` natives.
    //
    // Java parity: `MilestoningClassProcessor.addMilestoningProperty` +
    // `MilestoningPropertyProcessor.process`.
    crate::milestoning::synthesis::synthesize(model, &mut errors);

    // ---- Pass 2b: Function Bodies + Class/Assoc/Primitive bodies ----
    // Compile expression bodies using fully-resolved function signatures.
    // Order within Pass 2b doesn't matter — every signature in the model
    // (including all native function return types) is now hydrated, so any
    // body-shape expression sees the real types when resolving overloads.
    pass_define_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        auto_imports,
        model,
        island_lowerers,
        &mut errors,
    );

    // ---- Pass 2b': Class / Association / Primitive bodies ----
    // Constraint expressions, qualified-property bodies, and property
    // default values are body-shape: they call functions whose return
    // types must be known to dispatch operators correctly. They're
    // lowered here, *after* every function/native signature is hydrated,
    // so e.g. `'literal' + $x->toString()` inside a Primitive constraint
    // narrows `+` to the String overload via toString's real `:String[1]`
    // return type instead of the Pass 1 `Any` placeholder.
    pass_define_class_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        auto_imports,
        model,
        island_lowerers,
        &mut errors,
    );

    // ---- Pass 2b: Extension define_bodies hooks ----
    for ext in extensions {
        let mut ctx = DefineCtx {
            source_files,
            model,
            auto_imports,
            errors: &mut errors,
            scope: Some(&mut scope),
        };
        ext.define_bodies(&mut ctx);
    }

    // Rebuild derived indexes so a subsequent compile_repo_slice call sees
    // association-injected properties + specializations from this slice.
    // Cheap (O(N) over the new chunks) and keeps the resolver/property
    // lookup correct across the chain of repo compilations.
    model.rebuild_derived_indexes();

    // Restore the scope so the next compile_repo_slice or finalize_model
    // sees the accumulated per-compile state.
    model.compile_scope = scope;

    let chunks_after = model.chunks.len();
    (chunks_before..chunks_after, errors)
}

/// Final pass: type inference (Pass 2.5) + validation (Pass 3) over the
/// whole model.
///
/// Call once after all [`compile_repo_slice`] invocations have completed.
/// Walks every element regardless of which repo it came from — older repos'
/// elements are already valid but inference still needs a single pass to
/// populate `expr.type_info` for newly-added expressions.
#[must_use]
pub fn finalize_model(
    model: &mut PureModel,
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    // Defensive rebuild: cheap and ensures finalize is robust to callers
    // who skipped intermediate compile_repo_slice rebuilds.
    model.rebuild_derived_indexes();

    // ---- Pass 2.5: Type Inference ----
    pass_infer(model, &mut errors, None);

    // ---- Pass 2.5b: Milestoning Date Propagation ----
    // Rewrites `$x.address` (where address is a milestoned-target
    // property on Phase A's `original_milestoned_properties` slot)
    // into `$x.address($contextDate)` based on the in-scope milestoning
    // date. Runs post-inference so the rewriter reads resolved
    // `type_info` directly; pre-validation so leftover dateless
    // milestoned-target calls can be surfaced as validator errors
    // (B-4.2 fires inside the pass itself).
    crate::milestoning::propagation::propagate_dates(model, &mut errors);

    // ---- Pass 2.5c: Milestoning `%latest` usage validation ----
    // B-4.1 (`%latest` only in milestoning context) and B-4.3
    // (`%latest` forbidden in `getAllVersionsInRange`) fire here over
    // the post-propagation expression tree. Runs separately from the
    // propagation pass so the validator stays a pure walk — no
    // rewriting — and errors surface in a stable order.
    crate::milestoning::validate::validate_latest_usage(model, &mut errors);

    // ---- Pass 3: Validation ----
    errors.extend(crate::validate::validate(model, None));

    // Take scope so it's accessible alongside the (now-immutable) model
    // borrow that ValidateCtx requires. Restored at end so cross-compile
    // callers see the same scope on subsequent compile_repo_slice calls.
    let scope = std::mem::take(&mut model.compile_scope);

    // ---- Pass 3: Extension validate hooks ----
    for ext in extensions {
        let mut ctx = ValidateCtx {
            model,
            auto_imports,
            errors: &mut errors,
            scope: Some(&scope),
        };
        ext.validate(&mut ctx);
    }

    model.compile_scope = scope;

    errors
}

/// Result of an incremental recompile: the mutated model, accumulated
/// errors across the rebuilt chunks, and the chunk-ids that re-ran
/// (sorted for test determinism).
#[derive(Debug)]
pub struct IncrementalOutcome {
    /// The updated model.
    pub model: PureModel,
    /// Errors collected across all reruns plus the finalize step.
    pub errors: Vec<CompilationError>,
    /// Chunk ids that actually re-ran, sorted ascending.
    pub rerun_chunks: Vec<u16>,
}

/// Recompile a subset of `model.chunks` in place from pre-parsed
/// source files. The LSP server uses this as its hot-path
/// `did_change` handler — see T-20260513-01.
///
/// Contract:
///
/// - `rerun_order` lists chunk ids in topological repo order (each
///   chunk's upstream chunks appear earlier in the slice). The caller
///   computes this from its `chunk_dependents` graph.
/// - `chunk_inputs[chunk_id]` is the **full** source-file list for
///   that chunk's repo — both freshly re-parsed files and AST-cache
///   hits. The chunk is rebuilt from scratch using exactly these
///   inputs.
/// - Every `chunk_id` in `rerun_order` must already exist in
///   `model.chunks` (incremental recompile never grows the chunks
///   vector — Pass 1 writes in place into the pinned slot).
/// - Bootstrap chunk 0 is never a rerun target.
///
/// Each rerun chunk is rebuilt by:
///
/// 1. Tombstoning prior `(chunk_id, _)` entries from every package's
///    `children_elements` list (keeps the package tree consistent
///    with the freshly re-declared element set).
/// 2. Running Pass 1 ([`pass_declare_into`]) → Pass 1.5 (topo) →
///    Pass 2a (signatures) → Pass 2b (function bodies) → Pass 2b'
///    (class bodies) — the same passes [`compile_repo_slice_with_islands`]
///    runs, scoped to one chunk's source files. Extension `declare` /
///    `define_signatures` / `define_bodies` hooks run scoped to that
///    chunk's inputs.
/// 3. Pinning `chunk_id` so clean chunks' `(chunk_id, local_idx)`
///    refs into other clean chunks survive. Refs from clean chunks
///    into rerun chunks are by construction within the rerun set
///    itself — the caller's closure over `chunk_dependents`
///    guarantees this.
///
/// After all rerun chunks are rebuilt, `rebuild_derived_indexes`
/// runs whole-model (cheap), then Pass 2.5 ([`pass_infer`]) and
/// Pass 3 ([`crate::validate::validate`]) run bounded to the rerun
/// set, and extension `validate` hooks run whole-model (their
/// `RefCell` scratch state is rebuilt per compile anyway).
///
/// `chunk_inputs` is consumed by-move so callers don't pay an extra
/// clone on the AST cache they're already holding.
#[must_use]
pub fn compile_chunks_incremental(
    mut model: PureModel,
    mut chunk_inputs: HashMap<u16, Vec<SourceFile>>,
    rerun_order: Vec<u16>,
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
) -> IncrementalOutcome {
    let mut errors: Vec<CompilationError> = Vec::new();
    let rerun_set: HashSet<u16> = rerun_order.iter().copied().collect();

    for &chunk_id in &rerun_order {
        debug_assert!(chunk_id != 0, "bootstrap chunk is never a rerun target");
        debug_assert!(
            (chunk_id as usize) < model.chunks.len(),
            "compile_chunks_incremental: rerun_order references chunk_id {chunk_id} \
             not present in model.chunks (len = {len})",
            len = model.chunks.len(),
        );
        let Some(source_files) = chunk_inputs.remove(&chunk_id) else {
            continue;
        };
        recompile_chunk_in_place(
            chunk_id,
            &source_files,
            auto_imports,
            extensions,
            island_lowerers,
            &mut model,
            &mut errors,
        );
    }

    // Finalize: defensive whole-model index rebuild + bounded inference
    // and validation. Extension validate hooks run whole-model because
    // their cross-element scratch state isn't chunk-partitioned.
    model.rebuild_derived_indexes();
    pass_infer(&mut model, &mut errors, Some(&rerun_set));
    errors.extend(crate::validate::validate(&model, Some(&rerun_set)));

    // Take scope so validate can read it alongside the &model borrow.
    let scope = std::mem::take(&mut model.compile_scope);
    for ext in extensions {
        let mut ctx = ValidateCtx {
            model: &model,
            auto_imports,
            errors: &mut errors,
            scope: Some(&scope),
        };
        ext.validate(&mut ctx);
    }
    model.compile_scope = scope;

    let mut sorted_rerun: Vec<u16> = rerun_set.into_iter().collect();
    sorted_rerun.sort_unstable();
    IncrementalOutcome {
        model,
        errors,
        rerun_chunks: sorted_rerun,
    }
}

/// Rebuild a single chunk in place. Mirrors the per-chunk passes of
/// [`compile_repo_slice_with_islands`] but writes into the existing
/// `chunk_id` slot rather than allocating a new chunk.
fn recompile_chunk_in_place(
    chunk_id: u16,
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
    extensions: &[&dyn crate::extension::CompilerExtension],
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Per-chunk recompile takes the scope out of model for the duration
    // of this chunk's passes (declare → define_*); it's restored at the
    // end so subsequent chunks and the outer compile_chunks_incremental
    // finalize see the accumulated extension state.
    let mut scope = std::mem::take(&mut model.compile_scope);
    // ---- Tombstone: drop (chunk_id, _) ids from every package's
    // children_elements. The package arena itself stays — entries are
    // re-registered by pass_declare_into as elements are re-allocated.
    for pkg_idx in 0..model.global_packages.len() {
        let pkg = model.global_packages.get_mut(pkg_idx);
        pkg.children_elements.retain(|eid| match eid {
            ElementId::InstanceId { chunk_id: c, .. } => *c != chunk_id,
            ElementId::Package(_) => true,
        });
    }
    // Reset the chunk slot so pass_declare_into's debug_assert sees the
    // slot exists. The new content is written in by pass_declare_inner.
    model.chunks[chunk_id as usize] = ModelChunk::new(chunk_id);

    // ---- Pass 1: Declaration (in place at chunk_id) ----
    let (declarations, unit_mappings) = pass_declare_into(chunk_id, source_files, model, errors);

    // ---- Pass 1: Extension declare hooks ----
    for ext in extensions {
        let mut ctx = DeclareCtx {
            source_files,
            model,
            auto_imports,
            errors,
            scope: Some(&mut scope),
        };
        ext.declare(&mut ctx);
    }

    // ---- Pass 1.5: Topological Sort (within this chunk) ----
    let sorted = pass_topo_sort(&declarations, source_files, model, errors);

    // ---- Pass 2a: Signatures & Non-Function Elements ----
    let (id_to_decl, mut import_scope_cache) = pass_define_signatures(
        &sorted,
        source_files,
        &declarations,
        &unit_mappings,
        auto_imports,
        model,
        errors,
    );

    // ---- Pass 2a: Extension define_signatures hooks ----
    for ext in extensions {
        let mut ctx = DefineCtx {
            source_files,
            model,
            auto_imports,
            errors,
            scope: Some(&mut scope),
        };
        ext.define_signatures(&mut ctx);
    }

    // ---- Pass 2a': Milestoning Synthesis ----
    // See `compile_repo_slice_with_islands` for the rationale; this is the
    // incremental-rerun mirror.
    crate::milestoning::synthesis::synthesize(model, errors);

    // ---- Pass 2b: Function Bodies ----
    pass_define_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        auto_imports,
        model,
        island_lowerers,
        errors,
    );

    // ---- Pass 2b': Class / Association / Primitive bodies ----
    pass_define_class_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        auto_imports,
        model,
        island_lowerers,
        errors,
    );

    // ---- Pass 2b: Extension define_bodies hooks ----
    for ext in extensions {
        let mut ctx = DefineCtx {
            source_files,
            model,
            auto_imports,
            errors,
            scope: Some(&mut scope),
        };
        ext.define_bodies(&mut ctx);
    }

    // Rebuild derived indexes so a subsequent rerun chunk in the same
    // call sees this chunk's freshly-registered associations and
    // specializations. Matches compile_repo_slice_with_islands.
    model.rebuild_derived_indexes();
}

/// Convenience macro for `compile()` with optional auto-imports.
///
/// ```ignore
/// // No auto-imports (equivalent to compile(files, &[]))
/// compile!(files);
///
/// // With auto-imports
/// compile!(files, &auto_imports);
/// ```
#[macro_export]
macro_rules! compile {
    ($source_files:expr) => {
        $crate::pipeline::compile($source_files, &[])
    };
    ($source_files:expr, $auto_imports:expr) => {
        $crate::pipeline::compile($source_files, $auto_imports)
    };
}

/// A single declared element, linking its AST source to its assigned ID.
#[derive(Debug, Clone)]
struct Declaration {
    /// Assigned element ID.
    id: ElementId,
    /// Index into the `source_files` array.
    file_idx: usize,
    /// Index of the section within the source file.
    section_idx: usize,
    /// Index of the element within the section.
    element_idx: usize,
}

/// Tracks unit `ElementId`s allocated for a measure during Pass 1.
#[derive(Debug, Clone)]
struct UnitMapping {
    /// The canonical unit's `ElementId`, if present.
    canonical: Option<ElementId>,
    /// Non-canonical unit `ElementId`s, in order.
    non_canonical: Vec<ElementId>,
}

/// Pass 1 — assigns `ElementId`s, allocates element shells, and builds the
/// package tree.
///
/// Returns a map from fully qualified name to declaration(s), and a map from
/// measure `ElementId` to its allocated unit `ElementId`s.
///
/// **Function overloads:** Pure allows multiple function definitions with the
/// same FQN (overloaded by parameter types). Each overload gets its own
/// `ElementId` and `Declaration`, stored in the `Vec`. Non-function elements
/// still produce `DuplicateElement` errors on collision.
/// Resolves M3 supertype strings (`TypeExpr::Generic`) to resolved `TypeExpr::Named`.
///
/// The M3 parser stores supertypes AND property types as
/// `TypeExpr::Generic("ClassName")` because forward references are common
/// in m3.pure. After all M3 elements are registered in their packages,
/// this pass resolves those names to actual `ElementId`s.
///
/// This enables:
/// - `is_subtype()` to walk the M3 type hierarchy correctly
///   (e.g., `Class <: Type <: PackageableElement <: Any`).
/// - Property-access type inference through inherited properties
///   (e.g., `.package` on a `Package` value — `package` is declared on
///   `PackageableElement` with type `Package[0..1]`, which before this
///   pass is stored as `Generic("Package")` and would widen to `Any`).
///
/// Names whose *declared* class type parameter includes the same identifier
/// (e.g. the M3 `Property` class has a type parameter `T`) remain generic —
/// those are real type variables.
/// Wire up `Any`'s two M3 reflective properties — `classifierGenericType:
/// GenericType[0..1]` and `elementOverride: ElementOverride[0..1]` —
/// after `m3_parser` has registered the rest of the metamodel.
///
/// The bootstrap allocates `Any` with `properties: vec![]` so the
/// canonical slot ID stays predictable; m3.pure declares these
/// properties via the dense `^Root.children[...]` syntax that
/// `m3_parser` currently skips. Filling them in here keeps the
/// reflective surface complete without re-parsing the bootstrap
/// region or growing `m3_parser` to handle the slot syntax.
///
/// Looks up `GenericType` and `ElementOverride` by their canonical
/// FQN and silently no-ops if either resolution fails (e.g. the
/// model was built without the M3 metamodel registered, as in some
/// micro-tests).
fn wire_any_reflective_properties(model: &mut PureModel) {
    use crate::bootstrap::{ANY_ID, BOOTSTRAP_CHUNK_ID};
    use crate::nodes::class::Property;
    use crate::types::{Multiplicity, TypeExpr};
    use legend_pure_parser_ast::SourceInfo;

    let synth = SourceInfo::new("<bootstrap-Any-properties>", 0, 0, 0, 0);
    let generic_type_id = model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("type"),
        SmolStr::new("generics"),
        SmolStr::new("GenericType"),
    ]);
    let element_override_id = model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("type"),
        SmolStr::new("ElementOverride"),
    ]);
    let Some(generic_type_id) = generic_type_id else {
        return;
    };
    let Some(element_override_id) = element_override_id else {
        return;
    };

    let chunk = &mut model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    let Element::Class(any_cls) = chunk.elements.get_mut(ANY_ID.local_idx()) else {
        return;
    };
    // Guard against double-injection if the pipeline runs twice (unit
    // tests, hot-reload scenarios) — bail when the slots are already
    // populated.
    if any_cls
        .properties
        .iter()
        .any(|p| p.name == "classifierGenericType" || p.name == "elementOverride")
    {
        return;
    }
    any_cls.properties.push(Property {
        name: SmolStr::new("classifierGenericType"),
        type_expr: TypeExpr::Named {
            element: generic_type_id,
            type_arguments: vec![],
            multiplicity_arguments: Vec::new(),
            value_arguments: vec![],
            source_info: None,
        },
        multiplicity: Multiplicity::ZeroOrOne,
        source_info: synth.clone(),
        aggregation: None,
        default_value: None,
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
    });
    any_cls.properties.push(Property {
        name: SmolStr::new("elementOverride"),
        type_expr: TypeExpr::Named {
            element: element_override_id,
            type_arguments: vec![],
            multiplicity_arguments: Vec::new(),
            value_arguments: vec![],
            source_info: None,
        },
        multiplicity: Multiplicity::ZeroOrOne,
        source_info: synth,
        aggregation: None,
        default_value: None,
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
    });
}

fn resolve_m3_supertypes(model: &mut PureModel) {
    use crate::bootstrap::BOOTSTRAP_CHUNK_ID;
    use crate::types::TypeExpr;

    // Collect all M3 element IDs from chunk 0
    let chunk = &model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    let m3_count = chunk.elements.len();

    // Build a name → ElementId lookup for M3 classes (same-chunk resolution)
    let mut name_to_id: HashMap<SmolStr, ElementId> = HashMap::new();
    for local_idx in 0..m3_count {
        let eid = ElementId::InstanceId {
            chunk_id: BOOTSTRAP_CHUNK_ID,
            local_idx,
        };
        let name = model.get_node(eid).name.clone();
        name_to_id.insert(name, eid);
    }

    // Resolve Generic → Named on supertypes AND properties/qualified-properties.
    // A Generic("X") remains generic only when "X" is one of the class's own
    // type parameters; otherwise, resolve it against the chunk-0 class set.
    let chunk = &mut model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    for local_idx in 0..m3_count {
        let element = chunk.elements.get_mut(local_idx);
        let Element::Class(c) = element else {
            continue;
        };
        let type_params: std::collections::HashSet<SmolStr> =
            c.type_parameters.iter().map(|tp| tp.name.clone()).collect();

        // Resolves M3 stub forms into their final TypeExpr shapes:
        //
        //  - `Generic(name)` where `name` is NOT a class type-parameter →
        //    look up the chunk-0 class set and rewrite to `Named { id }`.
        //  - `Named { ANY_ID, …, value_arguments: [String(rawType)] }` —
        //    the parametric-marker form `m3_parser::build_pack_typeexpr`
        //    produces when a property type carries `<type_args | mult_args>`.
        //    Rewrite element to the resolved class id, drop the marker
        //    String, and recurse into the inner type/mult args.
        //
        // Recursive (closure-style) so nested cases like
        // `Class.properties: Property<Class<T>, Any>[*]` resolve all the
        // way down. Implemented via a fn so it can call itself.
        #[allow(clippy::items_after_statements)] // nested fn is intentional — it captures no caller state and recurses; lifting it out separates the recursive routine from its only caller
        fn resolve_in_place(
            ty: &mut TypeExpr,
            name_to_id: &HashMap<SmolStr, ElementId>,
            type_params: &std::collections::HashSet<SmolStr>,
        ) {
            match ty {
                TypeExpr::Generic(name) if !type_params.contains(name) => {
                    if let Some(&resolved_id) = name_to_id.get(name.as_str()) {
                        *ty = TypeExpr::Named {
                            element: resolved_id,
                            type_arguments: vec![],
                            multiplicity_arguments: Vec::new(),
                            value_arguments: vec![],
                            source_info: None,
                        };
                    }
                }
                TypeExpr::Named {
                    element,
                    type_arguments,
                    value_arguments,
                    ..
                } => {
                    // Detect the m3_parser sentinel marker: ANY_ID with
                    // a single `ConstValue::String(rawType)` in
                    // value_arguments. Rewrite the element to the
                    // resolved class id and clear the marker.
                    if *element == crate::bootstrap::ANY_ID
                        && value_arguments.len() == 1
                        && let crate::types::ConstValue::String(raw_name) = &value_arguments[0]
                        && let Some(&resolved_id) = name_to_id.get(raw_name.as_str())
                    {
                        *element = resolved_id;
                        value_arguments.clear();
                    }
                    // Recurse into inner type-arguments so `Property<Class<T>, Any>`
                    // resolves the nested `Class<T>` too.
                    for inner in type_arguments.iter_mut() {
                        resolve_in_place(inner, name_to_id, type_params);
                    }
                }
                _ => {}
            }
        }
        let resolve_in_place = |ty: &mut TypeExpr| {
            resolve_in_place(ty, &name_to_id, &type_params);
        };

        for st in &mut c.super_types {
            resolve_in_place(st);
        }
        for p in &mut c.properties {
            resolve_in_place(&mut p.type_expr);
        }
        for qp in &mut c.qualified_properties {
            resolve_in_place(&mut qp.return_type);
            // QP parameter list is `Arc<[Parameter]>`. The M3 metamodel
            // chunk has just been built by `m3_parser`/`bootstrap` and
            // not yet shared, so `Arc::get_mut` returns `Some`. If a
            // future change clones a Class/QP between construction and
            // here, the strong count exceeds 1 and we'd silently
            // deep-clone every QP's parameter list — defeating the
            // optimisation. Fail loudly instead.
            let Some(params_mut) = std::sync::Arc::get_mut(&mut qp.parameters) else {
                panic!(
                    "M3 QP parameters Arc<[Parameter]> must be uniquely \
                     owned at resolve time; refcount is \
                     {} (a Class/QP was cloned between m3_parser/bootstrap \
                     and resolve_m3_supertypes)",
                    std::sync::Arc::strong_count(&qp.parameters),
                );
            };
            for param in params_mut.iter_mut() {
                resolve_in_place(&mut param.type_expr);
            }
        }
    }

    // Pure semantics: every class implicitly extends `Any` unless it
    // declares an explicit generalization. m3.pure has classes like
    // `Referenceable`, `ProtocolInfo`, `AggregationKind` etc. with no
    // `generalizations` slot — Java's compiler treats them as
    // extending `Any` automatically. Without this, our supertype
    // walk from `Function → Referenceable → ?` stops short and
    // `Function.classifierGenericType` (declared on `Any`) becomes
    // unfindable. Inject `Any` as the default supertype here.
    let any_id = crate::bootstrap::ANY_ID;
    let chunk = &mut model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    for local_idx in 0..m3_count {
        let element = chunk.elements.get_mut(local_idx);
        let Element::Class(c) = element else {
            continue;
        };
        let eid = ElementId::InstanceId {
            chunk_id: BOOTSTRAP_CHUNK_ID,
            local_idx,
        };
        if eid == any_id {
            continue;
        }
        if c.super_types.is_empty() {
            c.super_types.push(TypeExpr::Named {
                element: any_id,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            });
        }
    }
}

#[tracing::instrument(
    level = "info",
    name = "pass_declare",
    skip_all,
    fields(n_source_files = source_files.len()),
)]
fn pass_declare(
    source_files: &[SourceFile],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<SmolStr, Vec<Declaration>>,
    HashMap<ElementId, UnitMapping>,
) {
    #[allow(clippy::cast_possible_truncation)] // chunks.len() is bounded by u16 in practice
    let chunk_id = model.chunks.len() as u16;
    pass_declare_inner(chunk_id, false, source_files, model, errors)
}

/// Pass 1, scoped to a specific `chunk_id` that **already exists** in
/// `model.chunks`. Used by the incremental recompile path
/// ([`compile_chunks_incremental`]) to rebuild a chunk in place
/// without growing the chunk vector.
///
/// Callers must tombstone any prior `children_elements` entries
/// pointing into `chunk_id` from `model.global_packages` before
/// invoking this — otherwise the package tree retains stale refs.
fn pass_declare_into(
    chunk_id: u16,
    source_files: &[SourceFile],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<SmolStr, Vec<Declaration>>,
    HashMap<ElementId, UnitMapping>,
) {
    debug_assert!(
        (chunk_id as usize) < model.chunks.len(),
        "pass_declare_into requires the chunk slot to already exist"
    );
    pass_declare_inner(chunk_id, true, source_files, model, errors)
}

fn pass_declare_inner(
    chunk_id: u16,
    in_place: bool,
    source_files: &[SourceFile],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<SmolStr, Vec<Declaration>>,
    HashMap<ElementId, UnitMapping>,
) {
    let mut declarations: HashMap<SmolStr, Vec<Declaration>> = HashMap::new();
    let mut unit_mappings: HashMap<ElementId, UnitMapping> = HashMap::new();
    let mut chunk = ModelChunk::new(chunk_id);

    for (file_idx, source_file) in source_files.iter().enumerate() {
        for (section_idx, section) in source_file.sections.iter().enumerate() {
            for (element_idx, element) in section.elements.iter().enumerate() {
                // DSL-defined elements are declared by their owning
                // CompilerExtension's `declare` hook (see
                // `compile_with_extensions`). Skip here so the M3
                // shell-allocation path doesn't run on them.
                if matches!(element, ast::Element::DSLElement(_)) {
                    continue;
                }

                let simple_name = ast_element_name(element);
                let source_info = ast_element_source(element);
                let name_source_info = ast_element_name_source(element);

                // Resolve package path
                let pkg_path = ast_element_package_path(element);
                let package_id = if pkg_path.is_empty() {
                    model.root_package
                } else {
                    model.get_or_create_package(&pkg_path)
                };

                // For functions, compute the mangled name at declaration time.
                // This is the element name (like a class name is its element name).
                let element_name = match element {
                    ast::Element::Function(f) => {
                        use legend_pure_parser_ast::element::FunctionSignature;
                        SmolStr::new(f.mangled_name())
                    }
                    ast::Element::NativeFunction(f) => {
                        use legend_pure_parser_ast::element::FunctionSignature;
                        SmolStr::new(f.mangled_name())
                    }
                    _ => simple_name.clone(),
                };

                // Build fully qualified name
                let fqn = build_fqn(&pkg_path, &element_name);

                // Same mangled FQN ⇒ same signature ⇒ true duplicate.
                // Distinct overloads have distinct mangled names and never
                // collide here.
                if declarations.contains_key(&fqn) {
                    errors.push(CompilationError {
                        message: format!("Duplicate element: '{fqn}'"),
                        source_info: source_info.clone(),
                        kind: CompilationErrorKind::DuplicateElement { name: fqn.clone() },
                    });
                    continue;
                }

                // Allocate shell
                let shell = create_shell(element);
                let local_idx = chunk.alloc_element(
                    ElementNode {
                        name: element_name.clone(),
                        source_info: source_info.clone(),
                        name_source_info: name_source_info.clone(),
                        parent_package: package_id,
                    },
                    shell,
                );

                let id = ElementId::InstanceId {
                    chunk_id,
                    local_idx,
                };
                model.register_element(package_id, id);

                declarations
                    .entry(fqn.clone())
                    .or_default()
                    .push(Declaration {
                        id,
                        file_idx,
                        section_idx,
                        element_idx,
                    });

                // For Measures: allocate Unit shells now
                if let ast::Element::Measure(measure_def) = element {
                    let mapping = allocate_unit_shells(
                        measure_def,
                        id,
                        &fqn,
                        chunk_id,
                        package_id,
                        &mut chunk,
                        model,
                        &mut declarations,
                        file_idx,
                        section_idx,
                        element_idx,
                    );
                    unit_mappings.insert(id, mapping);
                }
            }
        }
    }

    // Push the chunk if there are *any* source files for this slice —
    // even if zero M3 elements were declared. DSL extensions allocate
    // their `Element::DSLInstance` rows into this same chunk via
    // `declare()`, and need it to exist (and to be the last chunk so
    // their `chunks.len() - 1` lookup hits it). A source file with
    // only `###Mapping` / `###Diagram` / `###Relational` sections
    // produces zero M3 declarations but is otherwise valid.
    //
    // An empty source list (no files at all — e.g. an empty repo in a
    // classpath) stays a no-op: nothing to allocate into the chunk
    // for, no chunk pushed.
    //
    // Incremental recompile (`in_place == true`) writes directly into
    // the existing slot rather than growing the chunks vector.
    if !source_files.is_empty() {
        if in_place {
            model.chunks[chunk_id as usize] = chunk;
        } else {
            model.chunks.push(chunk);
        }
    }
    (declarations, unit_mappings)
}

/// Allocates `Element::Unit` shells for each unit in a measure definition.
///
/// Units get their own `ElementId` so they can be referenced in type positions
/// (e.g., `prop: Kilogram[1]`). Each unit is registered in the same package
/// as the parent measure.
#[allow(clippy::too_many_arguments)]
fn allocate_unit_shells(
    measure_def: &ast::MeasureDef,
    measure_id: ElementId,
    measure_fqn: &str,
    chunk_id: u16,
    package_id: crate::ids::PackageId,
    chunk: &mut ModelChunk,
    model: &mut PureModel,
    declarations: &mut HashMap<SmolStr, Vec<Declaration>>,
    file_idx: usize,
    section_idx: usize,
    element_idx: usize,
) -> UnitMapping {
    let mut canonical = None;
    let mut non_canonical = Vec::new();

    // Helper to allocate a single unit
    let mut alloc_unit = |unit_def: &ast::UnitDef| -> ElementId {
        let unit_name = &unit_def.name;
        let unit_fqn = SmolStr::new(format!("{measure_fqn}~{unit_name}"));

        let unit_shell = Element::Unit(Unit {
            measure: measure_id,
            conversion_expression: None, // Hydrated in Pass 2
        });

        let local_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new(format!("{}~{unit_name}", measure_def.name.value)),
                source_info: unit_def.source_info.clone(),
                name_source_info: unit_def.source_info.clone(),
                parent_package: package_id,
            },
            unit_shell,
        );

        let unit_id = ElementId::InstanceId {
            chunk_id,
            local_idx,
        };
        model.register_element(package_id, unit_id);

        // SAFETY INVARIANT: These AST coordinates point to the parent Measure
        // element, not the unit itself. This is safe because `pass_define`
        // skips `Element::Unit` before looking up the AST. If that skip is
        // ever removed, units would be re-hydrated as duplicate Measures.
        declarations.entry(unit_fqn).or_default().push(Declaration {
            id: unit_id,
            file_idx,
            section_idx,
            element_idx,
        });

        unit_id
    };

    // Canonical unit
    if let Some(ref canon) = measure_def.canonical_unit {
        canonical = Some(alloc_unit(canon));
    }

    // Non-canonical units
    for unit_def in &measure_def.non_canonical_units {
        non_canonical.push(alloc_unit(unit_def));
    }

    UnitMapping {
        canonical,
        non_canonical,
    }
}

/// Pass 1.5 — builds a dependency DAG from supertypes and sorts via Kahn's algorithm.
///
/// Returns an ordered list of element IDs safe for definition.
#[tracing::instrument(
    level = "info",
    name = "pass_topo_sort",
    skip_all,
    fields(n_declarations = declarations.len()),
)]
fn pass_topo_sort(
    declarations: &HashMap<SmolStr, Vec<Declaration>>,
    source_files: &[SourceFile],
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) -> Vec<ElementId> {
    // Build adjacency list: id → list of hard dependencies (supertypes)
    let mut in_degree: HashMap<ElementId, usize> = HashMap::new();
    let mut dependents: HashMap<ElementId, Vec<ElementId>> = HashMap::new();

    for decls in declarations.values() {
        for decl in decls {
            in_degree.entry(decl.id).or_insert(0);
        }
    }

    for decls in declarations.values() {
        for decl in decls {
            let element = get_ast_element(source_files, decl);
            let hard_deps = extract_hard_dependencies(element, declarations, model);

            for dep_id in hard_deps {
                dependents.entry(dep_id).or_default().push(decl.id);
                *in_degree.entry(decl.id).or_insert(0) += 1;
            }
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<ElementId> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let total_decls: usize = declarations.values().map(std::vec::Vec::len).sum();
    let mut sorted = Vec::with_capacity(total_decls);

    while let Some(id) = queue.pop_front() {
        sorted.push(id);
        if let Some(deps) = dependents.get(&id) {
            for &dep_id in deps {
                let Some(deg) = in_degree.get_mut(&dep_id) else {
                    unreachable!("dep_id was inserted into in_degree during graph construction");
                };
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(dep_id);
                }
            }
        }
    }

    // Check for cycles
    if sorted.len() < total_decls {
        let cyclic: Vec<_> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(&id, _)| id)
            .collect();

        for id in &cyclic {
            let node = model.get_node(*id);
            errors.push(CompilationError {
                message: format!("Cyclic inheritance detected involving '{}'", node.name),
                source_info: node.source_info.clone(),
                kind: CompilationErrorKind::CyclicInheritance {
                    element_name: node.name.clone(),
                },
            });
        }
    }

    sorted
}

/// Extracts hard dependencies (supertypes) from an AST element.
fn extract_hard_dependencies(
    element: &ast::Element,
    declarations: &HashMap<SmolStr, Vec<Declaration>>,
    _model: &PureModel,
) -> Vec<ElementId> {
    match element {
        ast::Element::Class(class_def) => class_def
            .super_types
            .iter()
            .filter_map(|type_ref| {
                let fqn = SmolStr::new(type_ref.full_path());
                declarations
                    .get(&fqn)
                    .and_then(|ds| ds.first())
                    .map(|d| d.id)
            })
            .collect(),
        _ => vec![],
    }
}

/// Pass 2a — hydrates shells in topological order, resolving everything
/// EXCEPT function expression bodies. Returns the lookup maps and caches
/// for reuse in Pass 2b.
#[allow(clippy::type_complexity)]
#[tracing::instrument(
    level = "info",
    name = "pass_define_signatures",
    skip_all,
    fields(n_sorted = sorted.len()),
)]
fn pass_define_signatures<'a>(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    declarations: &'a HashMap<SmolStr, Vec<Declaration>>,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<ElementId, &'a Declaration>,
    HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
) {
    use crate::resolve::ImportScope;

    // Build reverse lookup: ElementId → Declaration
    let id_to_decl: HashMap<ElementId, &Declaration> = declarations
        .values()
        .flat_map(|ds| ds.iter())
        .map(|d| (d.id, d))
        .collect();

    // Cache per-section import scopes. Resolve caches are per-element
    // (allocated inside the loop) — sharing across elements is unsound
    // because results depend on `self_package` (T-20260510-01).
    let mut import_scope_cache: HashMap<(usize, usize), Vec<ImportScope>> = HashMap::new();

    for &id in sorted {
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        // Skip units — they were fully populated during Pass 1 (allocate_unit_shells)
        let chunk = &model.chunks[chunk_id as usize];
        if matches!(chunk.elements.get(local_idx), Element::Unit(_)) {
            continue;
        }

        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Build or retrieve the import scope for this element's section.
        // The cached `Vec` MUST stay immutable across elements — the
        // implicit self-package now lives on the per-element ctx
        // (`self_package`) rather than being pushed onto this scope
        // (T-20260510-01: pre-fix mutation leaked element A's package
        // into element B's resolution within the same section).
        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });

        // Per-element resolve cache. Pre-fix this was per-section, but
        // resolution results depend on `self_package` (which varies per
        // element); sharing across elements re-introduced the leak even
        // with the scope mutation removed.
        let mut resolve_cache: HashMap<SmolStr, crate::resolve::ResolveResult> = HashMap::new();

        // Extract type + multiplicity parameters from the AST element
        // (Class<T,V|m>, function<T|m>).
        let type_params = ast_type_parameters(ast_element);
        let mult_params = ast_multiplicity_parameters(ast_element);
        let self_pkg = ast_element.package();

        let mut ctx = ResolutionContext {
            model,
            import_scopes,
            resolve_cache: &mut resolve_cache,
            type_parameters: &type_params,
            multiplicity_parameters: &mult_params,
            self_package: self_pkg,
            variable_types: HashMap::new(),
            island_lowerers: &[],
        };

        // Hydrate everything EXCEPT function bodies (bodies resolved in Pass 2b)
        let hydrated = hydrate_element_signature(ast_element, id, unit_mappings, &mut ctx, errors);

        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        *chunk.elements.get_mut(local_idx) = hydrated;
    }

    (id_to_decl, import_scope_cache)
}

/// Pass 2b — compile function expression bodies.
///
/// At this point all function signatures (params, return types) are fully
/// resolved, enabling type-based dispatch in `resolve_function_call`.
///
/// Takes `&mut` references to the per-section import-scope cache so the
/// follow-up `pass_define_class_bodies` reuses the same populated scope.
/// Resolve caches are per-element (allocated inside the loop) — sharing
/// across elements is unsound because results depend on `self_package`.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    level = "info",
    name = "pass_define_bodies",
    skip_all,
    fields(n_sorted = sorted.len()),
)]
fn pass_define_bodies(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    id_to_decl: &HashMap<ElementId, &Declaration>,
    import_scope_cache: &mut HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
    errors: &mut Vec<CompilationError>,
) {
    for &id in sorted {
        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Only process functions with bodies
        let body_exprs = match ast_element {
            ast::Element::Function(f) if !f.body.is_empty() => &f.body,
            _ => continue,
        };

        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });

        // Per-element resolve cache (see Pass 2a note) — sharing across
        // elements is unsound when `self_package` varies.
        let mut resolve_cache: HashMap<SmolStr, crate::resolve::ResolveResult> = HashMap::new();

        let type_params = ast_type_parameters(ast_element);
        let mult_params = ast_multiplicity_parameters(ast_element);
        let self_pkg = ast_element.package();

        // Seed variable scope with the function's own resolved parameters
        let mut variable_types = HashMap::new();
        if let Element::Function(f) = model.get_element(id) {
            for param in f.parameters.iter() {
                variable_types.insert(
                    param.name.clone(),
                    (param.type_expr.clone(), param.multiplicity.clone()),
                );
            }
        }

        let mut ctx = ResolutionContext {
            model,
            import_scopes,
            resolve_cache: &mut resolve_cache,
            type_parameters: &type_params,
            multiplicity_parameters: &mult_params,
            self_package: self_pkg,
            variable_types,
            island_lowerers,
        };

        let body = crate::lower::lower_expression_body(body_exprs, &mut ctx, errors);

        // Patch the body into the already-resolved function
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        if let Element::Function(func) = chunk.elements.get_mut(local_idx) {
            func.body = body.into();
        }
    }
}

/// Pass 2b' — compile body-shape expressions on Class / Association /
/// Primitive elements: constraint functions and messages, qualified-property
/// bodies, and property default values.
///
/// These were left as placeholders by Pass 2a (`vec![]` for QP bodies and
/// constraint lists; `None` for default values) because their lowering
/// drives `resolve_function_call`, which needs every native function's
/// real return type to narrow operator overloads correctly. Lowering them
/// here — after both Pass 2a (signatures) and Pass 2b (function bodies) —
/// guarantees the model is fully populated before any class-side body
/// expression resolves a call.
///
/// Skips elements whose AST has no body-shape items, so the cost is
/// proportional to the number of constraints / QP bodies / default values
/// in the program, not the total element count.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    level = "info",
    name = "pass_define_class_bodies",
    skip_all,
    fields(n_sorted = sorted.len()),
)]
fn pass_define_class_bodies(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    id_to_decl: &HashMap<ElementId, &Declaration>,
    import_scope_cache: &mut HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    island_lowerers: &[Box<dyn crate::island_lower::IslandLowerer>],
    errors: &mut Vec<CompilationError>,
) {
    for &id in sorted {
        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Only Classes / Associations / Primitives carry deferred bodies.
        if !matches!(
            ast_element,
            ast::Element::Class(_) | ast::Element::Association(_) | ast::Element::Primitive(_)
        ) {
            continue;
        }

        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });

        // Per-element resolve cache (see Pass 2a note — sharing across
        // elements is unsound because results depend on `self_package`).
        let mut resolve_cache: HashMap<SmolStr, crate::resolve::ResolveResult> = HashMap::new();

        let type_params = ast_type_parameters(ast_element);
        let mult_params = ast_multiplicity_parameters(ast_element);
        let self_pkg = ast_element.package();

        // Per-element variable scope: seeded with type-variable parameters
        // from parametric Classes / Primitives so `$x` inside a constraint
        // resolves against the class's declared `(x:Integer[1])`.
        //
        // Also seed `$this` for Class / Association / Primitive bodies —
        // constraints, QP bodies, and property defaults all reference the
        // owning instance via `$this`. Without this, dispatch on
        // `$this.foo->bar()` chains can't infer the receiver type, and
        // generic-overloaded functions (e.g. `elementToPath`) collapse to
        // an `Any` arg and become ambiguous.
        let mut variable_types = HashMap::new();
        let this_type: Option<crate::types::TypeExpr> = match model.get_element(id) {
            Element::Class(c) => {
                for tvp in &c.type_variable_parameters {
                    variable_types.insert(
                        tvp.name.clone(),
                        (tvp.type_expr.clone(), tvp.multiplicity.clone()),
                    );
                }
                let type_arguments: Vec<crate::types::TypeExpr> = c
                    .type_parameters
                    .iter()
                    .map(|tp| crate::types::TypeExpr::Generic(tp.name.clone()))
                    .collect();
                let multiplicity_arguments: Vec<crate::types::Multiplicity> = c
                    .multiplicity_parameters
                    .iter()
                    .map(|name| crate::types::Multiplicity::Variable(name.clone()))
                    .collect();
                Some(crate::types::TypeExpr::Named {
                    element: id,
                    type_arguments,
                    multiplicity_arguments,
                    value_arguments: Vec::new(),
                    source_info: None,
                })
            }
            Element::Association(_) => Some(crate::types::TypeExpr::Named {
                element: id,
                type_arguments: Vec::new(),
                multiplicity_arguments: Vec::new(),
                value_arguments: Vec::new(),
                source_info: None,
            }),
            Element::PrimitiveType(p) => {
                for tvp in &p.type_variable_parameters {
                    variable_types.insert(
                        tvp.name.clone(),
                        (tvp.type_expr.clone(), tvp.multiplicity.clone()),
                    );
                }
                Some(crate::types::TypeExpr::Named {
                    element: id,
                    type_arguments: Vec::new(),
                    multiplicity_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                    source_info: None,
                })
            }
            _ => None,
        };
        if let Some(this_te) = this_type {
            variable_types.insert(
                SmolStr::new("this"),
                (this_te, crate::types::Multiplicity::PureOne),
            );
        }

        // Compute the owner's FQN once for any constraint diagnostics
        // emitted inline by `lower_constraints`. Cheap (joins
        // package-segment names with `::`) and only used by Class /
        // PrimitiveType arms.
        let owner_fqn: SmolStr =
            SmolStr::new(crate::purem::fqn_path::element_fqn_path(model, id).join("::"));

        // Lower body-shape items into owned locals. This temporarily
        // borrows `model` mutably via `ctx`; we drop ctx before patching.
        let (new_constraints, new_qp_bodies, new_default_values) = {
            let mut ctx = ResolutionContext {
                model,
                import_scopes,
                resolve_cache: &mut resolve_cache,
                type_parameters: &type_params,
                multiplicity_parameters: &mult_params,
                self_package: self_pkg,
                variable_types,
                island_lowerers,
            };
            match ast_element {
                ast::Element::Class(c) => (
                    lower_constraints(&c.constraints, &owner_fqn, &mut ctx, errors),
                    lower_qualified_property_bodies(&c.qualified_properties, &mut ctx, errors),
                    lower_property_default_values(&c.properties, &mut ctx, errors),
                ),
                ast::Element::Association(a) => (
                    Vec::new(),
                    lower_qualified_property_bodies(&a.qualified_properties, &mut ctx, errors),
                    lower_property_default_values(&a.properties, &mut ctx, errors),
                ),
                ast::Element::Primitive(p) => (
                    lower_constraints(&p.constraints, &owner_fqn, &mut ctx, errors),
                    Vec::new(),
                    Vec::new(),
                ),
                _ => unreachable!(),
            }
        };

        // Patch the lowered bodies into the model element.
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        match chunk.elements.get_mut(local_idx) {
            Element::Class(c) => {
                c.constraints = new_constraints;
                patch_qp_bodies(&mut c.qualified_properties, new_qp_bodies);
                patch_property_default_values(&mut c.properties, new_default_values);
            }
            Element::Association(a) => {
                patch_qp_bodies(&mut a.qualified_properties, new_qp_bodies);
                patch_property_default_values(&mut a.properties, new_default_values);
            }
            Element::PrimitiveType(p) => {
                p.constraints = new_constraints;
            }
            _ => {}
        }
    }
}

/// Patches lowered QP bodies into existing `QualifiedProperty` entries.
/// AST QP count may exceed model QP count if signature lowering filtered
/// some out (`return_type` didn't resolve), so we zip and drop any extras.
fn patch_qp_bodies(
    model_qps: &mut [class::QualifiedProperty],
    new_bodies: Vec<Vec<crate::types::ValueSpec>>,
) {
    for (qp, body) in model_qps.iter_mut().zip(new_bodies.into_iter()) {
        qp.body = body.into();
    }
}

/// Patches lowered default values into existing Property entries. Same
/// alignment caveat as `patch_qp_bodies`.
fn patch_property_default_values(
    model_props: &mut [class::Property],
    new_defaults: Vec<Option<crate::types::ValueSpec>>,
) {
    for (p, dv) in model_props.iter_mut().zip(new_defaults.into_iter()) {
        p.default_value = dv;
    }
}

/// Builds the import scope for a specific section.
///
/// Explicit imports use the AST `Package` directly (the parser already
/// built the recursive tree). Auto-imports arrive as path strings and
/// get parsed into `Package` once at startup via [`ImportScope::from_path_str`].
fn build_import_scope(
    source_files: &[SourceFile],
    file_idx: usize,
    section_idx: usize,
    auto_imports: &[SmolStr],
) -> Vec<crate::resolve::ImportScope> {
    use crate::resolve::ImportScope;

    let section = &source_files[file_idx].sections[section_idx];

    // Wrap the AST Package directly — zero conversion needed
    let mut scope: Vec<ImportScope> = section
        .imports
        .iter()
        .map(|import| ImportScope::from_package(import.path.clone()))
        .collect();

    // Append auto-imports (deduplicating by Display string)
    for auto in auto_imports {
        let auto_str = auto.as_str();
        if !scope.iter().any(|s| s.package.to_string() == auto_str) {
            scope.push(ImportScope::from_path_str(auto_str));
        }
    }

    scope
}

/// Creates an empty shell for an AST element.
#[allow(clippy::too_many_lines)] // dispatch table over every Element variant
fn create_shell(element: &ast::Element) -> Element {
    match element {
        // Pass-1 shells carry every *syntactic* field already known
        // from the AST — type/multiplicity-parameter arity, and
        // Profile stereotype/tag name lists. This is what lets the
        // resolver-eager checks (`resolve_type_ref`'s type-arg
        // completeness, `resolve_stereotypes`'s name-existence)
        // fire correctly during Pass 2a regardless of topological
        // hydration order: the *target* of the reference already
        // advertises its declared arity / name list at Pass 1.
        ast::Element::Class(c) => Element::Class(Class {
            type_parameters: c
                .type_parameters
                .iter()
                .map(|name| crate::nodes::class::TypeParameter::invariant(name.clone()))
                .collect(),
            multiplicity_parameters: c.multiplicity_parameters.clone(),
            type_variable_parameters: vec![],
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
            original_milestoned_properties: vec![],
        }),
        ast::Element::Enumeration(_) => Element::Enumeration(Enumeration {
            values: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }),
        ast::Element::Function(f) => {
            let placeholder_params: Vec<Parameter> = f
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: p.name.clone(),
                    type_expr: TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: vec![],
                        multiplicity_arguments: Vec::new(),
                        value_arguments: vec![],
                        source_info: None,
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: p.source_info.clone(),
                })
                .collect();
            Element::Function(Function {
                function_name: f.name.value.clone(),
                is_native: false,
                parameters: placeholder_params.into(),
                return_type: TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                },
                return_multiplicity: Multiplicity::PureOne,
                body: Vec::new().into(),
                stereotypes: vec![],
                tagged_values: vec![],
            })
        }
        ast::Element::NativeFunction(f) => {
            let placeholder_params: Vec<Parameter> = f
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: p.name.clone(),
                    type_expr: TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: vec![],
                        multiplicity_arguments: Vec::new(),
                        value_arguments: vec![],
                        source_info: None,
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: p.source_info.clone(),
                })
                .collect();
            Element::Function(Function {
                function_name: f.name.value.clone(),
                is_native: true,
                parameters: placeholder_params.into(),
                return_type: TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                },
                return_multiplicity: Multiplicity::PureOne,
                body: Vec::new().into(),
                stereotypes: vec![],
                tagged_values: vec![],
            })
        }
        ast::Element::Profile(p) => Element::Profile(Profile {
            stereotypes: p.stereotype_names.clone(),
            tags: p.tag_names.clone(),
        }),
        ast::Element::Association(_) => Element::Association(Association {
            properties: vec![],
            qualified_properties: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
            original_milestoned_properties: vec![],
        }),
        ast::Element::Measure(_) => Element::Measure(Measure {
            canonical_unit: None,
            non_canonical_units: vec![],
        }),
        ast::Element::Primitive(_) => Element::PrimitiveType(PrimitiveType {
            super_type: None,
            super_type_value_arguments: Vec::new(),
            type_variable_parameters: Vec::new(),
            constraints: Vec::new(),
        }),
        ast::Element::DSLElement(_) => {
            // Unreachable in practice — `pass_declare` skips
            // `ast::Element::DSLElement` so that DSL crates' own
            // `CompilerExtension::declare` hook can allocate shells
            // and IDs without M3's loop interfering. If this ever
            // fires it means the skip was lost.
            unreachable!(
                "DSL elements are handled by the owning CompilerExtension, \
                 not by the M3 pass_declare loop"
            )
        }
    }
}

/// Hydrates an AST element into its full Pure representation.
///
/// This resolves type references (properties, parameters, return types),
/// annotations (stereotypes, tagged values), and structural fields.
/// Expression bodies remain as placeholders — full expression lowering
/// is deferred to Phase 4+.
#[allow(clippy::too_many_lines)]
fn hydrate_element_signature(
    element: &ast::Element,
    element_id: ElementId,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Element {
    match element {
        ast::Element::Class(class_def) => {
            let class_name = class_def.name.value.clone();
            let class_si = class_def.source_info.clone();

            // Super types — resolve, then validate immediately.
            let super_types: Vec<TypeExpr> = class_def
                .super_types
                .iter()
                .filter_map(|type_ref| resolve::resolve_type_ref(type_ref, ctx, errors))
                .collect();
            crate::validate::validate_super_types(
                ctx.model,
                element_id,
                &class_name,
                &super_types,
                &class_si,
                errors,
            );

            // Properties — signatures only. Default-value bodies are lowered
            // in Pass 2b (`pass_define_class_bodies`) once all function
            // signatures are hydrated, so type-based dispatch in any
            // operator/function call inside a default value sees real
            // return types instead of Pass 1 placeholders.
            let properties = lower_property_signatures(&class_def.properties, ctx, errors);
            crate::validate::validate_duplicate_properties(&class_name, &properties, errors);

            // Qualified properties — signatures only; bodies deferred to
            // Pass 2b for the same reason.
            let qualified_properties =
                lower_qualified_property_signatures(&class_def.qualified_properties, ctx, errors);

            // Access-level Step A: `<<access.X>>` on a class
            // property / qualified property is rejected.
            crate::validate::validate_no_access_on_properties(
                ctx.model,
                element_id,
                &properties,
                &qualified_properties,
                errors,
            );

            // Constraints — fully deferred to Pass 2b. Constraint expressions
            // are body-shape: they call functions that may not yet have
            // hydrated signatures during Pass 2a topo-ordered hydration.
            let constraints = Vec::new();

            // Annotations — resolver folds in profile-kind + name
            // existence checks. After hydration, also flag multiple
            // `<<access.X>>` stereotypes on the class itself.
            let stereotypes = resolve::resolve_stereotypes(&class_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&class_def.tagged_values, ctx, errors);
            crate::validate::validate_no_multiple_access_levels(
                ctx.model,
                element_id,
                &stereotypes,
                &class_si,
                errors,
            );

            // Milestoning declare-side validators (A4.1–A4.3). A4.4 fires
            // from inside the synthesis pass — it depends on the post-Pass-2a
            // candidate edge-point name. The hydration-inline trio here only
            // reads own stereotypes + own properties + supertype IDs, so it
            // runs cleanly against the slice we've already built.
            if let Some(temporal_profile) = crate::milestoning::resolve_temporal_profile(ctx.model)
            {
                crate::milestoning::validate::validate_at_most_one_temporal_stereotype(
                    &stereotypes,
                    temporal_profile,
                    &class_name,
                    &class_si,
                    errors,
                );
                crate::milestoning::validate::validate_reserved_property_names(
                    &properties,
                    &stereotypes,
                    temporal_profile,
                    &class_name,
                    errors,
                );
                let super_temporal: Vec<(
                    SmolStr,
                    Option<crate::milestoning::MilestoningStereotype>,
                )> = super_types
                    .iter()
                    .filter_map(|st| match st {
                        TypeExpr::Named { element, .. } => Some(*element),
                        _ => None,
                    })
                    .map(|sid| {
                        let name = ctx.model.element_name(sid).clone();
                        let kind = crate::milestoning::inherited_temporal_stereotype(
                            ctx.model,
                            sid,
                            temporal_profile,
                        );
                        (name, kind)
                    })
                    .collect();
                crate::milestoning::validate::validate_temporal_hierarchy_consistency(
                    &stereotypes,
                    &super_temporal,
                    temporal_profile,
                    &class_name,
                    &class_si,
                    errors,
                );
            }

            let type_variable_parameters =
                lower_type_variable_parameters(&class_def.type_variable_parameters, ctx, errors);
            Element::Class(Class {
                type_parameters: class_def
                    .type_parameters
                    .iter()
                    .map(|name| crate::nodes::class::TypeParameter::invariant(name.clone()))
                    .collect(),
                multiplicity_parameters: class_def.multiplicity_parameters.clone(),
                type_variable_parameters,
                super_types,
                properties,
                qualified_properties,
                constraints,
                stereotypes,
                tagged_values,
                original_milestoned_properties: vec![],
            })
        }
        ast::Element::Enumeration(enum_def) => {
            let stereotypes = resolve::resolve_stereotypes(&enum_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&enum_def.tagged_values, ctx, errors);
            crate::validate::validate_no_multiple_access_levels(
                ctx.model,
                element_id,
                &stereotypes,
                &enum_def.source_info,
                errors,
            );

            Element::Enumeration(Enumeration {
                values: enum_def
                    .values
                    .iter()
                    .map(|v| {
                        let v_stereos = resolve::resolve_stereotypes(&v.stereotypes, ctx, errors);
                        let v_tvs = resolve::resolve_tagged_values(&v.tagged_values, ctx, errors);
                        EnumValue {
                            name: v.name.clone(),
                            source_info: v.source_info.clone(),
                            stereotypes: v_stereos,
                            tagged_values: v_tvs,
                        }
                    })
                    .collect(),
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Profile(prof_def) => Element::Profile(Profile {
            stereotypes: prof_def.stereotype_names.clone(),
            tags: prof_def.tag_names.clone(),
        }),
        ast::Element::Function(func_def) => {
            let parameters = lower_parameters(&func_def.parameters, ctx, errors);
            let return_type = resolve::resolve_type_spec(&func_def.return_type, ctx, errors)
                .unwrap_or(TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                });
            let return_multiplicity = resolve::resolve_multiplicity_with_validation(
                &func_def.return_multiplicity,
                &func_def.source_info,
                ctx,
                errors,
            );
            let stereotypes = resolve::resolve_stereotypes(&func_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&func_def.tagged_values, ctx, errors);
            crate::validate::validate_no_multiple_access_levels(
                ctx.model,
                element_id,
                &stereotypes,
                &func_def.source_info,
                errors,
            );

            Element::Function(Function {
                function_name: func_def.name.value.clone(),
                is_native: false,
                parameters: parameters.into(),
                return_type,
                return_multiplicity,
                body: Vec::new().into(), // Bodies resolved in Pass 2b
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Association(assoc_def) => {
            let assoc_name = assoc_def.name.value.clone();
            let assoc_si = assoc_def.source_info.clone();

            // Properties / QPs — signatures only; default values & QP bodies
            // deferred to Pass 2b (see Class arm above for rationale).
            let properties = lower_property_signatures(&assoc_def.properties, ctx, errors);
            crate::validate::validate_association(
                ctx.model,
                &assoc_name,
                &properties,
                &assoc_si,
                errors,
            );

            let qualified_properties =
                lower_qualified_property_signatures(&assoc_def.qualified_properties, ctx, errors);
            crate::validate::validate_no_access_on_properties(
                ctx.model,
                element_id,
                &properties,
                &qualified_properties,
                errors,
            );

            let stereotypes = resolve::resolve_stereotypes(&assoc_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&assoc_def.tagged_values, ctx, errors);
            crate::validate::validate_no_multiple_access_levels(
                ctx.model,
                element_id,
                &stereotypes,
                &assoc_si,
                errors,
            );

            Element::Association(Association {
                properties,
                qualified_properties,
                stereotypes,
                tagged_values,
                original_milestoned_properties: vec![],
            })
        }
        ast::Element::NativeFunction(func_def) => {
            let parameters = lower_parameters(&func_def.parameters, ctx, errors);
            let return_type = resolve::resolve_type_spec(&func_def.return_type, ctx, errors)
                .unwrap_or(TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                });
            let return_multiplicity = resolve::resolve_multiplicity_with_validation(
                &func_def.return_multiplicity,
                &func_def.source_info,
                ctx,
                errors,
            );
            let stereotypes = resolve::resolve_stereotypes(&func_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&func_def.tagged_values, ctx, errors);
            crate::validate::validate_no_multiple_access_levels(
                ctx.model,
                element_id,
                &stereotypes,
                &func_def.source_info,
                errors,
            );

            Element::Function(Function {
                function_name: func_def.name.value.clone(),
                is_native: true,
                parameters: parameters.into(),
                return_type,
                return_multiplicity,
                body: Vec::new().into(),
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Measure(_measure_def) => {
            // Unit ElementIds were allocated in Pass 1; look them up
            let mapping = unit_mappings.get(&element_id);
            Element::Measure(Measure {
                canonical_unit: mapping.and_then(|m| m.canonical),
                non_canonical_units: mapping.map(|m| m.non_canonical.clone()).unwrap_or_default(),
            })
        }
        ast::Element::Primitive(prim_def) => {
            let super_named = resolve::resolve_type_ref(&prim_def.super_type, ctx, errors);
            let (super_type, super_type_value_arguments) = match super_named {
                Some(TypeExpr::Named {
                    element,
                    value_arguments,
                    ..
                }) => (Some(element), value_arguments),
                _ => (None, Vec::new()),
            };
            let type_variable_parameters =
                lower_type_variable_parameters(&prim_def.type_variable_parameters, ctx, errors);
            // Constraints — fully deferred to Pass 2b (see Class arm).
            Element::PrimitiveType(PrimitiveType {
                super_type,
                super_type_value_arguments,
                type_variable_parameters,
                constraints: Vec::new(),
            })
        }
        ast::Element::DSLElement(_) => unreachable!(
            "DSL elements are hydrated by their owning CompilerExtension's \
             define_signatures hook, never by hydrate_element_signature"
        ),
    }
}

/// Lowers AST properties to Pure properties — **signatures only**.
///
/// Default-value expressions are body-shape and need every native function
/// signature to be hydrated before they can resolve operator overloads
/// correctly, so they are deferred to Pass 2b
/// (`pass_define_class_bodies`). The returned `Property.default_value` is
/// always `None` from this function — Pass 2b patches the value in-place.
fn lower_property_signatures(
    props: &[ast::Property],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::Property> {
    props
        .iter()
        .filter_map(|p| {
            let type_expr = resolve::resolve_type_spec(&p.type_ref, ctx, errors)?;
            let multiplicity = resolve::resolve_multiplicity_with_validation(
                &p.multiplicity,
                &p.source_info,
                ctx,
                errors,
            );
            let aggregation = p.aggregation.map(lower_aggregation_kind);
            let stereotypes = resolve::resolve_stereotypes(&p.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(&p.tagged_values, ctx, errors);

            Some(class::Property {
                name: p.name.clone(),
                source_info: p.source_info.clone(),
                type_expr,
                multiplicity,
                aggregation,
                default_value: None, // patched in Pass 2b
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers AST qualified properties to Pure qualified properties —
/// **signatures only**. Bodies are deferred to Pass 2b for the same
/// reason as [`lower_property_signatures`]. Returned `body` is always
/// `vec![]`; Pass 2b patches in the lowered body.
fn lower_qualified_property_signatures(
    qprops: &[ast::QualifiedProperty],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::QualifiedProperty> {
    qprops
        .iter()
        .filter_map(|qp| {
            let return_type = resolve::resolve_type_spec(&qp.return_type, ctx, errors)?;
            let return_multiplicity = resolve::resolve_multiplicity_with_validation(
                &qp.return_multiplicity,
                &qp.source_info,
                ctx,
                errors,
            );
            let parameters = lower_parameters(&qp.parameters, ctx, errors);
            let stereotypes = resolve::resolve_stereotypes(&qp.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(&qp.tagged_values, ctx, errors);

            Some(class::QualifiedProperty {
                name: qp.name.clone(),
                source_info: qp.source_info.clone(),
                parameters: parameters.into(),
                return_type,
                return_multiplicity,
                body: Vec::new().into(), // patched in Pass 2b
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers QP bodies for all qualified properties of the given AST list.
/// Returns one `Vec<Expression>` per QP, in the same order; `vec![]` if
/// the QP was filtered out of the signature pass (`return_type` couldn't
/// resolve) so the index doesn't get out of sync with the model.
fn lower_qualified_property_bodies(
    qprops: &[ast::QualifiedProperty],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<Vec<crate::types::ValueSpec>> {
    qprops
        .iter()
        .map(|qp| crate::lower::lower_expression_body(&qp.body, ctx, errors))
        .collect()
}

/// Lowers default-value expressions for the given AST property list.
/// Returns one `Option<Expression>` per AST property, in order — `None`
/// when the property had no default or when lowering produced no value.
fn lower_property_default_values(
    props: &[ast::Property],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<Option<crate::types::ValueSpec>> {
    props
        .iter()
        .map(|p| {
            p.default_value
                .as_ref()
                .and_then(|dv| crate::lower::lower_expression(dv, ctx, errors))
        })
        .collect()
}

/// Lowers AST parameters to Pure parameters.
fn lower_parameters(
    params: &[legend_pure_parser_ast::annotation::Parameter],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params
        .iter()
        .filter_map(|p| {
            // Skip untyped lambda params — they need type inference (Phase 5+)
            let type_ref = p.type_ref.as_ref()?;
            let mult = p.multiplicity.as_ref()?;
            let type_expr = resolve::resolve_type_ref(type_ref, ctx, errors)?;
            let multiplicity =
                resolve::resolve_multiplicity_with_validation(mult, &p.source_info, ctx, errors);

            Some(crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity,
                source_info: p.source_info.clone(),
            })
        })
        .collect()
}

/// Lowers AST `TypeVariableParameter`s to compiled `Parameter`s.
/// Used by Class and Primitive to carry parametric-value declarations
/// (e.g. the `x:Integer[1]` in `Primitive P(x:Integer[1])`).
fn lower_type_variable_parameters(
    params: &[legend_pure_parser_ast::type_ref::TypeVariableParameter],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params
        .iter()
        .filter_map(|p| {
            let type_expr = resolve::resolve_type_ref(&p.type_ref, ctx, errors)?;
            Some(crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity: resolve::resolve_multiplicity_with_validation(
                    &p.multiplicity,
                    &p.source_info,
                    ctx,
                    errors,
                ),
                source_info: p.source_info.clone(),
            })
        })
        .collect()
}

/// Lowers AST constraints to Pure constraints.
///
/// Constraint expressions are checked for type compatibility inline as
/// they're lowered: the `function` body must evaluate to `Boolean[1]`,
/// and an optional `message` must evaluate to `String[1]`. Both checks
/// run at the source of truth (where the constraint is constructed)
/// rather than as a post-hoc cross-chunk validator. Inference is
/// available here because Pass 2a (signatures) is fully hydrated by
/// the time `pass_define_class_bodies` invokes this function.
fn lower_constraints(
    constraints: &[ast::Constraint],
    owner_fqn: &SmolStr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::Constraint> {
    constraints
        .iter()
        .enumerate()
        .filter_map(|(idx, c)| {
            let function = crate::lower::lower_expression(&c.function_definition, ctx, errors)?;
            let message = c
                .message
                .as_ref()
                .and_then(|m| crate::lower::lower_expression(m, ctx, errors));

            let constraint_id = c
                .name
                .clone()
                .unwrap_or_else(|| SmolStr::new(idx.to_string()));

            check_constraint_slot_type(
                &function,
                owner_fqn,
                &constraint_id,
                ConstraintSlotName::Body,
                &c.source_info,
                ctx,
                errors,
            );
            if let Some(msg) = &message {
                check_constraint_slot_type(
                    msg,
                    owner_fqn,
                    &constraint_id,
                    ConstraintSlotName::Message,
                    &c.source_info,
                    ctx,
                    errors,
                );
            }

            Some(class::Constraint {
                name: c.name.clone(),
                source_info: c.source_info.clone(),
                function,
                enforcement_level: c.enforcement_level.clone(),
                external_id: c.external_id.clone(),
                message,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum ConstraintSlotName {
    /// `Constraint.function` — predicate, must be `Boolean[1]`.
    Body,
    /// `Constraint.message` — failure message, must be `String[1]`.
    Message,
}

impl ConstraintSlotName {
    fn label(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Message => "message",
        }
    }

    fn expected_label(self) -> &'static str {
        match self {
            Self::Body => "Boolean[1]",
            Self::Message => "String[1]",
        }
    }

    fn expected_type(self) -> crate::types::TypeExpr {
        let element = match self {
            Self::Body => crate::bootstrap::BOOLEAN_ID,
            Self::Message => crate::bootstrap::STRING_ID,
        };
        crate::types::TypeExpr::Named {
            element,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: vec![],
            source_info: None,
        }
    }
}

/// Type-check one constraint slot (body or message) against its
/// required signature. Inlined into [`lower_constraints`] so the
/// diagnostic fires the moment the constraint is built — no cross-
/// chunk pass needed. Skips silently when inference can't recover a
/// type (a separate compile error will already exist for the
/// underlying expression problem).
fn check_constraint_slot_type(
    expr: &crate::types::ValueSpec,
    owner_fqn: &SmolStr,
    constraint_id: &SmolStr,
    slot: ConstraintSlotName,
    constraint_source: &SourceInfo,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) {
    let Some(actual_ty) =
        crate::resolve::infer_typeexpr_from_valuespec(expr, ctx.model, &ctx.variable_types)
    else {
        return;
    };
    let Some(actual_mult) =
        crate::resolve::infer_multiplicity_from_valuespec(expr, ctx.model, &ctx.variable_types)
    else {
        return;
    };
    let expected_ty = slot.expected_type();
    let expected_mult = crate::types::Multiplicity::PureOne;
    let type_ok =
        crate::resolve::is_type_compatible_structural(&actual_ty, &expected_ty, ctx.model);
    let mult_ok = crate::resolve::is_multiplicity_compatible(Some(&actual_mult), &expected_mult);
    if type_ok && mult_ok {
        return;
    }
    let actual = crate::infer::render_type(ctx.model, &actual_ty, &actual_mult);
    let expected_label = slot.expected_label();
    errors.push(CompilationError {
        message: format!(
            "Constraint '{constraint_id}' {slot_label} of '{owner_fqn}' is '{actual}', expected '{expected_label}'",
            slot_label = slot.label(),
        ),
        source_info: constraint_source.clone(),
        kind: CompilationErrorKind::ConstraintBodyTypeMismatch {
            owner_fqn: owner_fqn.clone(),
            constraint_id: constraint_id.clone(),
            slot: SmolStr::new_static(slot.label()),
            expected: SmolStr::new_static(expected_label),
            actual: SmolStr::new(actual),
        },
    });
}

/// Converts an AST `AggregationKind` to the Pure equivalent.
fn lower_aggregation_kind(kind: ast::AggregationKind) -> class::AggregationKind {
    match kind {
        ast::AggregationKind::None => class::AggregationKind::None,
        ast::AggregationKind::Shared => class::AggregationKind::Shared,
        ast::AggregationKind::Composite => class::AggregationKind::Composite,
    }
}

/// Extracts the simple name from an AST element.
fn ast_element_name(element: &ast::Element) -> SmolStr {
    use legend_pure_parser_ast::element::PackageableElement;
    element.name().clone()
}

/// Extracts the source info from an AST element.
fn ast_element_source(element: &ast::Element) -> &SourceInfo {
    element.source_info()
}

/// Extracts the source span of the element's **name identifier** (the
/// `XTestClass` in `Class meta::pure::…::XTestClass { … }`), falling
/// back to the full declaration span for element kinds that don't
/// carry a separate name span.
///
/// Matches Java Pure's `SourceInformation.line`/`column` (distinct from
/// `startLine`/`startColumn`).
fn ast_element_name_source(element: &ast::Element) -> &SourceInfo {
    match element {
        ast::Element::Class(c) => c.name.source_info(),
        ast::Element::Function(f) => f.name.source_info(),
        ast::Element::NativeFunction(f) => f.name.source_info(),
        ast::Element::Association(a) => a.name.source_info(),
        ast::Element::Enumeration(e) => e.name.source_info(),
        ast::Element::Profile(p) => p.name.source_info(),
        ast::Element::Measure(m) => m.name.source_info(),
        ast::Element::Primitive(p) => p.name.source_info(),
        // DSL elements expose their name via the trait directly;
        // a DSL crate manages its own source-info lookups. Core's
        // `pass_declare` skips DSL elements before reaching here.
        ast::Element::DSLElement(e) => e.source_info(),
    }
}

/// Extracts the package path segments from an AST element.
fn ast_element_package_path(element: &ast::Element) -> Vec<SmolStr> {
    use legend_pure_parser_ast::element::PackageableElement;
    match element.package() {
        Some(pkg) => pkg.segments().into_iter().cloned().collect(),
        None => vec![],
    }
}

/// Extracts type parameter names from an AST element.
///
/// Classes and functions can declare type parameters (e.g., `Class<T, V>`,
/// `function<Z|m>`). These names must be treated as generic type variables
/// during resolution, not as packageable element references.
fn ast_type_parameters(element: &ast::Element) -> Vec<SmolStr> {
    match element {
        ast::Element::Class(c) => c.type_parameters.clone(),
        ast::Element::Function(f) => f.type_parameters.clone(),
        ast::Element::NativeFunction(f) => f.type_parameters.clone(),
        _ => vec![],
    }
}

/// Extracts multiplicity parameter names from an AST element. Sibling
/// to [`ast_type_parameters`] — names declared in the `<…|m, n>`
/// clause that are in scope for the element's signature lowering.
/// Used by `ResolutionContext::multiplicity_parameters` to validate
/// signature-position multiplicity-variable references via
/// [`crate::resolve::resolve_multiplicity_with_validation`].
fn ast_multiplicity_parameters(element: &ast::Element) -> Vec<SmolStr> {
    match element {
        ast::Element::Class(c) => c.multiplicity_parameters.clone(),
        ast::Element::Function(f) => f.multiplicity_parameters.clone(),
        ast::Element::NativeFunction(f) => f.multiplicity_parameters.clone(),
        _ => vec![],
    }
}

/// Builds a fully qualified name from package path + element name.
fn build_fqn(pkg_path: &[SmolStr], name: &SmolStr) -> SmolStr {
    if pkg_path.is_empty() {
        name.clone()
    } else {
        let mut fqn = String::new();
        for (i, seg) in pkg_path.iter().enumerate() {
            if i > 0 {
                fqn.push_str("::");
            }
            fqn.push_str(seg);
        }
        fqn.push_str("::");
        fqn.push_str(name);
        SmolStr::new(&fqn)
    }
}

/// Retrieves the AST element from source files given a declaration.
fn get_ast_element<'a>(source_files: &'a [SourceFile], decl: &Declaration) -> &'a ast::Element {
    &source_files[decl.file_idx].sections[decl.section_idx].elements[decl.element_idx]
}

/// Runs bottom-up type inference over all function and qualified property bodies.
///
/// For each function, infers types for every expression in the body and sets
/// `type_info` on each expression node in place.
///
/// `chunk_set` bounds inference to a subset of chunks (LSP-side
/// incremental recompile, T-20260513-01). `None` = whole-model,
/// preserves the pre-incremental behaviour.
#[tracing::instrument(level = "info", name = "pass_infer", skip_all)]
fn pass_infer(
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
    chunk_set: Option<&HashSet<u16>>,
) {
    use crate::infer;

    // Collect (chunk_idx, element_idx, params, body) pairs, avoiding borrow conflicts.
    // We clone the minimal data needed, then write back after inference.
    struct InferTarget {
        chunk_idx: usize,
        local_idx: u32,
        /// `true` = Function body, `false` = `QualifiedProperty` body (by qp index)
        kind: TargetKind,
        params: Vec<crate::types::Parameter>,
        body: Vec<crate::types::ValueSpec>,
    }

    enum TargetKind {
        FunctionBody,
        QualifiedProperty(usize),
        /// Class property default-value. `usize` indexes
        /// `Class.properties`.
        ClassPropertyDefault(usize),
        /// Association property default-value. `usize` indexes
        /// `Association.properties`.
        AssociationPropertyDefault(usize),
    }

    let mut targets: Vec<InferTarget> = Vec::new();

    // Walk every non-bootstrap chunk. Bootstrap (0) has no function /
    // QP bodies. Earlier this routine processed only `chunks.last()`
    // — same bug as `validate.rs` had: with multi-repo loaders
    // (`core_platform_pure::repo::load`) earlier chunks' expressions
    // never had `type_info` populated, which silently degraded
    // dispatch precision and downstream type-mismatch detection.
    for (chunk_idx, chunk) in model.chunks.iter().enumerate().skip(1) {
        if let Some(set) = chunk_set
            && !set.contains(&chunk.chunk_id)
        {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            match element {
                Element::Function(f) if !f.body.is_empty() => {
                    targets.push(InferTarget {
                        chunk_idx,
                        local_idx,
                        kind: TargetKind::FunctionBody,
                        params: f.parameters.to_vec(),
                        body: f.body.to_vec(),
                    });
                }
                Element::Class(c) => {
                    // `$this` is implicitly bound to the receiving
                    // instance inside any class qualified-property
                    // body. Build a synthetic Parameter that the
                    // inference scope will pick up via
                    // `Scope::from_params`, so `$this` resolves the
                    // same way an explicit parameter would.
                    //
                    // Type-variable parameters declared on the class
                    // (e.g. `Class C(x:Integer[1]) [...]`) are
                    // similarly in-scope inside QP bodies — Java
                    // Pure threads them through `eval_qualified_property`
                    // at runtime.
                    let class_id = ElementId::InstanceId {
                        chunk_id: chunk.chunk_id,
                        local_idx,
                    };
                    let this_param = crate::types::Parameter {
                        name: SmolStr::new("this"),
                        type_expr: crate::types::TypeExpr::Named {
                            element: class_id,
                            type_arguments: Vec::new(),
                            multiplicity_arguments: Vec::new(),
                            value_arguments: Vec::new(),
                            source_info: None,
                        },
                        multiplicity: crate::types::Multiplicity::PureOne,
                        source_info: chunk.nodes.get(local_idx).source_info.clone(),
                    };
                    for (qp_idx, qp) in c.qualified_properties.iter().enumerate() {
                        if !qp.body.is_empty() {
                            let mut params: Vec<crate::types::Parameter> = Vec::with_capacity(
                                qp.parameters.len() + 1 + c.type_variable_parameters.len(),
                            );
                            params.push(this_param.clone());
                            params.extend(c.type_variable_parameters.iter().cloned());
                            params.extend(qp.parameters.iter().cloned());
                            targets.push(InferTarget {
                                chunk_idx,
                                local_idx,
                                kind: TargetKind::QualifiedProperty(qp_idx),
                                params,
                                body: qp.body.to_vec(),
                            });
                        }
                    }
                    // Property default-values — inferred so the
                    // `validate_property_default_values` cross-chunk
                    // pass can read `type_info` to check compat
                    // (T-20260511-01). No params: defaults can't
                    // reference `$this` (no instance exists yet).
                    for (p_idx, prop) in c.properties.iter().enumerate() {
                        if let Some(dv) = &prop.default_value {
                            targets.push(InferTarget {
                                chunk_idx,
                                local_idx,
                                kind: TargetKind::ClassPropertyDefault(p_idx),
                                params: Vec::new(),
                                body: vec![dv.clone()],
                            });
                        }
                    }
                }
                Element::Association(a) => {
                    for (p_idx, prop) in a.properties.iter().enumerate() {
                        if let Some(dv) = &prop.default_value {
                            targets.push(InferTarget {
                                chunk_idx,
                                local_idx,
                                kind: TargetKind::AssociationPropertyDefault(p_idx),
                                params: Vec::new(),
                                body: vec![dv.clone()],
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Run inference on cloned bodies, then write back.
    for target in &mut targets {
        infer::infer_function_body(model, &target.params, &mut target.body, errors);
    }

    // After inference, validate every body's last expression
    // against the declaring function/QP's declared return
    // signature. Catches `function foo(): Integer[1] { $x * 1.5 }`
    // (returns Float not Integer) and `function foo(): Integer[1]
    // { $stop->head() }` (returns [0..1] not [1]).
    for target in &targets {
        let chunk = &model.chunks[target.chunk_idx];
        let element = chunk.elements.get(target.local_idx);
        let node = chunk.nodes.get(target.local_idx);
        match &target.kind {
            TargetKind::FunctionBody => {
                if let Element::Function(f) = element {
                    infer::check_body_return_signature(
                        model,
                        &node.name,
                        &node.source_info,
                        &target.body,
                        &f.return_type,
                        &f.return_multiplicity,
                        errors,
                    );
                }
            }
            TargetKind::QualifiedProperty(qp_idx) => {
                if let Element::Class(c) = element {
                    let qp = &c.qualified_properties[*qp_idx];
                    infer::check_body_return_signature(
                        model,
                        &qp.name,
                        &qp.source_info,
                        &target.body,
                        &qp.return_type,
                        &qp.return_multiplicity,
                        errors,
                    );
                }
            }
            // Default-value type/multiplicity compat is checked by
            // `validate::validate_property_default_values` after
            // inference completes; nothing to do here.
            TargetKind::ClassPropertyDefault(_) | TargetKind::AssociationPropertyDefault(_) => {}
        }
    }

    // Write back the typed bodies.
    for target in targets {
        let element = model.chunks[target.chunk_idx]
            .elements
            .get_mut(target.local_idx);
        match target.kind {
            TargetKind::FunctionBody => {
                if let Element::Function(f) = element {
                    f.body = target.body.into();
                }
            }
            TargetKind::QualifiedProperty(qp_idx) => {
                if let Element::Class(c) = element {
                    c.qualified_properties[qp_idx].body = target.body.into();
                }
            }
            TargetKind::ClassPropertyDefault(p_idx) => {
                if let Element::Class(c) = element {
                    let mut body = target.body;
                    if let Some(dv) = body.pop() {
                        c.properties[p_idx].default_value = Some(dv);
                    }
                }
            }
            TargetKind::AssociationPropertyDefault(p_idx) => {
                if let Element::Association(a) = element {
                    let mut body = target.body;
                    if let Some(dv) = body.pop() {
                        a.properties[p_idx].default_value = Some(dv);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_fqn_root() {
        let fqn = build_fqn(&[], &SmolStr::new("Person"));
        assert_eq!(fqn, "Person");
    }

    #[test]
    fn build_fqn_with_package() {
        let fqn = build_fqn(
            &[SmolStr::new("model"), SmolStr::new("domain")],
            &SmolStr::new("Person"),
        );
        assert_eq!(fqn, "model::domain::Person");
    }

    #[test]
    fn compile_empty_input() {
        let result = compile!(&[]);
        assert!(result.is_ok());
        let model = result.unwrap();
        // Bootstrap chunk only — empty source list no longer allocates
        // a stray empty user chunk (per-repo composition contract).
        assert_eq!(model.chunks.len(), 1);
    }
}
