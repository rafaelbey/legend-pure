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

//! Copy and slice lowering: `^$source(prop=…, other+=…)` → `copy(…)`,
//! and `[start:stop:step]` → `range(…)`.
//!
//! Step 4 of the lowering encapsulation plan. Both desugarings emit
//! plain `FunctionCall` nodes and contain no shared helpers; cohabit
//! because they are syntactically adjacent (subscript family) and
//! both are short enough that splitting again would add boilerplate
//! without benefit.

use legend_pure_parser_ast::expression as ast_expr;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::ResolutionContext;
use crate::types::{ExprKind, FunctionCallData, ResolvedType, ValueSpec};

use super::{lower_expression, typed, untyped};

/// Lowers `^$source(prop='val', other += $vals)` →
/// `FunctionCall("copy", [$source, key1, val1, augmented1, ...])`.
///
/// Matches the Java M3 desugaring: the source variable becomes the first
/// argument, followed by `(key, value, augmented_bool)` triples for each
/// property override. The augmented flag distinguishes `=` (replace,
/// `mutate_set`) from `+=` (append, `mutate_add`) — Java carries this as
/// the `KeyValue.add` slot; we encode it inline so the runtime needs no
/// schema lookup.
pub(super) fn lower_copy(
    e: &ast_expr::CopyExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> ValueSpec {
    let source_var = untyped(
        ExprKind::Variable {
            name: e.source.clone(),
        },
        e.source_info.clone(),
    );

    let mut arguments = Vec::with_capacity(1 + e.assignments.len() * 3);
    arguments.push(source_var);

    for kv in &e.assignments {
        // Drop the whole triple if value lowering failed — see the matching
        // comment in `lower_new_instance` for the stride-preservation rationale.
        let Some(val) = lower_expression(&kv.value, ctx, errors) else {
            continue;
        };
        arguments.push(untyped(
            ExprKind::StringLiteral(SmolStr::new(kv.key.as_str())),
            kv.source_info.clone(),
        ));
        arguments.push(val);
        arguments.push(untyped(
            ExprKind::BooleanLiteral(kv.augmented),
            kv.source_info.clone(),
        ));
    }

    // Pre-set `type_info` from the source variable's declared type
    // when we know it. `^$x(field=val)` is a structural clone, so the
    // result's type is exactly `$x`'s type. Capturing it at lower
    // time means downstream consumers (`set_and_return`'s honour-
    // pre-set rule, `infer_typeexpr_from_valuespec`'s early-return,
    // `infer_let_type`'s preset path) all see the right type without
    // any of them needing to special-case the `function_name == "copy"`
    // branch. This is the canonical "lowering captures type;
    // consumers read from `type_info`" pattern documented at
    // `reference_type_info_capture.md`.
    let kind = ExprKind::FunctionCall(FunctionCallData {
        function: None,
        function_name: SmolStr::new_static("copy"),
        arguments,
    });
    if let Some((source_te, source_mult)) = ctx.variable_types.get(&e.source).cloned() {
        return typed(
            kind,
            e.source_info.clone(),
            ResolvedType {
                type_expr: source_te,
                multiplicity: source_mult,
            },
        );
    }
    untyped(kind, e.source_info.clone())
}

/// Lowers `[start:stop]` or `[start:stop:step]` → `FunctionCall("range", args)`.
///
/// Matches the Java M3 desugaring of slice (subscript) expressions to
/// `range(start, stop)` or `range(start, stop, step)`. Missing `start`
/// defaults to integer literal `0`.
pub(super) fn lower_slice(
    e: &ast_expr::SliceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let start = match &e.start {
        Some(s) => lower_expression(s, ctx, errors)?,
        None => untyped(ExprKind::IntegerLiteral(0), e.source_info.clone()),
    };
    let stop = lower_expression(&e.stop, ctx, errors)?;

    let mut arguments = vec![start, stop];
    if let Some(ref step) = e.step
        && let Some(step_val) = lower_expression(step, ctx, errors)
    {
        arguments.push(step_val);
    }

    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new_static("range"),
            arguments,
        }),
        e.source_info.clone(),
    ))
}
