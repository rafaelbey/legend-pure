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
//! ## Coverage
//!
//! - **Phase 1**: Literals, Variables, Collections, Groups
//! - **Phase 2**: Operators (→ `FunctionCall`), Function Application, Arrow,
//!   Member Access, Type References, Packageable Element Refs
//! - **Phase 3**: Lambda, Let (→ `FunctionCall("letFunction")`),
//!   New Instance (→ `FunctionCall("new")`), Column (placeholder)
//! - **Phase 4**: Copy (→ `FunctionCall("copy")`),
//!   Slice (→ `FunctionCall("range")`)
//!
//! Island expressions produce a diagnostic — full lowering is deferred.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::{SourceInfo, Spanned};
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{DateValue, ExprKind, ValueSpec};

/// Convenience: wrap an `ExprKind` into a `ValueSpec` with no type info.
fn untyped(kind: ExprKind, source_info: SourceInfo) -> ValueSpec {
    ValueSpec {
        kind: Box::new(kind),
        source_info,
        type_info: None,
    }
}

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
        ast_expr::Expression::Copy(e) => lower_copy(e, ctx, errors),
        ast_expr::Expression::Slice(e) => lower_slice(e, ctx, errors),
        ast_expr::Expression::UnitInstance(e) => lower_unit_instance(e, ctx, errors),
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
        ast_expr::Literal::Integer(i) => Some(untyped(
            ExprKind::IntegerLiteral(i.value),
            i.source_info.clone(),
        )),
        ast_expr::Literal::Float(f) => Some(untyped(
            ExprKind::FloatLiteral(f.value),
            f.source_info.clone(),
        )),
        ast_expr::Literal::Decimal(d) => {
            let decimal = d.value.parse::<rust_decimal::Decimal>().ok()?;
            Some(untyped(
                ExprKind::DecimalLiteral(decimal),
                d.source_info.clone(),
            ))
        }
        ast_expr::Literal::String(s) => Some(untyped(
            ExprKind::StringLiteral(SmolStr::new(&s.value)),
            s.source_info.clone(),
        )),
        ast_expr::Literal::Boolean(b) => Some(untyped(
            ExprKind::BooleanLiteral(b.value),
            b.source_info.clone(),
        )),
        ast_expr::Literal::StrictDate(d) => {
            let dv = parse_strict_date(&d.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), d.source_info.clone()))
        }
        ast_expr::Literal::DateTime(d) => {
            let dv = parse_datetime(&d.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), d.source_info.clone()))
        }
        ast_expr::Literal::StrictTime(t) => {
            let dv = parse_strict_time(&t.value)?;
            Some(untyped(ExprKind::DateLiteral(dv), t.source_info.clone()))
        }
    }
}

// ---------------------------------------------------------------------------
// Variable lowering
// ---------------------------------------------------------------------------

/// Lowers a variable reference `$name`.
fn lower_variable(var: &ast_expr::Variable) -> ValueSpec {
    untyped(
        ExprKind::Variable {
            name: var.name.clone(),
        },
        var.source_info.clone(),
    )
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
    untyped(ExprKind::Collection { elements }, coll.source_info.clone())
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
    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new(name),
            arguments: vec![l, r],
        },
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
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new(name),
            arguments: vec![inner],
        },
        source_info.clone(),
    ))
}

/// Lowers arithmetic: `a + b` → `FunctionCall("plus", [a, b])`.
fn lower_arithmetic(
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
/// For `plus`, when either operand is statically recognisable as a
/// String (literal or an already-resolved `plus_String_*` subtree),
/// emit the exact mangled FQN `plus_String_MANY__String_1_` so the
/// runtime dispatches directly to `StringPlus` instead of falling
/// through the prefix fallback to the numeric variant. Until Pass 2
/// type inference is wired into operator lowering this is our
/// compile-time hook for `'prefix' + $x` chains.
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
    let resolved_name: SmolStr =
        if name == "plus" && (is_string_typed_spec(&l.kind) || is_string_typed_spec(&r.kind)) {
            SmolStr::new_static("plus_String_MANY__String_1_")
        } else {
            SmolStr::new(name)
        };
    let collection = untyped(
        ExprKind::Collection {
            elements: vec![l, r],
        },
        source_info.clone(),
    );
    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: resolved_name,
            arguments: vec![collection],
        },
        source_info.clone(),
    ))
}

/// Static hint: is this lowered `ExprKind` producing a String at runtime?
/// Recognises string literals and nested `plus_String_*` calls so a chain
/// like `'a' + $b + $c` propagates the string dispatch through every
/// nested `+`.
fn is_string_typed_spec(kind: &ExprKind) -> bool {
    match kind {
        ExprKind::StringLiteral(_) => true,
        ExprKind::FunctionCall { function_name, .. } => {
            function_name.as_str() == "plus_String_MANY__String_1_"
        }
        _ => false,
    }
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
        Some(untyped(
            ExprKind::FunctionCall {
                function: None,
                function_name: SmolStr::new_static("not"),
                arguments: vec![inner],
            },
            e.source_info.clone(),
        ))
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

/// Lowers `-expr` → `FunctionCall("minus", [Collection([expr])])`.
///
/// Mirrors the variadic lowering used for `a - b` — `minus` has the single
/// signature `(Number[*]):Number[1]`, so even the unary form feeds a
/// singleton collection. Java Pure's `Minus.execute` handles `size == 1`
/// as unary negate (`0 - x`); our native does the same.
fn lower_unary_minus(
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
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new("minus"),
            arguments: vec![collection],
        },
        e.source_info.clone(),
    ))
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
    // Lower arguments FIRST — their compiled types drive dispatch
    let arguments: Vec<ValueSpec> = e
        .arguments
        .iter()
        .filter_map(|a| lower_expression(a, ctx, errors))
        .collect();

    let function_id = resolve::resolve_function_call(
        &e.function,
        e.arguments.len(),
        &arguments,
        &e.source_info,
        ctx,
        errors,
    );

    Some(untyped(
        ExprKind::FunctionCall {
            function: function_id,
            function_name: SmolStr::new(e.function.name.as_str()),
            arguments,
        },
        e.source_info.clone(),
    ))
}

/// Lowers `expr->func(args)` → `FunctionCall` with target prepended.
///
/// `$x->filter(p)` becomes `FunctionCall("filter", [$x, p])`.
fn lower_arrow_function(
    e: &ast_expr::ArrowFunction,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    // Lower target and arguments FIRST — their compiled types drive dispatch
    let target = lower_expression(&e.target, ctx, errors)?;

    let mut arguments = Vec::with_capacity(1 + e.arguments.len());
    arguments.push(target);
    for arg in &e.arguments {
        if let Some(lowered) = lower_expression(arg, ctx, errors) {
            arguments.push(lowered);
        }
    }

    // AST arg count = target + explicit args
    let function_id = resolve::resolve_function_call(
        &e.function,
        1 + e.arguments.len(),
        &arguments,
        &e.source_info,
        ctx,
        errors,
    );

    Some(untyped(
        ExprKind::FunctionCall {
            function: function_id,
            function_name: SmolStr::new(e.function.name.as_str()),
            arguments,
        },
        e.source_info.clone(),
    ))
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
            Some(untyped(
                ExprKind::PropertyAccess {
                    target: Box::new(target),
                    property: SmolStr::new(s.member.as_str()),
                },
                s.source_info.clone(),
            ))
        }
        ast_expr::MemberAccess::Qualified(q) => {
            let target = lower_expression(&q.target, ctx, errors)?;
            let arguments: Vec<ValueSpec> = q
                .arguments
                .iter()
                .filter_map(|a| lower_expression(a, ctx, errors))
                .collect();
            Some(untyped(
                ExprKind::QualifiedPropertyAccess {
                    target: Box::new(target),
                    property: SmolStr::new(q.member.as_str()),
                    arguments,
                },
                q.source_info.clone(),
            ))
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
    let type_expr = resolve::resolve_type_spec(&e.type_ref, ctx, errors)?;
    Some(untyped(
        ExprKind::TypeReference { type_expr },
        e.source_info.clone(),
    ))
}

/// Lowers a bare element reference: `String`, `my::Enum` → `PackageableElementRef`.
fn lower_packageable_element_ref(
    e: &ast_expr::PackageableElementRef,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let element_id = resolve::resolve_element_ptr(&e.element, &e.source_info, ctx, errors)?;
    Some(untyped(
        ExprKind::PackageableElementRef {
            element: element_id,
        },
        e.source_info.clone(),
    ))
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

    // Save outer variable scope, register lambda params
    let outer_vars = ctx.variable_types.clone();
    for param in &parameters {
        ctx.variable_types.insert(
            param.name.clone(),
            (param.type_expr.clone(), param.multiplicity.clone()),
        );
    }

    let body = lower_expression_body(&e.body, ctx, errors);

    // Restore outer scope (lambda params don't leak)
    ctx.variable_types = outer_vars;

    Some(untyped(
        ExprKind::Lambda { parameters, body },
        e.source_info.clone(),
    ))
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

    // Register the variable type for downstream dispatch
    let var_type = infer_let_type(&value, ctx);
    if let Some(vt) = var_type {
        ctx.variable_types.insert(SmolStr::new(e.name.as_str()), vt);
    }

    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("letFunction"),
            arguments: vec![
                untyped(
                    ExprKind::StringLiteral(SmolStr::new(e.name.as_str())),
                    e.source_info.clone(),
                ),
                value,
            ],
        },
        e.source_info.clone(),
    ))
}

/// Infers the type and multiplicity of a let-bound value for variable tracking.
fn infer_let_type(
    value: &ValueSpec,
    ctx: &ResolutionContext<'_>,
) -> Option<(crate::types::TypeExpr, crate::types::Multiplicity)> {
    use crate::bootstrap;
    use crate::types::{ExprKind, Multiplicity, TypeExpr};

    let named = |eid: crate::ids::ElementId| TypeExpr::Named {
        element: eid,
        type_arguments: vec![],
        value_arguments: vec![],
    };

    match value.kind.as_ref() {
        ExprKind::IntegerLiteral(_) => Some((named(bootstrap::INTEGER_ID), Multiplicity::PureOne)),
        ExprKind::FloatLiteral(_) => Some((named(bootstrap::FLOAT_ID), Multiplicity::PureOne)),
        ExprKind::DecimalLiteral(_) => Some((named(bootstrap::DECIMAL_ID), Multiplicity::PureOne)),
        ExprKind::StringLiteral(_) => Some((named(bootstrap::STRING_ID), Multiplicity::PureOne)),
        ExprKind::BooleanLiteral(_) => Some((named(bootstrap::BOOLEAN_ID), Multiplicity::PureOne)),
        ExprKind::FunctionCall {
            function,
            arguments,
            ..
        } => function.and_then(|fid| {
            if let crate::model::Element::Function(f) = ctx.model.get_element(fid) {
                // Bind generic type/multiplicity variables from the call
                // arguments, then substitute into the declared return type
                // and multiplicity. This turns `cast<T|m>(x, @Class<Any>)`
                // from `(T, m)` into `(Class<Any>, [1])`.
                let bindings = crate::resolve::infer_generic_bindings(
                    &f.parameters,
                    arguments,
                    ctx.model,
                    &ctx.variable_types,
                );
                Some((
                    crate::resolve::substitute_type(&f.return_type, &bindings.ty),
                    crate::resolve::substitute_mult(&f.return_multiplicity, &bindings.mult),
                ))
            } else {
                None
            }
        }),
        ExprKind::Variable { name } => ctx.variable_types.get(name).cloned(),
        ExprKind::Collection { elements } => {
            // Compute the LUB (least upper bound) of ALL element types.
            // e.g., [1, 2, 5] → Integer, [1, 2.5] → Number, ['a', 'b'] → String
            let mut lub_type: Option<TypeExpr> = None;
            for elem in elements {
                if let Some((te, _)) = infer_let_type(elem, ctx) {
                    lub_type = Some(match lub_type {
                        None => te,
                        Some(current) => {
                            // Compute LUB at TypeExpr level — extract ElementIds and
                            // find the common supertype.
                            if let (
                                TypeExpr::Named { element: a, .. },
                                TypeExpr::Named { element: b, .. },
                            ) = (&current, &te)
                            {
                                let lub_id =
                                    crate::resolve::least_upper_bound_ids(*a, *b, ctx.model);
                                TypeExpr::Named {
                                    element: lub_id,
                                    type_arguments: vec![],
                                    value_arguments: vec![],
                                }
                            } else {
                                // Mixed or non-Named types — fall back to current
                                current
                            }
                        }
                    });
                }
            }

            // Multiplicity is derived from the element count:
            // [1,2,5] → [3], [x] → [1], [] → [0]
            let n = elements.len() as u32;
            let mult = match n {
                0 => Multiplicity::Range {
                    lower: 0,
                    upper: Some(0),
                },
                1 => Multiplicity::PureOne,
                _ => Multiplicity::Range {
                    lower: n,
                    upper: Some(n),
                },
            };

            lub_type.map(|te| (te, mult))
        }
        ExprKind::PackageableElementRef { element } => {
            // Bare element ref: `let c = ClassWithDefault` — the variable
            // holds a reference to the metaclass. Share the metatype lookup
            // with `infer_type_from_valuespec` via bootstrap::metatype_of
            // so new element kinds don't silently diverge between the two.
            crate::bootstrap::metatype_of(ctx.model, ctx.model.get_element(*element))
                .map(|eid| (named(eid), Multiplicity::PureOne))
        }
        _ => None,
    }
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
    arguments.push(untyped(
        ExprKind::PackageableElementRef { element: class_id },
        e.source_info.clone(),
    ));
    arguments.push(untyped(
        ExprKind::StringLiteral(SmolStr::new(e.class.name.as_str())),
        e.source_info.clone(),
    ));
    for kv in &e.assignments {
        arguments.push(untyped(
            ExprKind::StringLiteral(SmolStr::new(kv.key.as_str())),
            kv.source_info.clone(),
        ));
        if let Some(val) = lower_expression(&kv.value, ctx, errors) {
            arguments.push(val);
        }
    }

    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("new"),
            arguments,
        },
        e.source_info.clone(),
    ))
}

// ---------------------------------------------------------------------------
// Copy expression
// ---------------------------------------------------------------------------

/// Lowers `^$source(prop='val')` → `FunctionCall("copy", [$source, key1, val1, ...])`.
///
/// Matches the Java M3 desugaring: the source variable becomes the first
/// argument, followed by alternating string-name/value pairs for each
/// property override.
fn lower_copy(
    e: &ast_expr::CopyExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let source_var = untyped(
        ExprKind::Variable {
            name: e.source.clone(),
        },
        e.source_info.clone(),
    );

    let mut arguments = Vec::with_capacity(1 + e.assignments.len() * 2);
    arguments.push(source_var);

    for kv in &e.assignments {
        arguments.push(untyped(
            ExprKind::StringLiteral(SmolStr::new(kv.key.as_str())),
            kv.source_info.clone(),
        ));
        if let Some(val) = lower_expression(&kv.value, ctx, errors) {
            arguments.push(val);
        }
    }

    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("copy"),
            arguments,
        },
        e.source_info.clone(),
    ))
}

// ---------------------------------------------------------------------------
// Slice expression
// ---------------------------------------------------------------------------

/// Lowers `[start:stop]` or `[start:stop:step]` → `FunctionCall("range", args)`.
///
/// Matches the Java M3 desugaring of slice (subscript) expressions to
/// `range(start, stop)` or `range(start, stop, step)`. Missing `start`
/// defaults to integer literal `0`.
fn lower_slice(
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
    if let Some(ref step) = e.step {
        if let Some(step_val) = lower_expression(step, ctx, errors) {
            arguments.push(step_val);
        }
    }

    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("range"),
            arguments,
        },
        e.source_info.clone(),
    ))
}

// ---------------------------------------------------------------------------
// Column (TDS — placeholder)
// ---------------------------------------------------------------------------

/// Lowers a column expression to a placeholder `ValueSpec::Column`.
///
/// Full TDS column lowering is deferred — the source info is preserved
/// for diagnostics and protocol output.
fn lower_column(e: &ast_expr::ColumnBuilderExpr) -> ValueSpec {
    untyped(ExprKind::Column, e.source_info.clone())
}

// ---------------------------------------------------------------------------
// Date parsing helpers
// ---------------------------------------------------------------------------

/// Parses `"2024-01-15"`, `"2024-01"`, or `"2024"` →
/// `DateValue::StrictDate` with appropriate precision (`month` / `day`
/// are `None` when the corresponding segment is missing).
fn parse_strict_date(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let parts: Vec<&str> = s.split('-').collect();
    match parts.len() {
        1 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: None,
            day: None,
        }),
        2 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: Some(parts[1].parse().ok()?),
            day: None,
        }),
        3 => Some(DateValue::StrictDate {
            year: parts[0].parse().ok()?,
            month: Some(parts[1].parse().ok()?),
            day: Some(parts[2].parse().ok()?),
        }),
        _ => None,
    }
}

/// Parses `"2024-01-15T10:30:00"` (or with subseconds) → `DateValue::DateTime`.
fn parse_datetime(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (date_part, time_part) = s.split_once('T')?;
    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }

    // Split the time component into `main_HH:MM[:SS]`, optional `.frac`,
    // and optional `±HHMM` TZ marker — each may be present or absent.
    let (time_body, tz_offset_minutes) = split_tz(time_part);
    let (time_main, subsec_str) = match time_body.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (time_body, ""),
    };

    let time_parts: Vec<&str> = time_main.split(':').collect();
    if time_parts.len() < 2 {
        return None;
    }

    let (nanos, digits) = parse_subsecond_parts(subsec_str);
    let has_seconds = time_parts.len() >= 3;

    Some(DateValue::DateTime {
        year: date_parts[0].parse().ok()?,
        month: date_parts[1].parse().ok()?,
        day: date_parts[2].parse().ok()?,
        hour: time_parts[0].parse().ok()?,
        minute: time_parts[1].parse().ok()?,
        second: time_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
        subsecond_digits: digits,
        has_seconds,
        tz_offset_minutes,
    })
}

/// Parses `"10:30:00"` → `DateValue::StrictTime`.
fn parse_strict_time(s: &str) -> Option<DateValue> {
    let s = s.strip_prefix('%').unwrap_or(s);
    let (time_body, _tz) = split_tz(s);
    let (time_main, subsec_str) = match time_body.split_once('.') {
        Some((main, frac)) => (main, frac),
        None => (time_body, ""),
    };
    let parts: Vec<&str> = time_main.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let (nanos, digits) = parse_subsecond_parts(subsec_str);
    Some(DateValue::StrictTime {
        hour: parts[0].parse().ok()?,
        minute: parts[1].parse().ok()?,
        second: parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
        subsecond_nanos: nanos,
        subsecond_digits: digits,
    })
}

/// Split a time body on its trailing `±HHMM` offset marker.
///
/// Returns `(body_without_tz, Some(offset_minutes))` when a marker is
/// present, else `(body, None)`. The `-` / `+` matching targets the LAST
/// occurrence so `-0500` doesn't get confused with fractional-second
/// separators (`.` takes precedence).
fn split_tz(s: &str) -> (&str, Option<i16>) {
    // Find the last `+` or `-` that has exactly 4 trailing digits (HHMM).
    for (idx, _) in s.char_indices().rev() {
        let byte = s.as_bytes()[idx];
        if byte == b'+' || byte == b'-' {
            let tail = &s[idx..];
            if tail.len() == 5 && tail[1..].as_bytes().iter().all(u8::is_ascii_digit) {
                let sign: i16 = if byte == b'+' { 1 } else { -1 };
                let hh: i16 = tail[1..3].parse().unwrap_or(0);
                let mm: i16 = tail[3..5].parse().unwrap_or(0);
                return (&s[..idx], Some(sign * (hh * 60 + mm)));
            }
        }
    }
    (s, None)
}

/// Split a fractional-second string into `(nanoseconds, digits_present)`.
///
/// `digits_present` counts the source digits (1–9, capped at 9). `0`
/// means no fractional component was supplied. The nanos value is
/// padded to 9 digits on the right so 3-digit `.352` becomes
/// `352_000_000` nanoseconds.
fn parse_subsecond_parts(frac: &str) -> (i32, u8) {
    if frac.is_empty() {
        return (0, 0);
    }
    let trimmed: String = frac.chars().take(9).collect();
    let digits: u8 = trimmed.len() as u8;
    let mut padded = String::with_capacity(9);
    padded.push_str(&trimmed);
    while padded.len() < 9 {
        padded.push('0');
    }
    (padded.parse().unwrap_or(0), digits)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lowers a unit instance expression (`5 RomanLength~Pes`) to a `newUnit` call.
///
/// In the Pure semantic model, `5 RomanLength~Pes` desugars to
/// `newUnit(RomanLength~Pes, 5)`.
fn lower_unit_instance(
    e: &ast_expr::UnitInstanceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let value_vs = lower_expression(&e.value, ctx, errors)?;
    let unit_ref = lower_packageable_element_ref(
        &ast_expr::PackageableElementRef {
            element: e.unit.clone(),
            source_info: e.source_info.clone(),
        },
        ctx,
        errors,
    )?;

    Some(untyped(
        ExprKind::FunctionCall {
            function: None,
            function_name: SmolStr::new_static("newUnit"),
            arguments: vec![unit_ref, value_vs],
        },
        e.source_info.clone(),
    ))
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
                month: Some(1),
                day: Some(15)
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
                month: Some(3),
                day: Some(20)
            }
        );
    }

    #[test]
    fn parse_strict_date_year_only() {
        let dv = parse_strict_date("%2024").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: None,
                day: None,
            }
        );
    }

    #[test]
    fn parse_strict_date_year_month() {
        let dv = parse_strict_date("%2024-03").unwrap();
        assert_eq!(
            dv,
            DateValue::StrictDate {
                year: 2024,
                month: Some(3),
                day: None,
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
                subsecond_nanos: 0,
                subsecond_digits: 0,
                has_seconds: true,
                tz_offset_minutes: None,
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
                subsecond_nanos: 123_000_000,
                subsecond_digits: 3,
                has_seconds: true,
                tz_offset_minutes: None,
            }
        );
    }

    #[test]
    fn parse_datetime_with_tz() {
        let dv = parse_datetime("%2024-01-15T10:30:45.352-0500").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2024,
                month: 1,
                day: 15,
                hour: 10,
                minute: 30,
                second: 45,
                subsecond_nanos: 352_000_000,
                subsecond_digits: 3,
                has_seconds: true,
                tz_offset_minutes: Some(-300),
            }
        );
    }

    #[test]
    fn parse_datetime_minute_only() {
        let dv = parse_datetime("%2014-1-1T0:00+0000").unwrap();
        assert_eq!(
            dv,
            DateValue::DateTime {
                year: 2014,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                subsecond_nanos: 0,
                subsecond_digits: 0,
                has_seconds: false,
                tz_offset_minutes: Some(0),
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
                subsecond_nanos: 0,
                subsecond_digits: 0,
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
                subsecond_nanos: 500_000_000,
                subsecond_digits: 1,
            }
        );
    }

    #[test]
    fn parse_subsecond_parts_padding() {
        assert_eq!(parse_subsecond_parts("1"), (100_000_000, 1));
        assert_eq!(parse_subsecond_parts("12"), (120_000_000, 2));
        assert_eq!(parse_subsecond_parts("123"), (123_000_000, 3));
        assert_eq!(parse_subsecond_parts("123456789"), (123_456_789, 9));
        assert_eq!(parse_subsecond_parts(""), (0, 0));
    }
}
