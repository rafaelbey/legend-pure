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

//! Unit-instance and navigation-path lowering: `5 RomanLength~Pes` →
//! `newUnit(unit, value)`, and `#/StartType/p1(args)/p2!alias#` →
//! `ExprKind::PathLiteral`.
//!
//! Step 4 of the lowering encapsulation plan. Both targets are short
//! and orthogonal to the other AST kinds; they cohabit because each
//! is too small to merit its own file but both are syntactically
//! distinctive enough to deserve separation from the dispatch
//! catch-all in `lower/mod.rs`.

use legend_pure_parser_ast::expression as ast_expr;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::{lower_expression, type_ref, untyped};

/// Lowers a unit instance expression (`5 RomanLength~Pes`) to a `newUnit` call.
///
/// In the Pure semantic model, `5 RomanLength~Pes` desugars to
/// `newUnit(RomanLength~Pes, 5)`.
pub(super) fn lower_unit_instance(
    e: &ast_expr::UnitInstanceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let value_vs = lower_expression(&e.value, ctx, errors)?;
    let unit_ref = type_ref::lower_packageable_element_ref(
        &ast_expr::PackageableElementRef {
            element: e.unit.clone(),
            source_info: e.source_info.clone(),
        },
        ctx,
        errors,
    )?;

    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new_static("newUnit"),
            arguments: vec![unit_ref, value_vs],
        }),
        e.source_info.clone(),
    ))
}

/// Lowers `#/StartType/p1(args)/p2!alias#` → `ExprKind::PathLiteral`.
///
/// Strategy:
///  1. Resolve `start_type` via the standard type-spec resolver so type
///     args (`Firm<Any>`) are bound consistently with the rest of the
///     pipeline.
///  2. Lower each step's parameters as ordinary expressions (so enum
///     stubs, scalar literals, and collection literals all flow through
///     the existing lowering machinery).
///  3. *Validate the first step's property exists on the start type.*
///     This is a cheap correctness gate that catches most typos.
///     Subsequent steps' properties resolve at runtime — the chain
///     "running type" depends on each previous step's return type with
///     type-arg substitution, which is straightforward at evaluation
///     time but expensive to fully model statically. The Stage-4
///     `evaluate(Path,U)` native re-resolves through the chain.
///
/// On a property-not-found error, lowering still returns the
/// `PathLiteral` so downstream passes can keep going — the recorded
/// error is enough to fail compilation cleanly without cascading.
pub(super) fn lower_navigation_path(
    e: &ast_expr::NavigationPath,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    use crate::types::{PathStepLowered, TypeExpr};

    let start_type = resolve::resolve_type_ref(&e.start_type, ctx, errors)?;

    // Validate the first step's property exists on the start type.
    // (Subsequent steps validated at runtime — see fn doc.)
    if let Some(first_step) = e.path.first()
        && let TypeExpr::Named { element, .. } = &start_type
        && resolve::find_property_with_inheritance(*element, &first_step.property, ctx.model)
            .is_none()
    {
        errors.push(CompilationError {
            message: format!(
                "Navigation path: property '{}' not found on type '{}'",
                first_step.property,
                e.start_type.full_path(),
            ),
            source_info: first_step.source_info.clone(),
            kind: crate::error::CompilationErrorKind::UnknownProperty {
                type_name: SmolStr::new(e.start_type.full_path()),
                property_name: first_step.property.clone(),
            },
        });
    }

    let steps: Vec<PathStepLowered> = e
        .path
        .iter()
        .map(|step| {
            let parameters: Vec<ValueSpec> = step
                .parameters
                .iter()
                .filter_map(|p| lower_expression(p, ctx, errors))
                .collect();
            PathStepLowered {
                property_name: step.property.clone(),
                parameters,
                source_info: step.source_info.clone(),
            }
        })
        .collect();

    Some(untyped(
        ExprKind::PathLiteral {
            start_type,
            steps,
            name: e.name.clone(),
        },
        e.source_info.clone(),
    ))
}
