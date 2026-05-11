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

//! Reference index for IDE navigation.
//!
//! After [`PureModel`] is built, [`build_reference_index`] walks the
//! resolved model and emits one [`Reference`] per source-level reference
//! site (function call, property access, stereotype, type ref, etc.).
//! References are stored two ways:
//!
//! - **By file**: `Vec<Reference>` per canonical source path. Used by
//!   `goto_definition` / cmd-hover — find the smallest range containing
//!   the cursor, return its target.
//! - **By target**: `HashMap<ElementId, Vec<…>>`. Used by `find_usages`
//!   — given the target element, list every site that points at it.
//!
//! The data model is **uniform**: every clickable thing in a Pure file
//! becomes a `Reference`. Adding a new ref kind = one push during the
//! build walk, no parallel locator extension.

use legend_pure_parser_ast::SourceInfo;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::HashMap;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::nodes::class::Class;
use crate::types::{ExprKind, Parameter, TypeExpr, ValueSpec};

/// What kind of source reference this is. Carried for IDE rendering
/// hints (semantic-token coloring, distinct hover labels) and for
/// future filtering of find-usages by reference kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefKind {
    /// `foo()` or `x->foo()` — call to a user-defined function.
    FunctionCall,
    /// `$x.foo` — property access on a class instance.
    PropertyCall,
    /// `$x.qp(args)` — qualified-property invocation.
    QualifiedPropertyCall,
    /// `$x` — variable reference; target is a parameter or local.
    Variable,
    /// `String` / `Person` etc. — packageable element reference in
    /// type position (`extends`, parameter type, return type, …).
    TypeRef,
    /// `MyEnum.VALUE` — enum value reference.
    EnumValue,
    /// `<<Profile.stereo>>` — stereotype reference inside an
    /// annotation block.
    StereotypeRef,
    /// `{Profile.tag = '…'}` — tagged-value reference inside an
    /// annotation block.
    TaggedValueRef,
}

/// One source reference: a clickable region in a `.pure` file plus
/// the location it should navigate to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// Source range that activates the reference — the click region
    /// the IDE underlines on cmd-hover.
    pub range: SourceInfo,
    /// What kind of reference this is.
    pub kind: RefKind,
    /// Target element (the thing being referenced). `None` for
    /// references whose target is purely positional, e.g. a Variable
    /// pointing at a Parameter declaration with no element ID.
    pub target_element: Option<ElementId>,
    /// Where the IDE should navigate to. Often the
    /// `name_source_info` of the target element; for variables it's
    /// the parameter's source range.
    pub target: SourceInfo,
}

/// Index of all source references in a [`crate::model::PureModel`].
///
/// Built once after compile by [`build_reference_index`]. Two
/// access patterns:
///
/// - [`Self::find_at`] for cursor → target lookup (goto-def).
/// - [`Self::usages_of`] for target → cursor sites (find-usages).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReferenceIndex {
    /// References keyed by the canonical source path of the file
    /// they live in. Within each entry, references are sorted by
    /// (start_line, start_column) for binary-search-friendly lookup.
    pub by_file: HashMap<SmolStr, Vec<Reference>>,
    /// References keyed by their target element. Inverted view of
    /// `by_file`, populated lazily on first access.
    pub by_target: HashMap<ElementId, Vec<RefLocation>>,
}

/// A reverse-index entry: where a target is referenced from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefLocation {
    /// Canonical source path of the file containing the reference.
    pub canonical_path: SmolStr,
    /// Source range of the reference within that file.
    pub range: SourceInfo,
    /// Which kind of reference (helpful for filtering).
    pub kind: RefKind,
}

impl ReferenceIndex {
    /// Add a reference to both the per-file and per-target indexes.
    pub fn push(&mut self, reference: Reference) {
        let canonical = reference.range.source.clone();
        if let Some(target) = reference.target_element {
            self.by_target.entry(target).or_default().push(RefLocation {
                canonical_path: canonical.clone(),
                range: reference.range.clone(),
                kind: reference.kind,
            });
        }
        self.by_file.entry(canonical).or_default().push(reference);
    }

    /// Sort each per-file list by start position so [`Self::find_at`]
    /// can stop at the first non-matching range. Call this once after
    /// all references are pushed.
    pub fn finalize(&mut self) {
        for refs in self.by_file.values_mut() {
            refs.sort_by(|a, b| {
                (a.range.start_line, a.range.start_column)
                    .cmp(&(b.range.start_line, b.range.start_column))
            });
        }
    }

    /// Find the **smallest** reference whose source range contains
    /// the cursor. Multiple overlapping references can sit at one
    /// position (e.g. `foo()` is both a FunctionCall and a TypeRef
    /// in some contexts); the narrowest wins so cmd-hover targets
    /// the most specific node under the cursor.
    ///
    /// Coordinates are 1-indexed (matching [`SourceInfo`]).
    #[must_use]
    pub fn find_at(&self, canonical_path: &str, line: u32, column: u32) -> Option<&Reference> {
        let refs = self.by_file.get(canonical_path)?;
        let mut best: Option<&Reference> = None;
        for r in refs {
            if !contains(&r.range, line, column) {
                continue;
            }
            best = match best {
                None => Some(r),
                Some(prev) if area(&r.range) < area(&prev.range) => Some(r),
                _ => best,
            };
        }
        best
    }

    /// All references targeting the given element. `None` when the
    /// element has zero references in the model.
    #[must_use]
    pub fn usages_of(&self, target: ElementId) -> Option<&[RefLocation]> {
        self.by_target.get(&target).map(Vec::as_slice)
    }
}

fn contains(span: &SourceInfo, line: u32, column: u32) -> bool {
    // `end_column` is **inclusive** here — matches the lexer's
    // convention (`span_from` in `crates/lexer/src/lexer.rs`
    // subtracts 1 to land on the last char) and locate's
    // `Cursor::within`. Treating it as exclusive caused clicks on
    // the last column of any reference to silently miss.
    if line < span.start_line || line > span.end_line {
        return false;
    }
    if line == span.start_line && column < span.start_column {
        return false;
    }
    if line == span.end_line && column > span.end_column {
        return false;
    }
    true
}

fn area(span: &SourceInfo) -> u64 {
    let lines = u64::from(span.end_line.saturating_sub(span.start_line));
    let cols = u64::from(span.end_column.saturating_sub(span.start_column));
    lines.saturating_mul(1024).saturating_add(cols)
}

/// Walk a fully-built [`PureModel`] and emit one [`Reference`] per
/// source-level reference site. Single visitor pattern — every kind
/// of clickable region (stereotypes, tagged values, function calls
/// in expression bodies, package-element references, type references,
/// enum value references, …) flows through one call to `visit`.
///
/// Callers can:
/// - Build a [`ReferenceIndex`] (the most common case — see
///   [`build_reference_index`]).
/// - Implement bespoke collectors (find-references with filtering,
///   semantic-token producers, deprecated-symbol scans).
/// - Be DSL extensions that contribute their own references on top
///   of the core walk via [`crate::extension::CompilerExtension::walk_references`].
pub fn walk_references(model: &PureModel, visit: &mut dyn FnMut(Reference)) {
    for chunk in &model.chunks {
        for (idx, _node) in chunk.nodes.iter() {
            let element_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: idx,
            };
            let element = chunk.elements.get(idx);
            walk_element_references(model, element, element_id, visit);
        }
    }
}

/// Build a [`ReferenceIndex`] by walking the model.
///
/// Convenience wrapper over [`walk_references`] that pushes each
/// emitted reference into the index. Cheap (single linear walk over
/// chunks) so callers can rebuild after each compile.
#[must_use]
pub fn build_reference_index(model: &PureModel) -> ReferenceIndex {
    build_reference_index_with_extensions(model, &[])
}

/// Build a [`ReferenceIndex`] including DSL-extension contributions.
///
/// Walks the core model first (stereotypes, tagged values, function
/// calls, type refs, enum values), then asks each extension to
/// contribute its own reference sites — Mapping class refs, Relational
/// table refs, etc. The same lookup machinery
/// (`find_at` / `usages_of`) then serves both core and DSL refs
/// uniformly to the IDE.
#[must_use]
pub fn build_reference_index_with_extensions(
    model: &PureModel,
    extensions: &[&dyn crate::extension::CompilerExtension],
) -> ReferenceIndex {
    let mut index = ReferenceIndex::default();
    walk_references(model, &mut |r| index.push(r));
    for ext in extensions {
        ext.walk_references(model, &mut |r| index.push(r));
    }
    index.finalize();
    index
}

fn walk_element_references(
    model: &PureModel,
    element: &Element,
    _owner: ElementId,
    visit: &mut dyn FnMut(Reference),
) {
    match element {
        Element::Function(f) => {
            walk_stereotypes(model, &f.stereotypes, visit);
            walk_tagged_values(model, &f.tagged_values, visit);
            for p in f.parameters.iter() {
                walk_type_expr(model, &p.type_expr, visit);
            }
            walk_type_expr(model, &f.return_type, visit);
            // Body expressions get the function's parameters as the
            // outermost scope so `$paramName` references can resolve.
            let scope: [&[Parameter]; 1] = [f.parameters.as_ref()];
            for expr in f.body.iter() {
                walk_value_spec_in_scope(model, expr, &scope, visit);
            }
        }
        Element::Class(c) => {
            walk_stereotypes(model, &c.stereotypes, visit);
            walk_tagged_values(model, &c.tagged_values, visit);
            for st in &c.super_types {
                walk_type_expr(model, st, visit);
            }
            for prop in &c.properties {
                walk_stereotypes(model, &prop.stereotypes, visit);
                walk_tagged_values(model, &prop.tagged_values, visit);
                walk_type_expr(model, &prop.type_expr, visit);
                if let Some(dv) = &prop.default_value {
                    walk_value_spec_references(model, dv, visit);
                }
            }
            for qp in &c.qualified_properties {
                walk_stereotypes(model, &qp.stereotypes, visit);
                walk_tagged_values(model, &qp.tagged_values, visit);
                for p in qp.parameters.iter() {
                    walk_type_expr(model, &p.type_expr, visit);
                }
                walk_type_expr(model, &qp.return_type, visit);
                let scope: [&[Parameter]; 1] = [qp.parameters.as_ref()];
                for expr in qp.body.iter() {
                    walk_value_spec_in_scope(model, expr, &scope, visit);
                }
            }
            for con in &c.constraints {
                walk_value_spec_references(model, &con.function, visit);
                if let Some(m) = &con.message {
                    walk_value_spec_references(model, m, visit);
                }
            }
        }
        Element::Association(a) => {
            walk_stereotypes(model, &a.stereotypes, visit);
            walk_tagged_values(model, &a.tagged_values, visit);
            for prop in &a.properties {
                walk_stereotypes(model, &prop.stereotypes, visit);
                walk_tagged_values(model, &prop.tagged_values, visit);
                walk_type_expr(model, &prop.type_expr, visit);
            }
            for qp in &a.qualified_properties {
                walk_stereotypes(model, &qp.stereotypes, visit);
                walk_tagged_values(model, &qp.tagged_values, visit);
                for p in qp.parameters.iter() {
                    walk_type_expr(model, &p.type_expr, visit);
                }
                walk_type_expr(model, &qp.return_type, visit);
                let scope: [&[Parameter]; 1] = [qp.parameters.as_ref()];
                for expr in qp.body.iter() {
                    walk_value_spec_in_scope(model, expr, &scope, visit);
                }
            }
        }
        Element::Enumeration(e) => {
            walk_stereotypes(model, &e.stereotypes, visit);
            walk_tagged_values(model, &e.tagged_values, visit);
        }
        // Profile / Measure / Unit / PrimitiveType /
        // PackageableMultiplicity / Package: nothing to walk for
        // now. DSLInstance refs come via the DSL's
        // `CompilerExtension::walk_references` hook.
        _ => {}
    }
}

fn walk_stereotypes(
    model: &PureModel,
    stereotypes: &[StereotypeRef],
    visit: &mut dyn FnMut(Reference),
) {
    for st in stereotypes {
        let Some(range) = &st.source_info else {
            continue;
        };
        if matches!(st.profile, ElementId::Package(_)) {
            continue;
        }
        let Some(profile_element) = model.try_get_element(st.profile) else {
            continue;
        };
        // Prefer the matching stereotype declaration's source span as
        // the jump target. Falls back to the Profile name span when
        // unavailable (m3-bootstrap / snapshot profiles).
        let target = stereotype_decl_span(profile_element, &st.value)
            .unwrap_or_else(|| model.get_node(st.profile).name_source_info.clone());
        visit(Reference {
            range: range.clone(),
            kind: RefKind::StereotypeRef,
            target_element: Some(st.profile),
            target,
        });
    }
}

/// Look up a stereotype name on a Profile element and return the
/// source span of its declaration line. Returns `None` for the
/// bootstrap-source sentinel — m3-bootstrap profiles fall back to
/// the Profile's own name span at the call site.
fn stereotype_decl_span(profile_element: &Element, name: &SmolStr) -> Option<SourceInfo> {
    let Element::Profile(profile) = profile_element else {
        return None;
    };
    let decl = profile.stereotypes.iter().find(|n| &n.value == name)?;
    if decl.source_info.source.as_str() == crate::nodes::profile::BOOTSTRAP_SOURCE {
        return None;
    }
    Some(decl.source_info.clone())
}

/// Same as [`walk_stereotypes`] but for tagged values. Click on
/// `{Profile.tag = '…'}` jumps to the `tag` declaration line
/// inside the Profile body when the parser captured a span.
fn walk_tagged_values(
    model: &PureModel,
    tagged_values: &[TaggedValueRef],
    visit: &mut dyn FnMut(Reference),
) {
    for tv in tagged_values {
        let Some(range) = &tv.source_info else {
            continue;
        };
        if matches!(tv.profile, ElementId::Package(_)) {
            continue;
        }
        let Some(profile_element) = model.try_get_element(tv.profile) else {
            continue;
        };
        let target = tag_decl_span(profile_element, &tv.tag)
            .unwrap_or_else(|| model.get_node(tv.profile).name_source_info.clone());
        visit(Reference {
            range: range.clone(),
            kind: RefKind::TaggedValueRef,
            target_element: Some(tv.profile),
            target,
        });
    }
}

/// Convenience entry-point used by call sites that don't have a
/// scope stack handy (default-value expressions, constraints).
fn walk_value_spec_references(
    model: &PureModel,
    vs: &ValueSpec,
    visit: &mut dyn FnMut(Reference),
) {
    walk_value_spec_in_scope(model, vs, &[], visit);
}

/// Recursively walk a [`ValueSpec`] tree and emit one [`Reference`]
/// per resolved cross-element pointer. Single match arm per
/// [`ExprKind`] variant — adding a new "this expression points at
/// element X" rule = one push call here.
///
/// `scope` is a stack of parameter slices, innermost last. Each
/// nested [`ExprKind::Lambda`] pushes its parameters; each
/// [`ExprKind::Variable`] reads from the stack innermost-first to
/// find the binding parameter (function / QP / lambda). Let-binding
/// scope (the `letFunction("name", value)` desugar) is not yet
/// resolved by the index — those Variables fall through to the
/// locate-based handler.
fn walk_value_spec_in_scope(
    model: &PureModel,
    vs: &ValueSpec,
    scope: &[&[Parameter]],
    visit: &mut dyn FnMut(Reference),
) {
    match &*vs.kind {
        ExprKind::FunctionCall(d) => {
            if let Some(target_element) = d.function
                && let Some(target) = element_target_span(model, target_element)
            {
                let range = call_name_span(vs, d.arguments.first(), &d.function_name)
                    .unwrap_or_else(|| vs.source_info.clone());
                visit(Reference {
                    range,
                    kind: RefKind::FunctionCall,
                    target_element: Some(target_element),
                    target,
                });
            }
            for arg in &d.arguments {
                walk_value_spec_in_scope(model, arg, scope, visit);
            }
        }
        ExprKind::PropertyCall(d) => {
            // `$x.foo` — recover the property's declaring class from
            // the receiver's `type_info`, then look up the property
            // by name and emit a Reference targeting its source span.
            //
            // Enum-value access (`abc::Abc.A`) parses + lowers as a
            // PropertyCall too (the lowering doesn't have a special
            // case for static enum-value access; the receiver is a
            // `PackageableElementRef` to the enum). The property
            // lookup naturally returns None for enums, so fall back
            // to enum-value resolution: when the receiver is a
            // `PackageableElementRef` to an `Enumeration` and
            // `function_name` matches a declared value, emit a
            // Reference targeting that value's source span.
            if let Some(receiver) = d.arguments.first() {
                let prop_target =
                    property_decl_span(model, receiver, &d.function_name, /*qualified*/ false)
                        .or_else(|| enum_value_decl_span(model, receiver, &d.function_name));
                if let Some(target) = prop_target {
                    // `vs.source_info` is the member identifier's span
                    // (set in `lower_member_access`). That's the
                    // navigable region — clicks elsewhere on the
                    // receiver land on the receiver's own ref via the
                    // argument recursion below.
                    visit(Reference {
                        range: vs.source_info.clone(),
                        kind: RefKind::PropertyCall,
                        target_element: None,
                        target,
                    });
                }
            }
            for arg in &d.arguments {
                walk_value_spec_in_scope(model, arg, scope, visit);
            }
        }
        ExprKind::QualifiedPropertyCall(d) => {
            if let Some(receiver) = d.arguments.first() {
                if let Some(qp_target) =
                    property_decl_span(model, receiver, &d.function_name, /*qualified*/ true)
                {
                    visit(Reference {
                        range: vs.source_info.clone(),
                        kind: RefKind::QualifiedPropertyCall,
                        target_element: None,
                        target: qp_target,
                    });
                }
            }
            for arg in &d.arguments {
                walk_value_spec_in_scope(model, arg, scope, visit);
            }
        }
        ExprKind::Variable { name } => {
            // Search the scope stack innermost-first.
            for params in scope.iter().rev() {
                if let Some(p) = params.iter().find(|p| &p.name == name) {
                    visit(Reference {
                        range: vs.source_info.clone(),
                        kind: RefKind::Variable,
                        target_element: None,
                        target: p.source_info.clone(),
                    });
                    return;
                }
            }
            // Unbound variable — likely a let-binding the index
            // doesn't yet resolve. Locate-based fallback may pick it
            // up, but emit nothing here.
        }
        ExprKind::PackageableElementRef { element } => {
            if let Some(target) = element_target_span(model, *element) {
                visit(Reference {
                    range: vs.source_info.clone(),
                    kind: RefKind::TypeRef,
                    target_element: Some(*element),
                    target,
                });
            }
        }
        ExprKind::EnumValue { enum_element, .. } => {
            if let Some(target) = element_target_span(model, *enum_element) {
                visit(Reference {
                    range: vs.source_info.clone(),
                    kind: RefKind::EnumValue,
                    target_element: Some(*enum_element),
                    target,
                });
            }
        }
        ExprKind::TypeReference { type_expr } => {
            if let TypeExpr::Named { element, .. } = type_expr
                && let Some(target) = element_target_span(model, *element)
            {
                visit(Reference {
                    range: vs.source_info.clone(),
                    kind: RefKind::TypeRef,
                    target_element: Some(*element),
                    target,
                });
            }
        }
        ExprKind::Lambda { parameters, body } => {
            // Push the lambda's parameters onto the scope stack for
            // the body walk; pop on return.
            let mut nested: Vec<&[Parameter]> = scope.iter().copied().collect();
            nested.push(parameters);
            for expr in body {
                walk_value_spec_in_scope(model, expr, &nested, visit);
            }
        }
        ExprKind::Collection { elements } => {
            for el in elements {
                walk_value_spec_in_scope(model, el, scope, visit);
            }
        }
        _ => {}
    }
}

/// Given a target element, return its `name_source_info` span (the
/// IDE jumps to that). Returns `None` for Package targets and for
/// IDs the model doesn't know about.
fn element_target_span(model: &PureModel, target: ElementId) -> Option<SourceInfo> {
    if matches!(target, ElementId::Package(_)) {
        return None;
    }
    if model.try_get_element(target).is_none() {
        return None;
    }
    Some(model.get_node(target).name_source_info.clone())
}

/// Compute the source range of the function name within a
/// `FunctionCall` expression. Mirrors the narrowing logic in the
/// LSP layer's `narrow_value_spec_span`: handles both direct
/// (`pkg::fn(args)` — name at start) and arrow (`x->fn(args)` —
/// name after target + `->`) forms. Returns `None` when the
/// heuristic can't safely narrow (multi-line, etc.); caller falls
/// back to the full ValueSpec span.
fn call_name_span(
    vs: &ValueSpec,
    first_arg: Option<&ValueSpec>,
    name: &SmolStr,
) -> Option<SourceInfo> {
    let outer = &vs.source_info;
    if let Some(arg0) = first_arg
        && arg0.source_info.start_line == outer.start_line
        && arg0.source_info.start_column == outer.start_column
    {
        // Arrow form: name lives just past `arg0.end + ->`.
        if arg0.source_info.end_line != outer.end_line {
            return None;
        }
        let name_start_col = arg0.source_info.end_column.checked_add(2)?;
        let name_len: u32 = name.len().try_into().ok()?;
        let name_end_col = name_start_col.checked_add(name_len)?;
        Some(SourceInfo::new(
            outer.source.as_str(),
            arg0.source_info.end_line,
            name_start_col,
            arg0.source_info.end_line,
            name_end_col,
        ))
    } else {
        // Direct form: name at the expression's leading position.
        if outer.start_line != outer.end_line {
            return None;
        }
        let name_len: u32 = name.len().try_into().ok()?;
        let end_col = outer.start_column.checked_add(name_len)?;
        Some(SourceInfo::new(
            outer.source.as_str(),
            outer.start_line,
            outer.start_column,
            outer.start_line,
            end_col,
        ))
    }
}

/// Look up a tag name on a Profile element and return the source
/// span of its declaration line. Same bootstrap-sentinel guard as
/// [`stereotype_decl_span`].
fn tag_decl_span(profile_element: &Element, name: &SmolStr) -> Option<SourceInfo> {
    let Element::Profile(profile) = profile_element else {
        return None;
    };
    let decl = profile.tags.iter().find(|n| &n.value == name)?;
    if decl.source_info.source.as_str() == crate::nodes::profile::BOOTSTRAP_SOURCE {
        return None;
    }
    Some(decl.source_info.clone())
}

/// Resolve a `PropertyCall` / `QualifiedPropertyCall` to its
/// declaring class member's source span via the receiver's
/// `type_info`.
///
/// `qualified` selects which list to search (`properties` for
/// PropertyCall, `qualified_properties` for QPCall). Falls back to
/// the OTHER list if the primary lookup misses, since the parser
/// can't always tell the kinds apart statically and the resolver
/// picks the right one.
///
/// Walks the supertype chain too — `$person.address` lands on the
/// `address` declaration even when it's inherited.
fn property_decl_span(
    model: &PureModel,
    receiver: &ValueSpec,
    name: &smol_str::SmolStr,
    qualified: bool,
) -> Option<SourceInfo> {
    let receiver_ty = receiver.type_info.as_deref()?;
    let TypeExpr::Named { element, .. } = &receiver_ty.type_expr else {
        return None;
    };
    let mut visited = std::collections::HashSet::new();
    walk_class_chain(model, *element, &mut visited, &mut |class| {
        if !qualified
            && let Some(p) = class.properties.iter().find(|p| &p.name == name)
        {
            return Some(p.source_info.clone());
        }
        if let Some(qp) = class
            .qualified_properties
            .iter()
            .find(|q| &q.name == name)
        {
            return Some(qp.source_info.clone());
        }
        // Fallback: try the OTHER list — the parser sometimes
        // produces PropertyCall when the resolved member is a QP and
        // vice versa, depending on disambiguation order.
        if qualified
            && let Some(p) = class.properties.iter().find(|p| &p.name == name)
        {
            return Some(p.source_info.clone());
        }
        None
    })
}

/// Enum-value navigation fallback for `Enum.Value` expressions.
///
/// The lowering pipeline turns `abc::Abc.A` into
/// `PropertyCall(function_name="A", arguments=[PackageableElementRef("abc::Abc")])`
/// — there's no distinct `ExprKind::EnumValue` produced in
/// non-test code. `property_decl_span` naturally returns `None`
/// for this shape because `Abc` is an Enumeration (no
/// properties). This fallback recovers the navigation: if the
/// receiver is a `PackageableElementRef` whose target is an
/// `Enumeration` AND `name` matches one of its declared values,
/// return that value's source span. Caller emits the Reference.
fn enum_value_decl_span(
    model: &PureModel,
    receiver: &ValueSpec,
    name: &smol_str::SmolStr,
) -> Option<SourceInfo> {
    // Static `Enum.Value` form: the receiver is a bare element
    // reference (lowered as `PackageableElementRef`). Dynamic enum
    // navigation (`$some_enum_var.A`) isn't covered here — that'd
    // need the receiver's `type_info`, which the user-facing flow
    // doesn't exercise today.
    let crate::types::ExprKind::PackageableElementRef { element } = &*receiver.kind else {
        return None;
    };
    let Element::Enumeration(enum_def) = model.try_get_element(*element)? else {
        return None;
    };
    enum_def
        .values
        .iter()
        .find(|v| &v.name == name)
        .map(|v| v.source_info.clone())
}

/// Walk the receiver class + every transitive super-class, calling
/// `visit` on each. Returns the first `Some` result. Used by
/// property navigation to find inherited members.
fn walk_class_chain<R>(
    model: &PureModel,
    start: ElementId,
    visited: &mut std::collections::HashSet<ElementId>,
    visit: &mut dyn FnMut(&Class) -> Option<R>,
) -> Option<R> {
    if !visited.insert(start) {
        return None;
    }
    let Element::Class(class) = model.try_get_element(start)? else {
        return None;
    };
    if let Some(hit) = visit(class) {
        return Some(hit);
    }
    for st in &class.super_types {
        if let TypeExpr::Named { element, .. } = st
            && let Some(hit) = walk_class_chain(model, *element, visited, visit)
        {
            return Some(hit);
        }
    }
    None
}

/// Recursively walk a [`TypeExpr`] tree and emit one [`Reference`]
/// per `Named` variant whose source span the resolver populated.
/// Recurses into generic arguments so clicks on inner types
/// (`String` in `List<String>`) refine to the inner span via the
/// index's smallest-containing-range lookup.
fn walk_type_expr(model: &PureModel, ty: &TypeExpr, visit: &mut dyn FnMut(Reference)) {
    match ty {
        TypeExpr::Named {
            element,
            source_info,
            type_arguments,
            ..
        } => {
            if let Some(range) = source_info
                && let Some(target) = element_target_span(model, *element)
            {
                visit(Reference {
                    range: range.clone(),
                    kind: RefKind::TypeRef,
                    target_element: Some(*element),
                    target,
                });
            }
            for arg in type_arguments {
                walk_type_expr(model, arg, visit);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (pty, _) in parameters {
                walk_type_expr(model, pty, visit);
            }
            walk_type_expr(model, return_type, visit);
        }
        TypeExpr::AlgebraUnion(a, b) => {
            walk_type_expr(model, a, visit);
            walk_type_expr(model, b, visit);
        }
        TypeExpr::Relation(_) | TypeExpr::Generic(_) | TypeExpr::Unresolved => {}
    }
}
