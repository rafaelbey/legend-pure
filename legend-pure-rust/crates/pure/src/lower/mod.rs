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
use crate::resolve::ResolutionContext;
use crate::types::{ExprKind, ResolvedType, ValueSpec};

// Sub-module split: AST-kind processors live next to the data they
// transform. This is the lowering-side analog of the three-seam
// pattern in `crate::validate`. Adding a new kind = one match arm in
// `lower_expression` + one file under `lower/<kind>.rs`. See plan
// `~/.claude/plans/do-we-have-enought-quiet-swing.md` Step 4.
pub(super) mod collection;
pub(super) mod copy_slice;
pub(super) mod function_app;
pub(super) mod lambda;
pub(super) mod let_expr;
pub(super) mod literal;
pub(super) mod member_access;
pub(super) mod new_instance;
pub(super) mod operator;
pub(super) mod relation;
pub(super) mod type_ref;
pub(super) mod unit_navpath;

// Re-export so the crate-level path
// `crate::lower::build_packageable_element_ref` stays stable for the
// few external callers (Pass 2.5's `set_and_return`, the new-instance
// class-arg synthesizer) — they don't need to know which submodule
// owns the helper. Same treatment for
// `lower_lambda_with_expected_types`, which is reached from outside
// `lower/` (resolve.rs's lambda-param filler).
pub(crate) use lambda::lower_lambda_with_expected_types;
pub(crate) use type_ref::build_packageable_element_ref;

/// Convenience: wrap an `ExprKind` into a `ValueSpec` with no type info.
pub(super) fn untyped(kind: ExprKind, source_info: SourceInfo) -> ValueSpec {
    ValueSpec {
        kind: Box::new(kind),
        source_info,
        type_info: None,
    }
}

/// Convenience: wrap an `ExprKind` into a `ValueSpec` whose `type_info`
/// is pre-set at lowering time. Honoured by `set_and_return` in Pass 2.5,
/// matching the `lower_new_instance` pattern for parametric type capture.
pub(super) fn typed(kind: ExprKind, source_info: SourceInfo, ty: ResolvedType) -> ValueSpec {
    ValueSpec {
        kind: Box::new(kind),
        source_info,
        type_info: Some(Box::new(ty)),
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
        ast_expr::Expression::Literal(lit) => literal::lower_literal(lit),
        ast_expr::Expression::Variable(var) => Some(literal::lower_variable(var)),
        ast_expr::Expression::Collection(coll) => {
            Some(collection::lower_collection(coll, ctx, errors))
        }
        ast_expr::Expression::Group(inner) => lower_expression(inner, ctx, errors),

        // Phase 2 — Operators → FunctionCall
        ast_expr::Expression::Arithmetic(e) => operator::lower_arithmetic(e, ctx, errors),
        ast_expr::Expression::Comparison(e) => operator::lower_comparison(e, ctx, errors),
        ast_expr::Expression::Logical(e) => operator::lower_logical(e, ctx, errors),
        ast_expr::Expression::Bitwise(e) => operator::lower_bitwise(e, ctx, errors),
        ast_expr::Expression::Not(e) => operator::lower_unary_not(e, ctx, errors),
        ast_expr::Expression::UnaryMinus(e) => operator::lower_unary_minus(e, ctx, errors),
        ast_expr::Expression::BitwiseNot(e) => operator::lower_bitwise_not(e, ctx, errors),

        // Phase 2 — Function & member access
        ast_expr::Expression::FunctionApplication(e) => {
            function_app::lower_function_application(e, ctx, errors)
        }
        ast_expr::Expression::ArrowFunction(e) => {
            Some(function_app::lower_arrow_function(e, ctx, errors))
        }
        ast_expr::Expression::MemberAccess(e) => member_access::lower_member_access(e, ctx, errors),
        ast_expr::Expression::TypeReferenceExpr(e) => {
            type_ref::lower_type_reference(e, ctx, errors)
        }
        ast_expr::Expression::PackageableElementRef(e) => {
            type_ref::lower_packageable_element_ref(e, ctx, errors)
        }

        // Phase 3 — Lambda, Let, New, Column, Island
        ast_expr::Expression::Lambda(e) => lambda::lower_lambda(e, ctx, errors),
        ast_expr::Expression::Let(e) => let_expr::lower_let(e, ctx, errors),
        ast_expr::Expression::NewInstance(e) => new_instance::lower_new_instance(e, ctx, errors),
        ast_expr::Expression::Column(e) => relation::lower_column(e, ctx, errors),
        ast_expr::Expression::Island(island) => {
            // Dispatch to a registered IslandLowerer (one per DSL crate
            // that owns an island grammar). The lowerer returns a
            // synthetic AST expression we recurse on — keeping this
            // crate ignorant of any DSL's content shape. Callers that
            // didn't register lowerers (empty slice) still get the
            // legacy "Island expression lowering not yet implemented"
            // diagnostic so untriaged islands stay visible.
            if let Some(synthetic) =
                crate::island_lower::dispatch_island_lower(island, ctx.island_lowerers)
            {
                lower_expression(&synthetic, ctx, errors)
            } else {
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
        ast_expr::Expression::Copy(e) => Some(copy_slice::lower_copy(e, ctx, errors)),
        ast_expr::Expression::Slice(e) => copy_slice::lower_slice(e, ctx, errors),
        ast_expr::Expression::UnitInstance(e) => unit_navpath::lower_unit_instance(e, ctx, errors),
        ast_expr::Expression::NavigationPath(e) => {
            unit_navpath::lower_navigation_path(e, ctx, errors)
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

// `lower_literal`, `lower_variable`, and the date-literal parsing
// helpers (`parse_strict_date`, `parse_datetime`, `parse_strict_time`,
// `split_tz`, `parse_subsecond_parts`) live in `lower/literal.rs`.

// `lower_collection` lives in `lower/collection.rs`.

// `binary_op`, `unary_op`, `variadic_op`, `synthetic_unqualified_ptr`,
// `lower_arithmetic`, `lower_comparison`, `lower_logical`,
// `lower_bitwise`, `lower_unary_not`, `lower_unary_minus`, and
// `lower_bitwise_not` live in `lower/operator.rs`.

// ---------------------------------------------------------------------------
// Function application & arrow
// ---------------------------------------------------------------------------

// `lower_function_application`, `lower_arrow_function`,
// `lower_args_with_lambda_inference`, `lower_qp_call_args`,
// `unwrap_group`, `candidates_by_arity`, `slots_with_placeholders`, and
// `find_qp_params_for_arity` live in `lower/function_app.rs`.
// `lower_qp_call_args` is reached via `super::function_app::*` from
// `lower/member_access.rs` for the QP-call dispatch.

// `compute_lambda_param_expectations` and `expectations_from_callee_params`
// live in `crate::inference::lambda` (Step 3e). Their call sites above
// reach in via the fully-qualified path; the type alias
// `LambdaExpectations` is re-aliased locally for readability.

// `lower_member_access` and `desugar_all_to_getall` live in
// `lower/member_access.rs`.

// `lower_type_reference`, `lower_packageable_element_ref`, and
// `build_packageable_element_ref` live in `lower/type_ref.rs`. The
// last is re-exported above so its `pub(crate)` path
// (`crate::lower::build_packageable_element_ref`) stays stable.

// `lower_lambda`, `lower_lambda_with_expected_types`,
// `lower_lambda_parameters`, `format_uninferred_lambda_message`, and
// `is_concrete_type` live in `lower/lambda.rs`.
// `lower_lambda_with_expected_types` is re-exported above so its
// `pub(crate)` path stays stable.

// ---------------------------------------------------------------------------
// Let
// ---------------------------------------------------------------------------

// `lower_let` and `infer_let_type` live in `lower/let_expr.rs`.

// `lower_new_instance` lives in `lower/new_instance.rs`.

// `lower_copy` and `lower_slice` live in `lower/copy_slice.rs`.

// `lower_column`, `classify_col_spec_kind`, `lower_relation_columns`,
// `lower_relation_columns_from_specs`, `resolve_relation_type_id`,
// `resolve_col_spec_array_id`, `resolve_col_spec_id`, and
// `resolve_relation_class_id` all live in `lower/relation.rs`.

// `lower_unit_instance` and `lower_navigation_path` live in
// `lower/unit_navpath.rs`. Date-literal parsing tests live in
// `lower/literal.rs` next to the helpers they exercise.
