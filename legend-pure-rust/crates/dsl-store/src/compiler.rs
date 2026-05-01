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

//! [`CompilerExtension`] implementation for the relation-store DSL.
//!
//! `RelationStoreAccessorRef` is an *expression-level* island
//! (unlike Diagram, which contributes whole `Element::DSLElement`
//! entries). The extension therefore walks function bodies' AST in
//! Pass 2b (`define_bodies`), discovers each `#>{ … }#` island, and
//! validates that:
//!
//! 1. `path[0]` resolves to a [`PureModel`] element, and
//! 2. that element extends `meta::pure::store::Store`.
//!
//! Lowering of the island into a real [`PureModel`] value is
//! deferred — the core lowering pass still reports
//! `UnsupportedExpression { kind: "Island" }` for every island. A
//! future commit can add a real value-spec lowering once dsl-store
//! grows its compiled representation.

use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::expression::{Expression, MemberAccess};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DefineCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element as ModelElement, PureModel};
use legend_pure_parser_pure::types::TypeExpr;
use smol_str::SmolStr;

use crate::ast::RelationStoreAccessorRef;

/// Compiler extension for the relation-store DSL.
///
/// One instance per
/// [`compile_with_extensions`](legend_pure_parser_pure::pipeline::compile_with_extensions)
/// invocation.
#[derive(Default)]
pub struct RelationStoreExtension;

impl RelationStoreExtension {
    /// Construct a fresh extension.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CompilerExtension for RelationStoreExtension {
    fn name(&self) -> &'static str {
        "dsl-store"
    }

    fn define_bodies(&self, ctx: &mut DefineCtx<'_>) {
        // If the Store metaclass isn't present in this compilation
        // unit, there's nothing to validate against — skip silently.
        // The end-to-end test ensures the metaclass is loaded; ad-hoc
        // compilations without dsl-store sources skip the check.
        let Some(store_id) = resolve_store_class(ctx.model) else {
            return;
        };

        for source_file in ctx.source_files {
            walk_source_file(source_file, store_id, ctx.model, ctx.errors);
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

fn resolve_store_class(model: &PureModel) -> Option<ElementId> {
    model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("store"),
        SmolStr::new("Store"),
    ])
}

fn resolve_fqn(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    model.resolve_by_path(&segments)
}

fn extends_class(model: &PureModel, candidate: ElementId, target: ElementId) -> bool {
    if candidate == target {
        return true;
    }
    let ModelElement::Class(class) = model.get_element(candidate) else {
        return false;
    };
    for super_ty in &class.super_types {
        if let TypeExpr::Named { element, .. } = super_ty
            && extends_class(model, *element, target)
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Source-file traversal
// ---------------------------------------------------------------------------

fn walk_source_file(
    sf: &SourceFile,
    store_id: ElementId,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    for elem in sf.all_elements() {
        if let AstElement::Function(f) = elem {
            for expr in &f.body {
                walk_expression(expr, store_id, model, errors);
            }
        }
    }
}

fn walk_expression(
    expr: &Expression,
    store_id: ElementId,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    match expr {
        Expression::Island(isl) => {
            validate_island(isl, store_id, model, errors);
            // An island's content can't itself contain Pure expressions
            // through this enum, so no further recursion needed here.
        }
        Expression::FunctionApplication(app) => {
            for arg in &app.arguments {
                walk_expression(arg, store_id, model, errors);
            }
        }
        Expression::ArrowFunction(arrow) => {
            walk_expression(&arrow.target, store_id, model, errors);
            for arg in &arrow.arguments {
                walk_expression(arg, store_id, model, errors);
            }
        }
        Expression::MemberAccess(MemberAccess::Simple(m)) => {
            walk_expression(&m.target, store_id, model, errors);
        }
        Expression::MemberAccess(MemberAccess::Qualified(m)) => {
            walk_expression(&m.target, store_id, model, errors);
            for arg in &m.arguments {
                walk_expression(arg, store_id, model, errors);
            }
        }
        Expression::Lambda(lam) => {
            for e in &lam.body {
                walk_expression(e, store_id, model, errors);
            }
        }
        Expression::Let(l) => {
            walk_expression(&l.value, store_id, model, errors);
        }
        Expression::Collection(c) => {
            for e in &c.elements {
                walk_expression(e, store_id, model, errors);
            }
        }
        Expression::Slice(s) => {
            if let Some(start) = &s.start {
                walk_expression(start, store_id, model, errors);
            }
            walk_expression(&s.stop, store_id, model, errors);
            if let Some(step) = &s.step {
                walk_expression(step, store_id, model, errors);
            }
        }
        Expression::Group(inner) => walk_expression(inner, store_id, model, errors),
        Expression::Arithmetic(a) => {
            walk_expression(&a.left, store_id, model, errors);
            walk_expression(&a.right, store_id, model, errors);
        }
        Expression::Comparison(c) => {
            walk_expression(&c.left, store_id, model, errors);
            walk_expression(&c.right, store_id, model, errors);
        }
        Expression::Logical(l) => {
            walk_expression(&l.left, store_id, model, errors);
            walk_expression(&l.right, store_id, model, errors);
        }
        Expression::Bitwise(b) => {
            walk_expression(&b.left, store_id, model, errors);
            walk_expression(&b.right, store_id, model, errors);
        }
        Expression::Not(n) => walk_expression(&n.operand, store_id, model, errors),
        Expression::UnaryMinus(u) => walk_expression(&u.operand, store_id, model, errors),
        Expression::BitwiseNot(b) => walk_expression(&b.operand, store_id, model, errors),
        Expression::Copy(c) => {
            for kv in &c.assignments {
                walk_expression(&kv.value, store_id, model, errors);
            }
        }
        Expression::NewInstance(_)
        | Expression::Column(_)
        | Expression::PackageableElementRef(_)
        | Expression::TypeReferenceExpr(_)
        | Expression::UnitInstance(_)
        | Expression::Variable(_)
        | Expression::Literal(_) => {}
    }
}

fn validate_island(
    isl: &IslandExpression,
    store_id: ElementId,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    let Some(accessor) = isl
        .content
        .as_any()
        .downcast_ref::<RelationStoreAccessorRef>()
    else {
        // Some other DSL's island; not our problem.
        return;
    };

    let Some(first) = accessor.path.first() else {
        // Parser already rejects this case, but guard defensively.
        return;
    };

    let Some(resolved) = resolve_fqn(model, first.as_str()) else {
        errors.push(CompilationError {
            message: format!("RelationStoreAccessor references unresolved store '{first}'"),
            source_info: accessor.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: first.clone(),
            },
        });
        return;
    };

    if !extends_class(model, resolved, store_id) {
        errors.push(CompilationError {
            message: format!(
                "RelationStoreAccessor path[0] '{first}' is not a subclass of \
                 meta::pure::store::Store"
            ),
            source_info: accessor.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: first.clone(),
            },
        });
    }
}
