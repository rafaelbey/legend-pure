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

//! [`CompilerExtension`] for graph-fetch islands (`#{ Class { … } }#`).
//!
//! Walks every function body in `define_bodies`, downcasts each
//! [`crate::ast::RootGraphFetchTree`] island, resolves its root class
//! against the [`PureModel`], and validates each property name against
//! the resolved class. Sub-trees recurse with the property's target
//! class; sub-type trees recurse with the cast class.
//!
//! Validation only — like dsl-tds, the core lowering pass still
//! reports `UnsupportedExpression { kind: "Island" }` for the
//! expression value itself. This extension surfaces graph-fetch-
//! specific diagnostics (unresolved class, unknown property) instead
//! of the generic "Island expression lowering not yet implemented"
//! message, matching Java's `RootGraphFetchTreeProcessor` +
//! `RootGraphFetchTreeValidator` behaviour.

use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::expression::{Expression, MemberAccess};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DefineCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element as ModelElement, PureModel};
use smol_str::SmolStr;

use crate::ast::{PropertyGraphFetchTree, RootGraphFetchTree, SubTypeGraphFetchTree};

/// Compiler extension for the graph-fetch inline-island DSL.
#[derive(Default)]
pub struct GraphFetchExtension;

impl GraphFetchExtension {
    /// Construct a fresh extension.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CompilerExtension for GraphFetchExtension {
    fn name(&self) -> &'static str {
        "dsl-graph"
    }

    fn define_bodies(&self, ctx: &mut DefineCtx<'_>) {
        for source_file in ctx.source_files {
            walk_source_file(source_file, ctx.model, ctx.errors);
        }
    }
}

// ---------------------------------------------------------------------------
// FQN resolution
// ---------------------------------------------------------------------------

fn ptr_segments(ptr: &PackageableElementPtr) -> Vec<SmolStr> {
    let mut segs: Vec<SmolStr> = ptr
        .package
        .as_ref()
        .map(|pkg| pkg.segments().iter().map(|i| (*i).clone()).collect())
        .unwrap_or_default();
    segs.push(ptr.name.clone());
    segs
}

fn ptr_display(ptr: &PackageableElementPtr) -> String {
    ptr.to_string()
}

fn resolve_class(model: &PureModel, ptr: &PackageableElementPtr) -> Option<ElementId> {
    let segs = ptr_segments(ptr);
    let id = model.resolve_by_path(&segs)?;
    matches!(model.get_element(id), ModelElement::Class(_)).then_some(id)
}

// ---------------------------------------------------------------------------
// Property lookup (declared + qualified, walking supertypes)
// ---------------------------------------------------------------------------

/// Whether `class_id` (or any of its transitive supertypes) declares
/// a property named `prop_name` — either a regular `Property` or a
/// `QualifiedProperty`. Mirrors Java's
/// `_Class.allProperties()` / `allQualifiedProperties()`.
fn class_has_property(model: &PureModel, class_id: ElementId, prop_name: &str) -> bool {
    use std::collections::HashSet;
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut stack: Vec<ElementId> = vec![class_id];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let ModelElement::Class(c) = model.get_element(id) {
            if c.properties.iter().any(|p| p.name == prop_name) {
                return true;
            }
            if c.qualified_properties.iter().any(|q| q.name == prop_name) {
                return true;
            }
            for st in &c.super_types {
                if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                    stack.push(*element);
                }
            }
        }
    }
    false
}

/// Resolve `prop_name` on `class_id` (walking supertypes) and return
/// the property's declared raw type as an `ElementId` if it's a Class.
/// Returns `None` for unknown properties or properties whose type isn't
/// a resolvable Class (e.g. primitives, generic-parameter `T`, etc.) —
/// in that case sub-tree validation against the property is skipped.
fn class_property_type_id(
    model: &PureModel,
    class_id: ElementId,
    prop_name: &str,
) -> Option<ElementId> {
    use std::collections::HashSet;
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut stack: Vec<ElementId> = vec![class_id];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let ModelElement::Class(c) = model.get_element(id) {
            if let Some(p) = c.properties.iter().find(|p| p.name == prop_name) {
                if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } =
                    &p.type_expr
                    && matches!(model.get_element(*element), ModelElement::Class(_))
                {
                    return Some(*element);
                }
                return None;
            }
            if let Some(q) = c.qualified_properties.iter().find(|q| q.name == prop_name) {
                if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } =
                    &q.return_type
                    && matches!(model.get_element(*element), ModelElement::Class(_))
                {
                    return Some(*element);
                }
                return None;
            }
            for st in &c.super_types {
                if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                    stack.push(*element);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Source-file traversal
// ---------------------------------------------------------------------------

fn walk_source_file(sf: &SourceFile, model: &PureModel, errors: &mut Vec<CompilationError>) {
    for elem in sf.all_elements() {
        if let AstElement::Function(f) = elem {
            for expr in &f.body {
                walk_expression(expr, model, errors);
            }
        }
    }
}

fn walk_expression(expr: &Expression, model: &PureModel, errors: &mut Vec<CompilationError>) {
    match expr {
        Expression::Island(isl) => validate_island(isl, model, errors),
        Expression::FunctionApplication(app) => {
            for arg in &app.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::ArrowFunction(arrow) => {
            walk_expression(&arrow.target, model, errors);
            for arg in &arrow.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::MemberAccess(MemberAccess::Simple(m)) => {
            walk_expression(&m.target, model, errors);
        }
        Expression::MemberAccess(MemberAccess::Qualified(m)) => {
            walk_expression(&m.target, model, errors);
            for arg in &m.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::Lambda(lam) => {
            for e in &lam.body {
                walk_expression(e, model, errors);
            }
        }
        Expression::Let(l) => walk_expression(&l.value, model, errors),
        Expression::Collection(c) => {
            for e in &c.elements {
                walk_expression(e, model, errors);
            }
        }
        Expression::Slice(s) => {
            if let Some(start) = &s.start {
                walk_expression(start, model, errors);
            }
            walk_expression(&s.stop, model, errors);
            if let Some(step) = &s.step {
                walk_expression(step, model, errors);
            }
        }
        Expression::Group(inner) => walk_expression(inner, model, errors),
        Expression::Arithmetic(a) => {
            walk_expression(&a.left, model, errors);
            walk_expression(&a.right, model, errors);
        }
        Expression::Comparison(c) => {
            walk_expression(&c.left, model, errors);
            walk_expression(&c.right, model, errors);
        }
        Expression::Logical(l) => {
            walk_expression(&l.left, model, errors);
            walk_expression(&l.right, model, errors);
        }
        Expression::Bitwise(b) => {
            walk_expression(&b.left, model, errors);
            walk_expression(&b.right, model, errors);
        }
        Expression::Not(n) => walk_expression(&n.operand, model, errors),
        Expression::UnaryMinus(u) => walk_expression(&u.operand, model, errors),
        Expression::BitwiseNot(b) => walk_expression(&b.operand, model, errors),
        Expression::Copy(c) => {
            for kv in &c.assignments {
                walk_expression(&kv.value, model, errors);
            }
        }
        Expression::NewInstance(n) => {
            for kv in &n.assignments {
                walk_expression(&kv.value, model, errors);
            }
        }
        Expression::Column(_)
        | Expression::PackageableElementRef(_)
        | Expression::TypeReferenceExpr(_)
        | Expression::UnitInstance(_)
        | Expression::Variable(_)
        | Expression::Literal(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Graph-fetch validation
// ---------------------------------------------------------------------------

fn validate_island(isl: &IslandExpression, model: &PureModel, errors: &mut Vec<CompilationError>) {
    let Some(root) = isl.content.as_any().downcast_ref::<RootGraphFetchTree>() else {
        return;
    };
    let Some(class_id) = resolve_class(model, &root.class) else {
        errors.push(CompilationError {
            message: format!(
                "Graph-fetch root class '{}' does not resolve to a Class",
                ptr_display(&root.class)
            ),
            source_info: root.class.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: SmolStr::new(ptr_display(&root.class)),
            },
        });
        return;
    };
    for sub in &root.sub_trees {
        validate_property_tree(sub, class_id, model, errors);
    }
    for sub_type in &root.sub_type_trees {
        validate_sub_type_tree(sub_type, class_id, model, errors);
    }
}

fn validate_property_tree(
    tree: &PropertyGraphFetchTree,
    parent_class: ElementId,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    let prop_name = tree.property.as_str();
    if !class_has_property(model, parent_class, prop_name) {
        let class_name = model.element_name(parent_class).clone();
        errors.push(CompilationError {
            message: format!(
                "Graph-fetch property '{prop_name}' not found on class '{class_name}'"
            ),
            source_info: tree.source_info.clone(),
            kind: CompilationErrorKind::UnknownProperty {
                type_name: class_name,
                property_name: SmolStr::new(prop_name),
            },
        });
        return;
    }
    // Recurse into nested sub-trees with the property's target class
    // (best-effort — primitive-typed properties skip recursion).
    if (!tree.sub_trees.is_empty() || !tree.sub_type_trees.is_empty())
        && let Some(target_class) = class_property_type_id(model, parent_class, prop_name)
    {
        for sub in &tree.sub_trees {
            validate_property_tree(sub, target_class, model, errors);
        }
        for sub_type in &tree.sub_type_trees {
            validate_sub_type_tree(sub_type, target_class, model, errors);
        }
    }
}

fn validate_sub_type_tree(
    tree: &SubTypeGraphFetchTree,
    _parent_class: ElementId,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    let Some(sub_class_id) = resolve_class(model, &tree.sub_type_class) else {
        errors.push(CompilationError {
            message: format!(
                "Graph-fetch subtype class '{}' does not resolve to a Class",
                ptr_display(&tree.sub_type_class)
            ),
            source_info: tree.sub_type_class.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: SmolStr::new(ptr_display(&tree.sub_type_class)),
            },
        });
        return;
    };
    for sub in &tree.sub_trees {
        validate_property_tree(sub, sub_class_id, model, errors);
    }
    for nested in &tree.sub_type_trees {
        validate_sub_type_tree(nested, sub_class_id, model, errors);
    }
}
