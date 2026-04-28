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

use legend_pure_parser_ast::section::SourceFile;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::model::PureModel;

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
}
