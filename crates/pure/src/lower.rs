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
//! - **Literals**: Integer, Float, Decimal, String, Boolean, Date/Time
//! - **Variables**: `$name` references
//! - **Collections**: `[1, 2, 3]`
//! - **Groups**: `(expr)` — unwrapped transparently
//!
//! Remaining expression variants (operators, function calls, member access,
//! lambdas, etc.) are handled in subsequent phases.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::Spanned;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::ResolutionContext;
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
        ast_expr::Expression::Literal(lit) => lower_literal(lit),
        ast_expr::Expression::Variable(var) => Some(lower_variable(var)),
        ast_expr::Expression::Collection(coll) => Some(lower_collection(coll, ctx, errors)),
        ast_expr::Expression::Group(inner) => lower_expression(inner, ctx, errors),

        // Phase 2+ stubs — produce a diagnostic-friendly placeholder
        _ => {
            let source_info = expr.source_info().clone();
            errors.push(CompilationError {
                message: format!(
                    "Expression lowering not yet implemented for {:?}",
                    expression_kind_name(expr)
                ),
                source_info: source_info.clone(),
                kind: crate::error::CompilationErrorKind::UnsupportedExpression {
                    kind: SmolStr::new(expression_kind_name(expr)),
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

/// Returns a human-readable name for an expression variant (for diagnostics).
fn expression_kind_name(expr: &ast_expr::Expression) -> &'static str {
    match expr {
        ast_expr::Expression::Literal(_) => "Literal",
        ast_expr::Expression::Variable(_) => "Variable",
        ast_expr::Expression::Arithmetic(_) => "Arithmetic",
        ast_expr::Expression::Comparison(_) => "Comparison",
        ast_expr::Expression::Logical(_) => "Logical",
        ast_expr::Expression::Bitwise(_) => "Bitwise",
        ast_expr::Expression::Not(_) => "Not",
        ast_expr::Expression::UnaryMinus(_) => "UnaryMinus",
        ast_expr::Expression::BitwiseNot(_) => "BitwiseNot",
        ast_expr::Expression::FunctionApplication(_) => "FunctionApplication",
        ast_expr::Expression::ArrowFunction(_) => "ArrowFunction",
        ast_expr::Expression::MemberAccess(_) => "MemberAccess",
        ast_expr::Expression::PackageableElementRef(_) => "PackageableElementRef",
        ast_expr::Expression::TypeReferenceExpr(_) => "TypeReferenceExpr",
        ast_expr::Expression::Lambda(_) => "Lambda",
        ast_expr::Expression::Let(_) => "Let",
        ast_expr::Expression::Collection(_) => "Collection",
        ast_expr::Expression::NewInstance(_) => "NewInstance",
        ast_expr::Expression::Column(_) => "Column",
        ast_expr::Expression::Island(_) => "Island",
        ast_expr::Expression::Group(_) => "Group",
    }
}

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
