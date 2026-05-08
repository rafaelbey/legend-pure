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

//! Resolve a flat list of FQNs into per-kind seed buckets for codegen.
//!
//! Powers the "bootstrap from a manifest" workflow used by both the
//! `legend java-bindings --bindings-file` CLI flag and the
//! annotation-processor JNI entry point: a single text file lists Pure
//! FQNs (one per line, blank and `#`-prefixed lines ignored), and this
//! helper walks them, dispatches each entry by element kind, and
//! returns the per-kind buckets the [`generate`](crate::generate) entry
//! point consumes.

use legend_pure_parser_pure::model::{Element, PureModel};

use crate::model::{CodegenError, FqnInput, element_kind};

/// Result of dispatching a kind-agnostic bindings list.
///
/// Each bucket is in the same order entries were encountered in the
/// input list, so callers preserve any deliberate ordering the manifest
/// author chose (e.g. metadata functions last so they show up in a
/// predictable spot in the generated facade).
#[derive(Debug, Default, Clone)]
pub struct DispatchedBindings {
    /// FQNs that resolved to a `Function` element.
    pub functions: Vec<FqnInput>,
    /// FQNs that resolved to a `Class` element.
    pub classes: Vec<FqnInput>,
    /// FQNs that resolved to an `Association` element.
    pub associations: Vec<FqnInput>,
}

/// Walk `lines` and, for each entry, resolve it against `model` and
/// route it to the matching slot.
///
/// Entries are taken verbatim — callers are responsible for stripping
/// blank/comment lines first. (The CLI does this in
/// `commands/java_bindings.rs::run`; the JNI shim does it in
/// `crates/jni/src/codegen.rs`.)
///
/// Entries whose FQN resolves to a `Function` end up in
/// [`DispatchedBindings::functions`], `Class` in
/// [`DispatchedBindings::classes`], `Association` in
/// [`DispatchedBindings::associations`]. Any other element kind
/// triggers [`CodegenError::BindingsFileEntryWrongKind`]. Unresolved
/// FQNs trigger [`CodegenError::BindingsFileEntryUnresolved`].
///
/// # Errors
///
/// See above.
pub fn dispatch_bindings_by_kind(
    model: &PureModel,
    lines: &[String],
) -> Result<DispatchedBindings, CodegenError> {
    let mut out = DispatchedBindings::default();
    for raw in lines {
        let id = model
            .resolve_fqn_str(raw)
            .ok_or_else(|| CodegenError::BindingsFileEntryUnresolved { fqn: raw.clone() })?;
        match model.get_element(id) {
            Element::Function(_) => out.functions.push(FqnInput::new(raw.clone())),
            Element::Class(_) => out.classes.push(FqnInput::new(raw.clone())),
            Element::Association(_) => out.associations.push(FqnInput::new(raw.clone())),
            other => {
                return Err(CodegenError::BindingsFileEntryWrongKind {
                    fqn: raw.clone(),
                    kind: element_kind(other),
                });
            }
        }
    }
    Ok(out)
}
