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

//! Lambda-parameter type filling against an enclosing
//! `Function<{T->X}>` expectation. Java analog:
//! `org.finos.legend.pure.m3.compiler.postprocessing.inference.TypeInference`
//! at lines 108-148
//! (`processParamTypesOfLambdaUsedAsAFunctionExpressionParamValue`).
//!
//! # Step 3e (current state)
//!
//! The **lower-side** half lives here:
//! [`compute_lambda_param_expectations`] +
//! [`expectations_from_callee_params`]. They read the candidate's
//! parameter type, find the embedded `FunctionType` slot, substitute
//! any already-known bindings into its parameter types, and produce
//! the per-slot `(TypeExpr, Multiplicity)` lists that
//! `lower_lambda_with_expected_types` consumes. This is what makes
//! `[1,2,3]->filter(x | $x->plus(1))` type `x` as `Integer[1]`.
//!
//! The **resolve-side** half — `crate::resolve::infer_generic_bindings`'s
//! second pass and the `bind_from_lambda_body` helper — still lives
//! in `resolve.rs`. Walks each lambda arg (or every Lambda inside a
//! Collection arg), infers the lambda body's last expression with
//! the lambda's params in scope, and binds the FunctionType's
//! return-type variable from that body type. This is what closed
//! `getAllTypeGeneralisations`'s inference precision and the
//! `match([λ1, λ2])` pattern. It will move here in a follow-up
//! sub-step once the lower-side cohabitation is verified.
//!
//! Both paths solve adjacent problems (typing the lambda's *params*
//! at lower time vs. binding the FunctionType's *return* at infer
//! time) but cohabit in this module so the lambda-parameter
//! algorithm has one home.
//!
//! ## Java's deferred-lambda case
//!
//! Java's `processParamTypes…` returns `true` ("not done") when the
//! template's parameter types aren't yet concrete. The orchestrator
//! then re-processes the lambda after later args have populated
//! bindings. We don't yet implement this deferral — when our second
//! pass can't bind a lambda's params, the param falls back to
//! `TypeExpr::Unresolved`. Tracked in BACKLOG: "Per-call-site
//! specialisation for let-bound lambdas".
//!
//! Plan: `~/.claude/plans/do-we-have-enought-quiet-swing.md`.

use crate::resolve::{self, ResolutionContext};
use crate::types::ValueSpec;

/// Per-slot lambda expectations: an outer `Vec` indexed by argument
/// position, with `Some(Vec<Option<(TypeExpr, Multiplicity)>>)` for
/// lambda slots whose params can be bound, and `None` for non-lambda
/// slots or unresolvable lambda slots. The inner inner `Option`s let
/// individual lambda parameters fall back to `Unresolved` when their
/// expected type stays generic — matches the eager-failure shape the
/// `lower_lambda_parameters` consumer expects.
pub(crate) type LambdaExpectations =
    Vec<Option<Vec<Option<(crate::types::TypeExpr, crate::types::Multiplicity)>>>>;

/// For each argument slot, computes the expected lambda parameter
/// `(TypeExpr, Multiplicity)` list when the slot is a lambda whose matching
/// callee parameter is `Function<{T[m]->U[n]}>` and the call's generic
/// variables can be bound from the surrounding non-lambda args.
///
/// Slots that aren't lambdas, or whose matching param isn't a function type,
/// or whose substituted parameter types remain non-concrete, return `None`.
pub(crate) fn compute_lambda_param_expectations(
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
pub(crate) fn expectations_from_callee_params(
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
