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

//! Expression lowering: AST `Expression` → semantic `ValueSpec`.
//!
//! This module recursively converts the parser's syntactic expression tree
//! into the compiler's semantic expression type, where all names are resolved
//! to [`ElementId`](crate::ids::ElementId)s, operators are desugared to
//! function calls, and grouping parentheses are eliminated.
//!
//! ## Phase 1 Coverage
//!
//! ## Coverage
//!
//! - **Phase 1**: Literals, Variables, Collections, Groups
//! - **Phase 2**: Operators (→ `FunctionCall`), Function Application, Arrow,
//!   Member Access, Type References, Packageable Element Refs
//! - **Phase 3**: Lambda, Let (→ `FunctionCall("letFunction")`),
//!   New Instance (→ `FunctionCall("new")`), Column (placeholder)
//!
//! Island expressions produce a diagnostic — full lowering is deferred.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::{SourceInfo, Spanned};
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{DateValue, ValueSpec};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Lowers an AST expression to a semantic [`ValueSpec`].
///
/// Returns `None` if the expression cannot be lowered (error pushed to `errors`).
pub(crate) fn lower_expression(
    expr: &ast_expr::Expression,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    match expr {
        // Phase 1
        ast_expr::Expression::Literal(lit) => lower_literal(lit),
        ast_expr::Expression::Variable(var) => Some(lower_variable(var)),
        ast_expr::Expression::Collection(coll) => Some(lower_collection(coll, ctx, errors)),
        ast_expr::Expression::Group(inner) => lower_expression(inner, ctx, errors),

        // Phase 2 — Operators → FunctionCall
        ast_expr::Expression::Arithmetic(e) => lower_arithmetic(e, ctx, errors),
        ast_expr::Expression::Comparison(e) => lower_comparison(e, ctx, errors),
        ast_expr::Expression::Logical(e) => lower_logical(e, ctx, errors),
        ast_expr::Expression::Bitwise(e) => lower_bitwise(e, ctx, errors),
        ast_expr::Expression::Not(e) => lower_unary_not(e, ctx, errors),
        ast_expr::Expression::UnaryMinus(e) => lower_unary_minus(e, ctx, errors),
        ast_expr::Expression::BitwiseNot(e) => lower_bitwise_not(e, ctx, errors),

        // Phase 2 — Function & member access
        ast_expr::Expression::FunctionApplication(e) => lower_function_application(e, ctx, errors),
        ast_expr::Expression::ArrowFunction(e) => lower_arrow_function(e, ctx, errors),
        ast_expr::Expression::MemberAccess(e) => lower_member_access(e, ctx, errors),
        ast_expr::Expression::TypeReferenceExpr(e) => lower_type_reference(e, ctx, errors),
        ast_expr::Expression::PackageableElementRef(e) => {
            lower_packageable_element_ref(e, ctx, errors)
        }

        // Phase 3 — Lambda, Let, New, Column, Island
        ast_expr::Expression::Lambda(e) => lower_lambda(e, ctx, errors),
        ast_expr::Expression::Let(e) => lower_let(e, ctx, errors),
        ast_expr::Expression::NewInstance(e) => lower_new_instance(e, ctx, errors),
        ast_expr::Expression::Column(e) => Some(lower_column(e)),
        ast_expr::Expression::Island(_) => {
            // Island lowering is deferred — the AST node is sufficient
            // for protocol serialization and composer roundtripping.
            let source_info = expr.source_info().clone();
            errors.push(CompilationError {
                message: "Island expression lowering not yet implemented".to_string(),
                source_info: source_info.clone(),
                kind: crate::error::CompilationErrorKind::UnsupportedExpression {
                    kind: SmolStr::new_static("Island"),
                },
            });
            None
        }
    }
}

/// Lowers a sequence of AST expressions (e.g., a function body).
///
/// Expressions that fail to lower are skipped (errors are accumulated).
pub(crate) fn lower_expression_body(
    body: &[ast_expr::Expression],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<ValueSpec> {
    body.iter()
        .filter_map(|expr| lower_expression(expr, ctx, errors))
        .collect()
}

// ---------------------------------------------------------------------------
// Literal lowering
// ---------------------------------------------------------------------------

/// Lowers an AST literal to a `ValueSpec`.
fn lower_literal(lit: &ast_expr::Literal) -> Option<ValueSpec> {
    match lit {
        ast_expr::Literal::Integer(i) => {
            Some(ValueSpec::IntegerLiteral(i.value, i.source_info.clone()))
        }
        ast_expr::Literal::Float(f) => {
            Some(ValueSpec::FloatLiteral(f.value, f.source_info.clone()))
        }
        ast_expr::Literal::Decimal(d) => {
            let decimal = d.value.parse::<rust_decimal::Decimal>().ok()?;
            Some(ValueSpec::DecimalLiteral(decimal, d.source_info.clone()))
        }
        ast_expr::Literal::String(s) => Some(ValueSpec::StringLiteral(
            SmolStr::new(&s.value),
            s.source_info.clone(),
        )),
        ast_expr::Literal::Boolean(b) => {
            Some(ValueSpec::BooleanLiteral(b.value, b.source_info.clone()))
        }
        ast_expr::Literal::StrictDate(d) => {
            let dv = parse_strict_date(&d.value)?;
            Some(ValueSpec::DateLiteral(dv, d.source_info.clone()))
        }
        ast_expr::Literal::DateTime(d) => {
            let dv = parse_datetime(&d.value)?;
            Some(ValueSpec::DateLiteral(dv, d.source_info.clone()))
        }
        ast_expr::Literal::StrictTime(t) => {
            let dv = parse_strict_time(&t.value)?;
            Some(ValueSpec::DateLiteral(dv, t.source_info.clone()))
        }
    }
}

// ---------------------------------------------------------------------------
// Variable lowering
// ---------------------------------------------------------------------------

/// Lowers a variable reference `$name`.
fn lower_variable(var: &ast_expr::Variable) -> ValueSpec {
    ValueSpec::Variable {
        name: var.name.clone(),
        source_info: var.source_info.clone(),
    }
}

// ---------------------------------------------------------------------------
// Collection lowering
// ---------------------------------------------------------------------------

/// Lowers a collection literal `[a, b, c]`.
fn lower_collection(
    coll: &ast_expr::CollectionExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> ValueSpec {
    let elements = coll
        .elements
        .iter()
        .filter_map(|e| lower_expression(e, ctx, errors))
        .collect();
    ValueSpec::Collection {
        elements,
        source_info: coll.source_info.clone(),
    }
}

// ---------------------------------------------------------------------------
// Operator desugaring → FunctionCall
// ---------------------------------------------------------------------------

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
    Some(ValueSpec::FunctionCall {
        function: None,
        function_name: SmolStr::new(name),
        arguments: vec![l, r],
        source_info: source_info.clone(),
    })
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
    Some(ValueSpec::FunctionCall {
        function: None,
        function_name: SmolStr::new(name),
        arguments: vec![inner],
        source_info: source_info.clone(),
    })
}

/// Lowers arithmetic: `a + b` → `FunctionCall("plus", [a, b])`.
fn lower_arithmetic(
    e: &ast_expr::ArithmeticExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let name = match e.op {
        ast_expr::ArithmeticOp::Plus => "plus",
        ast_expr::ArithmeticOp::Minus => "minus",
        ast_expr::ArithmeticOp::Times => "times",
        ast_expr::ArithmeticOp::Divide => "divide",
    };
    binary_op(name, &e.left, &e.right, &e.source_info, ctx, errors)
}

/// Lowers comparison: `a == b` → `FunctionCall("equal", [a, b])`.
///
/// `!=` desugars to `not(equal(a, b))` matching the Java M3 behavior.
fn lower_comparison(
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
        Some(ValueSpec::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("not"),
            arguments: vec![inner],
            source_info: e.source_info.clone(),
        })
    } else {
        Some(inner)
    }
}

/// Lowers logical: `a && b` → `FunctionCall("and", [a, b])`.
fn lower_logical(
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
fn lower_bitwise(
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
fn lower_unary_not(
    e: &ast_expr::NotExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    unary_op("not", &e.operand, &e.source_info, ctx, errors)
}

/// Lowers `-expr` → `FunctionCall("minus", [expr])`.
fn lower_unary_minus(
    e: &ast_expr::UnaryMinusExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    unary_op("minus", &e.operand, &e.source_info, ctx, errors)
}

/// Lowers `~~~expr` → `FunctionCall("bitwiseNot", [expr])`.
fn lower_bitwise_not(
    e: &ast_expr::BitwiseNotExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    unary_op("bitwiseNot", &e.operand, &e.source_info, ctx, errors)
}

// ---------------------------------------------------------------------------
// Function application & arrow
// ---------------------------------------------------------------------------

/// Lowers `func(args)` → `FunctionCall`.
///
/// Resolves the function name through import-aware resolution. If resolution
/// fails, the call is still produced with `function: None` so downstream
/// code can see the structure.
#[allow(clippy::unnecessary_wraps)] // consistent signature with other lower_* fns
fn lower_function_application(
    e: &ast_expr::FunctionApplication,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let function_id = resolve::resolve_element_ptr(&e.function, &e.source_info, ctx, errors);

    let arguments: Vec<ValueSpec> = e
        .arguments
        .iter()
        .filter_map(|a| lower_expression(a, ctx, errors))
        .collect();

    Some(ValueSpec::FunctionCall {
        function: function_id,
        function_name: SmolStr::new(e.function.name.as_str()),
        arguments,
        source_info: e.source_info.clone(),
    })
}

/// Lowers `expr->func(args)` → `FunctionCall` with target prepended.
///
/// `$x->filter(p)` becomes `FunctionCall("filter", [$x, p])`.
fn lower_arrow_function(
    e: &ast_expr::ArrowFunction,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let function_id = resolve::resolve_element_ptr(&e.function, &e.source_info, ctx, errors);

    let target = lower_expression(&e.target, ctx, errors)?;

    let mut arguments = Vec::with_capacity(1 + e.arguments.len());
    arguments.push(target);
    for arg in &e.arguments {
        if let Some(lowered) = lower_expression(arg, ctx, errors) {
            arguments.push(lowered);
        }
    }

    Some(ValueSpec::FunctionCall {
        function: function_id,
        function_name: SmolStr::new(e.function.name.as_str()),
        arguments,
        source_info: e.source_info.clone(),
    })
}

// ---------------------------------------------------------------------------
// Member access
// ---------------------------------------------------------------------------

/// Lowers member access (dot): `$x.name` or `$x.derived('arg')`.
fn lower_member_access(
    e: &ast_expr::MemberAccess,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    match e {
        ast_expr::MemberAccess::Simple(s) => {
            let target = lower_expression(&s.target, ctx, errors)?;
            Some(ValueSpec::PropertyAccess {
                target: Box::new(target),
                property: SmolStr::new(s.member.as_str()),
                source_info: s.source_info.clone(),
            })
        }
        ast_expr::MemberAccess::Qualified(q) => {
            let target = lower_expression(&q.target, ctx, errors)?;
            let arguments: Vec<ValueSpec> = q
                .arguments
                .iter()
                .filter_map(|a| lower_expression(a, ctx, errors))
                .collect();
            Some(ValueSpec::QualifiedPropertyAccess {
                target: Box::new(target),
                property: SmolStr::new(q.member.as_str()),
                arguments,
                source_info: q.source_info.clone(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Type reference & element reference
// ---------------------------------------------------------------------------

/// Lowers `@MyType` → `TypeReference`.
fn lower_type_reference(
    e: &ast_expr::TypeReferenceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let type_expr = resolve::resolve_type_ref(&e.type_ref, ctx, errors)?;
    Some(ValueSpec::TypeReference {
        type_expr,
        source_info: e.source_info.clone(),
    })
}

/// Lowers a bare element reference: `String`, `my::Enum` → `PackageableElementRef`.
fn lower_packageable_element_ref(
    e: &ast_expr::PackageableElementRef,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let element_id = resolve::resolve_element_ptr(&e.element, &e.source_info, ctx, errors)?;
    Some(ValueSpec::PackageableElementRef {
        element: element_id,
        source_info: e.source_info.clone(),
    })
}

// ---------------------------------------------------------------------------
// Lambda
// ---------------------------------------------------------------------------

/// Lowers a lambda: `{x: String[1] | $x + 'hello'}` → `ValueSpec::Lambda`.
///
/// Lambda parameters are lowered the same way as function parameters.
/// Untyped parameters (inferred lambdas like `x | $x + 1`) are kept with
/// best-effort type information — full inference is deferred.
#[allow(clippy::unnecessary_wraps)] // consistent signature with other lower_* fns
fn lower_lambda(
    e: &ast_expr::Lambda,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let parameters = lower_lambda_parameters(&e.parameters, ctx, errors);
    let body = lower_expression_body(&e.body, ctx, errors);
    Some(ValueSpec::Lambda {
        parameters,
        body,
        source_info: e.source_info.clone(),
    })
}

/// Lower lambda parameters — same as function parameters but tolerates
/// missing type/multiplicity (untyped lambda params need type inference).
fn lower_lambda_parameters(
    params: &[legend_pure_parser_ast::annotation::Parameter],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params
        .iter()
        .map(|p| {
            let type_expr = p
                .type_ref
                .as_ref()
                .and_then(|tr| resolve::resolve_type_ref(tr, ctx, errors))
                .unwrap_or(crate::types::TypeExpr::Named {
                    element: crate::bootstrap::ANY_ID,
                    type_arguments: vec![],
                    value_arguments: vec![],
                });
            let multiplicity = p.multiplicity.as_ref().map_or(
                crate::types::Multiplicity::PureOne,
                resolve::lower_multiplicity,
            );
            crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity,
                source_info: p.source_info.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Let
// ---------------------------------------------------------------------------

/// Lowers `let x = expr` → `FunctionCall("letFunction", [name, value])`.
///
/// Matches the Java M3 desugaring: the variable name becomes a string
/// literal, and the value is the lowered RHS expression.
fn lower_let(
    e: &ast_expr::LetExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let value = lower_expression(&e.value, ctx, errors)?;
    Some(ValueSpec::FunctionCall {
        function: None,
        function_name: SmolStr::new_static("letFunction"),
        arguments: vec![
            ValueSpec::StringLiteral(SmolStr::new(e.name.as_str()), e.source_info.clone()),
            value,
        ],
        source_info: e.source_info.clone(),
    })
}

// ---------------------------------------------------------------------------
// New instance
// ---------------------------------------------------------------------------

/// Lowers `^MyClass(prop='val')` → `FunctionCall("new", [class, name, kvs...])`.
///
/// Matches the Java M3 desugaring: the class is resolved as an element
/// reference, followed by the class name as a string, then key-value pairs
/// as alternating string-name/value arguments.
fn lower_new_instance(
    e: &ast_expr::NewInstanceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let class_id = resolve::resolve_element_ptr(&e.class, &e.source_info, ctx, errors)?;

    // Build argument list: class_ref, className_string, key1, val1, key2, val2, ...
    let mut arguments = Vec::with_capacity(2 + e.assignments.len() * 2);
    arguments.push(ValueSpec::PackageableElementRef {
        element: class_id,
        source_info: e.source_info.clone(),
    });
    arguments.push(ValueSpec::StringLiteral(
        SmolStr::new(e.class.name.as_str()),
        e.source_info.clone(),
    ));
    for kv in &e.assignments {
        arguments.push(ValueSpec::StringLiteral(
            SmolStr::new(kv.key.as_str()),
            kv.source_info.clone(),
        ));
        if let Some(val) = lower_expression(&kv.value, ctx, errors) {
            arguments.push(val);
        }
    }

    Some(ValueSpec::FunctionCall {
        function: None,
        function_name: SmolStr::new_static("new"),
        arguments,
        source_info: e.source_info.clone(),
    })
}

// ---------------------------------------------------------------------------
// Column (TDS — placeholder)
// ---------------------------------------------------------------------------

/// Lowers a column expression to a placeholder `ValueSpec::Column`.
///
/// Full TDS column lowering is deferred — the source info is preserved
/// for diagnostics and protocol output.
fn lower_column(e: &ast_expr::ColumnExpression) -> ValueSpec {
    let source_info = match e {
        ast_expr::ColumnExpression::Name(c) => c.source_info.clone(),
        ast_expr::ColumnExpression::WithLambda(c) => c.source_info.clone(),
        ast_expr::ColumnExpression::Typed(c) => c.source_info.clone(),
        ast_expr::ColumnExpression::WithFunction(c) => c.source_info.clone(),
    };
    ValueSpec::Column { source_info }
}

// ---------------------------------------------------------------------------
// Date parsing helpers
// ---------------------------------------------------------------------------

/// Parses `"2024-01-15"` → `DateValue::StrictDate`.
fn parse_strict_date(s: &str) -> Option<DateValue> {
    // Format: YYYY-MM-DD (possibly with leading %)
    let s = s.strip_prefix('%').unwrap_or(s);
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(DateValue::StrictDate {
        year: parts[0].parse().ok()?,
        month: parts[1].parse().ok()?,
        day: parts[2].parse().ok()?,
    })
}

/// Parses `"2024-01-15T10:30:00"` (or with subseconds) → `DateValue::DateTime`.
fn parse_datetime(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (date_part, time_part) = s.split_once('T')?;
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }

    // Time may have fractional seconds: HH:MM:SS or HH:MM:SS.nnn
    let (time_main, subsec_str) = match time_part.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (time_part, ""),
    };
    // Strip timezone suffix if present (e.g., "+0000")
    let time_main = time_main
        .split_once('+')
        .map_or(time_main, |(main, _)| main);
    let time_main = time_main
        .split_once('-')
        .map_or(time_main, |(main, _)| main);

    let time_parts: Vec<&str> = time_main.split(':').collect();
    if time_parts.len() < 2 {
        return None;
    }

    let nanos = parse_subsecond_nanos(subsec_str);

    Some(DateValue::DateTime {
        year: date_parts[0].parse().ok()?,
        month: date_parts[1].parse().ok()?,
        day: date_parts[2].parse().ok()?,
        hour: time_parts[0].parse().ok()?,
        minute: time_parts[1].parse().ok()?,
        second: time_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
    })
}

/// Parses `"10:30:00"` → `DateValue::StrictTime`.
fn parse_strict_time(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (time_main, subsec_str) = match s.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (s, ""),
    };
    let parts: Vec<&str> = time_main.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let nanos = parse_subsecond_nanos(subsec_str);
    Some(DateValue::StrictTime {
        hour: parts[0].parse().ok()?,
        minute: parts[1].parse().ok()?,
        second: parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
    })
}

/// Converts a fractional seconds string (e.g., `"123"`, `"12345"`) to nanoseconds.
fn parse_subsecond_nanos(frac: &str) -> u32 {
    if frac.is_empty() {
        return 0;
    }
    // Strip any trailing timezone info from the frac part
    let frac = frac.split_once('+').map_or(frac, |(main, _)| main);
    let frac = frac.split_once('-').map_or(frac, |(main, _)| main);
    // Pad/truncate to 9 digits
    let mut padded = String::with_capacity(9);
    for (i, c) in frac.chars().enumerate() {
        if i >= 9 {
            break;
        }
        padded.push(c);
    }
    while padded.len() < 9 {
        padded.push('0');
    }
    padded.parse().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DateValue;

    #[test]
    fn parse_strict_date_basic() {
        let dv = parse_strict_date("2024-01-15").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: 1,
                day: 15
            }
        );
    }

    #[test]
    fn parse_strict_date_with_percent() {
        let dv = parse_strict_date("%2024-03-20").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: 3,
                day: 20
            }
        );
    }

    #[test]
    fn parse_datetime_basic() {
        let dv = parse_datetime("2024-01-15T10:30:00").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 0,
                subsecond_nanos: 0
            }
        );
    }

    #[test]
    fn parse_datetime_with_subseconds() {
        let dv = parse_datetime("%2024-01-15T10:30:45.123").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 45,
                subsecond_nanos: 123_000_000
            }
        );
    }

    #[test]
    fn parse_strict_time_basic() {
        let dv = parse_strict_time("10:30:00").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictTime {
                hour: 10,
                minute: 30,
                second: 0,
                subsecond_nanos: 0
            }
        );
    }

    #[test]
    fn parse_strict_time_with_nanos() {
        let dv = parse_strict_time("%14:05:30.5").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictTime {
                hour: 14,
                minute: 5,
                second: 30,
                subsecond_nanos: 500_000_000
            }
        );
    }

    #[test]
    fn parse_subsecond_nanos_padding() {
        assert_eq!(parse_subsecond_nanos("1"), 100_000_000);
        assert_eq!(parse_subsecond_nanos("12"), 120_000_000);
        assert_eq!(parse_subsecond_nanos("123"), 123_000_000);
        assert_eq!(parse_subsecond_nanos("123456789"), 123_456_789);
        assert_eq!(parse_subsecond_nanos(""), 0);
    }
}
