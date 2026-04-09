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

//! Expression composer with operator precedence handling.
//!
//! The core challenge is parenthesization: `1 + 2 * 3` must NOT add parens,
//! but `(1 + 2) * 3` must. We track the parent operator's precedence level
//! and only emit parentheses when the child's precedence is lower.

use legend_pure_parser_ast::annotation::{PackageableElementPtr, Parameter};
use legend_pure_parser_ast::element::PackageableElement as _;
use legend_pure_parser_ast::expression::{
    ArithmeticExpr, ArithmeticOp, ArrowFunction, BitwiseExpr, BitwiseNotExpr, BitwiseOp,
    CollectionExpr, ColumnExpression, ComparisonExpr, ComparisonOp, Expression,
    FunctionApplication, Lambda, LetExpr, Literal, LogicalExpr, LogicalOp, MemberAccess,
    NewInstanceExpr, NotExpr, PackageableElementRef, TypeReferenceExpr, UnaryMinusExpr, Variable,
};

use crate::identifier::{escape_pure_string, maybe_quote};
use crate::writer::IndentWriter;

// ---------------------------------------------------------------------------
// Precedence
// ---------------------------------------------------------------------------

/// Operator precedence levels (higher = binds tighter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Precedence {
    /// Lowest — used as the initial "no parent" context.
    None = 0,
    /// `||`
    Or = 1,
    /// `&&`
    And = 2,
    /// `==`, `!=`
    Equality = 3,
    /// `<`, `<=`, `>`, `>=`
    Relational = 4,
    /// `+`, `-`
    Additive = 5,
    /// `*`, `/`
    Multiplicative = 6,
}

/// Returns the precedence level for an arithmetic operator.
fn arithmetic_precedence(op: ArithmeticOp) -> Precedence {
    match op {
        ArithmeticOp::Plus | ArithmeticOp::Minus => Precedence::Additive,
        ArithmeticOp::Times | ArithmeticOp::Divide => Precedence::Multiplicative,
    }
}

/// Returns the precedence level for a comparison operator.
fn comparison_precedence(op: ComparisonOp) -> Precedence {
    match op {
        ComparisonOp::Equal | ComparisonOp::NotEqual => Precedence::Equality,
        _ => Precedence::Relational,
    }
}

/// Returns the precedence level for a logical operator.
fn logical_precedence(op: LogicalOp) -> Precedence {
    match op {
        LogicalOp::And => Precedence::And,
        LogicalOp::Or => Precedence::Or,
    }
}

/// Returns the precedence of an expression (for parenthesization decisions).
fn expr_precedence(expr: &Expression) -> Precedence {
    match expr {
        Expression::Arithmetic(e) => arithmetic_precedence(e.op),
        Expression::Comparison(e) => comparison_precedence(e.op),
        Expression::Logical(e) => logical_precedence(e.op),
        _ => Precedence::None,
    }
}

/// Checks if `child` needs parentheses when nested under a parent at `parent_prec`.
///
/// We need parens when:
/// 1. Child has lower precedence than parent
/// 2. Child has EQUAL precedence but is on the right side of a non-associative or
///    right-side of a left-associative operator where grouping matters
fn needs_parens(child: &Expression, parent_prec: Precedence, is_right: bool) -> bool {
    let child_prec = expr_precedence(child);
    if child_prec == Precedence::None {
        return false;
    }
    if child_prec < parent_prec {
        return true;
    }
    if child_prec == parent_prec && is_right {
        // Right-associativity check: `1 - (2 - 3)` needs parens on the right
        // But `1 + 2 + 3` doesn't (left-associative, chains naturally)
        // Division and subtraction are not right-associative
        if let Expression::Arithmetic(ae) = child {
            return matches!(ae.op, ArithmeticOp::Minus | ArithmeticOp::Divide)
                || matches!(
                    parent_prec,
                    Precedence::Multiplicative | Precedence::Additive
                );
        }
        // Comparison and logical should also get parens on the right at same level
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Expression composer
// ---------------------------------------------------------------------------

/// Writes an expression to the writer.
pub fn compose_expression(w: &mut IndentWriter, expr: &Expression) {
    compose_expression_prec(w, expr, Precedence::None, false);
}

/// Inner expression composer with precedence tracking.
fn compose_expression_prec(
    w: &mut IndentWriter,
    expr: &Expression,
    parent_prec: Precedence,
    is_right: bool,
) {
    let wrap = needs_parens(expr, parent_prec, is_right);
    if wrap {
        w.write("(");
    }

    match expr {
        Expression::Literal(lit) => compose_literal(w, lit),
        Expression::Variable(var) => compose_variable(w, var),
        Expression::Arithmetic(e) => compose_arithmetic(w, e),
        Expression::Comparison(e) => compose_comparison(w, e),
        Expression::Logical(e) => compose_logical(w, e),
        Expression::Bitwise(e) => compose_bitwise(w, e),
        Expression::Not(e) => compose_not(w, e),
        Expression::UnaryMinus(e) => compose_unary_minus(w, e),
        Expression::BitwiseNot(e) => compose_bitwise_not(w, e),
        Expression::FunctionApplication(e) => compose_function_application(w, e),
        Expression::ArrowFunction(e) => compose_arrow_function(w, e),
        Expression::MemberAccess(e) => compose_member_access(w, e),
        Expression::PackageableElementRef(e) => compose_element_ref(w, e),
        Expression::TypeReferenceExpr(e) => compose_type_reference_expr(w, e),
        Expression::Lambda(e) => compose_lambda(w, e),
        Expression::Let(e) => compose_let(w, e),
        Expression::Collection(e) => compose_collection(w, e),
        Expression::NewInstance(e) => compose_new_instance(w, e),
        Expression::Column(e) => compose_column(w, e),
        Expression::Island(e) => crate::island::compose_island(w, e),
        Expression::Group(inner) => {
            w.write("(");
            compose_expression_prec(w, inner, Precedence::None, false);
            w.write(")");
        }
    }

    if wrap {
        w.write(")");
    }
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

fn compose_literal(w: &mut IndentWriter, lit: &Literal) {
    match lit {
        Literal::Integer(i) => w.write(&i.value.to_string()),
        Literal::Float(f) => {
            let s = f.value.to_string();
            // Ensure float has a decimal point
            if s.contains('.') {
                w.write(&s);
            } else {
                w.write(&format!("{s}.0"));
            }
        }
        Literal::Decimal(d) => {
            w.write(&d.value);
            // Append 'D' suffix if not already present
            if !d.value.ends_with('D') && !d.value.ends_with('d') {
                w.write("D");
            }
        }
        Literal::String(s) => {
            w.write("'");
            w.write(&escape_pure_string(&s.value));
            w.write("'");
        }
        Literal::Boolean(b) => w.write(if b.value { "true" } else { "false" }),
        Literal::StrictDate(d) => {
            // %latest is a keyword, not a date literal
            if d.value == "%latest" {
                w.write("%latest");
            } else {
                w.write("%");
                w.write(&d.value);
            }
        }
        Literal::DateTime(d) => {
            w.write("%");
            w.write(&d.value);
        }
        Literal::StrictTime(t) => {
            w.write("%");
            w.write(&t.value);
        }
    }
}

fn compose_variable(w: &mut IndentWriter, var: &Variable) {
    w.write("$");
    w.write(&maybe_quote(&var.name));
}

// ---------------------------------------------------------------------------
// Binary operators
// ---------------------------------------------------------------------------

fn compose_arithmetic(w: &mut IndentWriter, e: &ArithmeticExpr) {
    let prec = arithmetic_precedence(e.op);
    compose_expression_prec(w, &e.left, prec, false);
    w.write(match e.op {
        ArithmeticOp::Plus => " + ",
        ArithmeticOp::Minus => " - ",
        ArithmeticOp::Times => " * ",
        ArithmeticOp::Divide => " / ",
    });
    compose_expression_prec(w, &e.right, prec, true);
}

fn compose_comparison(w: &mut IndentWriter, e: &ComparisonExpr) {
    let prec = comparison_precedence(e.op);
    compose_expression_prec(w, &e.left, prec, false);
    w.write(match e.op {
        ComparisonOp::Equal => " == ",
        ComparisonOp::NotEqual => " != ",
        ComparisonOp::LessThan => " < ",
        ComparisonOp::LessThanOrEqual => " <= ",
        ComparisonOp::GreaterThan => " > ",
        ComparisonOp::GreaterThanOrEqual => " >= ",
    });
    compose_expression_prec(w, &e.right, prec, true);
}

fn compose_logical(w: &mut IndentWriter, e: &LogicalExpr) {
    let prec = logical_precedence(e.op);
    compose_expression_prec(w, &e.left, prec, false);
    w.write(match e.op {
        LogicalOp::And => " && ",
        LogicalOp::Or => " || ",
    });
    compose_expression_prec(w, &e.right, prec, true);
}

fn compose_bitwise(w: &mut IndentWriter, e: &BitwiseExpr) {
    compose_expression(w, &e.left);
    w.write(match e.op {
        BitwiseOp::And => " &&& ",
        BitwiseOp::Or => " ||| ",
        BitwiseOp::Xor => " ^^^ ",
        BitwiseOp::ShiftLeft => " <<< ",
        BitwiseOp::ShiftRight => " >>> ",
    });
    compose_expression(w, &e.right);
}

// ---------------------------------------------------------------------------
// Unary operators
// ---------------------------------------------------------------------------

fn compose_not(w: &mut IndentWriter, e: &NotExpr) {
    w.write("!");
    compose_expression(w, &e.operand);
}

fn compose_unary_minus(w: &mut IndentWriter, e: &UnaryMinusExpr) {
    w.write("-");
    compose_expression(w, &e.operand);
}

fn compose_bitwise_not(w: &mut IndentWriter, e: &BitwiseNotExpr) {
    w.write("~~~");
    compose_expression(w, &e.operand);
}

// ---------------------------------------------------------------------------
// Function calls
// ---------------------------------------------------------------------------

fn compose_function_application(w: &mut IndentWriter, e: &FunctionApplication) {
    compose_element_ptr(w, &e.function);
    w.write("(");
    compose_function_args(w, &e.arguments);
    w.write(")");
}

fn compose_arrow_function(w: &mut IndentWriter, e: &ArrowFunction) {
    compose_expression(w, &e.target);
    w.write("->");
    compose_element_ptr(w, &e.function);
    w.write("(");
    compose_function_args(w, &e.arguments);
    w.write(")");
}

/// Composes a bare packageable element reference (no parens).
fn compose_element_ref(w: &mut IndentWriter, e: &PackageableElementRef) {
    compose_element_ptr(w, &e.element);
}

/// Composes function arguments as a comma-separated list.
fn compose_function_args(w: &mut IndentWriter, args: &[Expression]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_expression(w, arg);
    }
}

fn compose_member_access(w: &mut IndentWriter, e: &MemberAccess) {
    match e {
        MemberAccess::Simple(s) => {
            compose_expression(w, &s.target);
            w.write(".");
            w.write(&maybe_quote(&s.member));
        }
        MemberAccess::Qualified(q) => {
            compose_expression(w, &q.target);
            w.write(".");
            w.write(&maybe_quote(&q.member));
            w.write("(");
            for (i, arg) in q.arguments.iter().enumerate() {
                if i > 0 {
                    w.write(", ");
                }
                compose_expression(w, arg);
            }
            w.write(")");
        }
    }
}

fn compose_type_reference_expr(w: &mut IndentWriter, e: &TypeReferenceExpr) {
    w.write("@");
    crate::type_ref::compose_type_reference(w, &e.type_ref);
}

// ---------------------------------------------------------------------------
// Complex expressions
// ---------------------------------------------------------------------------

fn compose_lambda(w: &mut IndentWriter, e: &Lambda) {
    // Determine rendering form per Java grammar rules:
    //   - 0 untyped params, bare:   `|body`
    //   - 1 untyped param, bare:    `x|body`
    //   - Multi untyped, braced:    `{x, y|body}`
    //   - Any typed, braced:        `{x: Type[1]|body}`
    let all_untyped = e.parameters.iter().all(|p| p.type_ref.is_none());
    let needs_braces = !all_untyped || e.parameters.len() > 1;

    if needs_braces {
        w.write("{");
    }

    if e.parameters.is_empty() {
        // No-param: `|body`
        w.write("|");
    } else if all_untyped && e.parameters.len() == 1 {
        // Single untyped bare: `x|body`
        w.write(&maybe_quote(&e.parameters[0].name));
        w.write("|");
    } else {
        // Typed or multi-param (braced)
        for (i, p) in e.parameters.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            compose_parameter(w, p);
        }
        w.write("|");
    }

    // Body
    if e.body.len() > 1 {
        w.newline();
        for (i, expr) in e.body.iter().enumerate() {
            compose_expression(w, expr);
            if i < e.body.len() - 1 {
                w.write(";");
            }
            w.newline();
        }
    } else if let Some(expr) = e.body.first() {
        compose_expression(w, expr);
    }

    if needs_braces {
        w.write("}");
    }
}

fn compose_let(w: &mut IndentWriter, e: &LetExpr) {
    w.write("let ");
    w.write(&maybe_quote(&e.name));
    w.write(" = ");
    compose_expression(w, &e.value);
}

fn compose_collection(w: &mut IndentWriter, e: &CollectionExpr) {
    w.write("[");
    for (i, elem) in e.elements.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_expression(w, elem);
    }
    w.write("]");
}

fn compose_new_instance(w: &mut IndentWriter, e: &NewInstanceExpr) {
    w.write("^");
    compose_element_ptr(w, &e.class);
    w.write("(");
    for (i, kv) in e.assignments.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        w.write(&maybe_quote(&kv.key));
        w.write("=");
        compose_expression(w, &kv.value);
    }
    w.write(")");
}

fn compose_column(w: &mut IndentWriter, e: &ColumnExpression) {
    match e {
        ColumnExpression::Name(c) => {
            w.write("~");
            w.write(&maybe_quote(&c.name));
        }
        ColumnExpression::WithLambda(c) => {
            w.write("~");
            w.write(&maybe_quote(&c.name));
            w.write(": ");
            compose_lambda(w, &c.lambda);
        }
        ColumnExpression::Typed(c) => {
            w.write("~[");
            w.write(&maybe_quote(&c.name));
            w.write(": ");
            crate::type_ref::compose_type_reference(w, &c.type_ref);
            w.write("]");
        }
        ColumnExpression::WithFunction(c) => {
            w.write("~");
            w.write(&maybe_quote(&c.name));
            w.write(": ");
            compose_element_ptr(w, &c.function);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Writes a `PackageableElementPtr` as `pkg::name`.
pub fn compose_element_ptr(w: &mut IndentWriter, ptr: &PackageableElementPtr) {
    if let Some(pkg) = ptr.package() {
        compose_package(w, pkg);
        w.write("::");
    }
    w.write(&maybe_quote(&ptr.name));
}

/// Writes a `Package` path with quoting.
pub fn compose_package(w: &mut IndentWriter, pkg: &legend_pure_parser_ast::type_ref::Package) {
    let segments = pkg.segments();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            w.write("::");
        }
        w.write(&maybe_quote(seg));
    }
}

/// Writes a parameter: `name: Type[mult]` or just `name` for untyped lambda params.
pub fn compose_parameter(w: &mut IndentWriter, p: &Parameter) {
    w.write(&maybe_quote(&p.name));
    if let (Some(type_ref), Some(mult)) = (&p.type_ref, &p.multiplicity) {
        w.write(": ");
        crate::type_ref::compose_type_reference(w, type_ref);
        w.write(&mult.to_string());
    }
}

/// Writes a body expression list (e.g., for function or qualified property bodies).
///
/// Uses `;` as a **terminator** for statements in a body block.
///
/// When `terminate_last` is `false`, the last expression omits the trailing `;`
/// (used for function bodies where the last expression is the implicit return).
/// When `terminate_last` is `true`, ALL expressions get `;` (used for qualified
/// property bodies).
///
/// Callers are responsible for calling `push_indent()`/`pop_indent()` to set
/// the correct indentation level before and after this function.
pub fn compose_body(w: &mut IndentWriter, body: &[Expression], terminate_last: bool) {
    for (i, expr) in body.iter().enumerate() {
        compose_expression(w, expr);
        if terminate_last || i < body.len() - 1 {
            w.write(";");
        }
        w.newline();
    }
}
