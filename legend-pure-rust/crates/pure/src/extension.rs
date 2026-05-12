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

use std::collections::HashMap;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::section::SourceFile;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::model::PureModel;
use crate::types::{Multiplicity, Parameter, ResolvedType, TypeExpr};

/// Context passed to [`CompilerExtension::declare`].
///
/// Borrows the source files (read-only) and the in-progress model
/// (mutable). Errors collected during declaration go into `errors`.
pub struct DeclareCtx<'a> {
    /// Parsed source files for this compilation unit.
    pub source_files: &'a [SourceFile],
    /// The compiler's current model. Extensions allocate shells here.
    pub model: &'a mut PureModel,
    /// Auto-imports applied to every section in this compilation.
    pub auto_imports: &'a [SmolStr],
    /// Accumulated compilation errors.
    pub errors: &'a mut Vec<CompilationError>,
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

    /// Contribute references to the IDE's reference index.
    ///
    /// Called by [`crate::refs::build_reference_index`] (and any
    /// other [`crate::refs::walk_references`] caller) after all
    /// compile passes complete. The extension walks its own
    /// resolved data — typically `Element::DSLInstance` payloads
    /// written during `declare` — and pushes one
    /// [`crate::refs::Reference`] per source-level reference site.
    ///
    /// Default no-op so existing extensions that don't yet
    /// participate in IDE navigation keep compiling. The Mapping,
    /// Relational, and Store DSLs override to surface their own
    /// references (class refs in `: Pure { … }`, table refs in
    /// `Join` clauses, etc.).
    fn walk_references(
        &self,
        _model: &crate::model::PureModel,
        _visit: &mut dyn FnMut(crate::refs::Reference),
    ) {
    }
}

// ---------------------------------------------------------------------------
// Lower-and-infer wrapper
// ---------------------------------------------------------------------------

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
