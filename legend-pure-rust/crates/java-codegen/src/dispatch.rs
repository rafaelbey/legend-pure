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

//! Bindings-manifest parsing + per-kind seed dispatch.
//!
//! Powers the "bootstrap from a manifest" workflow used by both the
//! `legend java-bindings --bindings-file` CLI flag and the
//! annotation-processor JNI entry point. A manifest is a UTF-8 text
//! file with two kinds of lines:
//!
//! * **Directive lines** — `@<key>: <value>`, parsed by
//!   [`parse_manifest`]. Recognised keys:
//!   - `@pkg` — Java root package every emitted class lives under.
//!     Overrides any caller-supplied default.
//!   - `@functions-class` — simple class name for the static-functions
//!     facade (otherwise defaults to `PureFunctions`).
//!   - `@import` — *not parsed here.* The annotation processor resolves
//!     these against its compile classpath before invoking codegen and
//!     surfaces the result as [`Options::external_bindings`].
//!     See `legend-pure-runtime-rust-evaluator-bindings-ap` for the
//!     resolution rules.
//! * **FQN lines** — one Pure fully-qualified name per line. Blank and
//!   `#`-prefixed comment lines are ignored.
//!
//! [`dispatch_bindings_by_kind`] takes already-parsed FQN strings and
//! routes each to its kind bucket — it doesn't re-parse directives.

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

/// One imported manifest reference. Surfaced by [`parse_manifest`] so
/// the caller (typically an annotation processor) can resolve the
/// `path` against its own classpath/filesystem and merge the imported
/// FQNs into [`Options::external_bindings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestImport {
    /// Resource path as written in the `@import` directive — the
    /// caller decides how to resolve it (`Filer.getResource(...)` for
    /// the AP, an absolute filesystem path for the CLI, …).
    pub path: String,
}

/// Parsed manifest with directives separated from FQN body.
#[derive(Debug, Clone, Default)]
pub struct ParsedManifest {
    /// Java root package the manifest declares via `@pkg:`. Caller
    /// merges with any annotation-supplied default.
    pub pkg: Option<String>,
    /// Static-functions facade simple-name override declared via
    /// `@functions-class:`.
    pub functions_class: Option<String>,
    /// Imports declared via `@import:` — resolved by the caller.
    pub imports: Vec<ManifestImport>,
    /// Body lines: stripped FQNs, in order, with blank/comment lines
    /// removed.
    pub fqns: Vec<String>,
}

/// Parse a manifest's raw lines into a [`ParsedManifest`].
///
/// Directive lines (`@<key>: <value>`) are extracted; FQN lines are
/// trimmed and accumulated. Blank lines and `#`-prefixed comments are
/// ignored. An unknown directive key is a hard error so typos surface
/// loudly.
///
/// # Errors
///
/// Returns [`CodegenError::ManifestSyntax`] for malformed directive
/// lines (no colon, or unknown key).
pub fn parse_manifest(raw_lines: &[String]) -> Result<ParsedManifest, CodegenError> {
    let mut out = ParsedManifest::default();
    for raw in raw_lines {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('@') {
            let (key, value) =
                rest.split_once(':')
                    .ok_or_else(|| CodegenError::ManifestSyntax {
                        line: trimmed.to_owned(),
                        reason: "directive must be `@<key>: <value>`".to_owned(),
                    })?;
            let key = key.trim().to_owned();
            let value = value.trim().to_owned();
            match key.as_str() {
                "pkg" => out.pkg = Some(value),
                "functions-class" => out.functions_class = Some(value),
                "import" => out.imports.push(ManifestImport { path: value }),
                unknown => {
                    return Err(CodegenError::ManifestSyntax {
                        line: trimmed.to_owned(),
                        reason: format!(
                            "unknown directive `@{unknown}` — supported: \
                             @pkg, @functions-class, @import"
                        ),
                    });
                }
            }
            continue;
        }
        out.fqns.push(trimmed.to_owned());
    }
    Ok(out)
}

/// Walk `lines` and, for each entry, resolve it against `model` and
/// route it to the matching slot.
///
/// Entries are taken verbatim — callers are responsible for parsing
/// directives (via [`parse_manifest`]) and stripping blank/comment
/// lines first.
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
