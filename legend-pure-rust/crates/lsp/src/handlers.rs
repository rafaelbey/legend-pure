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
//! tokio runtime or `tower_lsp_server::Client`.

use std::collections::{HashMap, HashSet};

use legend_pure_ide::ReferenceIndex;
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::locate::{Located, LocatedKind};
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ResolvedType, TypeExpr};
use tower_lsp_server::ls_types::{
    CodeLens, Command, Diagnostic, DocumentSymbol, Hover, HoverContents, Location, LocationLink,
    MarkupContent, MarkupKind, OneOf, Position, SymbolKind, Uri, WorkspaceSymbol,
};

use crate::convert::{self, range_from_source_info};
use crate::diagnostics;
use crate::workspace::Workspace;

/// Build the diagnostic list for a single source path.
#[must_use]
pub fn diagnostics_for(errors: &[CompilationError]) -> Vec<Diagnostic> {
    errors.iter().map(diagnostics::to_diagnostic).collect()
}

/// One cycle of `textDocument/publishDiagnostics` calls, prepared from
/// the current workspace state.
///
/// `entries` is what to publish this cycle: one `(uri, diagnostics)`
/// pair per URI, with `diagnostics` possibly empty for stale-clearing
/// or for open buffers with no errors. `next_previously_published`
/// captures the URIs that received non-empty diagnostics, to be fed
/// back in as `previously_published` on the next call so any URI that
/// drops out gets an empty publish next time.
#[derive(Debug, Clone, Default)]
pub struct PublishPlan {
    /// `(uri, diagnostics)` pairs to feed `Client::publish_diagnostics`.
    pub entries: Vec<(Uri, Vec<Diagnostic>)>,
    /// URIs that received non-empty diagnostics this cycle. Track this
    /// across cycles so the next cycle can clear any URI that drops
    /// out of the set.
    pub next_previously_published: HashSet<Uri>,
}

/// Plan one round of diagnostic publishing for a workspace.
///
/// Three sources contribute URIs to the publish set:
///
/// 1. Every canonical source in [`Workspace::diagnostics`] gets
///    resolved to a `file://` URI via
///    [`Workspace::file_uri_for_canonical`]. Canonicals that don't
///    resolve (embedded `.purem`, bootstrap chunk 0, fixtures without
///    a `source_root`) are filtered out — there's no on-disk file the
///    client can navigate to.
/// 2. Every open buffer gets at least an empty publish, so a clean
///    open file visibly shows "no errors" right after didOpen.
/// 3. Every URI in `previously_published` that isn't in the
///    diagnostic set this cycle gets an empty publish — the
///    stale-clear path that makes workspace-wide publishing safe.
///
/// Resolved diagnostics for an open buffer override its empty entry;
/// resolved diagnostics for a stale URI also override its empty entry
/// (the stale-clear only fires if the canonical truly dropped out).
pub fn plan_diagnostics_publish(
    workspace: &Workspace,
    previously_published: &HashSet<Uri>,
) -> PublishPlan {
    let mut publishes: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();

    for (canonical, errors) in &workspace.diagnostics {
        if let Some(uri) = workspace.file_uri_for_canonical(canonical) {
            publishes.insert(uri, diagnostics_for(errors));
        }
    }

    for uri in workspace.open_buffers.keys() {
        publishes.entry(uri.clone()).or_default();
    }

    for uri in previously_published {
        publishes.entry(uri.clone()).or_default();
    }

    let next_previously_published = publishes
        .iter()
        .filter(|(_, diags)| !diags.is_empty())
        .map(|(uri, _)| uri.clone())
        .collect();

    PublishPlan {
        entries: publishes.into_iter().collect(),
        next_previously_published,
    }
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
        Element::DSLInstance(d) => {
            return Some(format!("**{}** `{}`", d.dsl_name, model.element_name(id)));
        }
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
    file_uri: &Uri,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
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

/// If a [`ValueSpec`] kind directly references a resolved element,
/// return that element's `ElementId`. None for variables (need
/// scope tracking), literals, lambdas, etc.
#[allow(clippy::match_same_arms)] // patterns intentionally distinct — each variant carries a different field binding even when the result shape coincides
fn resolve_value_spec_target(vs: &legend_pure_parser_pure::types::ValueSpec) -> Option<ElementId> {
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

/// Resolve every workspace reference to whatever symbol sits under
/// the cursor, returning LSP-shaped [`Location`]s.
///
/// Two-tier cursor resolution:
///   1. **Reference-site hit** — the cursor is on an existing
///      reference (e.g. `extends Foo`, `String[1]` in a parameter
///      type). The reference's `target_element` is the symbol the
///      user is asking about.
///   2. **Definition-site hit** — the cursor sits on an element's
///      own name span (the user clicked the declaration). Use that
///      element's `ElementId`.
///
/// V1 limitation: cursor on a property identifier (`$x.propA`),
/// qualified-property call, or local variable returns an empty
/// `Vec` — those reference kinds carry `target_element: None` in
/// today's index, so the reverse-index has no entry to return. A
/// future change to richer reference targets (property +
/// qualified-property granularity) will populate the V1
/// no-op cases.
///
/// `include_declaration` honours the LSP `ReferenceContext` flag:
/// when `true`, the declaration's own `name_source_info` is
/// appended to the result. Per LSP 3.17, the declaration is
/// considered one of the references.
///
/// `uri_for_canonical` resolves canonical compiler paths to
/// on-disk URLs. Reference sites whose canonical path doesn't
/// resolve are silently skipped — they may live in repos with no
/// on-disk source (embedded / `.purem`), and an LSP client can't
/// navigate to a file that doesn't exist.
#[must_use]
pub fn references_for_position(
    model: &PureModel,
    references: Option<&ReferenceIndex>,
    canonical_path: &str,
    position: Position,
    include_declaration: bool,
    file_uri: &Uri,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
) -> Vec<Location> {
    let (line, column) = convert::position_to_1indexed(position);

    // Resolve cursor → target ElementId.
    let target_id = references
        .and_then(|idx| idx.find_at(canonical_path, line, column))
        .and_then(|r| r.target_element)
        .or_else(|| {
            let located = model.locate(canonical_path, line, column)?;
            if !matches!(located.kind, LocatedKind::Element) {
                return None;
            }
            let node = model.get_node(located.element);
            // Inclusive-end semantics — `name_source_info` from the
            // compiler is **inclusive** at end_column (matches
            // `refs::contains`), unlike the LSP `Range` convention.
            // For a 1-char element name like `Class abc::A`, the
            // span is `c13-c13`; an exclusive-end check would reject
            // every cursor.
            if !cursor_in_source_info_inclusive(&node.name_source_info, line, column) {
                return None;
            }
            Some(located.element)
        });
    let Some(target_id) = target_id else {
        return Vec::new();
    };

    // Lookup reverse-index entries; map each to an LSP Location.
    // Skip entries whose canonical path doesn't resolve to a URI —
    // happens for embedded/.purem-backed repos that have no on-disk
    // source. Dropping is correct: the LSP client can't navigate
    // there anyway.
    let mut out: Vec<Location> = references
        .and_then(|idx| idx.usages_of(target_id))
        .map(|locs| {
            locs.iter()
                .filter_map(|loc| {
                    Some(Location {
                        uri: uri_for_canonical_or_click(
                            loc.canonical_path.as_str(),
                            file_uri,
                            uri_for_canonical,
                        )?,
                        range: range_from_source_info(&loc.range),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if include_declaration
        && let Some(decl_loc) = element_location(model, target_id, file_uri, uri_for_canonical)
    {
        out.push(decl_loc);
    }
    out
}

/// Choose the click URI when the target lives in the same file as
/// the click, otherwise delegate to the cross-file resolver. Same
/// rule as [`location_at_source_info`] but takes a canonical path
/// directly (without a wrapping [`SourceInfo`]).
fn uri_for_canonical_or_click(
    canonical: &str,
    file_uri: &Uri,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
) -> Option<Uri> {
    if let Some(click_path) = file_uri.to_file_path()
        && click_path.ends_with(canonical.trim_start_matches('/'))
    {
        return Some(file_uri.clone());
    }
    uri_for_canonical(canonical)
}

/// Variant of [`cursor_in_source_info`] with **inclusive** end —
/// matches the compiler's [`SourceInfo`] convention (used by
/// `refs::contains`). Necessary when checking cursor presence
/// against compiler-emitted spans like `name_source_info`, which
/// can be a degenerate 1-character span (`c13-c13`).
fn cursor_in_source_info_inclusive(si: &SourceInfo, line: u32, column: u32) -> bool {
    if line < si.start_line || line > si.end_line {
        return false;
    }
    if line == si.start_line && column < si.start_column {
        return false;
    }
    if line == si.end_line && column > si.end_column {
        return false;
    }
    true
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
    file_uri: &Uri,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
) -> Option<Location> {
    let target_canonical = si.source.as_str();
    let target_uri = if let Some(click_path) = file_uri.to_file_path()
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
    file_uri: &Uri,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
) -> Option<Location> {
    model.try_get_element(id)?;
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
        Element::Profile(_) | Element::Association(_) => SymbolKind::INTERFACE,
        Element::Measure(_) => SymbolKind::STRUCT,
        Element::PrimitiveType(_) => SymbolKind::TYPE_PARAMETER,
        Element::Unit(_) | Element::PackageableMultiplicity(_) => SymbolKind::CONSTANT,
        Element::Package(_) => SymbolKind::PACKAGE,
        Element::DSLInstance(_) => SymbolKind::OBJECT,
    }
}

/// Cap on the number of workspace symbols returned in one
/// `workspace/symbol` response. IntelliJ takes whatever the server
/// returns, sorts it client-side, and re-queries on every keystroke;
/// returning every member of a 1.6 K-element platform with members
/// expanded would yield 10 K+ entries — slow to serialize and pointless
/// since the user never scrolls past a few dozen. Tune up if real
/// usage shows the cap clipping legitimate matches.
const WORKSPACE_SYMBOL_LIMIT: usize = 5000;

/// Project-wide symbol search backing `workspace/symbol`. Walks every
/// non-bootstrap chunk, emits one entry per top-level element plus
/// one per class/association property + qualified property +
/// constraint and one per enum value. Filters by case-insensitive
/// substring on either the simple name or the FQN.
///
/// Bootstrap (chunk 0) is skipped — `Any`, `Nil`, primitives, and the
/// m3 metamodel are already reachable via goto-def and would just
/// clutter Cmd+O results.
///
/// `uri_for_canonical` is the [`crate::workspace::Workspace`]
/// resolver that translates a canonical source path to the URL the
/// IDE opened the file with. Symbols whose source can't be mapped
/// (embedded `.purem`, etc.) are dropped — IDEs can't open something
/// without a URL.
#[must_use]
pub fn workspace_symbols_for(
    model: &PureModel,
    query: &str,
    uri_for_canonical: &dyn Fn(&str) -> Option<Uri>,
) -> Vec<WorkspaceSymbol> {
    let needle = query.to_lowercase();
    let needle = needle.as_str();
    let matches = |simple: &str, fqn: &str| -> bool {
        if needle.is_empty() {
            return true;
        }
        simple.to_lowercase().contains(needle) || fqn.to_lowercase().contains(needle)
    };
    let make_uri = |si: &SourceInfo| -> Option<Uri> { uri_for_canonical(si.source.as_str()) };

    let mut out: Vec<WorkspaceSymbol> = Vec::new();
    'chunks: for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            // Bootstrap chunk — skip per scope.
            continue;
        }
        for (idx, node) in chunk.nodes.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: idx,
            };
            let element = chunk.elements.get(idx);
            let fqn_path = legend_pure_parser_pure::purem::fqn_path::element_fqn_path(model, id);
            let fqn = fqn_path.join("::");
            let container = if fqn_path.len() > 1 {
                Some(fqn_path[..fqn_path.len() - 1].join("::"))
            } else {
                None
            };

            if matches(node.name.as_str(), &fqn)
                && let Some(uri) = make_uri(&node.name_source_info)
            {
                push_symbol(
                    &mut out,
                    fqn.clone(),
                    container.clone(),
                    symbol_kind_for(element),
                    uri,
                    &node.name_source_info,
                );
                if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                    break 'chunks;
                }
            }

            // Drill into members. The element FQN is the container for
            // members surfaced below.
            match element {
                Element::Class(c) => {
                    for p in &c.properties {
                        if !matches(p.name.as_str(), &format!("{fqn}.{}", p.name)) {
                            continue;
                        }
                        let Some(uri) = make_uri(&p.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            p.name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::FIELD,
                            uri,
                            &p.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                    for q in &c.qualified_properties {
                        if !matches(q.name.as_str(), &format!("{fqn}.{}", q.name)) {
                            continue;
                        }
                        let Some(uri) = make_uri(&q.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            q.name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::METHOD,
                            uri,
                            &q.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                    for k in &c.constraints {
                        let constraint_name = k.name.as_deref().unwrap_or("<unnamed>");
                        if !matches(constraint_name, &format!("{fqn}.{constraint_name}")) {
                            continue;
                        }
                        let Some(uri) = make_uri(&k.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            constraint_name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::KEY,
                            uri,
                            &k.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                }
                Element::Association(a) => {
                    for p in &a.properties {
                        if !matches(p.name.as_str(), &format!("{fqn}.{}", p.name)) {
                            continue;
                        }
                        let Some(uri) = make_uri(&p.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            p.name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::FIELD,
                            uri,
                            &p.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                    for q in &a.qualified_properties {
                        if !matches(q.name.as_str(), &format!("{fqn}.{}", q.name)) {
                            continue;
                        }
                        let Some(uri) = make_uri(&q.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            q.name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::METHOD,
                            uri,
                            &q.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                }
                Element::Enumeration(e) => {
                    for v in &e.values {
                        if !matches(v.name.as_str(), &format!("{fqn}.{}", v.name)) {
                            continue;
                        }
                        let Some(uri) = make_uri(&v.source_info) else {
                            continue;
                        };
                        push_symbol(
                            &mut out,
                            v.name.to_string(),
                            Some(fqn.clone()),
                            SymbolKind::ENUM_MEMBER,
                            uri,
                            &v.source_info,
                        );
                        if out.len() >= WORKSPACE_SYMBOL_LIMIT {
                            break 'chunks;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn push_symbol(
    out: &mut Vec<WorkspaceSymbol>,
    name: String,
    container_name: Option<String>,
    kind: SymbolKind,
    uri: Uri,
    si: &SourceInfo,
) {
    // The modern `WorkspaceSymbol` shape — `location` is an `Either`
    // of full `Location` (with range) or just a `WorkspaceSymbolLocation`
    // (uri-only, deferring the range until `workspaceSymbol/resolve`).
    // Always emit the full `Location` since we know it from the model;
    // saves the round-trip resolve call clients would otherwise make.
    out.push(WorkspaceSymbol {
        name,
        kind,
        tags: None,
        container_name,
        location: OneOf::Left(Location {
            uri,
            range: range_from_source_info(si),
        }),
        data: None,
    });
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

/// Code lenses for runnable functions. Walks every function in the
/// file and emits:
///
/// - **`legend.runTest`** (`▶ Run test`) — for functions tagged with
///   the `test::Test` stereotype. Driven by the planned IntelliJ
///   test-runner integration.
/// - **`legend.run`** (`▶ Run`) — for *any* parameterless,
///   non-native function. Mirrors what IntelliJ gives Java
///   `main()` / Kotlin `fun main()`: any zero-arg entry point gets
///   a clickable gutter affordance. The two lenses can coexist on
///   the same function (a `<<test::Test>>`-tagged parameterless
///   function gets BOTH lenses); the IDE will stack them.
///
/// Both lenses pass the function's FQN as their single argument.
/// The server's `workspace/executeCommand` handler routes by the
/// `command` field.
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
            let element_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: idx,
            };
            let fqn = render_fqn(model, element_id);
            let name_range = range_from_source_info(&node.name_source_info);
            if is_test_stereotyped(func) {
                lenses.push(CodeLens {
                    range: name_range,
                    command: Some(Command {
                        title: "▶ Run test".to_string(),
                        command: "legend.runTest".to_string(),
                        arguments: Some(vec![serde_json::Value::String(fqn.clone())]),
                    }),
                    data: None,
                });
            }
            if is_pct_test_stereotyped(func) {
                lenses.push(CodeLens {
                    range: name_range,
                    command: Some(Command {
                        title: "▶ Run PCT".to_string(),
                        command: "legend.runPCT".to_string(),
                        arguments: Some(vec![serde_json::Value::String(fqn.clone())]),
                    }),
                    data: None,
                });
            }
            if is_parameterless_runnable(func) {
                lenses.push(CodeLens {
                    range: name_range,
                    command: Some(Command {
                        title: "▶ Run".to_string(),
                        command: "legend.run".to_string(),
                        arguments: Some(vec![serde_json::Value::String(fqn)]),
                    }),
                    data: None,
                });
            }
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

/// Whether the function carries the `<<PCT.test>>` stereotype.
///
/// PCT (Pure Compatibility Tests) functions take a single
/// `adapter:Function<Any>[1]` parameter — they're never
/// parameterless, so they never collide with the `legend.run`
/// lens. They DO need their own gutter affordance because the
/// click flow is materially different: the IDE prompts the user
/// to pick a PCT adapter, then dispatches
/// `legend.runPCT { fqn, adapterFqn }`, which routes the test
/// through `meta::pure::test::surveyor::runPCTTests` with that
/// adapter injected.
///
/// Loose match on the stereotype name (`"test"`, lowercase — note
/// that `<<test::Test>>` uses uppercase `"Test"`, so the two
/// stereotypes never collide). A stricter check would verify
/// `profile == meta::pure::test::pct::PCT`'s ElementId; the
/// IntelliJ runner can post-filter if we ever see collisions.
fn is_pct_test_stereotyped(func: &legend_pure_parser_pure::nodes::function::Function) -> bool {
    func.stereotypes.iter().any(|st| st.value == "test")
}

/// Whether a function deserves a generic `▶ Run` gutter affordance.
/// Same idea as IntelliJ's Java/Kotlin "runnable main" detection:
/// any zero-argument, user-defined function is a valid entry point.
/// Native functions are excluded — they're FFI shims, not Pure
/// source the user expects to evaluate from the IDE.
fn is_parameterless_runnable(func: &legend_pure_parser_pure::nodes::function::Function) -> bool {
    func.parameters.is_empty() && !func.is_native
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

/// JSON shape returned to the IDE from `workspace/executeCommand`.
///
/// The plugin parses this on the response, builds a `Notification`
/// (info on success, error otherwise). Designed to be small and
/// human-readable — the full evaluator output (e.g. a multi-line
/// collection rendering) is included in `value` / `error`, and
/// `summary` is the one-liner the plugin can use as the
/// notification title.
///
/// `extras` carries command-specific structured data — used by
/// `legend.listPctAdapters` to return the discovered adapter list
/// `[{"fqn": "...", "name": "..."}]` without inventing a separate
/// response shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecuteCommandResult {
    /// `true` iff the function evaluated without raising a
    /// `PureException` AND the workspace had a populated model.
    pub ok: bool,
    /// Function FQN that ran. Empty for commands that don't target
    /// a specific function (e.g. `legend.listPctAdapters`).
    pub fqn: String,
    /// Rendered return value when `ok`. `None` on error.
    pub value: Option<String>,
    /// Rendered error message when `!ok`. `None` on success.
    pub error: Option<String>,
    /// Command-specific structured payload. Skipped from JSON when
    /// absent so the wire shape stays small for the common case.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extras: Option<serde_json::Value>,
}

impl ExecuteCommandResult {
    /// One-line summary suitable for a notification title.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.ok {
            format!(
                "{} → {}",
                self.fqn,
                self.value.as_deref().unwrap_or("<no value>"),
            )
        } else {
            format!(
                "{}: {}",
                self.fqn,
                self.error.as_deref().unwrap_or("<unknown error>"),
            )
        }
    }
}

/// Evaluate a Pure function FQN against the workspace's compiled
/// model and render the result.
///
/// Drives the IDE's ▶ Run gutter icon: the
/// `PureRunLineMarkerContributor` sends
/// `workspace/executeCommand{ command: "legend.run", arguments:
/// [fqn] }`, which routes here. We open a runtime `Evaluator` over
/// the workspace's already-compiled `PureModel`, call
/// `evaluator.call(fqn, &[])`, and return a serializable
/// [`ExecuteCommandResult`].
///
/// Why this lives in the LSP and not as a shelled-out `legend run`
/// subprocess: the workspace already has the user's classpath +
/// open-buffer state compiled into `model`. The CLI path would
/// re-load just the embedded platform and miss every user-defined
/// function. Reuse-the-model wins on both correctness and latency.
pub async fn execute_legend_command(
    workspace: std::sync::Arc<tokio::sync::Mutex<Workspace>>,
    command: &str,
    arguments: &[serde_json::Value],
) -> ExecuteCommandResult {
    // Snapshot the model under the workspace lock. The evaluation
    // itself runs OUTSIDE the lock so a long-running function
    // doesn't block diagnostics / hover / etc. The model is
    // cloneable via Arc; copying the handle is cheap.
    let model_arc = {
        let ws = workspace.lock().await;
        ws.model.clone()
    };
    let Some(model_arc) = model_arc else {
        return ExecuteCommandResult {
            ok: false,
            fqn: arg_str(arguments, 0).unwrap_or_default(),
            value: None,
            error: Some("Workspace not yet compiled".into()),
            extras: None,
        };
    };

    // Run the eval on a blocking task — the tree-walking interpreter
    // is synchronous and can hold the thread for the function's
    // entire runtime. Tokio's blocking pool is the right home. The
    // actual evaluation logic lives in
    // [`legend_pure_runtime::runner`] so the LSP and the MCP server
    // share one implementation; this fn is the LSP-side adapter
    // (typed runner results → `ExecuteCommandResult` JSON shape).
    let command = command.to_string();
    let arg0 = arg_str(arguments, 0).unwrap_or_default();
    let arg1 = arg_str(arguments, 1).unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        // Wiring mirrors `legend test` (crates/cli/src/commands/test.rs):
        // relational natives + Mapping/Relational DSL populators. The
        // duplication is intentional pending the distributed-slice
        // backlog item that will auto-register both.
        let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
        let registry =
            legend_pure_runtime::native::NativeRegistry::with_extensions(&[&relational_ext]);
        let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
        let database_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
        let class_mapping_pop =
            legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
        let populators: &[&dyn legend_pure_runtime::dsl::DSLPopulator] =
            &[&mapping_pop, &database_pop, &class_mapping_pop];
        let model = model_arc.as_ref();
        match command.as_str() {
            "legend.runTest" => translate_test_result(
                "legend.runTest",
                &arg0,
                legend_pure_runtime::runner::run_test(model, &registry, populators, &arg0),
            ),
            "legend.runPCT" => translate_test_result(
                "legend.runPCT",
                &arg0,
                legend_pure_runtime::runner::run_pct(model, &registry, populators, &arg0, &arg1),
            ),
            "legend.listPctAdapters" => {
                translate_adapters_result(legend_pure_runtime::runner::list_pct_adapters(model))
            }
            _ => translate_run_result(
                &command,
                &arg0,
                legend_pure_runtime::runner::run_function(model, &registry, populators, &arg0),
            ),
        }
    })
    .await
    .unwrap_or_else(|join_err| ExecuteCommandResult {
        ok: false,
        fqn: arg_str(arguments, 0).unwrap_or_default(),
        value: None,
        error: Some(format!("Evaluation task panicked: {join_err}")),
        extras: None,
    })
}

/// Pull the Nth argument from an LSP `workspace/executeCommand`
/// arguments array as a string, normalizing the two shapes lsp4j
/// can serialize it as (raw `String` or `JsonValue::String`).
fn arg_str(arguments: &[serde_json::Value], i: usize) -> Option<String> {
    arguments.get(i).and_then(|v| v.as_str().map(String::from))
}

// ---------------------------------------------------------------------------
// Translators: typed `runtime::runner` results → `ExecuteCommandResult`
// JSON. The plugin's parsers depend on these exact shapes — keep them
// in lock-step with `clients/intellij/.../run/PureRunToolWindowService.kt`.
// ---------------------------------------------------------------------------

/// Translate a `legend.run` (or other plain function-call) outcome.
///
/// On success: `value` carries the rendered return value, `extras`
/// contains `stdout` if any was captured. On failure: `error`
/// carries the raw `PureException` text (preserving the original
/// `[command] message` prefix the IDE shows in the notification),
/// `extras.failures` carries a one-element list with the parsed
/// stack so the plugin renders clickable links the same way it does
/// for test failures.
fn translate_run_result(
    command: &str,
    fqn: &str,
    result: legend_pure_runtime::runner::RunResult,
) -> ExecuteCommandResult {
    if result.ok {
        ExecuteCommandResult {
            ok: true,
            fqn: fqn.to_string(),
            value: result.value,
            error: None,
            extras: build_extras(&result.stdout, None),
        }
    } else {
        // RunResult's contract guarantees Some(error) when ok=false,
        // but `expect` is forbidden in library code. Fall back to a
        // synthetic error that surfaces the contract violation through
        // the same channel rather than panicking.
        let err = result
            .error
            .unwrap_or_else(|| legend_pure_runtime::runner::RunError {
                message:
                    "RunResult { ok: false } missing structured error (producer contract violation)"
                        .to_string(),
                stack: Vec::new(),
                raw: String::new(),
            });
        let detail = serde_json::json!({
            "fqn": fqn,
            "message": err.message,
            "stack": err.stack,
        });
        ExecuteCommandResult {
            ok: false,
            fqn: fqn.to_string(),
            value: None,
            error: Some(format!("[{command}] {}", err.raw)),
            extras: build_extras(&result.stdout, Some(serde_json::json!([detail]))),
        }
    }
}

/// Translate a `legend.runTest` / `legend.runPCT` outcome (surveyor-
/// driven runs that produce a `TestRunResult` on success).
fn translate_test_result(
    command: &str,
    fqn: &str,
    result: Result<
        legend_pure_runtime::runner::TestRunResult,
        legend_pure_runtime::runner::RunnerError,
    >,
) -> ExecuteCommandResult {
    match result {
        Ok(r) => {
            let failures = if r.failures.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&r.failures).unwrap_or(serde_json::Value::Null))
            };
            ExecuteCommandResult {
                ok: r.ok,
                fqn: fqn.to_string(),
                value: Some(r.rendered.clone()),
                error: if r.ok { None } else { Some(r.rendered) },
                extras: build_extras(&r.stdout, failures),
            }
        }
        Err(e) => ExecuteCommandResult {
            ok: false,
            fqn: fqn.to_string(),
            value: None,
            error: Some(format!("[{command}] {e}")),
            extras: None,
        },
    }
}

/// Translate a `legend.listPctAdapters` outcome.
///
/// `extras` is a JSON array (not a wrapped object) for backwards
/// compatibility with the IntelliJ plugin's chooser parser, which
/// reads `extras` directly as `JsonArray`.
fn translate_adapters_result(
    result: Result<
        Vec<legend_pure_runtime::runner::PctAdapterInfo>,
        legend_pure_runtime::runner::RunnerError,
    >,
) -> ExecuteCommandResult {
    match result {
        Ok(adapters) => {
            let count = adapters.len();
            let json = serde_json::to_value(&adapters).unwrap_or(serde_json::Value::Null);
            ExecuteCommandResult {
                ok: true,
                fqn: String::new(),
                value: Some(format!("{count} adapter(s)")),
                error: None,
                extras: Some(json),
            }
        }
        Err(e) => ExecuteCommandResult {
            ok: false,
            fqn: String::new(),
            value: None,
            error: Some(format!("[legend.listPctAdapters] {e}")),
            extras: None,
        },
    }
}

/// Build an `extras` object combining `stdout` (if non-empty) and a
/// pre-built JSON value to merge under `failures`. Returns `None`
/// when neither is present so the wire payload stays small for the
/// green-no-output case.
fn build_extras(stdout: &str, failures: Option<serde_json::Value>) -> Option<serde_json::Value> {
    if stdout.is_empty() && failures.is_none() {
        return None;
    }
    let mut map = serde_json::Map::new();
    if !stdout.is_empty() {
        map.insert(
            "stdout".into(),
            serde_json::Value::String(stdout.to_string()),
        );
    }

    if let Some(f) = failures {
        map.insert("failures".into(), f);
    }
    Some(serde_json::Value::Object(map))
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
            Some(tower_lsp_server::ls_types::DiagnosticSeverity::ERROR)
        );
        // Range converts 1-indexed (2,3)-(2,8) to 0-indexed (1,2)-(1,7).
        assert_eq!(d.range.start.line, 1);
        assert_eq!(d.range.start.character, 2);
        assert_eq!(d.range.end.line, 1);
        assert_eq!(d.range.end.character, 7);
        // Code surfaces the kind tag.
        assert!(matches!(
            d.code.as_ref(),
            Some(tower_lsp_server::ls_types::NumberOrString::String(s)) if s == "unresolvedElement"
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
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        // Cursor inside `$name` body. The Variable should resolve to
        // the parameter `name`, declared at line 1 col 22 (1-indexed)
        // → 0-indexed col 21.
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        let index = legend_pure_ide::build_reference_index(&model);
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
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // The body line is line 8 (1-indexed). `$p.name` starts at
        // col 3; `name` itself is at col 6. LSP positions are 0-indexed
        // → (line 7, col 5+).
        let loc =
            definition_for_position(&model, None, "fixture.pure", pos(7, 6), &uri, no_cross_file);
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
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        let loc =
            definition_for_position(&model, None, "fixture.pure", pos(2, 4), &uri, no_cross_file);
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
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // `msg` starts at line 1 col 16 (1-indexed) → 0-indexed col 15.
        let loc = definition_for_position(
            &model,
            None,
            "fixture.pure",
            pos(0, 16),
            &uri,
            no_cross_file,
        )
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
        let app_uri = "file:///abs/proj/app.pure".parse::<Uri>().unwrap();
        let lib_disk_uri = "file:///abs/proj/lib.pure".parse::<Uri>().unwrap();

        let lib_disk_uri_clone = lib_disk_uri.clone();
        let resolver: &dyn Fn(&str) -> Option<Uri> = &move |canonical: &str| {
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
        let dead_resolver: &dyn Fn(&str) -> Option<Uri> = &|_| None;
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
    fn code_lenses_run_for_parameterless_untagged_function() {
        // Any parameterless non-native function — even without a
        // `<<test::Test>>` stereotype — should get a generic `▶ Run`
        // lens. Same UX as IntelliJ's `▶ main()` affordance.
        let plain = compile_fixture(
            "plain.pure",
            "function test::ordinary(): Integer[1]\n{\n  1\n}\n",
        );
        let lenses = code_lenses_for(&plain, "plain.pure");
        assert_eq!(
            lenses.len(),
            1,
            "expected exactly one ▶ Run lens; got {lenses:?}"
        );
        let cmd = lenses[0]
            .command
            .as_ref()
            .expect("lens must carry a command");
        assert_eq!(cmd.command, "legend.run");
        assert_eq!(cmd.title, "▶ Run");
        // `render_fqn` returns the mangled FQN form
        // (`test::ordinary__Integer_1_`) — same shape `legend.runTest`
        // has always emitted. Consistent with the test-runner path.
        let arg = cmd
            .arguments
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|v| v.as_str())
            .expect("FQN must be passed as the single argument");
        assert!(
            arg.starts_with("test::ordinary"),
            "FQN must start with the function FQN; got {arg:?}",
        );
    }

    #[test]
    fn code_lenses_skip_function_with_parameters() {
        // Functions taking arguments are NOT runnable from the gutter
        // (no UI to pick argument values yet — the user must use
        // a Run config).
        let with_params = compile_fixture(
            "with_params.pure",
            "function test::greet(name: String[1]): String[1]\n{\n  $name\n}\n",
        );
        assert_eq!(
            code_lenses_for(&with_params, "with_params.pure").len(),
            0,
            "functions with parameters must not emit a ▶ Run lens",
        );
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
        // A parameterless `<<test::Test>>`-tagged function gets
        // BOTH lenses stacked on the name range — `▶ Run test` (from
        // the stereotype) and `▶ Run` (from being parameterless).
        // The IDE renders them side-by-side; both are useful entry
        // points to the same function.
        assert_eq!(
            lenses.len(),
            2,
            "expected ▶ Run test + ▶ Run lenses; got {lenses:?}",
        );
        let commands: Vec<&str> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.command.as_str())
            .collect();
        assert!(
            commands.contains(&"legend.runTest"),
            "test-stereotyped fn must produce legend.runTest; got {commands:?}",
        );
        assert!(
            commands.contains(&"legend.run"),
            "parameterless fn must also produce legend.run; got {commands:?}",
        );
    }

    /// Resolves a canonical path → `file:///<path>` URL. The
    /// real workspace consults open buffers and source roots; for
    /// handler tests we just synthesize a `file://` URL so the
    /// handler emits a location that round-trips through
    /// `SymbolInformation`. Returning `None` would suppress the
    /// symbol entirely.
    fn fixture_uri_resolver() -> impl Fn(&str) -> Option<Uri> {
        |canonical| format!("file://{canonical}").parse::<Uri>().ok()
    }

    #[test]
    fn workspace_symbols_lists_top_level_class_in_empty_query() {
        let model = compile_fixture("fixture.pure", "Class test::Person {}");
        let resolver = fixture_uri_resolver();
        let syms = workspace_symbols_for(&model, "", &resolver);
        let persons: Vec<&WorkspaceSymbol> =
            syms.iter().filter(|s| s.name == "test::Person").collect();
        assert_eq!(
            persons.len(),
            1,
            "expected exactly one Person entry; got: {syms:?}"
        );
        assert_eq!(persons[0].kind, SymbolKind::CLASS);
        assert_eq!(persons[0].container_name.as_deref(), Some("test"));
    }

    #[test]
    fn workspace_symbols_matches_substring_case_insensitive() {
        let src = "Class test::Person {}\nClass test::Profile {}\n";
        let model = compile_fixture("fixture.pure", src);
        let resolver = fixture_uri_resolver();
        let syms = workspace_symbols_for(&model, "PerS", &resolver);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"test::Person"),
            "case-insensitive substring 'PerS' must match 'Person'; got {names:?}",
        );
        assert!(
            !names.contains(&"test::Profile"),
            "Profile must not match 'PerS'; got {names:?}",
        );
    }

    #[test]
    fn workspace_symbols_surfaces_properties_with_container_name() {
        let src = "Class test::Person\n{\n  name: String[1];\n}\n";
        let model = compile_fixture("fixture.pure", src);
        let resolver = fixture_uri_resolver();
        let syms = workspace_symbols_for(&model, "name", &resolver);
        let field = syms
            .iter()
            .find(|s| s.kind == SymbolKind::FIELD && s.name == "name")
            .unwrap_or_else(|| panic!("expected a FIELD symbol named 'name'; got: {syms:?}"));
        assert_eq!(
            field.container_name.as_deref(),
            Some("test::Person"),
            "property's container_name must be the class FQN",
        );
    }

    #[test]
    fn workspace_symbols_skips_bootstrap_chunk() {
        let model = compile_fixture("fixture.pure", "Class test::Person {}");
        let resolver = fixture_uri_resolver();
        let syms = workspace_symbols_for(&model, "", &resolver);
        for bootstrap_name in ["Any", "Nil", "Integer", "String", "Boolean"] {
            assert!(
                !syms.iter().any(|s| s.name == bootstrap_name
                    || s.name.ends_with(&format!("::{bootstrap_name}"))),
                "bootstrap symbol {bootstrap_name:?} must not appear in workspace results"
            );
        }
    }

    // ---------------------------------------------------------------
    // textDocument/references — T-20260511-07
    // ---------------------------------------------------------------

    /// Build the reverse-index for a one-file fixture. Same helper
    /// pattern as the existing `definition_resolves_variable_…` test.
    fn refs_for(model: &PureModel) -> ReferenceIndex {
        legend_pure_ide::build_reference_index(model)
    }

    #[test]
    fn references_class_used_in_extends_returns_use_site() {
        let src = "\
Class test::A {}

Class test::B extends test::A {}
";
        let model = compile_fixture("fixture.pure", src);
        let index = refs_for(&model);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // Cursor on `A` declaration. `Class test::A {}` — `A` starts
        // at col 13 (1-indexed) → 0-indexed col 12.
        let locs = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(0, 12),
            false,
            &uri,
            no_cross_file,
        );
        assert_eq!(
            locs.len(),
            1,
            "expected exactly one usage (the extends site); got: {locs:?}"
        );
        // The use site is on line 3 (`Class test::B extends test::A {}`),
        // 1-indexed; LSP is 0-indexed → line 2.
        assert_eq!(
            locs[0].range.start.line, 2,
            "use site must be on the extends line"
        );
    }

    #[test]
    fn references_click_on_use_site_returns_same_target() {
        let src = "\
Class test::A {}

Class test::B extends test::A {}
";
        let model = compile_fixture("fixture.pure", src);
        let index = refs_for(&model);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // Cursor on the `A` in `extends test::A` (use site).
        // Line 3 col 29 (1-indexed) → line 2, char 28 (0-indexed).
        let from_use_site = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(2, 28),
            false,
            &uri,
            no_cross_file,
        );
        // Cursor on `A` declaration (col 12 0-indexed).
        let from_decl = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(0, 12),
            false,
            &uri,
            no_cross_file,
        );
        assert_eq!(
            from_use_site, from_decl,
            "clicking a use site must resolve to the same target as clicking the declaration"
        );
    }

    #[test]
    fn references_include_declaration_appends_decl() {
        let src = "\
Class test::A {}

Class test::B extends test::A {}
";
        let model = compile_fixture("fixture.pure", src);
        let index = refs_for(&model);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // Without include_declaration — exactly the use sites.
        let without = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(0, 12),
            false,
            &uri,
            no_cross_file,
        );
        // With include_declaration — adds the declaration span at the end.
        let with = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(0, 12),
            true,
            &uri,
            no_cross_file,
        );
        assert_eq!(
            with.len(),
            without.len() + 1,
            "include_declaration=true must add exactly one Location (the decl); got: {with:?}"
        );
        let decl_range = with.last().expect("decl location present").range;
        // The declaration's `name_source_info` covers just `A` on line 1.
        assert_eq!(decl_range.start.line, 0);
    }

    #[test]
    fn references_empty_on_whitespace_cursor() {
        let src = "\
Class test::A {}

Class test::B extends test::A {}
";
        let model = compile_fixture("fixture.pure", src);
        let index = refs_for(&model);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // Cursor on the blank line 2 (0-indexed line 1, char 0).
        let locs = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(1, 0),
            false,
            &uri,
            no_cross_file,
        );
        assert!(
            locs.is_empty(),
            "cursor on whitespace must return no usages; got: {locs:?}"
        );
    }

    #[test]
    fn references_empty_on_property_call_v1_limitation() {
        // V1 scope: PropertyCall references carry target_element=None
        // and are NOT exposed via `usages_of`. Pin this so a future
        // change to richer reference targets doesn't silently flip
        // the result and surprise callers.
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
        let index = refs_for(&model);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // Cursor inside `$p.name` — `name` is at line 8 col 6
        // (1-indexed) → 0-indexed (7, 5).
        let locs = references_for_position(
            &model,
            Some(&index),
            "fixture.pure",
            pos(7, 5),
            false,
            &uri,
            no_cross_file,
        );
        assert!(
            locs.is_empty(),
            "V1 scope: property cursors must return empty; got: {locs:?}"
        );
    }

    /// Cross-file branch: hand-built model with two chunks — chunk 0
    /// has a class declared in `lib.pure`, chunk 1 has another class
    /// extending it from `app.pure`. Verifies that the resolver is
    /// consulted for cross-file targets (use-site URI = `app.pure`,
    /// not the click's `lib.pure`).
    ///
    /// Manual construction (instead of compile_fixture spanning two
    /// files) because `pipeline::compile`'s import resolution across
    /// hand-rolled chunks is fragile in test fixtures and the unit
    /// here is the URI-resolution branch, not the compile pipeline.
    #[test]
    fn references_cross_file_uri_resolves_via_resolver() {
        use legend_pure_ide::{RefKind, Reference, ReferenceIndex};
        use legend_pure_parser_ast::SourceInfo as SI;
        use legend_pure_parser_pure::ids::ElementId;
        use legend_pure_parser_pure::model::{
            Element as ModelElement, ElementNode, ModelChunk, PureModel,
        };
        use legend_pure_parser_pure::nodes::class::Class;

        let mut model = PureModel::new();
        let test_pkg = model.get_or_create_package(&[smol_str::SmolStr::new("test")]);

        let chunk_id: u16 = 0;
        let mut chunk = ModelChunk::new(chunk_id);
        // Class A declared in /proj/lib.pure.
        let class_body_si = SI::new("/proj/lib.pure", 1, 1, 1, 17);
        let class_name_si = SI::new("/proj/lib.pure", 1, 7, 1, 14);
        let class_idx = chunk.alloc_element(
            ElementNode {
                name: smol_str::SmolStr::new("A"),
                source_info: class_body_si.clone(),
                name_source_info: class_name_si.clone(),
                parent_package: test_pkg,
            },
            ModelElement::Class(Class {
                type_parameters: Vec::new(),
                multiplicity_parameters: Vec::new(),
                type_variable_parameters: Vec::new(),
                super_types: Vec::new(),
                properties: Vec::new(),
                qualified_properties: Vec::new(),
                constraints: Vec::new(),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
                original_milestoned_properties: Vec::new(),
            }),
        );
        let class_id = ElementId::InstanceId {
            chunk_id,
            local_idx: class_idx,
        };
        model.chunks.push(chunk);
        model.register_element(test_pkg, class_id);

        // Build a synthetic reference index: one use site in app.pure
        // pointing at lib.pure's class A.
        let mut index = ReferenceIndex::default();
        let use_site_si = SI::new("/proj/app.pure", 3, 23, 3, 30);
        index.push(Reference {
            range: use_site_si.clone(),
            kind: RefKind::TypeRef,
            target_element: Some(class_id),
            target: class_name_si.clone(),
        });
        index.finalize();

        // The click happens at the class declaration (in lib.pure).
        let lib_uri = "file:///abs/proj/lib.pure".parse::<Uri>().unwrap();
        let app_disk_uri = "file:///abs/proj/app.pure".parse::<Uri>().unwrap();
        let app_disk_uri_clone = app_disk_uri.clone();
        let resolver: &dyn Fn(&str) -> Option<Uri> = &move |canonical: &str| match canonical {
            "/proj/app.pure" => Some(app_disk_uri_clone.clone()),
            _ => None,
        };

        // Cursor on `A` in the class declaration (line 1 col 7 1-indexed
        // → line 0 char 6 0-indexed).
        let locs = references_for_position(
            &model,
            Some(&index),
            "/proj/lib.pure",
            pos(0, 6),
            false,
            &lib_uri,
            resolver,
        );
        assert_eq!(locs.len(), 1);
        assert_eq!(
            locs[0].uri, app_disk_uri,
            "cross-file URI must be resolved through the resolver",
        );
    }

    #[test]
    fn references_returns_empty_when_index_missing() {
        let src = "Class test::A {}\n";
        let model = compile_fixture("fixture.pure", src);
        let uri = "file:///fixture.pure".parse::<Uri>().unwrap();
        let no_cross_file: &dyn Fn(&str) -> Option<Uri> = &|_| None;
        // No ReferenceIndex passed — handler must still find the
        // declaration (via locate) and report zero use sites (the
        // index is what holds usages; without it, no usages
        // available). Pinning this so future changes don't
        // accidentally start panicking on a missing index.
        let locs = references_for_position(
            &model,
            None,
            "fixture.pure",
            pos(0, 12),
            false,
            &uri,
            no_cross_file,
        );
        assert!(
            locs.is_empty(),
            "without an index, references must return empty (not panic); got: {locs:?}"
        );
    }

    mod publish_plan {
        use super::*;
        use legend_pure_core_platform::repo::{OwnedSourceFile, Repo};
        use std::path::PathBuf;

        fn fs_repo(prefix: &str, root: PathBuf, files: &[&str]) -> Repo {
            Repo::Filesystem {
                prefix: prefix.to_string(),
                files: files
                    .iter()
                    .map(|p| OwnedSourceFile {
                        path: (*p).to_string(),
                        content: String::new(),
                    })
                    .collect(),
                meta: None,
                source_root: Some(root),
            }
        }

        fn err(source: &str, message: &str) -> CompilationError {
            CompilationError {
                message: message.to_string(),
                source_info: SourceInfo::new(source, 1, 1, 1, 1),
                kind: legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement {
                    path: smol_str::SmolStr::new("Foo"),
                },
            }
        }

        fn uri_of(s: &str) -> Uri {
            s.parse::<Uri>().expect("test URI must parse")
        }

        #[test]
        fn empty_workspace_yields_empty_plan() {
            let ws = Workspace::new(Vec::new(), Vec::new());
            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            assert!(plan.entries.is_empty());
            assert!(plan.next_previously_published.is_empty());
        }

        #[test]
        fn open_buffer_with_no_errors_gets_empty_publish() {
            // Even when no compile has run, an open file must receive
            // a single empty publish so the IDE shows "no errors" on
            // didOpen. Preserves the current LSP behaviour at server.rs
            // lines 88-92 in the old implementation.
            let mut ws = Workspace::new(Vec::new(), Vec::new());
            let uri = uri_of("file:///proj/a.pure");
            ws.set_open_buffer(uri.clone(), String::new());

            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            assert_eq!(plan.entries.len(), 1);
            assert_eq!(plan.entries[0].0, uri);
            assert!(plan.entries[0].1.is_empty());
            // Empty publish must NOT be tracked as previously-published
            // — there's nothing to clear next time.
            assert!(plan.next_previously_published.is_empty());
        }

        #[test]
        fn diagnostics_for_unopened_file_resolve_through_source_root() {
            // Workspace has a filesystem repo with on-disk root
            // /tmp/proj. A canonical /proj/lib.pure should resolve to
            // file:///tmp/proj/lib.pure even though the file is NOT in
            // open_buffers. This is the core "global diagnostics"
            // behaviour T-20260511-06 asks for.
            let mut ws = Workspace::new(
                vec![fs_repo(
                    "/proj",
                    PathBuf::from("/tmp/proj"),
                    &["/proj/lib.pure"],
                )],
                Vec::new(),
            );
            ws.diagnostics.insert(
                "/proj/lib.pure".to_string(),
                vec![err("/proj/lib.pure", "boom")],
            );

            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            assert_eq!(plan.entries.len(), 1);
            let (uri, diags) = &plan.entries[0];
            assert!(
                uri.as_str().contains("/tmp/proj/lib.pure"),
                "expected on-disk path in URI; got {uri:?}",
            );
            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0].message, "boom");
            // Non-empty publish for this URI — track it so the next
            // cycle can clear it if the user fixes the error.
            assert!(plan.next_previously_published.contains(uri));
        }

        #[test]
        fn synthetic_source_is_filtered_out() {
            // Canonical /platform/pure/... or anything not covered by
            // a Filesystem repo with source_root returns None from
            // file_uri_for_canonical. Those diagnostics must NOT be
            // published — there's no on-disk file the client can
            // navigate to.
            let mut ws = Workspace::new(Vec::new(), Vec::new());
            ws.diagnostics.insert(
                "/platform/pure/grammar/m3.pure".to_string(),
                vec![err("/platform/pure/grammar/m3.pure", "boom")],
            );

            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            assert!(plan.entries.is_empty());
            assert!(plan.next_previously_published.is_empty());
        }

        #[test]
        fn stale_uri_gets_empty_publish_when_dropped() {
            // Previously published a non-empty diag for B.pure; this
            // cycle B.pure no longer appears in ws.diagnostics. The
            // plan must emit an empty publish for B's URI so the
            // client clears its stale entry.
            let ws = Workspace::new(
                vec![fs_repo(
                    "/proj",
                    PathBuf::from("/tmp/proj"),
                    &["/proj/b.pure"],
                )],
                Vec::new(),
            );
            let b_uri = uri_of("file:///tmp/proj/b.pure");
            let mut prev = HashSet::new();
            prev.insert(b_uri.clone());

            let plan = plan_diagnostics_publish(&ws, &prev);
            assert_eq!(plan.entries.len(), 1);
            assert_eq!(plan.entries[0].0, b_uri);
            assert!(plan.entries[0].1.is_empty(), "stale entry must be cleared");
            assert!(plan.next_previously_published.is_empty());
        }

        #[test]
        fn open_buffer_with_errors_publishes_diagnostics_once() {
            // An open buffer that ALSO has compile errors should get a
            // single non-empty publish (the resolved diagnostics
            // override the empty open-buffer placeholder).
            let mut ws = Workspace::new(
                vec![fs_repo(
                    "/proj",
                    PathBuf::from("/tmp/proj"),
                    &["/proj/a.pure"],
                )],
                Vec::new(),
            );
            let a_uri = uri_of("file:///tmp/proj/a.pure");
            ws.set_open_buffer(a_uri.clone(), String::new());
            ws.diagnostics.insert(
                "/proj/a.pure".to_string(),
                vec![err("/proj/a.pure", "boom")],
            );

            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            assert_eq!(plan.entries.len(), 1, "must not double-publish");
            assert_eq!(plan.entries[0].0, a_uri);
            assert_eq!(plan.entries[0].1.len(), 1);
            assert!(plan.next_previously_published.contains(&a_uri));
        }

        /// End-to-end integration of `Workspace::compile()` and
        /// `plan_diagnostics_publish`: a real two-file workspace where
        /// editing A produces a compile error in B, and the planner
        /// surfaces B's URI even though B is not in `open_buffers`.
        /// This is the actual "global diagnostics" UX win T-20260511-06
        /// asks for; the rest of `publish_plan` tests cover planner
        /// correctness in isolation with hand-injected diagnostics.
        #[test]
        fn cross_file_error_surfaces_on_unopened_file() {
            use legend_pure_core_platform::repo::Repo;
            use std::fs;

            // The Filesystem canonical-tail match in `snapshot_repos`
            // requires the on-disk leaf directory to be named after the
            // repo. Build a descriptor-shaped layout:
            //   <tmp>/userproj.json    — declares dep on `platform`
            //   <tmp>/userproj/a.pure
            //   <tmp>/userproj/b.pure
            // → canonicals /userproj/a.pure, /userproj/b.pure.
            // Using a descriptor (rather than `Repo::from_filesystem`)
            // populates `RepoMeta`, which the topo-sorter needs for
            // multi-repo workspaces — `from_filesystem` produces a
            // `meta: None` repo that fails topo sort against the
            // embedded platform.
            let tmp = tempfile::tempdir().expect("tempdir");
            let descriptor = tmp.path().join("userproj.json");
            fs::write(
                &descriptor,
                r#"{"name":"userproj","pattern":".*","dependencies":["platform"]}"#,
            )
            .expect("write descriptor");
            let proj = tmp.path().join("userproj");
            fs::create_dir(&proj).expect("mkdir userproj");
            let a_disk = proj.join("a.pure");
            let b_disk = proj.join("b.pure");

            // Clean baseline: A defines `Foo` with a `name` property;
            // B references it. Both compile clean against the platform.
            let a_clean = "Class abc::Foo { name: String[1]; }\n";
            let b_src = "function abc::greet(f: abc::Foo[1]): String[1] { $f.name }\n";
            fs::write(&a_disk, a_clean).expect("write a.pure");
            fs::write(&b_disk, b_src).expect("write b.pure");

            // Embedded platform = M3 bootstrap; user project = the two
            // files above. Same shape as a real `legend lsp` workspace
            // with no DSL artifacts and one local project repo.
            let user_repo = Repo::from_descriptor(&descriptor).expect("descriptor repo loads");
            let mut repos = Repo::default_embedded();
            repos.push(user_repo);
            let mut ws = Workspace::new(repos, Vec::new());

            // Sanity: the clean baseline compiles without user-source
            // errors. (We can't assert exactly zero errors because the
            // embedded platform itself may emit informational warnings
            // through the same channel; what we care about is that
            // there are no errors attributed to b.pure.)
            ws.compile();
            assert!(
                !ws.diagnostics.contains_key("/userproj/b.pure"),
                "baseline must have no errors on b.pure; got {:?}",
                ws.diagnostics.get("/userproj/b.pure"),
            );

            // Break A by renaming the `name` property — B's `$f.name`
            // becomes unresolved. Use `set_open_buffer` so the next
            // compile sees the broken content via the snapshot overlay,
            // without touching the disk file (which `Repo::Filesystem`
            // slurped eagerly at construction and won't re-read).
            let a_broken = "Class abc::Foo { nickname: String[1]; }\n";
            let a_uri = Uri::from_file_path(&a_disk).expect("a uri");
            ws.set_open_buffer(a_uri.clone(), a_broken.to_string());
            ws.compile();

            // The error must now be attributed to b.pure — that's the
            // whole point of T-20260511-06. If validators ever start
            // attributing cross-file errors to the *defining* element
            // (a.pure) instead of the *use site* (b.pure), this fails
            // loudly.
            assert!(
                ws.diagnostics.contains_key("/userproj/b.pure"),
                "expected error attributed to b.pure; diagnostic keys: {:?}",
                ws.diagnostics.keys().collect::<Vec<_>>(),
            );

            // The planner must surface that error to b.pure's `file://`
            // URI even though b.pure is NOT open. Without
            // `file_uri_for_canonical` resolving through the
            // Filesystem repo's `source_root`, this entry would either
            // not appear or appear under a broken `file://userproj/...`
            // URI (the pre-fix behaviour).
            let plan = plan_diagnostics_publish(&ws, &HashSet::new());
            let b_uri = Uri::from_file_path(&b_disk).expect("b uri");
            let b_entry = plan.entries.iter().find(|(uri, _)| *uri == b_uri);
            let (_, b_diags) = b_entry.unwrap_or_else(|| {
                panic!(
                    "expected planner to publish for b.pure URI {b_uri:?}; got {:?}",
                    plan.entries.iter().map(|(u, _)| u).collect::<Vec<_>>(),
                )
            });
            assert!(
                !b_diags.is_empty(),
                "expected non-empty diagnostics for b.pure",
            );
            assert!(plan.next_previously_published.contains(&b_uri));

            // Stale-clear path: fix A back to the clean content. The
            // next compile clears b.pure's diagnostics, and the planner
            // must emit an empty publish for b.pure to clear it on the
            // client.
            ws.set_open_buffer(a_uri, a_clean.to_string());
            ws.compile();
            let prev = plan.next_previously_published.clone();
            let plan2 = plan_diagnostics_publish(&ws, &prev);
            let b_entry2 = plan2
                .entries
                .iter()
                .find(|(uri, _)| *uri == b_uri)
                .expect("stale-clear must emit entry for b.pure");
            assert!(
                b_entry2.1.is_empty(),
                "stale-clear must be an empty publish; got {:?}",
                b_entry2.1,
            );
            assert!(!plan2.next_previously_published.contains(&b_uri));
        }
    }
}
