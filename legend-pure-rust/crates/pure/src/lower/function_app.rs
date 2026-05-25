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

//! Function-application and arrow-function lowering, plus the shared
//! two-phase argument-lowering machinery that drives lambda-parameter
//! type inference at call sites.
//!
//! Step 4 of the lowering encapsulation plan; final big slice. Owns:
//!
//! - [`lower_function_application`] — `func(args)` →
//!   `FunctionCall { name, arguments }`.
//! - [`lower_arrow_function`] — `expr->func(args)` → `FunctionCall`
//!   with target prepended.
//! - [`lower_args_with_lambda_inference`] — the shared two-phase
//!   driver: phase 1 lowers non-lambdas, phase 2 lowers lambdas with
//!   per-slot expectations sourced from
//!   [`crate::inference::lambda::compute_lambda_param_expectations`].
//! - [`lower_qp_call_args`] — same two-phase shape but resolves the
//!   QP candidate from the receiver type's `qualified_properties`
//!   instead of a free function name. Re-exported from `lower/mod.rs`
//!   for `lower/member_access.rs` to reach without going via this
//!   module.
//! - [`unwrap_group`], [`candidates_by_arity`],
//!   [`slots_with_placeholders`], [`find_qp_params_for_arity`] —
//!   private helpers that exist solely to support the four entry
//!   points above.
//!
//! The lambda-side inference helpers (`compute_lambda_param_expectations`
//! and `expectations_from_callee_params`) live in
//! `crate::inference::lambda` per Step 3e of the plan, so this slice
//! does **not** carry them — it only consumes them.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::{lower_expression, lower_lambda_with_expected_types, untyped};

/// Local alias for the inference module's per-slot expectations
/// shape, kept here so the call-site annotations in this module
/// stay short.
type LambdaExpectations = crate::inference::lambda::LambdaExpectations;

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
pub(super) fn lower_function_application(
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
pub(super) fn lower_arrow_function(
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
        crate::inference::lambda::compute_lambda_param_expectations(fid, &slots, ctx)
    } else {
        (0..total_arity).map(|_| None).collect()
    };

    // Phase 2: lower the lambda slots with expected types where available.
    for (slot_idx, ast_arg) in ast_args.iter().enumerate() {
        let target_idx = slot_idx + prepend_offset;
        if slots[target_idx].is_some() {
            // A filled `FuncColSpec`/`AggColSpec` slot (`~name:c|…`) carries an
            // init lambda that phase 1 lowered with an `Any`/`Unresolved` row
            // param (the source-relation row type wasn't bound yet). If
            // `compute_lambda_param_expectations` produced a row expectation
            // for this slot, re-lower the init lambda(s) with it so `$c.col`
            // resolves to the column's element type — mirroring how direct
            // lambda args are typed.
            if let Some(expected) = lambda_expectations
                .get(target_idx)
                .and_then(|o| o.as_deref())
                && let ast_expr::Expression::Column(cb) = unwrap_group(ast_arg)
                && let Some(filled) = slots[target_idx].clone()
                && let Some(relowered) = super::relation::relower_colspec_with_row_type(
                    &filled, cb, expected, ctx, errors,
                )
            {
                slots[target_idx] = Some(relowered);
            }
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
        // Implicit self-package (T-20260510-01): same precedence as
        // `resolve_function_call`'s step 2b — only consulted when
        // explicit imports yielded nothing, so explicit imports keep
        // shadowing same-package siblings.
        if candidates.is_empty()
            && let Some(pkg) = ctx.self_package
            && let Some(pkg_id) = ctx.model.resolve_package(pkg)
        {
            let found = ctx.model.resolve_functions_by_name_in_package(pkg_id, name);
            push_filtered(found, &mut candidates);
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
        crate::inference::lambda::expectations_from_callee_params(&all_params, &slots, ctx)
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
