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
use legend_pure_parser_ast::type_ref as ast_type;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{
    ExprKind, FunctionCallData, Multiplicity, RelationColumnLowered, ResolvedType, TypeExpr,
    ValueSpec,
};

// Sub-module split: AST-kind processors live next to the data they
// transform. This is the lowering-side analog of the three-seam
// pattern in `crate::validate`. Adding a new kind = one match arm in
// `lower_expression` + one file under `lower/<kind>.rs`. See plan
// `~/.claude/plans/do-we-have-enought-quiet-swing.md` Step 4.
pub(super) mod collection;
pub(super) mod literal;
pub(super) mod member_access;
pub(super) mod operator;
pub(super) mod type_ref;

// Re-export so the crate-level path
// `crate::lower::build_packageable_element_ref` stays stable for the
// few external callers (Pass 2.5's `set_and_return`, the new-instance
// class-arg synthesizer) — they don't need to know which submodule
// owns the helper.
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
        ast_expr::Expression::FunctionApplication(e) => lower_function_application(e, ctx, errors),
        ast_expr::Expression::ArrowFunction(e) => Some(lower_arrow_function(e, ctx, errors)),
        ast_expr::Expression::MemberAccess(e) => member_access::lower_member_access(e, ctx, errors),
        ast_expr::Expression::TypeReferenceExpr(e) => type_ref::lower_type_reference(e, ctx, errors),
        ast_expr::Expression::PackageableElementRef(e) => {
            type_ref::lower_packageable_element_ref(e, ctx, errors)
        }

        // Phase 3 — Lambda, Let, New, Column, Island
        ast_expr::Expression::Lambda(e) => lower_lambda(e, ctx, errors),
        ast_expr::Expression::Let(e) => lower_let(e, ctx, errors),
        ast_expr::Expression::NewInstance(e) => lower_new_instance(e, ctx, errors),
        ast_expr::Expression::Column(e) => lower_column(e, ctx, errors),
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
        ast_expr::Expression::Copy(e) => Some(lower_copy(e, ctx, errors)),
        ast_expr::Expression::Slice(e) => lower_slice(e, ctx, errors),
        ast_expr::Expression::UnitInstance(e) => lower_unit_instance(e, ctx, errors),
        ast_expr::Expression::NavigationPath(e) => lower_navigation_path(e, ctx, errors),
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

/// Lowers `func(args)` → `FunctionCall`.
///
/// Resolves the function name through import-aware resolution. If resolution
/// fails, the call is still produced with `function: None` so downstream
/// code can see the structure.
///
/// Two-phase argument lowering: when the call has lambda arguments, we
/// pre-lower the non-lambda args first, then attempt to bind the call's
/// generic type/multiplicity variables from those concrete args (see
/// [`lower_args_with_lambda_inference`]). The lambda is then lowered with
/// expected parameter types so its body's dispatch sees the right types.
#[allow(clippy::unnecessary_wraps)] // consistent signature with other lower_* fns
fn lower_function_application(
    e: &ast_expr::FunctionApplication,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let arguments = lower_args_with_lambda_inference(
        &e.function,
        &e.arguments,
        None,
        e.arguments.len(),
        ctx,
        errors,
    );

    let function_id = resolve::resolve_function_call(
        &e.function,
        e.arguments.len(),
        &arguments,
        &e.source_info,
        ctx,
        errors,
    );

    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: function_id,
            function_name: SmolStr::new(e.function.name.as_str()),
            arguments,
        }),
        e.source_info.clone(),
    ))
}

/// Lowers `expr->func(args)` → `FunctionCall` with target prepended.
///
/// `$x->filter(p)` becomes `FunctionCall("filter", [$x, p])`.
///
/// Like [`lower_function_application`], uses two-phase lowering to feed
/// concrete call-site argument types into otherwise-untyped lambda params —
/// including when the arrow target *itself* is a lambda
/// (e.g. `{x, y | $x + $y}->eval('1', '2')`).
fn lower_arrow_function(
    e: &ast_expr::ArrowFunction,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> ValueSpec {
    // Treat the target as the first argument so it participates in the
    // same two-phase inference as the explicit args. If it's a lambda, it
    // will be deferred until the explicit args have been lowered.
    let mut all_args: Vec<ast_expr::Expression> = Vec::with_capacity(1 + e.arguments.len());
    all_args.push(*e.target.clone());
    all_args.extend(e.arguments.iter().cloned());

    let arguments = lower_args_with_lambda_inference(
        &e.function,
        &all_args,
        None,
        1 + e.arguments.len(),
        ctx,
        errors,
    );

    // AST arg count = target + explicit args
    let function_id = resolve::resolve_function_call(
        &e.function,
        1 + e.arguments.len(),
        &arguments,
        &e.source_info,
        ctx,
        errors,
    );

    untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: function_id,
            function_name: SmolStr::new(e.function.name.as_str()),
            arguments,
        }),
        e.source_info.clone(),
    )
}

/// Two-phase argument lowering with lambda-parameter type inference.
///
/// Phase 1: lower every non-lambda arg position (preserving slot indices via
/// `Option`s).
///
/// Phase 2: try to resolve the callee by `name + arity` alone; if exactly one
/// candidate matches, use its declared parameter list to infer the call's
/// generic type/multiplicity variables from the lowered phase-1 args, then
/// for each lambda slot extract the expected `FunctionType` (substituting the
/// inferred bindings) and lower the lambda with those expected param types.
/// If no unique candidate is found, lambdas fall back to the original
/// (untyped → `Any`) lowering.
///
/// `prepended` is the arrow-function target: it occupies position 0 and is
/// already lowered (or absent for plain function applications).
type LambdaExpectations =
    Vec<Option<Vec<Option<(crate::types::TypeExpr, crate::types::Multiplicity)>>>>;

fn lower_args_with_lambda_inference(
    function_ptr: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    ast_args: &[ast_expr::Expression],
    prepended: Option<ValueSpec>,
    total_arity: usize,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<ValueSpec> {
    // Slot 0 is the prepended target (for arrow); explicit args follow.
    let prepend_offset = usize::from(prepended.is_some());
    let mut slots: Vec<Option<ValueSpec>> = Vec::with_capacity(total_arity);
    if let Some(t) = prepended {
        slots.push(Some(t));
    }

    // Phase 1: lower every non-lambda explicit arg, leaving lambda slots empty.
    for ast_arg in ast_args {
        let underlying = unwrap_group(ast_arg);
        if matches!(underlying, ast_expr::Expression::Lambda(_)) {
            slots.push(None);
        } else {
            slots.push(lower_expression(ast_arg, ctx, errors));
        }
    }

    // Find a callable candidate by name + arity. If exactly one matches,
    // use it. If multiple match, mirror `resolve_function_call`'s
    // narrowing path (`resolve.rs:730-748`) and disambiguate by the
    // already-lowered non-lambda arg types — empty lambda slots become
    // unbound-variable placeholders that the narrower treats as
    // compatibility-don't-care. Falls back to no-expectations only when
    // narrowing yields 0 or >1 candidates.
    let by_arity = candidates_by_arity(function_ptr, total_arity, ctx);
    let candidate = match by_arity.len() {
        0 => None,
        1 => Some(by_arity[0]),
        _ => {
            let placeholder_args = slots_with_placeholders(&slots);
            let narrowed = resolve::narrow_candidates_by_type(
                &by_arity,
                &placeholder_args,
                ctx.model,
                &ctx.variable_types,
            );
            if narrowed.len() == 1 {
                Some(narrowed[0])
            } else {
                None
            }
        }
    };

    // Compute expected param types per lambda slot.
    let lambda_expectations: LambdaExpectations = if let Some(fid) = candidate {
        compute_lambda_param_expectations(fid, &slots, ctx)
    } else {
        (0..total_arity).map(|_| None).collect()
    };

    // Phase 2: lower the lambda slots with expected types where available.
    for (slot_idx, ast_arg) in ast_args.iter().enumerate() {
        let target_idx = slot_idx + prepend_offset;
        if slots[target_idx].is_some() {
            continue;
        }
        let underlying = unwrap_group(ast_arg);
        if let ast_expr::Expression::Lambda(lam) = underlying {
            let expected = lambda_expectations
                .get(target_idx)
                .and_then(|o| o.as_deref());
            slots[target_idx] = lower_lambda_with_expected_types(lam, expected, ctx, errors);
        } else {
            // Defensive — shouldn't happen because phase 1 lowered everything
            // that wasn't a Lambda — but stay tolerant.
            slots[target_idx] = lower_expression(ast_arg, ctx, errors);
        }
    }

    slots.into_iter().flatten().collect()
}

/// Strips outer `Group(..)` wrappers so lambda-classification sees through
/// `({x | $x + 1})`. The lambda still lowers via `lower_expression`, which
/// also unwraps groups.
fn unwrap_group(expr: &ast_expr::Expression) -> &ast_expr::Expression {
    let mut cur = expr;
    while let ast_expr::Expression::Group(inner) = cur {
        cur = inner;
    }
    cur
}

/// Returns every function `ElementId` matching `ptr.name()` with the
/// given total arity, searching the same import scopes used by
/// `resolve_function_call`. Empty when nothing matches.
///
/// Multi-candidate disambiguation lives at the call site: when more than
/// one matches arity, the orchestrator narrows by type using the lowered
/// non-lambda args (mirroring `resolve_function_call`'s narrowing path
/// at `resolve.rs:730-748`).
fn candidates_by_arity(
    ptr: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    arity: usize,
    ctx: &ResolutionContext<'_>,
) -> Vec<crate::ids::ElementId> {
    use crate::model::Element;
    let name = &ptr.name;
    let mut candidates: Vec<crate::ids::ElementId> = Vec::new();

    let push_filtered = |found: Vec<crate::ids::ElementId>, out: &mut Vec<_>| {
        for eid in found {
            if let Element::Function(f) = ctx.model.get_element(eid)
                && f.parameters.len() == arity
                && !out.contains(&eid)
            {
                out.push(eid);
            }
        }
    };

    if let Some(pkg) = ptr.package.as_ref() {
        if let Some(pkg_id) = ctx.model.resolve_package(pkg) {
            let found = ctx.model.resolve_functions_by_name_in_package(pkg_id, name);
            push_filtered(found, &mut candidates);
        }
    } else {
        let root = ctx
            .model
            .resolve_functions_by_name_in_package(ctx.model.root_package, name);
        push_filtered(root, &mut candidates);
        for scope in ctx.import_scopes {
            if let Some(pkg_id) = ctx.model.resolve_package(&scope.package) {
                let found = ctx.model.resolve_functions_by_name_in_package(pkg_id, name);
                push_filtered(found, &mut candidates);
            }
        }
    }

    candidates
}

/// Build a positional `Vec<ValueSpec>` from a `Vec<Option<ValueSpec>>`,
/// substituting an unbound-variable placeholder for empty slots. The
/// placeholder is invisible to `narrow_candidates_by_type`'s
/// `is_type_compatible` check (`infer_type_from_valuespec` returns `None`
/// for unbound variables, and `is_type_compatible(None, ...)` returns
/// `true`), so positional indexing is preserved without imposing a
/// constraint on the lambda slot.
fn slots_with_placeholders(slots: &[Option<ValueSpec>]) -> Vec<ValueSpec> {
    slots
        .iter()
        .map(|opt| {
            opt.clone().unwrap_or_else(|| ValueSpec {
                kind: Box::new(ExprKind::Variable {
                    name: SmolStr::new_static("__lambda_placeholder"),
                }),
                source_info: SourceInfo::new("<lambda-inference>", 0, 0, 0, 0),
                type_info: None,
            })
        })
        .collect()
}

/// Two-phase lowering of QP-call arguments, mirroring
/// [`lower_args_with_lambda_inference`] but resolving the QP candidate
/// from the receiver type's `qualified_properties` instead of a free
/// function name. Returns the receiver as `arguments[0]` followed by
/// each lowered explicit arg.
///
/// The QP-param expectation that flows into a lambda arg is the same
/// shape used elsewhere: `Function<{T[m]→V[n]}>` (or its
/// `Named<Function>[FunctionType]` form). When no QP signature is
/// resolvable (target type unknown, no matching arity, multiple
/// matches), the lambda lowers without expectations — and the eager
/// `CannotInferLambdaParameterTypes` diagnostic fires if any param
/// would land on `TypeExpr::Unresolved`.
pub(super) fn lower_qp_call_args(
    target: &ValueSpec,
    qp_name: &str,
    ast_args: &[ast_expr::Expression],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<ValueSpec> {
    use crate::types::TypeExpr;

    let arity = ast_args.len();
    // Receiver in slot 0; explicit args in slots 1..=arity.
    let total_slots = 1 + arity;
    let mut slots: Vec<Option<ValueSpec>> = Vec::with_capacity(total_slots);
    slots.push(Some(target.clone()));

    // Phase 1: lower every non-lambda explicit arg, leaving lambda slots empty.
    for ast_arg in ast_args {
        let underlying = unwrap_group(ast_arg);
        if matches!(underlying, ast_expr::Expression::Lambda(_)) {
            slots.push(None);
        } else {
            slots.push(lower_expression(ast_arg, ctx, errors));
        }
    }

    // Resolve the QP candidate from the receiver type. The receiver is
    // already lowered; read its TypeExpr to find the class.
    let receiver_eid =
        resolve::infer_typeexpr_from_valuespec(target, ctx.model, &ctx.variable_types).and_then(
            |te| match te {
                TypeExpr::Named { element, .. } => Some(element),
                _ => None,
            },
        );
    let qp_params: Option<Vec<crate::types::Parameter>> =
        receiver_eid.and_then(|eid| find_qp_params_for_arity(ctx.model, eid, qp_name, arity));

    let lambda_expectations: LambdaExpectations = if let Some(params) = qp_params {
        // Build a synthetic param list with the receiver as a leading
        // pseudo-param typed as the receiver itself — keeps slot
        // alignment consistent with `expectations_from_callee_params`,
        // which expects `params.len() == slots.len()`.
        let mut all_params = Vec::with_capacity(total_slots);
        let recv_param = crate::types::Parameter {
            name: SmolStr::new_static("this"),
            type_expr: resolve::infer_typeexpr_from_valuespec(
                target,
                ctx.model,
                &ctx.variable_types,
            )
            .unwrap_or(TypeExpr::Unresolved),
            multiplicity: crate::types::Multiplicity::PureOne,
            source_info: target.source_info.clone(),
        };
        all_params.push(recv_param);
        all_params.extend(params);
        expectations_from_callee_params(&all_params, &slots, ctx)
    } else {
        (0..total_slots).map(|_| None).collect()
    };

    // Phase 2: lower the lambda slots with expected types where available.
    for (slot_idx, ast_arg) in ast_args.iter().enumerate() {
        let target_idx = slot_idx + 1; // slot 0 is the receiver
        if slots[target_idx].is_some() {
            continue;
        }
        let underlying = unwrap_group(ast_arg);
        if let ast_expr::Expression::Lambda(lam) = underlying {
            let expected = lambda_expectations
                .get(target_idx)
                .and_then(|o| o.as_deref());
            slots[target_idx] = lower_lambda_with_expected_types(lam, expected, ctx, errors);
        } else {
            slots[target_idx] = lower_expression(ast_arg, ctx, errors);
        }
    }

    slots.into_iter().flatten().collect()
}

/// Find a `QualifiedProperty` on `receiver_eid` (its class, walking
/// supertypes) whose name and parameter count match. Returns the
/// parameter list when a unique match exists; `None` otherwise (so the
/// caller falls back to no-expectation lowering, and any resulting
/// type holes surface via the eager `CannotInferLambdaParameterTypes`
/// diagnostic).
fn find_qp_params_for_arity(
    model: &crate::model::PureModel,
    receiver_eid: crate::ids::ElementId,
    qp_name: &str,
    arity: usize,
) -> Option<Vec<crate::types::Parameter>> {
    use crate::model::Element;
    let mut current_eid = Some(receiver_eid);
    while let Some(eid) = current_eid {
        if let Element::Class(c) = model.get_element(eid) {
            let matches: Vec<&crate::nodes::class::QualifiedProperty> = c
                .qualified_properties
                .iter()
                .filter(|qp| qp.name.as_str() == qp_name && qp.parameters.len() == arity)
                .collect();
            if matches.len() == 1 {
                return Some(matches[0].parameters.to_vec());
            }
            if matches.len() > 1 {
                // Multiple overloads — the orchestrator can't pick one
                // without lowered arg types. Defer to the resolution
                // pass; lambda args lower without expectations.
                return None;
            }
            // Walk a single supertype if there's exactly one named
            // class supertype. Pure supports multi-inheritance but the
            // QP lookup walks linearly; keeping the conservative path
            // for now mirrors how `infer_qualified_property` resolves
            // through `super_types` until it hits a match.
            current_eid = c.super_types.iter().find_map(|st| match st {
                crate::types::TypeExpr::Named { element, .. } => Some(*element),
                _ => None,
            });
        } else {
            break;
        }
    }
    None
}

/// For each argument slot, computes the expected lambda parameter
/// `(TypeExpr, Multiplicity)` list when the slot is a lambda whose matching
/// callee parameter is `Function<{T[m]->U[n]}>` and the call's generic
/// variables can be bound from the surrounding non-lambda args.
///
/// Slots that aren't lambdas, or whose matching param isn't a function type,
/// or whose substituted parameter types remain non-concrete, return `None`.
fn compute_lambda_param_expectations(
    callee: crate::ids::ElementId,
    slots: &[Option<ValueSpec>],
    ctx: &ResolutionContext<'_>,
) -> LambdaExpectations {
    let crate::model::Element::Function(callee_fn) = ctx.model.get_element(callee) else {
        return (0..slots.len()).map(|_| None).collect();
    };
    expectations_from_callee_params(&callee_fn.parameters, slots, ctx)
}

/// Shared core: given a callee's declared parameter list and the
/// already-lowered argument slots (with `None` placeholders for lambda
/// positions), produce per-slot lambda expectations. Used by both the
/// function-call path (params come from `Function.parameters`) and the
/// QP-call path (params come from the matched `QualifiedProperty`).
fn expectations_from_callee_params(
    params: &[crate::types::Parameter],
    slots: &[Option<ValueSpec>],
    ctx: &ResolutionContext<'_>,
) -> LambdaExpectations {
    use crate::types::TypeExpr;
    if params.len() != slots.len() {
        return (0..slots.len()).map(|_| None).collect();
    }

    // Collect the (param, arg) pairs for non-lambda slots only — lambda
    // positions are still empty and would just contribute nothing.
    let mut pair_params: Vec<crate::types::Parameter> = Vec::new();
    let mut pair_args: Vec<ValueSpec> = Vec::new();
    for (param, slot) in params.iter().zip(slots.iter()) {
        if let Some(arg) = slot {
            pair_params.push(param.clone());
            pair_args.push(arg.clone());
        }
    }
    let bindings =
        resolve::infer_generic_bindings(&pair_params, &pair_args, ctx.model, &ctx.variable_types);

    // For each lambda slot, extract Function<{...}> and substitute bindings.
    slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            if slot.is_some() {
                return None;
            }
            let param = params.get(i)?;
            // The param type may be FunctionType directly, or
            // Named<Function>[FunctionType] — same shapes handled by the
            // existing inference pass in resolve.rs.
            let function_type: Option<&TypeExpr> = match &param.type_expr {
                TypeExpr::FunctionType { .. } => Some(&param.type_expr),
                TypeExpr::Named { type_arguments, .. } => type_arguments
                    .iter()
                    .find(|ta| matches!(ta, TypeExpr::FunctionType { .. })),
                _ => None,
            };
            let TypeExpr::FunctionType {
                parameters: ft_params,
                ..
            } = function_type?
            else {
                return None;
            };
            let expected = ft_params
                .iter()
                .map(|(ft_ty, ft_mult)| {
                    let ty = resolve::substitute_type(ft_ty, &bindings.ty);
                    let mult = resolve::substitute_mult(ft_mult, &bindings.mult);
                    Some((ty, mult))
                })
                .collect::<Vec<_>>();
            Some(expected)
        })
        .collect()
}

// `lower_member_access` and `desugar_all_to_getall` live in
// `lower/member_access.rs`.

// `lower_type_reference`, `lower_packageable_element_ref`, and
// `build_packageable_element_ref` live in `lower/type_ref.rs`. The
// last is re-exported above so its `pub(crate)` path
// (`crate::lower::build_packageable_element_ref`) stays stable.

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
    lower_lambda_with_expected_types(e, None, ctx, errors)
}

/// Lowers a lambda, optionally using `expected_types` to type the lambda's
/// otherwise-untyped parameters before lowering its body.
///
/// `expected_types` is provided by the application-lowering paths
/// (`lower_function_application` / `lower_arrow_function`) when the lambda
/// occupies a `Function<{T[m]->U[n]}>`-typed slot whose type/multiplicity
/// variables can be resolved from the surrounding call's other arguments.
/// Each entry of the slice corresponds positionally to a lambda parameter;
/// `None` means "no expectation for this slot" (keep current fallback).
///
/// Already-typed lambda parameters always take precedence over the
/// expectation, so this never overrides an explicit declaration.
#[allow(clippy::unnecessary_wraps)] // consistent signature with other lower_* fns
pub(crate) fn lower_lambda_with_expected_types(
    e: &ast_expr::Lambda,
    expected_types: Option<&[Option<(crate::types::TypeExpr, crate::types::Multiplicity)>]>,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let parameters = lower_lambda_parameters(&e.parameters, expected_types, ctx, errors);

    // Save outer variable scope, register lambda params (including any
    // `TypeExpr::Unresolved` entries set for un-inferable params).
    let outer_vars = ctx.variable_types.clone();
    for param in &parameters {
        ctx.variable_types.insert(
            param.name.clone(),
            (param.type_expr.clone(), param.multiplicity.clone()),
        );
    }

    let body = lower_expression_body(&e.body, ctx, errors);

    // Restore outer scope (lambda params don't leak).
    ctx.variable_types = outer_vars;

    Some(untyped(
        ExprKind::Lambda { parameters, body },
        e.source_info.clone(),
    ))
}

/// Lower lambda parameters — same as function parameters but tolerates
/// missing type/multiplicity (untyped lambda params need type inference).
///
/// When `expected_types` is provided, an untyped parameter (no `type_ref`)
/// adopts the corresponding `Some((ty, mult))` entry — but only when the
/// expected type is *concrete* (a `Named { .. }` other than `Any`, with no
/// remaining `Generic(_)` substructure). This keeps dispatch precise without
/// overcommitting on still-generic call sites.
///
/// **Eager inference-failure diagnostic.** A parameter with no source
/// annotation AND no caller-side expectation gets `TypeExpr::Unresolved`,
/// and a `CannotInferLambdaParameterTypes` error is pushed for **all**
/// such params at the lambda's source span. This is intentionally
/// eager: even when the body doesn't dispatch on the parameter today,
/// the lambda may be passed to user code (deactivate / reactivate /
/// reflective walks) where a type hole would either crash type-element
/// extraction or surface as a meaningless `Any`-vs-`Unresolved`
/// ambiguity. Failing at the lambda is the only place where the user
/// can fix it (annotate the parameter or restructure the call site to
/// flow an expectation).
///
/// `Generic(_)` expectations *don't* trigger the failure — they mean
/// we're inside a parametric outer context where the type variable is
/// in scope and may bind at the call site. Annotation isn't required.
fn lower_lambda_parameters(
    params: &[legend_pure_parser_ast::annotation::Parameter],
    expected_types: Option<&[Option<(crate::types::TypeExpr, crate::types::Multiplicity)>]>,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    let mut uninferred: Vec<SmolStr> = Vec::new();
    let mut anchor: Option<SourceInfo> = None;

    let lowered: Vec<crate::types::Parameter> = params
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let declared_type = p
                .type_ref
                .as_ref()
                .and_then(|tr| resolve::resolve_type_ref(tr, ctx, errors));
            let declared_mult = p.multiplicity.as_ref().map(resolve::lower_multiplicity);
            let expected = expected_types
                .and_then(|s| s.get(idx))
                .and_then(|o| o.as_ref());

            let is_uninferred = declared_type.is_none() && expected.is_none();
            if is_uninferred {
                uninferred.push(p.name.clone());
                if anchor.is_none() {
                    anchor = Some(p.source_info.clone());
                }
            }

            // Type: declared > expected (when concrete) > Unresolved.
            // The `Unresolved` marker is a type hole that the eager
            // diagnostic below flags; it must NOT be `Any`, which would
            // mean "user wrote :Any[1]".
            let type_expr = declared_type.unwrap_or_else(|| {
                expected
                    .map(|(t, _)| t.clone())
                    .filter(is_concrete_type)
                    .unwrap_or(crate::types::TypeExpr::Unresolved)
            });
            // Multiplicity: declared > expected (when not Variable) > [1].
            let multiplicity = declared_mult.unwrap_or_else(|| {
                expected
                    .map(|(_, m)| m.clone())
                    .filter(|m| !matches!(m, crate::types::Multiplicity::Variable(_)))
                    .unwrap_or(crate::types::Multiplicity::PureOne)
            });
            crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity,
                source_info: p.source_info.clone(),
            }
        })
        .collect();

    if !uninferred.is_empty() {
        // `anchor` is set whenever `uninferred` is non-empty — captured
        // on the first failing parameter on the same iteration.
        let source_info = anchor.unwrap_or_else(|| SourceInfo::new("<lambda>", 0, 0, 0, 0));
        errors.push(CompilationError {
            message: format_uninferred_lambda_message(&uninferred),
            source_info,
            kind: crate::error::CompilationErrorKind::CannotInferLambdaParameterTypes {
                names: uninferred,
            },
        });
    }

    lowered
}

/// Render the user-facing message for a `CannotInferLambdaParameterTypes`
/// diagnostic. Single-quoted, comma-separated parameter names plus an
/// annotation example with `<Type>` placeholders, preserving order so
/// the suggestion lines up with the source.
fn format_uninferred_lambda_message(names: &[SmolStr]) -> String {
    let plural = if names.len() > 1 { "s" } else { "" };
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    let example_params: Vec<String> = names.iter().map(|n| format!("{n}: <Type>[1]")).collect();
    format!(
        "Cannot infer type{plural} for lambda parameter{plural} {}: \
         annotate explicitly, e.g. `{{{} | …}}`",
        quoted.join(", "),
        example_params.join(", "),
    )
}

/// True iff `ty` is a fully resolved, non-`Any` named type. Type-arguments
/// must themselves be concrete; any `Generic(_)` lurking inside disqualifies
/// the type as "concrete enough" to commit lambda-param dispatch on.
fn is_concrete_type(ty: &crate::types::TypeExpr) -> bool {
    use crate::types::TypeExpr;
    match ty {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => *element != crate::bootstrap::ANY_ID && type_arguments.iter().all(is_concrete_type),
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => parameters.iter().all(|(t, _)| is_concrete_type(t)) && is_concrete_type(return_type),
        TypeExpr::Relation(_) => true,
        // `Generic` and `AlgebraUnion` are not concrete — they may bind
        // later. `Unresolved` is by definition not concrete: it's the
        // type-hole marker for an un-inferred lambda parameter.
        TypeExpr::Generic(_) | TypeExpr::AlgebraUnion(_, _) | TypeExpr::Unresolved => false,
    }
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
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new_static("letFunction"),
            arguments: vec![
                untyped(
                    ExprKind::StringLiteral(SmolStr::new(e.name.as_str())),
                    e.source_info.clone(),
                ),
                value,
            ],
        }),
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
        multiplicity_arguments: Vec::new(),
        value_arguments: vec![],
    };

    // Honour pre-set `type_info` first — same canonical
    // "lowering-captures-type / consumers-read-from-type_info" pattern
    // used by `infer_typeexpr_from_valuespec` (`reference_type_info_capture.md`).
    // Required for `^Class<T>(...)` (whose lowering pre-sets type_info
    // but leaves `function: None`) and for bare `P` references that pre-set
    // `Class<P>` via `build_packageable_element_ref`. Without this,
    // `let people = [^P()]` inferred as None and `$people`'s var_types
    // entry was missing, which then poisoned every downstream
    // narrowing that depended on it.
    if let Some(rt) = value.type_info.as_deref() {
        return Some((rt.type_expr.clone(), rt.multiplicity.clone()));
    }

    match value.kind.as_ref() {
        ExprKind::IntegerLiteral(_) => Some((named(bootstrap::INTEGER_ID), Multiplicity::PureOne)),
        ExprKind::FloatLiteral(_) => Some((named(bootstrap::FLOAT_ID), Multiplicity::PureOne)),
        ExprKind::DecimalLiteral(_) => Some((named(bootstrap::DECIMAL_ID), Multiplicity::PureOne)),
        ExprKind::StringLiteral(_) => Some((named(bootstrap::STRING_ID), Multiplicity::PureOne)),
        ExprKind::BooleanLiteral(_) => Some((named(bootstrap::BOOLEAN_ID), Multiplicity::PureOne)),
        ExprKind::FunctionCall(FunctionCallData {
            function,
            arguments,
            ..
        }) => function.and_then(|fid| {
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
        ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
            // Resolve the property's declared type + multiplicity and
            // combine with the receiver's multiplicity (`obj[m1].prop[m2]`
            // → `[m1*m2]`). Without this, `let x = $obj.collProp`
            // failed to bind `x` into `var_types`, leaving every
            // downstream `$x` reference type-unknown — which made
            // dispatch silently pick wrong overloads (e.g.
            // `String[*]->contains(String[1])` going to
            // `string::contains(String[1], String[1])`).
            let target = data.arguments.first()?;
            let target_te = crate::resolve::infer_typeexpr_from_valuespec(
                target,
                ctx.model,
                &ctx.variable_types,
            )?;
            let target_eid = match &target_te {
                TypeExpr::Named { element, .. } => *element,
                _ => return None,
            };
            let recv_mult = crate::resolve::infer_multiplicity_from_valuespec(
                target,
                ctx.model,
                &ctx.variable_types,
            )?;
            let prop_te = crate::resolve::find_property_with_inheritance(
                target_eid,
                &data.function_name,
                ctx.model,
            )?
            .0;
            let prop_mult = crate::resolve::find_property_multiplicity(
                target_eid,
                &data.function_name,
                ctx.model,
            )?;
            Some((
                prop_te,
                crate::resolve::multiplicity_product(&recv_mult, &prop_mult),
            ))
        }
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
                                    multiplicity_arguments: Vec::new(),
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
            #[allow(clippy::cast_possible_truncation)]
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

/// Lowers `^MyClass<TypeArg, …>(prop='val', other += $vals)` →
/// `FunctionCall("new", [class, name, [type_arg_refs], [type_var_vals], kvts...])`.
///
/// Position 2 is the (possibly empty) collection of resolved type-argument
/// elements — `^List<String>(values=…)` puts `[String]` there so the
/// runtime can hang the bindings off the new instance and `genericType()`
/// surface them via `typeArguments`. Position 0/1 stay as the class element
/// + simple name. Position 4.. carries `(key, value, augmented_bool)`
///   triples — the augmented flag distinguishes `=` (replace, `mutate_set`)
///   from `+=` (append, `mutate_add`). Java threads this as the `KeyValue.add`
///   slot; we encode it inline so the runtime needs no schema lookup.
fn lower_new_instance(
    e: &ast_expr::NewInstanceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let class_id = resolve::resolve_element_ptr(&e.class, &e.source_info, ctx, errors)?;

    let mut arguments = Vec::with_capacity(4 + e.assignments.len() * 3);
    arguments.push(build_packageable_element_ref(
        class_id,
        e.source_info.clone(),
        ctx.model,
    ));
    arguments.push(untyped(
        ExprKind::StringLiteral(SmolStr::new(e.class.name.as_str())),
        e.source_info.clone(),
    ));
    // Resolve each type argument fully — keep the FULL `TypeExpr` (with
    // its own nested type_arguments) for the call's `type_info` below.
    // Also extract the runtime-shaped element refs the New native
    // currently consumes from arguments[2]. Anonymous structural types
    // (function types, generics) and unresolved bindings are dropped
    // silently from the runtime stream — they have no element to
    // surface through `genericType().typeArguments` — but they're still
    // captured in the type_info if resolution succeeded.
    let resolved_type_args: Vec<crate::types::TypeExpr> = e
        .type_arguments
        .iter()
        .filter_map(|ta| resolve::resolve_type_ref(ta, ctx, errors))
        .collect();
    let type_arg_specs: Vec<ValueSpec> =
        resolved_type_args
            .iter()
            .zip(e.type_arguments.iter())
            .filter_map(|(resolved, ta)| match resolved {
                crate::types::TypeExpr::Named { element, .. } => Some(
                    build_packageable_element_ref(*element, ta.source_info.clone(), ctx.model),
                ),
                _ => None,
            })
            .collect();
    arguments.push(untyped(
        ExprKind::Collection {
            elements: type_arg_specs,
        },
        e.source_info.clone(),
    ));
    // Type-variable VALUES — `^MyClass(10)(text=…)` binds `x = 10` for
    // the `x:Integer[1]` parameter declared on `MyClass(x:Integer[1])`.
    // Lowered as a parallel collection so the runtime can store the
    // bindings on the heap and qualified properties / class
    // constraints can resolve `$x` against the receiver instance.
    let type_var_value_specs: Vec<ValueSpec> = e
        .type_variable_values
        .iter()
        .map(|tv| {
            let (kind, span) = match tv {
                ast_type::TypeVariableValue::Integer(n, s) => {
                    (ExprKind::IntegerLiteral(*n), s.clone())
                }
                ast_type::TypeVariableValue::String(s, sp) => {
                    (ExprKind::StringLiteral(SmolStr::new(s)), sp.clone())
                }
            };
            untyped(kind, span)
        })
        .collect();
    arguments.push(untyped(
        ExprKind::Collection {
            elements: type_var_value_specs,
        },
        e.source_info.clone(),
    ));
    for kv in &e.assignments {
        // Drop the whole triple if value lowering failed — emitting a
        // dangling key would desynchronise the (key, value, augmented)
        // stride the runtime walks. The error has already been recorded
        // by `lower_expression`.
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

    // Capture the parametric type the AST already carries — `^Class<T>(...)`
    // syntactically means "a `Class<T>` instance", so the lowered call
    // expresses that directly via `type_info` rather than relying on
    // downstream consumers (Pass 2.5, runtime `New::execute`, the
    // resolver) to re-derive it from the runtime-shaped arg stream.
    // This keeps the type-capture step (lowering reads the AST) decoupled
    // from each consumer's needs (`new`, `cast(@T)`, `class A extends T`,
    // future generic-aware operators) — they all read the same
    // `type_info`. Consumers that don't need parametrics (`^MyClass(...)`
    // with no `<T>`) get a bare `Named` here, indistinguishable from
    // what Pass 2.5 would have inferred.
    let type_info = Some(Box::new(crate::types::ResolvedType {
        type_expr: crate::types::TypeExpr::Named {
            element: class_id,
            type_arguments: resolved_type_args,
            multiplicity_arguments: Vec::new(),
            value_arguments: vec![],
        },
        multiplicity: crate::types::Multiplicity::PureOne,
    }));
    Some(ValueSpec {
        kind: Box::new(ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new_static("new"),
            arguments,
        })),
        source_info: e.source_info.clone(),
        type_info,
    })
}

// ---------------------------------------------------------------------------
// Copy expression
// ---------------------------------------------------------------------------

/// Lowers `^$source(prop='val', other += $vals)` →
/// `FunctionCall("copy", [$source, key1, val1, augmented1, ...])`.
///
/// Matches the Java M3 desugaring: the source variable becomes the first
/// argument, followed by `(key, value, augmented_bool)` triples for each
/// property override. The augmented flag distinguishes `=` (replace,
/// `mutate_set`) from `+=` (append, `mutate_add`) — Java carries this as
/// the `KeyValue.add` slot; we encode it inline so the runtime needs no
/// schema lookup.
fn lower_copy(
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

    untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: None,
            function_name: SmolStr::new_static("copy"),
            arguments,
        }),
        e.source_info.clone(),
    )
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

// ---------------------------------------------------------------------------
// Column / Relation literals
// ---------------------------------------------------------------------------

/// Lowers `~name` / `~name:Type[mult]` / `~[name:Type[mult], …]` to
/// either a `ColSpecLiteral` (single-column form, `is_array=false`)
/// or a `ColSpecArrayLiteral` (array form, `is_array=true`).
///
/// Mirrors the platform's `PCT.grammarCharacters` annotations on
/// `meta::pure::functions::relation::colSpec` (`'~'`) vs
/// `colSpecArray` (`'~[,]'`). The two shapes have distinct M3
/// classifiers (`ColSpec` vs `ColSpecArray`) and different reflective
/// slots (`name:String[1]` vs `names:String[*]`), so downstream
/// overload narrowing — e.g. `ascending(column:ColSpec<T>[1])` —
/// fails to resolve when a single `~name` is mis-lowered to
/// `ColSpecArray`.
///
/// Captures column triples (`name`, resolved `type_element`,
/// `multiplicity`) at lowering time so the runtime allocator can
/// materialise the heap shape without re-resolving names.
///
/// Lambda-bearing `~name:x|$x+1` columns and the `funcColSpec*` /
/// `aggColSpec*` shapes are not yet wired through this lowerer —
/// those columns drop their lambda payload here. Add a follow-up if
/// a subsequent test forces them.
fn lower_column(
    e: &ast_expr::ColumnBuilderExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let columns = lower_relation_columns_from_specs(&e.columns, ctx, errors);
    let kind = classify_col_spec_kind(&e.columns);
    let outer_id = match (e.is_array, kind) {
        (false, crate::types::ColSpecLiteralKind::Plain) => resolve_col_spec_id(ctx)?,
        (false, crate::types::ColSpecLiteralKind::Func) => {
            resolve_relation_class_id(ctx, "FuncColSpec")?
        }
        (false, crate::types::ColSpecLiteralKind::Agg) => {
            resolve_relation_class_id(ctx, "AggColSpec")?
        }
        (true, crate::types::ColSpecLiteralKind::Plain) => resolve_col_spec_array_id(ctx)?,
        (true, crate::types::ColSpecLiteralKind::Func) => {
            resolve_relation_class_id(ctx, "FuncColSpecArray")?
        }
        (true, crate::types::ColSpecLiteralKind::Agg) => {
            resolve_relation_class_id(ctx, "AggColSpecArray")?
        }
    };
    let type_expr = TypeExpr::Named {
        element: outer_id,
        type_arguments: vec![],
        multiplicity_arguments: Vec::new(),
        value_arguments: vec![],
    };
    if e.is_array {
        return Some(typed(
            ExprKind::ColSpecArrayLiteral { columns, kind },
            e.source_info.clone(),
            ResolvedType {
                type_expr,
                multiplicity: Multiplicity::PureOne,
            },
        ));
    }
    // Single-column form: take the first lowered column or synthesise
    // an `Any` placeholder if the lowerer dropped it (e.g. lambda
    // form, untyped name).
    let column = columns.into_iter().next().unwrap_or_else(|| {
        let name = e
            .columns
            .first()
            .map_or_else(|| SmolStr::new_static(""), |c| c.name.clone());
        RelationColumnLowered {
            name,
            type_element: crate::bootstrap::ANY_ID,
            multiplicity: Multiplicity::ZeroOrOne,
        }
    });
    Some(typed(
        ExprKind::ColSpecLiteral { column, kind },
        e.source_info.clone(),
        ResolvedType {
            type_expr,
            multiplicity: Multiplicity::PureOne,
        },
    ))
}

/// Classify a column-builder's columns into the three platform
/// shapes — plain (no lambda), `func` (one init lambda per column),
/// `agg` (init + reduce lambdas per column). Mixed shapes fall back
/// to `Plain` so the lowerer doesn't pretend to know which classifier
/// to pick. Mirrors the platform's
/// `colSpec` vs `funcColSpec` vs `aggColSpec` grammar dispatch.
fn classify_col_spec_kind(specs: &[ast_expr::ColumnSpec]) -> crate::types::ColSpecLiteralKind {
    use crate::types::ColSpecLiteralKind;
    if specs.is_empty() {
        return ColSpecLiteralKind::Plain;
    }
    let mut has_init = true;
    let mut has_agg = true;
    for c in specs {
        let is_lambda_init = matches!(&c.type_spec, Some(ast_expr::ColumnTypeSpec::Lambda(_)));
        if !is_lambda_init {
            has_init = false;
            has_agg = false;
            continue;
        }
        if c.extra_function.is_none() {
            has_agg = false;
        }
    }
    if has_agg {
        ColSpecLiteralKind::Agg
    } else if has_init {
        ColSpecLiteralKind::Func
    } else {
        ColSpecLiteralKind::Plain
    }
}

/// Resolves AST `RelationColumn`s into the lowered triple form. Used by
/// the `RelationLiteral` lowering path (`@(cols)`).
pub(super) fn lower_relation_columns(
    cols: &[ast_type::RelationColumn],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<RelationColumnLowered> {
    cols.iter()
        .filter_map(|c| {
            let type_expr = resolve::resolve_type_ref(&c.type_ref, ctx, errors)?;
            let TypeExpr::Named {
                element: type_element,
                ..
            } = type_expr
            else {
                return None;
            };
            let multiplicity = c
                .multiplicity
                .as_ref()
                .map_or(Multiplicity::ZeroOrOne, resolve::lower_multiplicity);
            Some(RelationColumnLowered {
                name: c.name.clone(),
                type_element,
                multiplicity,
            })
        })
        .collect()
}

/// Resolves AST `ColumnSpec`s into the lowered triple form. Used by the
/// `ColSpecArrayLiteral` lowering path (`~[cols]`).
///
/// Each input becomes a `RelationColumnLowered` regardless of shape:
///
/// - `name:Type[mult]` — typed column. Type resolves; multiplicity
///   parsed if present, else `[0..1]`.
/// - `name` (bare) and `name:lam|...` (lambda-bearing — Func/Agg) —
///   the type / lambda payload isn't preserved here yet, but the
///   column NAME must be kept so the runtime allocator populates the
///   `ColSpecArray.names` slot. Synthesise a placeholder
///   `(Any, [0..1])` so downstream consumers (`select`, `rename`,
///   `extend`) at least see the column name. Lambda-body preservation
///   is tracked separately — see `extend_func_col_spec_currently_drops_its_lambda`
///   in the engine smoke tests.
fn lower_relation_columns_from_specs(
    cols: &[ast_expr::ColumnSpec],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<RelationColumnLowered> {
    cols.iter()
        .map(|c| {
            // Typed column → resolve and use as-is.
            if let Some(ast_expr::ColumnTypeSpec::Typed(type_ref, mult)) = &c.type_spec {
                let resolved = resolve::resolve_type_ref(type_ref, ctx, errors);
                if let Some(TypeExpr::Named {
                    element: type_element,
                    ..
                }) = resolved
                {
                    let multiplicity = mult
                        .as_ref()
                        .map_or(Multiplicity::ZeroOrOne, resolve::lower_multiplicity);
                    return RelationColumnLowered {
                        name: c.name.clone(),
                        type_element,
                        multiplicity,
                    };
                }
                // Type didn't resolve to Named — fall through to the
                // placeholder so the column name survives.
            }
            // Bare name or lambda-bearing — placeholder type. Lambda
            // bodies are dropped here (see lower_column doc).
            RelationColumnLowered {
                name: c.name.clone(),
                type_element: crate::bootstrap::ANY_ID,
                multiplicity: Multiplicity::ZeroOrOne,
            }
        })
        .collect()
}

pub(super) fn resolve_relation_type_id(
    ctx: &mut ResolutionContext<'_>,
) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("RelationType"),
    ])
}

fn resolve_col_spec_array_id(ctx: &mut ResolutionContext<'_>) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("ColSpecArray"),
    ])
}

fn resolve_col_spec_id(ctx: &mut ResolutionContext<'_>) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("ColSpec"),
    ])
}

fn resolve_relation_class_id(
    ctx: &mut ResolutionContext<'_>,
    class_name: &str,
) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new(class_name),
    ])
}

// ---------------------------------------------------------------------------
// Date parsing helpers
// ---------------------------------------------------------------------------

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
fn lower_navigation_path(
    e: &ast_expr::NavigationPath,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    use crate::types::{PathStepLowered, TypeExpr};

    let start_type = resolve::resolve_type_ref(&e.start_type, ctx, errors)?;

    // Validate the first step's property exists on the start type.
    // (Subsequent steps validated at runtime — see fn doc.)
    if let Some(first_step) = e.path.first() {
        if let TypeExpr::Named { element, .. } = &start_type {
            if resolve::find_property_with_inheritance(*element, &first_step.property, ctx.model)
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
        }
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

// Date-literal parsing tests live in `lower/literal.rs` next to the
// helpers they exercise.
