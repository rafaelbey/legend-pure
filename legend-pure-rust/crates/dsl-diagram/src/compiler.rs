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

//! [`CompilerExtension`] implementation for the Diagram DSL.
//!
//! The extension walks each parsed source file's `###Diagram`
//! sections, downcasts every `Element::DSLElement` to `DiagramDef`,
//! and registers each diagram in two parallel places:
//!
//! 1. **The model graph** as `Element::DSLInstance` — this carries the
//!    diagram through `.purem` slice/merge automatically (the chunk
//!    machinery handles it just like any `Class` or `Function`). The
//!    payload is a Postcard-encoded [`DiagramSnapshot`] keyed by the
//!    `"Diagram"` DSL name.
//! 2. **Extension-owned state** (the legacy `Self::diagrams`
//!    `RefCell<HashMap>`) — kept for now so existing in-process
//!    consumers (tests, codegen, validate pass) keep working without
//!    a forced migration. Reads via `Self::diagrams` return whatever
//!    the in-process compile registered; reads via
//!    `Self::diagrams_from_model` decode from the graph and survive
//!    `.purem` round-trips.
//!
//! The dual-write keeps validate-pass logic intact (it still walks
//! the rich AST for source-info-bearing diagnostics) while making
//! `.purem` round-trips first-class. Migrating consumers to the
//! graph-walk side and deleting the `RefCell` is follow-up work.

use std::collections::HashMap;

use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{
    COMPILER_EXTENSIONS, CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx,
};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{
    DSLInstance, Element as ModelElement, ElementNode, PureModel,
};
use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::ast::{DiagramDef, DiagramView};

/// Stable name used to key Diagram payloads in `Element::DSLInstance`.
pub const DIAGRAM_DSL_NAME: &str = "Diagram";

/// FQN of the M3 metaclass `Diagram` instances are typed against.
pub const DIAGRAM_CLASSIFIER_FQN: &str = "meta::pure::diagram::Diagram";

/// Compiler extension for the Diagram DSL.
///
/// Stateless unit struct — per-compile data lives in
/// `DiagramCompileState` stashed in `ctx.scope`. Lets the
/// extension self-register via the
/// [`COMPILER_EXTENSIONS`] distributed slice so any binary that
/// depends on `legend-pure-dsl-diagram` picks it up automatically.
#[derive(Debug, Default)]
pub struct DiagramExtension;

/// Self-registration into the compiler's `COMPILER_EXTENSIONS`
/// distributed slice. Discoverable via
/// [`legend_pure_parser_pure::pipeline::compile`] (no per-binary
/// wiring required for downstream consumers).
#[distributed_slice(COMPILER_EXTENSIONS)]
static DIAGRAM_EXTENSION: &(dyn CompilerExtension + Sync) = &DiagramExtension;

/// Per-compile scratch state stashed in `ctx.scope`. Holds the
/// diagrams collected during `declare` so `validate` can re-walk
/// them with their AST in hand. Fresh per compile — the pipeline's
/// scope is reset between `compile_with_extensions` calls.
#[derive(Default)]
struct DiagramCompileState {
    /// Diagrams collected during `declare`, keyed by their FQN
    /// (`"pkg::sub::Name"`).
    diagrams: HashMap<SmolStr, RegisteredDiagram>,
}

/// One registered diagram alongside its bookkeeping ID.
#[derive(Debug, Clone)]
pub struct RegisteredDiagram {
    /// The original AST.
    pub def: DiagramDef,
    /// FQN this diagram is registered under.
    pub fqn: SmolStr,
}

/// Compiled, serializable form of a Diagram — what survives a `.purem`
/// round-trip.
///
/// The pilot intentionally captures only the **identifying** data
/// (FQN + classifier + per-view summary). Lossless `DiagramDef`
/// preservation through `.purem` is follow-up work and would need a
/// matching set of serializable view-summary types; it is not
/// load-bearing for the current pilot, whose job is to demonstrate the
/// `Element::DSLInstance` round-trip end-to-end.
///
/// The view summaries are deliberately string-based (`view_kind` is
/// the textual tag, `target_fqn` the FQN of the referenced element).
/// This keeps the snapshot decoupled from `legend-pure-parser-ast`'s
/// non-serde types (`Identifier`, `TypeReference`, `SourceInfo`,
/// `SpannedString`) so the architectural rule "ast crate has no serde"
/// stays intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramSnapshot {
    /// FQN this snapshot's diagram lives at, e.g.
    /// `"model::test::TinyDiagram"`.
    pub fqn: SmolStr,
    /// Optional geometry header (`width`, `height`) from the
    /// `Diagram name(width=…, height=…)` declaration.
    pub geometry: Option<DiagramGeometrySnapshot>,
    /// Per-view summary — kind tag + optional referenced FQN. The
    /// referenced FQN is `None` for `GeneralizationView` (which only
    /// references local TypeView ids, not model elements).
    pub views: Vec<DiagramViewSnapshot>,
}

/// Round-trippable form of `DiagramGeometry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramGeometrySnapshot {
    /// Width in diagram coordinates.
    pub width: f64,
    /// Height in diagram coordinates.
    pub height: f64,
}

/// One view's compiled-form summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramViewSnapshot {
    /// Discriminator: `"TypeView"`, `"AssociationView"`, `"PropertyView"`,
    /// `"GeneralizationView"`.
    pub view_kind: SmolStr,
    /// Local id of the view inside its diagram (referenced by `source=`
    /// / `target=` in edges).
    pub local_id: SmolStr,
    /// FQN of the referenced model element. `None` for views whose
    /// reference is non-element (e.g. `GeneralizationView`'s
    /// source/target are local TypeView ids, not model elements).
    pub target_fqn: Option<SmolStr>,
}

impl DiagramSnapshot {
    /// Build a snapshot from an in-memory `DiagramDef` AST.
    #[must_use]
    pub fn from_def(def: &DiagramDef, fqn: SmolStr) -> Self {
        let views = def
            .views
            .iter()
            .map(|v| match v {
                DiagramView::Type(t) => DiagramViewSnapshot {
                    view_kind: SmolStr::new("TypeView"),
                    local_id: t.id.clone(),
                    target_fqn: Some(SmolStr::new(t.type_ref.full_path())),
                },
                DiagramView::Association(a) => DiagramViewSnapshot {
                    view_kind: SmolStr::new("AssociationView"),
                    local_id: a.id.clone(),
                    target_fqn: Some(SmolStr::new(a.association.full_path())),
                },
                DiagramView::Property(p) => DiagramViewSnapshot {
                    view_kind: SmolStr::new("PropertyView"),
                    local_id: p.id.clone(),
                    target_fqn: Some(SmolStr::new(format!(
                        "{}.{}",
                        p.property.class.full_path(),
                        p.property.property
                    ))),
                },
                DiagramView::Generalization(g) => DiagramViewSnapshot {
                    view_kind: SmolStr::new("GeneralizationView"),
                    local_id: g.id.clone(),
                    target_fqn: None,
                },
            })
            .collect();
        let geometry = def.geometry.as_ref().map(|g| DiagramGeometrySnapshot {
            width: g.width,
            height: g.height,
        });
        Self {
            fqn,
            geometry,
            views,
        }
    }

    /// Encode for storage in `Element::DSLInstance.data`.
    ///
    /// # Errors
    /// Postcard never fails on well-typed inputs in practice, but the
    /// error is propagated rather than panicking.
    pub fn encode(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Decode a payload produced by [`Self::encode`].
    ///
    /// # Errors
    /// Returns the underlying Postcard error on corrupt or truncated input.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

impl DiagramExtension {
    /// Construct an empty extension.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all registered diagrams **as graph elements** —
    /// walks `model.elements()` for `Element::DSLInstance` entries
    /// keyed `"Diagram"` and decodes each payload.
    ///
    /// Survives `.purem` slice/merge: every diagram registered during
    /// the original compile reappears here after a fresh model is
    /// built from a serialised slice.
    ///
    /// Returns `(fqn, snapshot)` pairs in iteration order over the
    /// model's chunks. Decoding errors are dropped silently — a
    /// well-formed `.purem` will never produce them, and a corrupt
    /// payload should already have been caught by the writer's schema
    /// hash on read.
    #[must_use]
    pub fn diagrams_from_model(model: &PureModel) -> Vec<(SmolStr, DiagramSnapshot)> {
        let mut out = Vec::new();
        for chunk in &model.chunks {
            for (_, element) in chunk.elements.iter() {
                let ModelElement::DSLInstance(d) = element else {
                    continue;
                };
                if d.dsl_name.as_str() != DIAGRAM_DSL_NAME {
                    continue;
                }
                if let Ok(snapshot) = DiagramSnapshot::decode(&d.data) {
                    out.push((snapshot.fqn.clone(), snapshot));
                }
            }
        }
        out
    }
}

impl CompilerExtension for DiagramExtension {
    fn name(&self) -> &'static str {
        "diagram"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        // Pass 1 has already created (or reused) the slice's chunk
        // and pushed it onto `model.chunks`. The just-created chunk
        // is the last one — we allocate `Element::DSLInstance` rows
        // there alongside the M3 elements from the same source file.
        let chunk_id = (ctx.model.chunks.len().saturating_sub(1)) as u16;

        // Destructure ctx so model, errors, scope, source_files are
        // independently borrowable inside the loop. The scope chain
        // hands us a `&mut HashMap<SmolStr, RegisteredDiagram>` (the
        // per-compile registry) that lives for the duration of this
        // declare call.
        let DeclareCtx {
            source_files,
            model,
            errors,
            scope,
            ..
        } = ctx;
        // Per DeclareCtx's documented contract: stateful extensions
        // skip when `scope` is None. The normal pipeline wires it;
        // hand-built test contexts that exercise other behavior
        // legitimately omit it, in which case this extension has no
        // per-compile registry to populate and silently no-ops.
        let Some(scope) = scope.as_deref_mut() else {
            return;
        };
        let registry = &mut scope.get_or_default::<DiagramCompileState>().diagrams;

        for source_file in *source_files {
            for section in &source_file.sections {
                if section.kind.as_str() != crate::ast::SECTION_KIND {
                    continue;
                }
                for elem in &section.elements {
                    let AstElement::DSLElement(boxed) = elem else {
                        continue;
                    };
                    let Some(d) = boxed.as_any().downcast_ref::<DiagramDef>() else {
                        continue;
                    };

                    let fqn = build_fqn(d);
                    if registry.contains_key(&fqn) {
                        errors.push(CompilationError {
                            message: format!("Duplicate diagram '{fqn}'"),
                            source_info: d.source_info.clone(),
                            kind: CompilationErrorKind::DuplicateElement { name: fqn.clone() },
                        });
                        // First registration wins; skip the second so we
                        // don't double-register the graph element either.
                        continue;
                    }

                    // Dual-write 1/2 — per-compile registry (validate
                    // reads from here for AST-backed checks).
                    registry.insert(
                        fqn.clone(),
                        RegisteredDiagram {
                            def: d.clone(),
                            fqn: fqn.clone(),
                        },
                    );

                    // Dual-write 2/2 — model graph as `Element::DSLInstance`.
                    // Encodes a `DiagramSnapshot` so the diagram round-
                    // trips through `.purem` slice/merge alongside every
                    // M3 element without any new serialisation surface.
                    let snapshot = DiagramSnapshot::from_def(d, fqn.clone());
                    let data = match snapshot.encode() {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            errors.push(CompilationError {
                                message: format!(
                                    "Failed to encode DiagramSnapshot for '{fqn}': {e}"
                                ),
                                source_info: d.source_info.clone(),
                                kind: CompilationErrorKind::DuplicateElement { name: fqn.clone() },
                            });
                            continue;
                        }
                    };

                    let pkg_path = pkg_segments(d);
                    let package_id = if pkg_path.is_empty() {
                        model.root_package
                    } else {
                        model.get_or_create_package(&pkg_path)
                    };

                    let Some(chunk) = model.chunks.get_mut(chunk_id as usize) else {
                        continue; // pathological: no chunk yet
                    };
                    let local_idx = chunk.alloc_element(
                        ElementNode {
                            name: d.name.value.clone(),
                            source_info: d.source_info.clone(),
                            name_source_info: d.name.source_info.clone(),
                            parent_package: package_id,
                        },
                        ModelElement::DSLInstance(DSLInstance {
                            dsl_name: SmolStr::new(DIAGRAM_DSL_NAME),
                            classifier_fqn: SmolStr::new(DIAGRAM_CLASSIFIER_FQN),
                            data,
                        }),
                    );
                    let id = ElementId::InstanceId {
                        chunk_id,
                        local_idx,
                    };
                    model.register_element(package_id, id);
                }
            }
        }
    }

    fn define_signatures(&self, _ctx: &mut DefineCtx<'_>) {
        // No signature-shape work needed today: type references are
        // validated lazily in `validate`. If a future feature
        // requires resolving references before bodies of *other*
        // elements (M3 or another DSL) reach Pass 2b, do that here.
    }

    fn define_bodies(&self, _ctx: &mut DefineCtx<'_>) {
        // Diagrams have no executable bodies.
    }

    fn validate(&self, ctx: &mut ValidateCtx<'_>) {
        // Pull the per-compile registry the declare hook stashed in
        // the scope. Absent scope (hand-built ctx) or no diagrams
        // declared in this compile — both no-op cleanly.
        let Some(scope) = ctx.scope else { return };
        let Some(state) = scope.get::<DiagramCompileState>() else {
            return;
        };
        for (_fqn, reg) in state.diagrams.iter() {
            validate_diagram(&reg.def, ctx.model, ctx.errors);
        }
    }
}

fn validate_diagram(d: &DiagramDef, model: &PureModel, errors: &mut Vec<CompilationError>) {
    // Local TypeView IDs that subsequent edges may reference via
    // `source=` / `target=`.
    let local_type_view_ids: std::collections::HashSet<&str> = d
        .views
        .iter()
        .filter_map(|v| match v {
            DiagramView::Type(t) => Some(t.id.as_str()),
            _ => None,
        })
        .collect();

    for v in &d.views {
        match v {
            DiagramView::Type(t) => {
                if !is_class_or_type(model, &t.type_ref.full_path()) {
                    push_unresolved(
                        errors,
                        &format!(
                            "TypeView '{}' references unresolved type '{}'",
                            t.id,
                            t.type_ref.full_path()
                        ),
                        &t.source_info,
                        SmolStr::new(t.type_ref.full_path()),
                    );
                }
            }
            DiagramView::Association(a) => {
                if !is_association(model, &a.association.full_path()) {
                    push_unresolved(
                        errors,
                        &format!(
                            "AssociationView '{}' references unresolved association '{}'",
                            a.id,
                            a.association.full_path()
                        ),
                        &a.source_info,
                        SmolStr::new(a.association.full_path()),
                    );
                }
                check_edge_endpoints(
                    a.source.as_ref(),
                    a.target.as_ref(),
                    &local_type_view_ids,
                    &a.id,
                    "AssociationView",
                    &a.source_info,
                    errors,
                );
            }
            DiagramView::Property(p) => {
                let class_path = p.property.class.full_path();
                if !is_class_or_type(model, &class_path) {
                    push_unresolved(
                        errors,
                        &format!(
                            "PropertyView '{}' references unresolved class '{}'",
                            p.id, class_path
                        ),
                        &p.source_info,
                        SmolStr::new(class_path.clone()),
                    );
                } else if !class_has_property(model, &class_path, p.property.property.as_str()) {
                    push_unresolved(
                        errors,
                        &format!(
                            "PropertyView '{}' references unknown property '{}.{}'",
                            p.id, class_path, p.property.property
                        ),
                        &p.source_info,
                        SmolStr::new(format!("{class_path}.{}", p.property.property)),
                    );
                }
                check_edge_endpoints(
                    p.source.as_ref(),
                    p.target.as_ref(),
                    &local_type_view_ids,
                    &p.id,
                    "PropertyView",
                    &p.source_info,
                    errors,
                );
            }
            DiagramView::Generalization(g) => {
                check_edge_endpoints(
                    g.source.as_ref(),
                    g.target.as_ref(),
                    &local_type_view_ids,
                    &g.id,
                    "GeneralizationView",
                    &g.source_info,
                    errors,
                );
            }
        }
    }
}

fn check_edge_endpoints(
    source: Option<&SmolStr>,
    target: Option<&SmolStr>,
    local: &std::collections::HashSet<&str>,
    edge_id: &SmolStr,
    edge_kind: &str,
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    for (label, opt) in [("source", source), ("target", target)] {
        if let Some(id) = opt
            && !local.contains(id.as_str())
        {
            errors.push(CompilationError {
                message: format!(
                    "{edge_kind} '{edge_id}' {label}='{id}' is not a TypeView in this diagram"
                ),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: SmolStr::new(id.as_str()),
                },
            });
        }
    }
}

fn is_class_or_type(model: &PureModel, fqn: &str) -> bool {
    resolve(model, fqn).is_some_and(|id| {
        matches!(
            model.get_element(id),
            ModelElement::Class(_) | ModelElement::PrimitiveType(_) | ModelElement::Enumeration(_)
        )
    })
}

fn is_association(model: &PureModel, fqn: &str) -> bool {
    resolve(model, fqn)
        .is_some_and(|id| matches!(model.get_element(id), ModelElement::Association(_)))
}

fn class_has_property(model: &PureModel, class_fqn: &str, property: &str) -> bool {
    let Some(id) = resolve(model, class_fqn) else {
        return false;
    };
    let ModelElement::Class(c) = model.get_element(id) else {
        return false;
    };
    c.properties.iter().any(|p| p.name.as_str() == property)
        || c.qualified_properties
            .iter()
            .any(|q| q.name.as_str() == property)
}

fn resolve(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    model.resolve_by_path(&segments)
}

fn push_unresolved(
    errors: &mut Vec<CompilationError>,
    message: &str,
    source_info: &legend_pure_parser_ast::SourceInfo,
    path: SmolStr,
) {
    errors.push(CompilationError {
        message: message.to_string(),
        source_info: source_info.clone(),
        kind: CompilationErrorKind::UnresolvedElement { path },
    });
}

fn build_fqn(d: &DiagramDef) -> SmolStr {
    if let Some(pkg) = d.package() {
        SmolStr::new(format!("{pkg}::{}", d.name.value))
    } else {
        d.name.value.clone()
    }
}

/// Package path as `Vec<SmolStr>` segments for
/// [`PureModel::get_or_create_package`].
fn pkg_segments(d: &DiagramDef) -> Vec<SmolStr> {
    match d.package() {
        Some(pkg) => pkg.segments().into_iter().cloned().collect(),
        None => Vec::new(),
    }
}
