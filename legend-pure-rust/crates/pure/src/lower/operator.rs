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

//! Operator desugaring: arithmetic / comparison / logical / bitwise /
//! unary operator AST nodes → `FunctionCall` nodes.
//!
//! Step 4 of the lowering encapsulation plan; sibling slice to
//! `lower/literal.rs` and `lower/collection.rs`. Every operator desugars
//! through one of two helpers (`binary_op`, `unary_op`) — or through
//! `variadic_op` for `T[*]`-parameter natives (`plus`/`minus`/`times`).
//!
//! All paths route overload selection through
//! [`resolve::resolve_function_call`] so type-based narrowing picks the
//! right platform overload (e.g. `lessThan(Date,Date)` over the numeric
//! native).

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::{lower_expression, untyped};

/// Helper: build a binary operator `FunctionCall`.
fn binary_op(
    name: &str,
    left: &ast_expr::Expression,
    right: &ast_expr::Expression,
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let l = lower_expression(left, ctx, errors)?;
    let r = lower_expression(right, ctx, errors)?;
    let arguments = vec![l, r];
    // Route through `resolve_function_call` so type-based overload
    // narrowing picks the right `lessThan(Date,Date)` /
    // `lessThan(Boolean,Boolean)` / `lessThan(String,String)` etc.
    // platform overload instead of falling through the runtime's
    // prefix-name fallback which always picks the Number native.
    // Mirrors `variadic_op` for plus/minus/times.
    //
    // We use a *scratch* error buffer and discard those errors when
    // resolution fails: comparison/logical operators have always been
    // permitted to leave dispatch unresolved at compile time (the
    // runtime's prefix-name fallback then picks the only registered
    // native — which is correct when nothing better exists, and
    // tolerated by integration tests that compile without the
    // platform). When resolution *succeeds*, we commit the
    // ElementId so the runtime can dispatch directly to the right
    // overload; otherwise we leave `function: None` exactly like
    // before this change.
    let ptr = synthetic_unqualified_ptr(name, source_info);
    let mut scratch_errors: Vec<CompilationError> = Vec::new();
    let function_id =
        resolve::resolve_function_call(&ptr, 2, &arguments, source_info, ctx, &mut scratch_errors);
    if function_id.is_some() {
        // Resolution succeeded — propagate any non-fatal diagnostics
        // it produced (e.g. multi-candidate ambiguity warnings).
        errors.extend(scratch_errors);
    }
    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: function_id,
            function_name: SmolStr::new(name),
            arguments,
        }),
        source_info.clone(),
    ))
}

/// Helper: build a unary operator `FunctionCall`.
fn unary_op(
    name: &str,
    operand: &ast_expr::Expression,
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let inner = lower_expression(operand, ctx, errors)?;
    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new(name),
            arguments: vec![inner],
        }),
        source_info.clone(),
    ))
}

/// Lowers arithmetic: `a + b` → `FunctionCall("plus", [a, b])`.
pub(super) fn lower_arithmetic(
    e: &ast_expr::ArithmeticExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    match e.op {
        // `plus` / `minus` / `times` are declared in the platform with the
        // single-parameter signature `(Number[*]):Number[1]` (see
        // `platform/pure/grammar/functions/math/operation/{plus,minus,times}.pure`),
        // so the native always receives **one** collection argument. The
        // surface-level operator desugars to `op([left, right])`, matching
        // Java Pure's handoff where `Plus.execute` reads `params.get(0)`
        // and iterates `.values`.
        ast_expr::ArithmeticOp::Plus => {
            variadic_op("plus", &e.left, &e.right, &e.source_info, ctx, errors)
        }
        ast_expr::ArithmeticOp::Minus => {
            variadic_op("minus", &e.left, &e.right, &e.source_info, ctx, errors)
        }
        ast_expr::ArithmeticOp::Times => {
            variadic_op("times", &e.left, &e.right, &e.source_info, ctx, errors)
        }
        // `divide(Number[1], Number[1]):Float[1]` is genuinely pairwise —
        // the Pure signature takes two `[1]` arguments. Keep the pair
        // lowering untouched.
        ast_expr::ArithmeticOp::Divide => {
            binary_op("divide", &e.left, &e.right, &e.source_info, ctx, errors)
        }
    }
}

/// Lowers `a OP b` for a T[*]-parameter native: `op([left, right])`.
/// The single Collection argument matches the Java Pure dispatch shape —
/// `op.execute(params)` then reads `params.get(0).values` to iterate.
///
/// Routes overload selection through `resolve_function_call` — the same
/// machinery every other call site uses. The resolver narrows
/// `plus(Number[*])` vs `plus(String[*])` (etc.) by inferring the
/// operand collection's LUB type from `infer_type_from_valuespec`, which
/// transparently handles `cast(@T)` (via generic substitution on the
/// resolved `cast<T>` signature), `PropertyAccess` (declared property
/// type), variable declarations, literals, and nested call returns.
///
/// **Encapsulation:** operator code never special-cases `cast` or any
/// other type-shaping primitive; downstream of any value, the operator
/// sees the value's *declared type*, not its expression shape. New
/// type-shaping primitives (e.g. multiplicity coercions like `toOne`)
/// pick up the same treatment as soon as their inference lives in
/// `infer_type_from_valuespec`.
///
/// **Fail-fast:** when the resolver can't pick a unique overload, it
/// pushes a diagnostic and returns `None`; we propagate `None` so the
/// caller sees a compile-time miss instead of the runtime falling
/// through prefix-name dispatch (which used to silently pick numeric
/// `plus` and fail on String operands at runtime). Trace-driven
/// debugging: run with
///     `RUST_LOG=legend_pure_parser_pure::resolve=debug`
/// to see per-call narrowing decisions and identify the operand whose
/// type couldn't be inferred.
#[tracing::instrument(
    name = "variadic_op",
    level = "debug",
    skip(left, right, ctx, errors),
    fields(op = %name, src = %source_info.source, line = source_info.start_line),
)]
fn variadic_op(
    name: &str,
    left: &ast_expr::Expression,
    right: &ast_expr::Expression,
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let l = lower_expression(left, ctx, errors)?;
    let r = lower_expression(right, ctx, errors)?;
    let collection = untyped(
        ExprKind::Collection {
            elements: vec![l, r],
        },
        source_info.clone(),
    );
    let arguments = vec![collection];
    let ptr = synthetic_unqualified_ptr(name, source_info);
    let function_id =
        resolve::resolve_function_call(&ptr, 1, &arguments, source_info, ctx, errors)?;
    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: Some(function_id),
            function_name: SmolStr::new(name),
            arguments,
        }),
        source_info.clone(),
    ))
}

/// Build an unqualified `PackageableElementPtr` from a bare function
/// name, anchored at `source_info`. Lets [`variadic_op`] feed
/// `resolve_function_call` the same shape it would receive from a
/// user-written `plus(x, y)` call so overload resolution walks the
/// import scopes uniformly.
///
/// Exposed as `pub(super)` because `lower/mod.rs::lower_arrow_function`
/// (the `getAll` extension dispatch) shares this synthesis pattern.
pub(super) fn synthetic_unqualified_ptr(
    name: &str,
    source_info: &SourceInfo,
) -> legend_pure_parser_ast::annotation::PackageableElementPtr {
    use legend_pure_parser_ast::annotation::PackageableElementPtr;
    PackageableElementPtr {
        package: None,
        name: SmolStr::new(name),
        source_info: source_info.clone(),
    }
}

/// Lowers comparison: `a == b` → `FunctionCall("equal", [a, b])`.
///
/// `!=` desugars to `not(equal(a, b))` matching the Java M3 behavior.
pub(super) fn lower_comparison(
    e: &ast_expr::ComparisonExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let (name, negate) = match e.op {
        ast_expr::ComparisonOp::Equal => ("equal", false),
        ast_expr::ComparisonOp::NotEqual => ("equal", true),
        ast_expr::ComparisonOp::LessThan => ("lessThan", false),
        ast_expr::ComparisonOp::LessThanOrEqual => ("lessThanEqual", false),
        ast_expr::ComparisonOp::GreaterThan => ("greaterThan", false),
        ast_expr::ComparisonOp::GreaterThanOrEqual => ("greaterThanEqual", false),
    };
    let inner = binary_op(name, &e.left, &e.right, &e.source_info, ctx, errors)?;
    if negate {
        Some(untyped(
            ExprKind::FunctionCall(FunctionCallData {
                function: None,
                function_name: SmolStr::new_static("not"),
                arguments: vec![inner],
            }),
            e.source_info.clone(),
        ))
    } else {
        Some(inner)
    }
}

/// Lowers logical: `a && b` → `FunctionCall("and", [a, b])`.
pub(super) fn lower_logical(
    e: &ast_expr::LogicalExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let name = match e.op {
        ast_expr::LogicalOp::And => "and",
        ast_expr::LogicalOp::Or => "or",
    };
    binary_op(name, &e.left, &e.right, &e.source_info, ctx, errors)
}

/// Lowers bitwise: `a &&& b` → `FunctionCall("bitwiseAnd", [a, b])`.
pub(super) fn lower_bitwise(
    e: &ast_expr::BitwiseExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let name = match e.op {
        ast_expr::BitwiseOp::And => "bitwiseAnd",
        ast_expr::BitwiseOp::Or => "bitwiseOr",
        ast_expr::BitwiseOp::Xor => "bitwiseXor",
        ast_expr::BitwiseOp::ShiftLeft => "shiftLeft",
        ast_expr::BitwiseOp::ShiftRight => "shiftRight",
    };
    binary_op(name, &e.left, &e.right, &e.source_info, ctx, errors)
}

/// Lowers `!expr` → `FunctionCall("not", [expr])`.
pub(super) fn lower_unary_not(
    e: &ast_expr::NotExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    unary_op("not", &e.operand, &e.source_info, ctx, errors)
}

/// Lowers `-expr` → `FunctionCall("minus", [Collection([expr])])`.
///
/// Mirrors the variadic lowering used for `a - b` — `minus` has the single
/// signature `(Number[*]):Number[1]`, so even the unary form feeds a
/// singleton collection. Java Pure's `Minus.execute` handles `size == 1`
/// as unary negate (`0 - x`); our native does the same.
pub(super) fn lower_unary_minus(
    e: &ast_expr::UnaryMinusExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let operand = lower_expression(&e.operand, ctx, errors)?;
    let collection = untyped(
        ExprKind::Collection {
            elements: vec![operand],
        },
        e.source_info.clone(),
    );
    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new("minus"),
            arguments: vec![collection],
        }),
        e.source_info.clone(),
    ))
}

/// Lowers `~~~expr` → `FunctionCall("bitwiseNot", [expr])`.
pub(super) fn lower_bitwise_not(
    e: &ast_expr::BitwiseNotExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    unary_op("bitwiseNot", &e.operand, &e.source_info, ctx, errors)
}
