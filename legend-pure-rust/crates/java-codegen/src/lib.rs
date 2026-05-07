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
mod enums;
mod functions;
mod interfaces;
mod model;
mod naming;
mod types;

pub use crate::model::{CodegenError, FqnInput, JavaFile, Options};

use legend_pure_parser_pure::model::PureModel;

/// Generate Java wrapper sources for the requested Pure functions.
///
/// Returns one [`JavaFile`] per emitted Java source. The caller is
/// responsible for materializing those files on disk.
///
/// # Errors
///
/// Returns [`CodegenError`] when:
/// - a requested FQN does not resolve to a function (`UnresolvedFunction`)
/// - a requested function has a `Function<{...}>`-typed parameter or return
///   (`FunctionTypedParameter`) — deferred to v2
/// - a requested function has a relation-typed parameter or return
///   (`RelationTyped`)
/// - the generic-typed name set is otherwise malformed (`Internal`)
pub fn generate(
    model: &PureModel,
    fns: &[FqnInput],
    opts: &Options,
) -> Result<Vec<JavaFile>, CodegenError> {
    let resolved = model::resolve_requested(model, fns)?;
    let closure = closure::reachable_types(model, &resolved);

    let mut files: Vec<JavaFile> =
        Vec::with_capacity(2 + closure.classes.len() + closure.enums.len());

    files.push(functions::emit_functions_class(
        model, &resolved, &closure, opts,
    )?);
    for cls_id in &closure.classes {
        files.push(interfaces::emit_class_interface(
            model, *cls_id, &closure, opts,
        )?);
    }
    for enum_id in &closure.enums {
        files.push(enums::emit_enum(model, *enum_id, opts)?);
    }
    Ok(files)
}
