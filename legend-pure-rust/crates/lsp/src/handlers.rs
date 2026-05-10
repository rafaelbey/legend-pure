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

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::locate::{Located, LocatedKind};
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::refs::ReferenceIndex;
use legend_pure_parser_pure::types::{ResolvedType, TypeExpr};
use tower_lsp::lsp_types::{
    CodeLens, Command, Diagnostic, DocumentSymbol, Hover, HoverContents, Location, LocationLink,
    MarkupContent, MarkupKind, Position, SymbolKind, Url,
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
///
/// `uri_for_canonical` resolves a target's canonical source path
/// (the compiler's `/repo/file.pure` form) to an on-disk
/// `file://` URL when the target lives in a different file from
/// the click. Callers wired to a [`crate::workspace::Workspace`]
/// pass `&|c| ws.file_uri_for_canonical(c)`; tests that don't
/// care about cross-file goto can pass `&|_| None`.
pub fn definition_for_position(
    model: &PureModel,
    references: Option<&ReferenceIndex>,
    canonical_path: &str,
    position: Position,
    file_uri: &Url,
    uri_for_canonical: &dyn Fn(&str) -> Option<Url>,
) -> Option<LocationLink> {
    let (line, column) = convert::position_to_1indexed(position);
    // 1) Reference-index first: covers every navigable region the
    //    indexer emitted (stereotypes, tagged values, type refs at
    //    every position, function calls, property calls,
    //    qualified-property calls, variable refs, enum values,
    //    package-element refs, plus DSL-extension contributions).
    //    O(refs-in-file) lookup driven entirely by the resolver's
    //    own data — no locate walking.
    if let Some(idx) = references
        && let Some(reference) = idx.find_at(canonical_path, line, column)
    {
        let target_canonical = reference.target.source.as_str();
        let target_uri = if target_canonical == canonical_path {
            file_uri.clone()
        } else {
            uri_for_canonical(target_canonical)?
        };
        return Some(LocationLink {
            origin_selection_range: Some(range_from_source_info(&reference.range)),
            target_uri,
            target_range: range_from_source_info(&reference.target),
            target_selection_range: range_from_source_info(&reference.target),
        });
    }

    // 2) Definition-site no-op fallback: cursor sits on an element's
    //    own name (function/class header click). Returns the same
    //    location as both origin and target so the IDE doesn't move
    //    but the cmd-hover affordance still renders.
    let located = model.locate(canonical_path, line, column)?;
    let LocatedKind::Element = located.kind else {
        // Inside an element body but the index missed → genuinely no
        // navigable target (whitespace, literal, unresolved
        // expression). Return None rather than falling through to a
        // confusing parent-element jump.
        return None;
    };
    let node = model.get_node(located.element);
    if !cursor_in_source_info(&node.name_source_info, line, column) {
        return None;
    }
    let origin_selection_range = range_from_source_info(&node.name_source_info);
    let target_location = element_location(model, located.element, file_uri, uri_for_canonical)?;
    Some(LocationLink {
        origin_selection_range: Some(origin_selection_range),
        target_uri: target_location.uri,
        target_range: target_location.range,
        target_selection_range: target_location.range,
    })
}

/// Inclusive (line, column) test: is `(line, column)` (1-indexed)
/// inside `si`'s span? End boundary is exclusive — clicking exactly
/// at `end_column` is past the last character.
fn cursor_in_source_info(si: &SourceInfo, line: u32, column: u32) -> bool {
    if line < si.start_line || line > si.end_line {
        return false;
    }
    if line == si.start_line && column < si.start_column {
        return false;
    }
    if line == si.end_line && column >= si.end_column {
        return false;
    }
    true
}


/// Build a [`Location`] for a target span, choosing the click URI
/// when same-file, the resolver URI when cross-file. Shared helper
/// for cross-file goto.
fn location_at_source_info(
    si: &SourceInfo,
    file_uri: &Url,
    uri_for_canonical: &dyn Fn(&str) -> Option<Url>,
) -> Option<Location> {
    let target_canonical = si.source.as_str();
    let target_uri = if let Ok(click_path) = file_uri.to_file_path()
        && click_path.ends_with(target_canonical.trim_start_matches('/'))
    {
        file_uri.clone()
    } else {
        uri_for_canonical(target_canonical)?
    };
    Some(Location {
        uri: target_uri,
        range: range_from_source_info(si),
    })
}

/// Return a [`Location`] pointing at an element's name span. Returns
/// `None` for invalid IDs or Package targets (Packages have no source
/// of their own that's worth navigating to).
///
/// Two paths:
///
/// - **Same-file** — the target's canonical source path is a suffix
///   of the click URI's filesystem path. The click URI is reused so
///   the IDE keeps focus on the buffer the user is editing
///   (matters for unsaved-buffer goto-def).
/// - **Cross-file** — the target lives in a different file. We ask
///   `uri_for_canonical` to map the canonical to a real on-disk URL
///   (open buffer first, then `Repo::Filesystem::source_root`).
///   Returns `None` when the target's repo has no on-disk source
///   (embedded / `.purem`) — IDEs can't open something that doesn't
///   exist on disk.
fn element_location(
    model: &PureModel,
    id: ElementId,
    file_uri: &Url,
    uri_for_canonical: &dyn Fn(&str) -> Option<Url>,
) -> Option<Location> {
    if model.try_get_element(id).is_none() {
        return None;
    }
    if matches!(id, ElementId::Package(_)) {
        return None;
    }
    let node = model.get_node(id);
    location_at_source_info(&node.name_source_info, file_uri, uri_for_canonical)
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
    fn definition_resolves_variable_to_parameter_declaration() {
        let src = "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n";
        let model = compile_fixture("fixture.pure", src);
        let uri = Url::parse("file:///fixture.pure").unwrap();
        // Cursor inside `$name` body. The Variable should resolve to
        // the parameter `name`, declared at line 1 col 22 (1-indexed)
        // → 0-indexed col 21.
        let no_cross_file: &dyn Fn(&str) -> Option<Url> = &|_| None;
        let index = legend_pure_parser_pure::refs::build_reference_index(&model);
        let loc = definition_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(2, 2),
            &uri,
            no_cross_file,
        )
        .expect("definition must resolve");
        assert_eq!(loc.target_range.start.line, 0);
        assert_eq!(loc.target_range.start.character, 21);
        // origin_selection_range tells the IDE which source range to
        // underline on ⌘-hover. Must be populated.
        assert!(
            loc.origin_selection_range.is_some(),
            "LocationLink must populate origin_selection_range so the IDE can render the underline",
        );
    }

    #[test]
    fn definition_resolves_property_to_class_member() {
        let src = "\
Class test::Person
{
  name: String[1];
}

function test::nameOf(p: test::Person[1]): String[1]
{
  $p.name
}
";
        let model = compile_fixture("fixture.pure", src);
        let uri = Url::parse("file:///fixture.pure").unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Url> = &|_| None;
        // The body line is line 8 (1-indexed). `$p.name` starts at
        // col 3; `name` itself is at col 6. LSP positions are 0-indexed
        // → (line 7, col 5+).
        let loc = definition_for_position(&model, None, "fixture.pure", pos(7, 6), &uri, no_cross_file);
        // Property goto requires `type_info` populated on the
        // receiver, which only happens after a full Pass-2.5 compile.
        // `compile_fixture` produces a partial model without bodies
        // lowered to typed ValueSpecs, so this integration test is
        // *correctly* expected to return None today: the property-call
        // ValueSpec exists but its receiver lacks `type_info`, so we
        // fall through every resolve_value_spec_target case.
        //
        // Previously this test was a "soft pass" because the handler
        // had a fallback that navigated whitespace clicks to the
        // enclosing function header. That fallback is gone (it made
        // the entire function body appear navigable), so we now
        // assert the strict behaviour. The synthetic-model unit test
        // `property_target_location_resolves_via_receiver_type_info`
        // covers the success case.
        assert!(
            loc.is_none(),
            "without type_info, property goto correctly returns None; got {loc:?}",
        );
    }

    #[test]
    fn definition_returns_none_for_value_spec_without_target() {
        // A click on a string-literal body has no symbol target. The
        // handler used to fall back to the owning function's name span,
        // which was confusing — clicking on `'hello'` would jump to
        // `msg`, suggesting `'hello'` is defined there. Now it returns
        // None and the IDE shows "Cannot find declaration to go to".
        let src = "function test::msg(): String[1]\n{\n  'hello'\n}\n";
        let model = compile_fixture("fixture.pure", src);
        let uri = Url::parse("file:///fixture.pure").unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Url> = &|_| None;
        let loc = definition_for_position(&model, None, "fixture.pure", pos(2, 4), &uri, no_cross_file);
        assert!(
            loc.is_none(),
            "click on a literal must not navigate to the enclosing function: got {loc:?}",
        );
    }

    #[test]
    fn definition_returns_self_for_definition_site_click() {
        // Click on the function name itself (the definition site, not
        // a use-site). LocatedKind::Element is the located kind here,
        // so element_location is invoked directly and returns the
        // element's name span — no-op navigation.
        let src = "function test::msg(): String[1]\n{\n  'hello'\n}\n";
        let model = compile_fixture("fixture.pure", src);
        let uri = Url::parse("file:///fixture.pure").unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Url> = &|_| None;
        // `msg` starts at line 1 col 16 (1-indexed) → 0-indexed col 15.
        let loc = definition_for_position(&model, None, "fixture.pure", pos(0, 16), &uri, no_cross_file)
            .expect("definition-site click should still produce a location");
        assert_eq!(loc.target_range.start.line, 0);
        assert_eq!(loc.target_range.start.character, 15);
    }

    /// Verifies the cross-file branch of `element_location`: when the
    /// click URI's filesystem path doesn't match the target's
    /// canonical, the resolver is consulted and its URL is returned
    /// verbatim.
    ///
    /// Built manually instead of through `compile_fixture` because
    /// `pipeline::compile` on bare fixtures (without the platform)
    /// produces a partial model whose function bodies haven't been
    /// lowered to ValueSpecs — so `locate` can't find a navigable
    /// expression at a call site to drive the integration path. The
    /// unit test exercises the same `element_location` code path the
    /// integration would hit once body-lowering survives partial
    /// compiles.
    #[test]
    fn element_location_uses_resolver_for_cross_file_target() {
        use legend_pure_parser_ast::SourceInfo;
        use legend_pure_parser_pure::ids::ElementId;
        use legend_pure_parser_pure::model::{
            Element as ModelElement, ElementNode, ModelChunk, PureModel,
        };
        use legend_pure_parser_pure::nodes::function::Function;
        use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

        let mut model = PureModel::new();
        let test_pkg = model.get_or_create_package(&[smol_str::SmolStr::new("test")]);

        let chunk_id: u16 = 0;
        let mut chunk = ModelChunk::new(chunk_id);
        // Function declared in lib.pure (different file from the click).
        let lib_si = SourceInfo::new("/proj/lib.pure", 1, 1, 4, 1);
        let name_si = SourceInfo::new("/proj/lib.pure", 1, 16, 1, 21);
        let func_idx = chunk.alloc_element(
            ElementNode {
                name: smol_str::SmolStr::new("greet__String_1_"),
                source_info: lib_si,
                name_source_info: name_si,
                parent_package: test_pkg,
            },
            ModelElement::Function(Function {
                function_name: smol_str::SmolStr::new("greet"),
                is_native: false,
                parameters: std::sync::Arc::from(Vec::new()),
                return_type: TypeExpr::Unresolved,
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let func_id = ElementId::InstanceId {
            chunk_id,
            local_idx: func_idx,
        };
        model.chunks.push(chunk);
        model.register_element(test_pkg, func_id);

        // Click happened in app.pure; target lives in lib.pure.
        let app_uri = Url::parse("file:///abs/proj/app.pure").unwrap();
        let lib_disk_uri = Url::parse("file:///abs/proj/lib.pure").unwrap();

        let lib_disk_uri_clone = lib_disk_uri.clone();
        let resolver: &dyn Fn(&str) -> Option<Url> = &move |canonical: &str| {
            (canonical == "/proj/lib.pure").then(|| lib_disk_uri_clone.clone())
        };

        let loc = element_location(&model, func_id, &app_uri, resolver)
            .expect("cross-file element_location must resolve");
        assert_eq!(loc.uri, lib_disk_uri);
        // Range comes from the target's name span — col 16 (1-indexed)
        // → 0-indexed col 15.
        assert_eq!(loc.range.start.line, 0);
        assert_eq!(loc.range.start.character, 15);

        // And: when the resolver returns None for a cross-file target,
        // element_location returns None (no phantom click-URI fallback
        // to a wrong file).
        let dead_resolver: &dyn Fn(&str) -> Option<Url> = &|_| None;
        assert!(element_location(&model, func_id, &app_uri, dead_resolver).is_none());
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
                stereotypes: vec![
                    legend_pure_parser_pure::nodes::profile::bootstrap_spanned_name(
                        smol_str::SmolStr::new("Test"),
                    ),
                ],
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
                    source_info: None,
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
