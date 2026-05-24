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

//! Compiler extension hook contract.
//!
//! Each M2 DSL (Mapping, Diagram, standalone Relational, …) plugs into
//! the compiler pipeline by implementing [`CompilerExtension`]. The
//! pipeline invokes each registered extension's hooks once per pass:
//!
//! 1. [`CompilerExtension::declare`] — Pass 1: register element shells
//!    in the model. Same phase as the M3 declarations: assign `ElementId`s,
//!    populate the package tree.
//! 2. [`CompilerExtension::define_signatures`] — Pass 2a: resolve any
//!    type/element references that other passes need before bodies
//!    are lowered.
//! 3. [`CompilerExtension::define_bodies`] — Pass 2b: lower any
//!    body-shape parts (function bodies, constraint expressions,
//!    qualified-property bodies, default values).
//! 4. [`CompilerExtension::validate`] — Pass 3: read-only checks on
//!    the frozen model.
//!
//! M3 remains hardcoded in `pipeline::compile_with_extensions` rather
//! than being lifted into an extension impl — that refactor is bigger
//! and would risk breaking the platform-clean baseline. Extensions
//! run *after* the M3 passes, so their hooks see a model where every
//! M3 element (`Class`, `Function`, `Profile`, `Association`, …) is
//! already declared and resolved.
//!
//! # Lifecycle
//!
//! Extensions are passed as a slice to
//! [`pipeline::compile_with_extensions`](crate::pipeline::compile_with_extensions)
//! and consulted in the order they appear. The default
//! [`pipeline::compile`](crate::pipeline::compile) wraps that with an
//! empty slice, so existing call sites are unaffected.
//!
//! # Default impls
//!
//! All hooks default to no-ops, so an extension only implements the
//! phases it actually contributes to. An extension that adds a
//! purely-declarative DSL (e.g. Diagram's `Diagram ::Foo { ... }`) will
//! typically override `declare` + `define_signatures`; an extension
//! whose elements carry executable bodies (e.g. Mapping property
//! mappings) also overrides `define_bodies`.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::section::SourceFile;
use linkme::distributed_slice;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::model::PureModel;
use crate::types::{Multiplicity, Parameter, ResolvedType, TypeExpr};

/// Per-compile scratch arena threaded through every `CompilerExtension`
/// hook (`declare` → `define_signatures` → `define_bodies` →
/// `validate`).
///
/// Each extension stashes its per-compile state here keyed by
/// `TypeId`. The arena is freshly allocated by
/// [`crate::pipeline::compile_with_extensions`] and dropped when
/// compilation finishes — restores the per-compile isolation that
/// stateful extensions used to obtain via a `RefCell` field on their
/// own struct.
///
/// Lets a `CompilerExtension` be a stateless unit struct (so it can
/// be `Sync` and self-register via `#[distributed_slice]`) while
/// still carrying compile-time data between passes.
///
/// # Example
///
/// ```ignore
/// #[derive(Default)]
/// struct DiagramCompileState {
///     diagrams: HashMap<SmolStr, RegisteredDiagram>,
/// }
///
/// impl CompilerExtension for DiagramExtension {
///     fn declare(&self, ctx: &mut DeclareCtx<'_>) {
///         let state = ctx.scope.get_or_default::<DiagramCompileState>();
///         state.diagrams.insert(fqn, reg);
///     }
///
///     fn validate(&self, ctx: &mut ValidateCtx<'_>) {
///         let Some(state) = ctx.scope.get::<DiagramCompileState>() else { return };
///         for (fqn, reg) in &state.diagrams { … }
///     }
/// }
/// ```
pub struct CompileExtensionScope {
    /// `Send + Sync` bounds mirror [`crate::model::PureModel::extension_arenas`]
    /// so the model (which owns a `compile_scope`) can be shared
    /// across threads — notably by the LSP's multi-threaded executor.
    /// Extension state types stored here must satisfy these bounds;
    /// stateless `Copy` / plain data types are the natural fit.
    state: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl CompileExtensionScope {
    /// Construct an empty scope. Called once per compile by the
    /// pipeline; extension callers should not construct this directly.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: HashMap::new(),
        }
    }

    /// Get a mutable reference to the per-compile `T`, initialising
    /// it with `T::default()` on first access in this compile.
    ///
    /// `T: Send + Sync` mirrors the slice's storage bound so the
    /// containing [`crate::model::PureModel`] can stay `Send + Sync`.
    ///
    /// # Panics
    /// Never — the `TypeId` ↔ `T` invariant is upheld internally.
    pub fn get_or_default<T: Default + Any + Send + Sync>(&mut self) -> &mut T {
        let entry = self
            .state
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()));
        // Safe: the TypeId key uniquely identifies T.
        entry
            .downcast_mut::<T>()
            .unwrap_or_else(|| unreachable!("CompileExtensionScope: TypeId<T> entry mismatched"))
    }

    /// Get an immutable reference to the per-compile `T` if it has
    /// been initialised in this compile, else `None`.
    #[must_use]
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.state
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }
}

impl Default for CompileExtensionScope {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CompileExtensionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompileExtensionScope")
            .field("entries", &self.state.len())
            .finish()
    }
}

/// Context passed to [`CompilerExtension::declare`].
///
/// Borrows the source files (read-only) and the in-progress model
/// (mutable). Errors collected during declaration go into `errors`.
/// Per-compile state stashed in `scope` survives through subsequent
/// passes (`define_*`, `validate`).
pub struct DeclareCtx<'a> {
    /// Parsed source files for this compilation unit.
    pub source_files: &'a [SourceFile],
    /// The compiler's current model. Extensions allocate shells here.
    pub model: &'a mut PureModel,
    /// Auto-imports applied to every section in this compilation.
    pub auto_imports: &'a [SmolStr],
    /// Accumulated compilation errors.
    pub errors: &'a mut Vec<CompilationError>,
    /// Per-compile scratch arena — extensions stash data here keyed
    /// by `TypeId` and read it back in later passes. See
    /// [`CompileExtensionScope`].
    ///
    /// `None` when constructed outside the normal pipeline — typically
    /// in tests that build a ctx by hand and don't exercise the
    /// declare→validate scope-passing flow. Stateful extensions that
    /// need the scope should branch on `is_some()`.
    pub scope: Option<&'a mut CompileExtensionScope>,
}

/// Context passed to [`CompilerExtension::define_signatures`] and
/// [`CompilerExtension::define_bodies`].
///
/// Identical fields to [`DeclareCtx`] today; kept as a separate struct
/// so future passes can carry pass-specific state (resolve caches,
/// import scopes, etc.) without breaking the declare API.
pub struct DefineCtx<'a> {
    /// Parsed source files for this compilation unit.
    pub source_files: &'a [SourceFile],
    /// The compiler's current model.
    pub model: &'a mut PureModel,
    /// Auto-imports applied to every section in this compilation.
    pub auto_imports: &'a [SmolStr],
    /// Accumulated compilation errors.
    pub errors: &'a mut Vec<CompilationError>,
    /// Per-compile scratch arena — see [`CompileExtensionScope`]. May
    /// be `None` in hand-built test contexts.
    pub scope: Option<&'a mut CompileExtensionScope>,
}

/// Context passed to [`CompilerExtension::validate`].
///
/// `model` is read-only here — Pass 3 runs after the model is frozen.
pub struct ValidateCtx<'a> {
    /// The frozen model.
    pub model: &'a PureModel,
    /// Auto-imports applied to every section in this compilation. The
    /// list is the same one supplied to the `compile_with_extensions`
    /// caller; extensions need it to drive
    /// [`lower_and_infer_expression`] over user-supplied AST that
    /// references unqualified bootstrap identifiers (`String`,
    /// `Boolean`, `isEmpty`, …).
    pub auto_imports: &'a [SmolStr],
    /// Accumulated compilation errors.
    pub errors: &'a mut Vec<CompilationError>,
    /// Per-compile scratch arena — see [`CompileExtensionScope`].
    /// Read-only here (validate is a read pass over the frozen model;
    /// extensions consume data their `declare` / `define_*` hooks
    /// stashed earlier). May be `None` in hand-built test contexts.
    pub scope: Option<&'a CompileExtensionScope>,
}

/// Plug-in contract for M2 DSLs.
///
/// See the module-level docs for the lifecycle and pass ordering.
pub trait CompilerExtension {
    /// Stable identifier for the extension. Used in tracing spans and
    /// error messages so a failure can be traced back to the
    /// contributing extension.
    fn name(&self) -> &'static str;

    /// Pass 1 — declare. Default no-op.
    fn declare(&self, _ctx: &mut DeclareCtx<'_>) {}

    /// Pass 2a — resolve signatures. Default no-op.
    fn define_signatures(&self, _ctx: &mut DefineCtx<'_>) {}

    /// Pass 2b — lower bodies. Default no-op.
    fn define_bodies(&self, _ctx: &mut DefineCtx<'_>) {}

    /// Pass 3 — validate. Default no-op.
    fn validate(&self, _ctx: &mut ValidateCtx<'_>) {}

    // walk_references method removed in Phase 2.5 — IDE-side reference
    // contribution is now its own trait (`legend_pure_ide::IdeExtension`)
    // discovered via a separate distributed slice. DSLs that contribute
    // reference sites ship a sibling unit-struct IdeExtension impl
    // (see crates/dsl-mapping and crates/dsl-relational for the pattern).
    // Keeping walk_references on CompilerExtension would force pure to
    // depend on the ide crate (for Reference) — Phase 2.5 broke that
    // cycle by lifting refs out of pure entirely.

    /// Names of extensions that must run *before* this one within each pass.
    ///
    /// Default: no dependencies. The compiler pipeline iterates
    /// extensions in slice order within each pass (Pass 1 declare, Pass 2a
    /// signatures, Pass 2b bodies, Pass 3 validate); across passes there
    /// is a hard barrier (every extension's Pass-N completes before any
    /// extension's Pass-(N+1) starts), so cross-pass ordering is automatic.
    ///
    /// **In-pass** ordering only matters when extension B reads model
    /// state that extension A wrote *in the same pass*. Override this
    /// method when that's the case; the discovery helper uses the
    /// declared edges to topologically sort the slice once at startup.
    ///
    /// Cycles in the dependency graph panic at startup with the cycle
    /// path in the message.
    fn depends_on(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Lower an AST [`Expression`] to a `ValueSpec`, then run type
/// inference against `model` with the supplied variable `bindings` in
/// scope. Returns the inferred [`ResolvedType`] of the expression, or
/// `None` if lowering or inference produced no usable type.
///
/// Designed for compiler-extension consumers (Mapping DSL filter and
/// transform expressions; future Function-DSL bodies; Relational
/// derived columns) that own AST `Expression` nodes outside the main
/// pipeline and need to validate them against the model. Internally
/// constructs a `ResolutionContext` (private to the `pure` crate)
/// from the public inputs — extensions never see that type.
///
/// # Arguments
///
/// - `model` — the frozen [`PureModel`] containing the elements that
///   the expression may reference. Pass `ctx.model` from
///   [`ValidateCtx`].
/// - `auto_imports` — package paths whose elements resolve unqualified
///   inside `expr` (e.g. `meta::pure::metamodel::type` for primitive
///   types). Use the same list passed to
///   [`crate::pipeline::compile_with_extensions`].
/// - `expr` — the AST expression to lower and infer.
/// - `bindings` — variables visible at the start of the expression
///   (`(name, type, multiplicity)` triples). For Mapping bodies this
///   is `[("src", srcClass, Multiplicity::PureOne)]`.
/// - `errors` — accumulator. Lowering and inference both push here on
///   failure; partial results are still observable.
///
/// # Returns
///
/// `Some(ResolvedType)` when the expression lowered cleanly and
/// inference produced a top-level type; `None` if either step failed
/// (errors are still appended to `errors`).
///
/// # Limitations
///
/// - **Type parameters**: this wrapper passes an empty
///   `type_parameters: &[]` to the resolver. Expressions whose
///   surrounding context binds generic type parameters (e.g. a Mapping
///   over `pkg::List<X>` whose body refers to `X`) will report `X` as
///   "unresolved generic" rather than honoring the declared
///   parameter. When that case becomes load-bearing (likely Stage 4 —
///   `EnumerationMapping<T>` — or later when generic Mappings appear),
///   add a `type_parameters: &[SmolStr]` argument and thread it into
///   `ResolutionContext::type_parameters`. The fix is local to this
///   function.
#[allow(clippy::implicit_hasher)] // public API; HashMap default hasher is fine
pub fn lower_and_infer_expression(
    model: &PureModel,
    auto_imports: &[SmolStr],
    expr: &Expression,
    bindings: &[(SmolStr, TypeExpr, Multiplicity)],
    errors: &mut Vec<CompilationError>,
) -> Option<ResolvedType> {
    // Build the import scopes the resolver needs. We don't hold the
    // pub(crate) `ImportScope` type in the public surface — it's
    // constructed inside this function and dropped at return.
    let import_scopes: Vec<crate::resolve::ImportScope> = auto_imports
        .iter()
        .map(|p| crate::resolve::ImportScope::from_path_str(p.as_str()))
        .collect();
    let mut resolve_cache = HashMap::new();
    let variable_types: HashMap<SmolStr, (TypeExpr, Multiplicity)> = bindings
        .iter()
        .map(|(name, t, m)| (name.clone(), (t.clone(), m.clone())))
        .collect();
    let type_parameters: Vec<SmolStr> = Vec::new();
    let multiplicity_parameters: Vec<SmolStr> = Vec::new();

    let mut ctx = crate::resolve::ResolutionContext {
        model,
        import_scopes: &import_scopes,
        resolve_cache: &mut resolve_cache,
        type_parameters: &type_parameters,
        multiplicity_parameters: &multiplicity_parameters,
        // No anchoring element for free-floating DSL expressions; the
        // explicit `auto_imports` slice is the full visible namespace.
        self_package: None,
        variable_types,
        island_lowerers: &[],
    };

    // Lower AST → ValueSpec.
    let value_spec = crate::lower::lower_expression(expr, &mut ctx, errors)?;

    // Build the parameter list for inference. The `source_info` here
    // is synthetic — the binding originates from the consumer
    // (e.g. an injected `src` for a mapping body), not from real
    // source tokens.
    let synthetic_si = SourceInfo::new("<lower_and_infer_expression>", 0, 0, 0, 0);
    let params: Vec<Parameter> = bindings
        .iter()
        .map(|(name, t, m)| Parameter {
            name: name.clone(),
            type_expr: t.clone(),
            multiplicity: m.clone(),
            source_info: synthetic_si.clone(),
        })
        .collect();

    let mut body = vec![value_spec];
    crate::infer::infer_function_body(model, &params, &mut body, errors);

    body.into_iter().next()?.type_info.map(|t| *t)
}

/// Distributed slice into which each [`CompilerExtension`]-providing
/// crate registers its top-level extension instance.
///
/// Use the `#[distributed_slice]` attribute next to the extension
/// instance to make it discoverable by
/// [`discovered_compiler_extensions`] and by
/// [`crate::pipeline::compile`]:
///
/// ```ignore
/// use legend_pure_pure::extension::{CompilerExtension, COMPILER_EXTENSIONS};
/// use linkme::distributed_slice;
///
/// #[distributed_slice(COMPILER_EXTENSIONS)]
/// static MY_EXT: &(dyn CompilerExtension + Sync) = &MyExtension;
/// ```
///
/// The `+ Sync` bound is required because the slice is a `static`. A
/// `CompilerExtension` that carries per-compile mutable state (e.g.
/// `RefCell`-backed accumulators) cannot self-register through this
/// slice — those extensions remain explicit
/// [`crate::pipeline::compile_with_extensions`] callers until they
/// migrate their state into [`crate::model::Element::DSLInstance`].
#[distributed_slice]
pub static COMPILER_EXTENSIONS: [&'static (dyn CompilerExtension + Sync)] = [..];

/// Discover and topologically sort the [`COMPILER_EXTENSIONS`] slice.
///
/// Within each compile pass the pipeline runs extensions in the
/// returned order. The topo-sort respects each extension's
/// [`CompilerExtension::depends_on`] declaration so that an extension
/// reading another's in-pass output runs after its dependency.
///
/// # Panics
///
/// - Two extensions register the same [`CompilerExtension::name`].
/// - The dependency graph has a cycle. The panic message names the
///   cycle path.
/// - An extension declares a dependency on a name that is not
///   registered. The panic message names both ends.
#[must_use]
pub fn discovered_compiler_extensions() -> Vec<&'static dyn CompilerExtension> {
    topo_sort_compiler_extensions(COMPILER_EXTENSIONS.iter().copied())
}

/// Strict-validating topo-sort factored out for unit-testing without
/// touching the global [`COMPILER_EXTENSIONS`] slice.
#[must_use]
fn topo_sort_compiler_extensions<I>(extensions: I) -> Vec<&'static dyn CompilerExtension>
where
    I: IntoIterator<Item = &'static (dyn CompilerExtension + Sync)>,
{
    use std::collections::{BTreeMap, BTreeSet};

    let exts: Vec<&'static (dyn CompilerExtension + Sync)> = extensions.into_iter().collect();

    // 1. Validate uniqueness of names.
    let mut by_name: BTreeMap<&'static str, &'static (dyn CompilerExtension + Sync)> =
        BTreeMap::new();
    for ext in &exts {
        let name = ext.name();
        assert!(
            by_name.insert(name, *ext).is_none(),
            "discovered_compiler_extensions: two extensions registered under name `{name}`. \
             Extension names must be globally unique.",
        );
    }

    // 2. Validate every dependency edge points at a registered extension.
    for ext in &exts {
        let name = ext.name();
        for dep in ext.depends_on() {
            assert!(
                by_name.contains_key(dep),
                "discovered_compiler_extensions: extension `{name}` declares dependency on `{dep}`, \
                 which is not registered. Check that the providing crate is in your Cargo \
                 dependency graph and links its #[distributed_slice] static.",
            );
        }
    }

    // 3. Kahn's algorithm — BTreeSet so the output order is
    //    deterministic for equal-priority extensions.
    let mut in_degree: BTreeMap<&'static str, usize> =
        by_name.keys().map(|n| (*n, 0_usize)).collect();
    // Reverse adjacency: for each node, the set of nodes that depend on it.
    let mut dependents: BTreeMap<&'static str, BTreeSet<&'static str>> =
        by_name.keys().map(|n| (*n, BTreeSet::new())).collect();
    for ext in &exts {
        for dep in ext.depends_on() {
            // dep -> ext (dep must run first)
            *in_degree.get_mut(ext.name()).unwrap_or(&mut 0) += 1;
            if let Some(set) = dependents.get_mut(dep) {
                set.insert(ext.name());
            }
        }
    }

    let mut ready: BTreeSet<&'static str> = in_degree
        .iter()
        .filter_map(|(n, d)| if *d == 0 { Some(*n) } else { None })
        .collect();
    let mut out: Vec<&'static dyn CompilerExtension> = Vec::with_capacity(exts.len());
    while let Some(name) = ready.iter().next().copied() {
        ready.remove(&name);
        if let Some(ext) = by_name.get(name) {
            out.push(*ext);
        }
        if let Some(deps) = dependents.get(&name).cloned() {
            for dependent in deps {
                if let Some(d) = in_degree.get_mut(dependent) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        ready.insert(dependent);
                    }
                }
            }
        }
    }

    if out.len() != exts.len() {
        // Remaining nodes have non-zero in_degree → cycle.
        let remaining: Vec<&'static str> = in_degree
            .iter()
            .filter_map(|(n, d)| if *d > 0 { Some(*n) } else { None })
            .collect();
        panic!(
            "discovered_compiler_extensions: dependency cycle among extensions {remaining:?}. \
             Inspect each extension's `depends_on` declaration to find the cycle.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;
    struct C;
    struct ADup;
    struct BadDep;

    impl CompilerExtension for A {
        fn name(&self) -> &'static str {
            "A"
        }
    }
    impl CompilerExtension for B {
        fn name(&self) -> &'static str {
            "B"
        }
        fn depends_on(&self) -> &'static [&'static str] {
            &["A"]
        }
    }
    impl CompilerExtension for C {
        fn name(&self) -> &'static str {
            "C"
        }
        fn depends_on(&self) -> &'static [&'static str] {
            &["B"]
        }
    }
    impl CompilerExtension for ADup {
        fn name(&self) -> &'static str {
            "A"
        }
    }
    impl CompilerExtension for BadDep {
        fn name(&self) -> &'static str {
            "BadDep"
        }
        fn depends_on(&self) -> &'static [&'static str] {
            &["DoesNotExist"]
        }
    }

    #[test]
    fn discovered_empty_slice_returns_empty_vec() {
        // Phase-1 invariant; once Phase 3 lands the count goes up.
        assert!(discovered_compiler_extensions().is_empty());
    }

    #[test]
    fn topo_sort_orders_by_depends_on() {
        // Input order C, A, B but C depends on B which depends on A,
        // so expected order is A, B, C.
        let exts: [&'static (dyn CompilerExtension + Sync); 3] = [&C, &A, &B];
        let out = topo_sort_compiler_extensions(exts.iter().copied());
        let names: Vec<&str> = out.iter().map(|e| e.name()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
    }

    #[test]
    #[should_panic(expected = "two extensions registered under name `A`")]
    fn topo_sort_panics_on_duplicate_names() {
        let exts: [&'static (dyn CompilerExtension + Sync); 2] = [&A, &ADup];
        let _ = topo_sort_compiler_extensions(exts.iter().copied());
    }

    #[test]
    #[should_panic(expected = "declares dependency on `DoesNotExist`")]
    fn topo_sort_panics_on_unknown_dep() {
        let exts: [&'static (dyn CompilerExtension + Sync); 1] = [&BadDep];
        let _ = topo_sort_compiler_extensions(exts.iter().copied());
    }

    // Cyclic pair — Cyc1 depends on Cyc2, Cyc2 depends on Cyc1.
    struct Cyc1;
    struct Cyc2;
    impl CompilerExtension for Cyc1 {
        fn name(&self) -> &'static str {
            "Cyc1"
        }
        fn depends_on(&self) -> &'static [&'static str] {
            &["Cyc2"]
        }
    }
    impl CompilerExtension for Cyc2 {
        fn name(&self) -> &'static str {
            "Cyc2"
        }
        fn depends_on(&self) -> &'static [&'static str] {
            &["Cyc1"]
        }
    }

    #[test]
    #[should_panic(expected = "dependency cycle among extensions")]
    fn topo_sort_panics_on_cycle() {
        let exts: [&'static (dyn CompilerExtension + Sync); 2] = [&Cyc1, &Cyc2];
        let _ = topo_sort_compiler_extensions(exts.iter().copied());
    }
}
