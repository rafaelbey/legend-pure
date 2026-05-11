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
        Element::DSLInstance(d) => return Some(format!("**{}** `{}`", d.dsl_name, model.element_name(id))),
    };
    let name = model.element_name(id);
    Some(format!("**{label}** `{name}`"))
}

/// Resolve a definition jump for a position.
///
/// Resolution order, narrowest to widest:
///   1. If the cursor sits on a `ValueSpec` whose kind carries a
///      resolved [`ElementId`] (function call, packageable-element
///      ref, enum value), jump to that element's name span — this
///      is the IDE-natural "click on `String`, jump to String".
///   2. Otherwise fall back to the cursor's *owning* element's
///      name span — at least the user gets a stable navigation
///      target instead of nothing.
///
/// Both paths return the element's `name_source_info` (the
/// identifier-only span) rather than the full element body, so
/// IntelliJ highlights the name on goto-def, not the entire
/// declaration.
#[must_use]
pub fn definition_for_position(
    model: &PureModel,
    canonical_path: &str,
    position: Position,
    file_uri: &Url,
) -> Option<Location> {
    let (line, column) = convert::position_to_1indexed(position);
    let located = model.locate(canonical_path, line, column)?;

    // Try to resolve the cursor's specific symbol target first.
    if let LocatedKind::ValueSpec(vs) = located.kind
        && let Some(target) = resolve_value_spec_target(vs)
        && let Some(loc) = element_location(model, target, file_uri)
    {
        return Some(loc);
    }

    // Fallback: jump to the owning element's name span.
    element_location(model, located.element, file_uri)
}

/// If a [`ValueSpec`] kind directly references a resolved element,
/// return that element's `ElementId`. None for variables (need
/// scope tracking), literals, lambdas, etc.
fn resolve_value_spec_target(
    vs: &legend_pure_parser_pure::types::ValueSpec,
) -> Option<ElementId> {
    use legend_pure_parser_pure::types::ExprKind;
    match &*vs.kind {
        ExprKind::FunctionCall(d) | ExprKind::QualifiedPropertyCall(d) => d.function,
        // PropertyCall's `function_name` is the property name, not a
        // resolved element — finding the declaring class needs more
        // context, deferred.
        ExprKind::PackageableElementRef { element } => Some(*element),
        ExprKind::EnumValue { enum_element, .. } => Some(*enum_element),
        ExprKind::TypeReference {
            type_expr: legend_pure_parser_pure::types::TypeExpr::Named { element, .. },
        } => Some(*element),
        _ => None,
    }
}

/// Return a [`Location`] pointing at an element's name span. Returns
/// None for invalid IDs or Package targets (Packages have no source
/// of their own that's worth navigating to).
///
/// The URI returned is the **click-origin** URI rather than the
/// target's canonical source path. This means same-file goto-def
/// works (jumping from a body back to the function header, or from
/// a use-site to a sibling declaration in the same file). Cross-file
/// goto-def — e.g. clicking on `String` to jump into the platform
/// repo — requires mapping the target's canonical path back to a
/// real on-disk URL, which needs `Repo::Filesystem` to retain its
/// `source_root` (it currently doesn't). Tracked as a follow-up.
fn element_location(
    model: &PureModel,
    id: ElementId,
    file_uri: &Url,
) -> Option<Location> {
    model.try_get_element(id)?;
    if matches!(id, ElementId::Package(_)) {
        return None;
    }
    let node = model.get_node(id);
    // Only navigate when the target is in the same file as the
    // click — otherwise we'd return a `file://<canonical>` URI that
    // doesn't exist on disk and the IDE silently fails to open it.
    let click_path = file_uri.to_file_path().ok()?;
    let canonical_tail = node.source_info.source.trim_start_matches('/');
    if !click_path.ends_with(canonical_tail) {
        return None;
    }
    Some(Location {
        uri: file_uri.clone(),
        range: range_from_source_info(&node.name_source_info),
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
        Element::DSLInstance(_) => SymbolKind::OBJECT,
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
    // `get_node` panics for `ElementId::Package`; the caller (CodeLens
    // emission) only ever feeds in InstanceIds today, but a
    // partial-compile fixture once tripped this path. Fall back to the
    // element-name lookup which is total over both ID kinds.
    if matches!(id, ElementId::Package(_)) {
        return model.element_name(id).to_string();
    }
    let node = model.get_node(id);
    let mut parts: Vec<String> = vec![node.name.to_string()];
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

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_parser_parser::parse;
    use legend_pure_parser_pure::error::CompilationErrorKind;
    use legend_pure_parser_pure::pipeline;

    /// Compiles a single source file via the real pipeline (bootstrap
    /// pre-loaded). Returns the model whether the compile succeeds or
    /// produces a `PartialPureModel`, since handler tests want to
    /// exercise the model regardless of clean-vs-recovered status.
    fn compile_fixture(name: &str, source: &str) -> PureModel {
        let parsed = parse(source, name).expect("test fixture must parse");
        match pipeline::compile(&[parsed], &[]) {
            Ok(m) => m,
            Err(p) => p.model,
        }
    }

    fn pos(line: u32, col: u32) -> Position {
        Position {
            line,
            character: col,
        }
    }

    #[test]
    fn diagnostics_for_round_trips_message_and_severity() {
        let err = CompilationError {
            message: "boom".to_string(),
            source_info: legend_pure_parser_ast::SourceInfo::new("x.pure", 2, 3, 2, 8),
            kind: CompilationErrorKind::UnresolvedElement {
                path: smol_str::SmolStr::new("Foo"),
            },
        };
        let diags = diagnostics_for(&[err]);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.message, "boom");
        assert_eq!(
            d.severity,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
        );
        // Range converts 1-indexed (2,3)-(2,8) to 0-indexed (1,2)-(1,7).
        assert_eq!(d.range.start.line, 1);
        assert_eq!(d.range.start.character, 2);
        assert_eq!(d.range.end.line, 1);
        assert_eq!(d.range.end.character, 7);
        // Code surfaces the kind tag.
        assert!(matches!(
            d.code.as_ref(),
            Some(tower_lsp::lsp_types::NumberOrString::String(s)) if s == "unresolvedElement"
        ));
    }

    #[test]
    fn hover_renders_parameter_kind() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        // Cursor on the `name` parameter identifier (column 22).
        // LSP positions are 0-indexed → (line 0, char 21).
        let hover = hover_for_position(&model, "fixture.pure", pos(0, 21))
            .expect("hover must produce content for the parameter");
        match hover.contents {
            HoverContents::Markup(m) => {
                assert!(m.value.contains("**parameter**"), "got: {}", m.value);
                assert!(m.value.contains("`name`"), "got: {}", m.value);
            }
            other => panic!("expected markup hover, got {other:?}"),
        }
    }

    #[test]
    fn hover_returns_none_outside_known_file() {
        let model = compile_fixture(
            "fixture.pure",
            "function test::f(): Integer[1]\n{\n  1\n}\n",
        );
        assert!(hover_for_position(&model, "missing.pure", pos(0, 0)).is_none());
    }

    #[test]
    fn definition_jumps_to_element_header_span() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        let uri = Url::parse("file:///fixture.pure").unwrap();
        // Cursor inside the body — falls back to the owning element's
        // name span (just the function-name identifier, not the whole
        // body). For `function test::greet(...)` the name `greet`
        // starts at line 1 col 16 (1-indexed) → 0-indexed col 15.
        let loc = definition_for_position(&model, "fixture.pure", pos(2, 2), &uri)
            .expect("definition must resolve");
        assert_eq!(loc.range.start.line, 0);
        assert_eq!(loc.range.start.character, 15);
    }

    #[test]
    fn document_symbols_lists_one_per_top_level_element() {
        let src = "\
function test::greet(name: String[1]): String[1]
{
  $name
}

Class test::Person
{
  name: String[1];
}
";
        let model = compile_fixture("fixture.pure", src);
        let symbols = document_symbols_for(&model, "fixture.pure");
        // Two top-level elements in this file: a function and a class.
        assert_eq!(symbols.len(), 2);
        let kinds: Vec<_> = symbols.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SymbolKind::FUNCTION));
        assert!(kinds.contains(&SymbolKind::CLASS));
    }

    #[test]
    fn code_lenses_empty_for_untagged_function() {
        let plain = compile_fixture(
            "plain.pure",
            "function test::ordinary(): Integer[1]\n{\n  1\n}\n",
        );
        assert_eq!(code_lenses_for(&plain, "plain.pure").len(), 0);
    }

    /// Tagged-function lens emission.
    ///
    /// We build the model by hand here rather than going through
    /// `compile_fixture`. The compile path is a poor fit for this
    /// test because — without the platform's
    /// `meta::pure::profiles::test` Profile loaded — the resolver
    /// (correctly) drops the unresolvable stereotype and the
    /// resulting function ends up with `stereotypes = []`. That's
    /// the right behaviour for the resolver, but it means
    /// `is_test_stereotyped` has nothing to match on. Manually
    /// inserting a `Profile` + a `Function` whose `StereotypeRef`
    /// points at it lets us test the lens-detection logic in
    /// isolation, on a platform-free model.
    #[test]
    fn code_lenses_emit_for_test_stereotyped_function() {
        use legend_pure_parser_ast::SourceInfo;
        use legend_pure_parser_pure::annotations::StereotypeRef;
        use legend_pure_parser_pure::ids::ElementId;
        use legend_pure_parser_pure::model::{
            Element as ModelElement, ElementNode, ModelChunk, PureModel,
        };
        use legend_pure_parser_pure::nodes::function::Function;
        use legend_pure_parser_pure::nodes::profile::Profile;
        use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

        let mut model = PureModel::new();
        let test_pkg = model.get_or_create_package(&[smol_str::SmolStr::new("test")]);

        // `PureModel::new()` leaves `chunks` empty; we slot ours in
        // at index 0 with a matching chunk_id so `get_node` (which
        // indexes by chunk_id) doesn't trip.
        let chunk_id: u16 = 0;
        let mut chunk = ModelChunk::new(chunk_id);
        let fixture_si = SourceInfo::new("fixture.pure", 1, 1, 4, 1);

        // Profile element — declares the "Test" stereotype.
        let profile_local_idx = chunk.alloc_element(
            ElementNode {
                name: smol_str::SmolStr::new("test"),
                source_info: fixture_si.clone(),
                name_source_info: fixture_si.clone(),
                parent_package: test_pkg,
            },
            ModelElement::Profile(Profile {
                stereotypes: vec![smol_str::SmolStr::new("Test")],
                tags: Vec::new(),
            }),
        );
        let profile_id = ElementId::InstanceId {
            chunk_id,
            local_idx: profile_local_idx,
        };

        // Function element — references the Profile via StereotypeRef.
        let func_local_idx = chunk.alloc_element(
            ElementNode {
                name: smol_str::SmolStr::new("myCheck__Boolean_1_"),
                source_info: fixture_si.clone(),
                name_source_info: fixture_si.clone(),
                parent_package: test_pkg,
            },
            ModelElement::Function(Function {
                function_name: smol_str::SmolStr::new("myCheck"),
                is_native: false,
                parameters: std::sync::Arc::from(Vec::new()),
                return_type: TypeExpr::Unresolved,
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: vec![StereotypeRef {
                    profile: profile_id,
                    value: smol_str::SmolStr::new("Test"),
                }],
                tagged_values: Vec::new(),
            }),
        );
        let func_id = ElementId::InstanceId {
            chunk_id,
            local_idx: func_local_idx,
        };

        model.chunks.push(chunk);
        model.register_element(test_pkg, profile_id);
        model.register_element(test_pkg, func_id);

        let lenses = code_lenses_for(&model, "fixture.pure");
        assert_eq!(lenses.len(), 1, "expected one lens for the Test-tagged function");
        let cmd = lenses[0]
            .command
            .as_ref()
            .expect("lens must carry a command");
        assert_eq!(cmd.command, "legend.runTest");
        assert_eq!(cmd.title, "▶ Run test");
    }
}
