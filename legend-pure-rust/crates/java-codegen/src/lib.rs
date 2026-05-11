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

//! Java wrapper code generator for the JNI evaluator.
//!
//! Given a [`PureModel`](legend_pure_parser_pure::PureModel) and a list of
//! Pure function FQNs, produces a set of [`JavaFile`]s implementing typed
//! Java static-method facades plus interfaces for every reachable user
//! `Class`/`Enumeration` in the property graph. The generated code targets
//! the runtime support library in
//! `legend-pure-runtime/legend-pure-runtime-rust-evaluator` (see the
//! `org.finos.legend.pure.rust.proxy` package).
//!
//! The crate is **library-only** — it never touches the filesystem or the
//! JNI; the `legend java-bindings` CLI command in
//! `legend-pure-rust/crates/cli` is the I/O driver.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod closure;
mod dispatch;
mod enums;
mod functions;
mod interfaces;
mod model;
mod naming;
mod types;

pub use crate::dispatch::{
    DispatchedBindings, ManifestImport, ParsedManifest, dispatch_bindings_by_kind, parse_manifest,
};
pub use crate::model::{CodegenError, FqnInput, JavaFile, Options};

use legend_pure_parser_pure::model::PureModel;

/// Generate Java wrapper sources for the requested Pure elements.
///
/// `fns` populates the static-method facade. `extra_classes` and
/// `extra_associations` extend the reachability seed set so callers can
/// request interfaces for elements that aren't referenced by any
/// requested function (e.g. a class consumed only by user code, or an
/// association whose endpoints would otherwise be dropped). For an
/// association seed, both participating classes are added to the seed set.
///
/// Returns one [`JavaFile`] per emitted Java source. The caller is
/// responsible for materializing those files on disk.
///
/// # Errors
///
/// Returns [`CodegenError`] when:
/// - a requested function FQN does not resolve (`UnresolvedFunction`),
///   resolves to a non-function (`NotAFunction`), or has unsupported
///   parameter/return types (`FunctionTypedParameter`, `RelationTyped`,
///   `GenericTyped`);
/// - a class FQN passed via `extra_classes` does not resolve
///   (`UnresolvedClass`) or resolves to a non-class (`NotAClass`);
/// - an association FQN passed via `extra_associations` does not resolve
///   (`UnresolvedAssociation`) or resolves to a non-association
///   (`NotAnAssociation`).
pub fn generate(
    model: &PureModel,
    fns: &[FqnInput],
    extra_classes: &[FqnInput],
    extra_associations: &[FqnInput],
    opts: &Options,
) -> Result<Vec<JavaFile>, CodegenError> {
    let bootstrap = model::Bootstrap::resolve(model);
    let resolved = model::resolve_requested(model, fns)?;
    let extra_seeds = model::resolve_extra_seeds(model, extra_classes, extra_associations)?;
    let closure = closure::reachable_types(model, &resolved, &extra_seeds, bootstrap, opts);

    let mut files: Vec<JavaFile> =
        Vec::with_capacity(2 + closure.classes.len() + closure.enums.len());

    files.push(functions::emit_functions_class(
        model, &resolved, &closure, opts, bootstrap,
    )?);
    for cls_id in &closure.classes {
        files.push(interfaces::emit_class_interface(
            model, *cls_id, &closure, opts, bootstrap,
        )?);
    }
    for enum_id in &closure.enums {
        files.push(enums::emit_enum(model, *enum_id, opts));
    }
    Ok(files)
}
