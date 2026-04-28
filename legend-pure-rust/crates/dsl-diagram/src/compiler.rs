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
//! and registers it in extension-owned state keyed by FQN. Type
//! references inside views (`TypeView.type_ref`,
//! `AssociationView.association`, `PropertyView.property.class`)
//! are validated against the [`PureModel`] in the `validate` pass.
//!
//! The extension stores its own diagram registry rather than
//! contributing a `Box<dyn ModelDSLElement>` to `PureModel::Element`
//! — that route requires adding a new variant to the compiled
//! model enum and is deferred until cross-DSL references make it
//! load-bearing. For Diagram alone, "extension-owned per-FQN
//! storage" is sufficient and keeps the contract narrower.

use std::cell::RefCell;
use std::collections::HashMap;

use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::Element as ModelElement;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

use crate::ast::{DiagramDef, DiagramView};

/// Compiler extension for the Diagram DSL.
///
/// Construct one per [`compile_with_extensions`](legend_pure_parser_pure::pipeline::compile_with_extensions)
/// invocation; do not share across compilations because the
/// extension's internal map is reset each call.
#[derive(Default)]
pub struct DiagramExtension {
    /// Diagrams collected during `declare`, keyed by their FQN
    /// (`"pkg::sub::Name"`). Per-extension state — not stored in
    /// `PureModel`. Use [`Self::diagrams`] to inspect after compile.
    diagrams: RefCell<HashMap<SmolStr, RegisteredDiagram>>,
}

/// One registered diagram alongside its bookkeeping ID.
#[derive(Debug, Clone)]
pub struct RegisteredDiagram {
    /// The original AST.
    pub def: DiagramDef,
    /// FQN this diagram is registered under.
    pub fqn: SmolStr,
}

impl DiagramExtension {
    /// Construct an empty extension.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all registered diagrams keyed by FQN. Useful for
    /// tests, codegen, and CLI inspection after compile completes.
    #[must_use]
    pub fn diagrams(&self) -> HashMap<SmolStr, RegisteredDiagram> {
        self.diagrams.borrow().clone()
    }
}

impl CompilerExtension for DiagramExtension {
    fn name(&self) -> &'static str {
        "diagram"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let mut registry = self.diagrams.borrow_mut();
        for source_file in ctx.source_files.iter() {
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
                    if let Some(prev) = registry.insert(
                        fqn.clone(),
                        RegisteredDiagram {
                            def: d.clone(),
                            fqn: fqn.clone(),
                        },
                    ) {
                        ctx.errors.push(CompilationError {
                            message: format!("Duplicate diagram '{fqn}'"),
                            source_info: d.source_info.clone(),
                            kind: CompilationErrorKind::DuplicateElement { name: fqn },
                        });
                        // Restore prior registration so subsequent
                        // passes still see the earlier definition.
                        registry.insert(prev.fqn.clone(), prev);
                    }
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
        let registry = self.diagrams.borrow();
        for (_fqn, reg) in registry.iter() {
            validate_diagram(&reg.def, ctx.model, ctx.errors);
        }
    }
}

// ---------------------------------------------------------------------------
// Per-diagram validation
// ---------------------------------------------------------------------------

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
    resolve(model, fqn)
        .map(|id| {
            matches!(
                model.get_element(id),
                ModelElement::Class(_)
                    | ModelElement::PrimitiveType(_)
                    | ModelElement::Enumeration(_)
            )
        })
        .unwrap_or(false)
}

fn is_association(model: &PureModel, fqn: &str) -> bool {
    resolve(model, fqn)
        .map(|id| matches!(model.get_element(id), ModelElement::Association(_)))
        .unwrap_or(false)
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
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
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

// ---------------------------------------------------------------------------
// FQN
// ---------------------------------------------------------------------------

fn build_fqn(d: &DiagramDef) -> SmolStr {
    if let Some(pkg) = d.package() {
        SmolStr::new(format!("{pkg}::{}", d.name.value))
    } else {
        d.name.value.clone()
    }
}
