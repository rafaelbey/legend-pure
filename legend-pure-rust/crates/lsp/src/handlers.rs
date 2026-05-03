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

//! Pure functions implementing each LSP request — pulled out of the
//! `Backend` impl so they can be exercised in unit tests without a
//! tokio runtime or `tower_lsp::Client`.

use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::locate::{Located, LocatedKind};
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ResolvedType, TypeExpr};
use tower_lsp::lsp_types::{
    CodeLens, Command, Diagnostic, DocumentSymbol, Hover, HoverContents, Location, MarkupContent,
    MarkupKind, Position, SymbolKind, Url,
};

use crate::convert::{self, range_from_source_info};
use crate::diagnostics;

/// Build the diagnostic list for a single source path.
#[must_use]
pub fn diagnostics_for(errors: &[CompilationError]) -> Vec<Diagnostic> {
    errors.iter().map(diagnostics::to_diagnostic).collect()
}

/// Compute hover content for a position. Returns `None` when nothing
/// is under the cursor or the located node carries no inferred type.
#[must_use]
pub fn hover_for_position(
    model: &PureModel,
    canonical_path: &str,
    position: Position,
) -> Option<Hover> {
    let (line, column) = convert::position_to_1indexed(position);
    let located = model.locate(canonical_path, line, column)?;
    let body = render_hover(model, &located)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: body,
        }),
        range: Some(range_from_source_info(&located.span)),
    })
}

fn render_hover(model: &PureModel, located: &Located<'_>) -> Option<String> {
    match located.kind {
        LocatedKind::ValueSpec(_) => located.resolved_type().map(render_resolved_type),
        LocatedKind::Parameter(p) => Some(format!(
            "**parameter** `{}`: `{}`",
            p.name,
            render_type_expr(&p.type_expr),
        )),
        LocatedKind::Property(p) => Some(format!(
            "**property** `{}`: `{}`",
            p.name,
            render_type_expr(&p.type_expr),
        )),
        LocatedKind::QualifiedProperty(qp) => Some(format!(
            "**qualifiedProperty** `{}` → `{}`",
            qp.name,
            render_type_expr(&qp.return_type),
        )),
        LocatedKind::Constraint(c) => {
            let name = c.name.as_deref().unwrap_or("<unnamed>");
            Some(format!("**constraint** `{name}`"))
        }
        LocatedKind::Element => render_element_label(model, located.element),
    }
}

fn render_resolved_type(rt: &ResolvedType) -> String {
    format!("`{}`", render_type_expr(&rt.type_expr))
}

fn render_type_expr(ty: &TypeExpr) -> String {
    // The compiler doesn't ship a `Display` for `TypeExpr` yet, so
    // we render a best-effort string that's recognisable in hover
    // tooltips. Not stable — refine alongside a real renderer.
    match ty {
        TypeExpr::Named { element, .. } => format!("element({element})"),
        TypeExpr::FunctionType { .. } => "FunctionType".to_string(),
        TypeExpr::AlgebraUnion(a, b) => {
            format!("{} | {}", render_type_expr(a), render_type_expr(b),)
        }
        TypeExpr::Generic(name) => name.to_string(),
        TypeExpr::Relation(_) => "Relation".to_string(),
        TypeExpr::Unresolved => "?".to_string(),
    }
}

fn render_element_label(model: &PureModel, id: ElementId) -> Option<String> {
    let element = model.try_get_element(id)?;
    let label = match element {
        Element::Class(_) => "class",
        Element::Enumeration(_) => "enum",
        Element::Function(_) => "function",
        Element::Profile(_) => "profile",
        Element::Association(_) => "association",
        Element::Measure(_) => "measure",
        Element::PrimitiveType(_) => "primitive",
        Element::Unit(_) => "unit",
        Element::PackageableMultiplicity(_) => "multiplicity",
        Element::Package(_) => "package",
    };
    let name = model.element_name(id);
    Some(format!("**{label}** `{name}`"))
}

/// Resolve a definition jump for a position. Returns the location of
/// the cursor's owning element, which is the cheapest correct answer
/// for tier-1: it lets the user jump from the body of a function back
/// to the function header. Future work resolves identifier-under-cursor
/// to the declaring symbol.
#[must_use]
pub fn definition_for_position(
    model: &PureModel,
    canonical_path: &str,
    position: Position,
    file_uri: &Url,
) -> Option<Location> {
    let (line, column) = convert::position_to_1indexed(position);
    let located = model.locate(canonical_path, line, column)?;
    // For now: jump to the element header span.
    if model.try_get_element(located.element).is_none() {
        return None;
    }
    let node = model.get_node(located.element);
    Some(Location {
        uri: file_uri.clone(),
        range: range_from_source_info(&node.source_info),
    })
}

/// Document outline: every element whose `source_info.source` matches
/// `canonical_path` produces one [`DocumentSymbol`].
#[must_use]
pub fn document_symbols_for(model: &PureModel, canonical_path: &str) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    for chunk in &model.chunks {
        for (idx, node) in chunk.nodes.iter() {
            if node.source_info.source.as_str() != canonical_path {
                continue;
            }
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: idx,
            };
            let element = chunk.elements.get(idx);
            let kind = symbol_kind_for(element);
            #[allow(deprecated)]
            // `deprecated` field on DocumentSymbol is itself deprecated
            // upstream; we set it to None and silence the lint here.
            let symbol = DocumentSymbol {
                name: node.name.to_string(),
                detail: detail_for(element),
                kind,
                tags: None,
                deprecated: None,
                range: range_from_source_info(&node.source_info),
                selection_range: range_from_source_info(&node.name_source_info),
                children: None,
            };
            let _ = id;
            out.push(symbol);
        }
    }
    out
}

fn symbol_kind_for(element: &Element) -> SymbolKind {
    match element {
        Element::Class(_) => SymbolKind::CLASS,
        Element::Enumeration(_) => SymbolKind::ENUM,
        Element::Function(_) => SymbolKind::FUNCTION,
        Element::Profile(_) => SymbolKind::INTERFACE,
        Element::Association(_) => SymbolKind::INTERFACE,
        Element::Measure(_) => SymbolKind::STRUCT,
        Element::PrimitiveType(_) => SymbolKind::TYPE_PARAMETER,
        Element::Unit(_) => SymbolKind::CONSTANT,
        Element::PackageableMultiplicity(_) => SymbolKind::CONSTANT,
        Element::Package(_) => SymbolKind::PACKAGE,
    }
}

fn detail_for(element: &Element) -> Option<String> {
    match element {
        Element::Function(f) => Some(format!(
            "fn({} param{})",
            f.parameters.len(),
            if f.parameters.len() == 1 { "" } else { "s" },
        )),
        _ => None,
    }
}

/// Code lenses for test execution. Walks every function in the file
/// and emits a `legend.runTest` command for each function tagged with
/// the `test::Test` stereotype. This is the LSP-side hook the planned
/// IntelliJ run configuration will read.
#[must_use]
pub fn code_lenses_for(model: &PureModel, canonical_path: &str) -> Vec<CodeLens> {
    let mut lenses = Vec::new();
    for chunk in &model.chunks {
        for (idx, node) in chunk.nodes.iter() {
            if node.source_info.source.as_str() != canonical_path {
                continue;
            }
            let element = chunk.elements.get(idx);
            let Element::Function(func) = element else {
                continue;
            };
            if !is_test_stereotyped(func) {
                continue;
            }
            let element_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: idx,
            };
            let fqn = render_fqn(model, element_id);
            lenses.push(CodeLens {
                range: range_from_source_info(&node.name_source_info),
                command: Some(Command {
                    title: "▶ Run test".to_string(),
                    command: "legend.runTest".to_string(),
                    arguments: Some(vec![serde_json::Value::String(fqn)]),
                }),
                data: None,
            });
        }
    }
    lenses
}

fn is_test_stereotyped(func: &legend_pure_parser_pure::nodes::function::Function) -> bool {
    // Loose match on the stereotype name. A stricter check would
    // verify `profile == meta::pure::profiles::test`'s `ElementId`,
    // but the IntelliJ runner can post-filter if we ever see
    // collisions.
    func.stereotypes.iter().any(|st| st.value == "Test")
}

fn render_fqn(model: &PureModel, id: ElementId) -> String {
    // Best-effort FQN rendering — climb packages by ID.
    if model.try_get_element(id).is_none() {
        return model.element_name(id).to_string();
    }
    let node = model.get_node(id);
    let mut parts: Vec<String> = Vec::new();
    parts.push(node.name.to_string());
    let mut pkg_id = node.parent_package;
    loop {
        let pkg = model.get_package(pkg_id);
        if pkg.parent.is_none() {
            break;
        }
        parts.push(pkg.name.to_string());
        let Some(parent) = pkg.parent else {
            break;
        };
        pkg_id = parent;
    }
    parts.reverse();
    parts.join("::")
}
