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

//! Type-reference and packageable-element-reference lowering:
//! `@MyType` → `TypeReference`, `MyClass` / `my::Enum` →
//! `PackageableElementRef`.
//!
//! Step 4 of the lowering encapsulation plan; sibling slice to the
//! other `lower/<kind>.rs` files. Includes the `@(cols)` relation-type
//! literal path that produces a `RelationLiteral` (rather than a
//! `TypeReference`), and the centralised
//! [`build_packageable_element_ref`] helper that pre-sets `type_info`
//! to the element's parametric metatype shape — Java analog
//! `InstanceValueProcessor.getGenericType` Class-instance branch.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_ast::type_ref as ast_type;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, Multiplicity, ResolvedType, TypeExpr, ValueSpec};

use super::relation::{lower_relation_columns, resolve_relation_type_id};
use super::{typed, untyped};

/// Lowers `@MyType` → `TypeReference`.
///
/// Special case: `@(name:Type[mult], …)` (relation type at expression
/// position) lowers to `ExprKind::RelationLiteral` instead, with the
/// column metadata captured at lowering time. The runtime allocator
/// materialises a `RelationType` heap object whose `columns` slot
/// carries the lowered specs. The lowered `ValueSpec` carries
/// `type_info = RelationType<Any>[1]` so dispatch + inference see the
/// same shape `resolve_type_spec(TypeSpec::Relation)` reports.
pub(super) fn lower_type_reference(
    e: &ast_expr::TypeReferenceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    if let ast_type::TypeSpec::Relation(rt) = &e.type_ref {
        let columns = lower_relation_columns(&rt.columns, ctx, errors);
        let relation_type_id = resolve_relation_type_id(ctx)?;
        return Some(typed(
            ExprKind::RelationLiteral { columns },
            e.source_info.clone(),
            ResolvedType {
                type_expr: TypeExpr::Named {
                    element: relation_type_id,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                },
                multiplicity: Multiplicity::PureOne,
            },
        ));
    }
    let type_expr = resolve::resolve_type_spec(&e.type_ref, ctx, errors)?;
    Some(untyped(
        ExprKind::TypeReference { type_expr },
        e.source_info.clone(),
    ))
}

/// Lowers a bare element reference: `String`, `my::Enum` → `PackageableElementRef`.
///
/// Pre-sets `type_info` via [`build_packageable_element_ref`]. The AST
/// node `ExprKind::PackageableElementRef { element }` stays a pure
/// name-on-graph reference — type capture lives on the `ValueSpec`'s
/// `type_info` slot per `reference_type_info_capture.md`.
pub(super) fn lower_packageable_element_ref(
    e: &ast_expr::PackageableElementRef,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let element_id = resolve::resolve_element_ptr(&e.element, &e.source_info, ctx, errors)?;
    Some(build_packageable_element_ref(
        element_id,
        e.source_info.clone(),
        ctx.model,
    ))
}

/// Build a `PackageableElementRef` `ValueSpec` whose `type_info` carries
/// the element's parametric metatype shape — e.g. a class element `P`
/// produces `type_info = Class<P>[1]`. Mirrors Java's
/// `InstanceValueProcessor.getGenericType` Class-instance branch
/// (`InstanceValueProcessor.java:154-183`) which wraps the element as
/// the metatype's type-argument.
///
/// Centralising this construction keeps every `PackageableElementRef`
/// call site (lowering, `lower_new_instance`'s class arg + type-arg
/// specs) producing the same shape so generic substitution against
/// `new<T>(class:Class<T>[1], …)` always sees `T` bound to the actual
/// element rather than the bare metatype.
pub(crate) fn build_packageable_element_ref(
    element_id: crate::ids::ElementId,
    source_info: SourceInfo,
    model: &crate::model::PureModel,
) -> ValueSpec {
    let element = model.get_element(element_id);
    let type_info = crate::bootstrap::metatype_of(model, element).map(|metatype| {
        // For Function elements, lift the function's signature into a
        // structural `FunctionType` so callers like
        // `eval<T,V|m,n>(func:Function<{T[n]->V[m]}>, param:T[n]):V[m]`
        // can extract T/V/m/n from the function-ref's expected
        // FunctionType slot. Without this, the `type_arguments` carries
        // only a bare `Named<element_id>{}` and the eval/apply binding
        // pass falls through to no-op.
        //
        // Mirror of Java's `InstanceValueProcessor.getGenericType` which
        // produces `Function<{param_types -> return_type}>` when the
        // referent is a function definition.
        let inner_te = match element {
            crate::model::Element::Function(f) => {
                let parameters: Vec<(TypeExpr, Multiplicity)> = f
                    .parameters
                    .iter()
                    .map(|p| (p.type_expr.clone(), p.multiplicity.clone()))
                    .collect();
                TypeExpr::FunctionType {
                    parameters,
                    return_type: Box::new(f.return_type.clone()),
                    return_multiplicity: f.return_multiplicity.clone(),
                }
            }
            _ => TypeExpr::Named {
                element: element_id,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
            },
        };
        Box::new(ResolvedType {
            type_expr: TypeExpr::Named {
                element: metatype,
                type_arguments: vec![inner_te],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
            },
            multiplicity: Multiplicity::PureOne,
        })
    });
    ValueSpec {
        kind: Box::new(ExprKind::PackageableElementRef {
            element: element_id,
        }),
        source_info,
        type_info,
    }
}
