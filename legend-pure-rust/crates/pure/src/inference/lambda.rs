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
//! # Step 3e (consolidation complete)
//!
//! The **lower-side** half:
//! [`compute_lambda_param_expectations`] +
//! [`expectations_from_callee_params`]. They read the candidate's
//! parameter type, find the embedded `FunctionType` slot, substitute
//! any already-known bindings into its parameter types, and produce
//! the per-slot `(TypeExpr, Multiplicity)` lists that
//! `lower_lambda_with_expected_types` consumes. This is what makes
//! `[1,2,3]->filter(x | $x->plus(1))` type `x` as `Integer[1]`.
//!
//! The **resolve-side** half:
//! [`bind_from_lambda_body`]. Drills into a single lambda arg
//! against an expected `FunctionType` shape, infers the lambda
//! body's last expression's type with the lambda's params in scope,
//! and `bind_type`-merges the result against the FunctionType's
//! `return_type` variable. This closes `getAllTypeGeneralisations`'s
//! inference precision and the `match([λ1, λ2])` pattern.
//!
//! Both halves live here so the lambda-parameter algorithm has one
//! home. `crate::resolve::infer_generic_bindings`'s second pass
//! still drives the dispatch (collecting lambda args from
//! `Collection`-of-lambdas vs direct shapes) and calls
//! [`bind_from_lambda_body`] per lambda.
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

use crate::resolve::{self, ResolutionContext, VarTypes};
use crate::types::ValueSpec;

use super::GenericBindings;

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
    //
    // Two slot shapes carry a lambda whose param type we infer here:
    //   - an *empty* slot — a direct lambda arg (`coll->filter(c|…)`),
    //     deferred from phase 1; and
    //   - a *filled* `FuncColSpec`/`AggColSpec` slot whose lowered value
    //     embeds an init lambda (`rel->extend(~name:c|…)`). The ColSpec is
    //     a non-lambda arg from the call's view, so phase 1 already lowered
    //     it (with an `Any`/`Unresolved` row param); we recompute the row
    //     expectation here from the matched `FuncColSpec<{T[1]->…},Z>`
    //     param (its embedded `FunctionType`'s first param is the relation
    //     row `T`, now bound from the source-relation arg) so phase 2 can
    //     re-lower the init lambda body with a typed `$c`.
    slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let is_colspec_lambda = slot.as_ref().is_some_and(is_colspec_func_or_agg_with_init);
            if slot.is_some() && !is_colspec_lambda {
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
                    // Route through `make_concrete_type` /
                    // `make_concrete_mult` so the alpha-rename map (set
                    // by `infer_generic_bindings` to keep callee
                    // generics distinct from caller generics) is
                    // applied before substitution. Calling
                    // `substitute_type` directly with `bindings.ty`
                    // would miss because the HashMap keys are the
                    // renamed callee names, but `ft_ty` here is the
                    // callee's original param type expression.
                    let ty = bindings.make_concrete_type(ft_ty);
                    let mult = bindings.make_concrete_mult(ft_mult);
                    Some((ty, mult))
                })
                .collect::<Vec<_>>();
            Some(expected)
        })
        .collect()
}

/// True iff `vs` is a `FuncColSpec`/`AggColSpec` literal (single `~name:c|…`
/// or array `~[…]`) carrying at least one init lambda — i.e. a column spec
/// whose row param needs typing from the enclosing relation call. `Plain`
/// ColSpecs (no lambda) and all other values return false, preserving the
/// "filled slot → no expectation" default.
fn is_colspec_func_or_agg_with_init(vs: &ValueSpec) -> bool {
    use crate::types::{ColSpecLiteralKind, ExprKind};
    match vs.kind.as_ref() {
        ExprKind::ColSpecLiteral { column, kind } => {
            matches!(kind, ColSpecLiteralKind::Func | ColSpecLiteralKind::Agg)
                && column.init_lambda.is_some()
        }
        ExprKind::ColSpecArrayLiteral { columns, kind } => {
            matches!(kind, ColSpecLiteralKind::Func | ColSpecLiteralKind::Agg)
                && columns.iter().any(|c| c.init_lambda.is_some())
        }
        _ => false,
    }
}

/// Drill into a single lambda arg against an expected `FunctionType`
/// shape, infer the lambda body's last expression's type with the
/// lambda's params in scope, and `bind_type`-merge the result against
/// the FunctionType's `return_type` variable.
///
/// Called from `crate::resolve::infer_generic_bindings`'s second
/// pass, once for a direct-Lambda arg and once per `Lambda` inside a
/// `Collection`-of-lambdas arg. The resolve-side dispatch that
/// chooses which lambdas feed which slot stays in `resolve.rs`; this
/// function owns the per-lambda binding step.
pub(crate) fn bind_from_lambda_body(
    function_type: &crate::types::TypeExpr,
    arg: &ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
    bindings: &mut GenericBindings,
) {
    use crate::types::{ExprKind, Multiplicity, TypeExpr};
    let TypeExpr::FunctionType {
        parameters: ft_params,
        return_type,
        return_multiplicity: ft_return_mult,
    } = function_type
    else {
        return;
    };
    let ExprKind::Lambda {
        parameters: lambda_params,
        body: lambda_body,
    } = arg.kind.as_ref()
    else {
        return;
    };
    // Extend var_types with the lambda's own params, preferring the
    // lambda's declared type when concrete (mirrors Java's "explicit
    // annotation wins" rule). Without this, an explicitly-typed param
    // like `{inc:MappingInclude[1], sub:Store[0..1] | …}` against
    // `Function<{T[1], V[m] -> V[m]}>[1]` would have `sub` bound to
    // `Generic("V")` whenever V hadn't bound from a sibling arg yet —
    // and the lambda body would then type its uses against `Generic`
    // rather than `Store[0..1]`.
    let mut extended = var_types.clone();
    for (lp, (ft_ty, ft_mult)) in lambda_params.iter().zip(ft_params.iter()) {
        let lambda_ty_concrete =
            !matches!(lp.type_expr, TypeExpr::Generic(_) | TypeExpr::Unresolved);
        let ty = if lambda_ty_concrete {
            lp.type_expr.clone()
        } else {
            resolve::substitute_type(ft_ty, &bindings.ty)
        };
        let mult = if matches!(lp.multiplicity, Multiplicity::Variable(_)) {
            resolve::substitute_mult(ft_mult, &bindings.mult)
        } else {
            lp.multiplicity.clone()
        };
        extended.insert(lp.name.clone(), (ty, mult));
    }
    let Some(last_expr) = lambda_body.last() else {
        return;
    };
    // Prefer the full `TypeExpr` shape (preserves `type_arguments` for
    // parametric returns like `^List<Variant>(...)`) over the bare
    // `ElementId` from `infer_type_from_valuespec` — without
    // `type_arguments`, `Z` binds to bare `List` and property access on
    // `$result.values` falls back through `Generic("X")[*]` via the
    // class generic-substitution path, losing the inner `Variant`.
    // Fall back to the bare-element path when the richer query can't
    // produce a TypeExpr (rare; e.g. a Lambda body whose last expression
    // is itself an unresolved kind), so existing call sites keep their
    // behaviour.
    //
    // **Re-infer fresh, ignoring cached `type_info`.** The lambda
    // body's `type_info` was set at lower-time, when sibling-arg
    // generics (like reduce's V) hadn't yet bound — so the cached
    // dispatch result may be Any. Re-running inference here with the
    // freshly-extended `var_types` (where lambda params now carry the
    // substituted FT-param types) recovers the correct body type.
    // The cache is for downstream consumers that don't have a richer
    // extended scope; this code path *does* have one.
    // Resolve unresolved FunctionCalls in the lambda body. Lower-time
    // dispatch may have left `function = None` when a sibling arg read
    // `TypeExpr::Unresolved` (a lambda param whose type wasn't yet
    // inferrable). Now that `extended` carries the freshly-bound
    // lambda-param types from the surrounding FT slot, we can re-narrow.
    // Recursively walk the lambda body cloning the IR so the patched
    // copy carries the resolved `function` ids for downstream
    // `infer_typeexpr_from_valuespec_fresh`.
    let patched_last = redispatch_unresolved_calls(last_expr, model, &extended);
    let patched_last_ref = patched_last.as_ref().unwrap_or(last_expr);
    let body_te =
        match resolve::infer_typeexpr_from_valuespec_fresh(patched_last_ref, model, &extended) {
            Some(te) => te,
            None => {
                let Some(body_eid) =
                    resolve::infer_type_from_valuespec(patched_last_ref, model, &extended)
                else {
                    return;
                };
                TypeExpr::Named {
                    element: body_eid,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                }
            }
        };
    // **Override-on-narrow.** Pass-1's bare-FT-vs-Named<Function>
    // bridge in `bind_type_with_mode` already bound this return-slot
    // generic from the *lower-time* lambda-body type_info — which is
    // often `Any` when dispatch couldn't pick a unique candidate
    // (e.g. agg-lambda body `$y->average()` when the FT's V wasn't yet
    // bound from a sibling arg). LUB-merging the freshly re-inferred
    // `body_te` against that pessimistic Any keeps the binding at Any.
    // For lambda-body-driven bindings, pass-2 IS the authoritative
    // source — clear the affected Generic-key entries first so the
    // subsequent `bind_type` writes the concrete value rather than
    // LUB-widening into oblivion. Java parity: TypeInference treats
    // the lambda body as the V/U source after later passes resolve
    // sibling-arg generics; we approximate by giving pass-2 priority
    // for those keys.
    collect_generic_names_in_typeexpr(return_type, &mut |name| {
        bindings.ty.remove(name);
    });
    resolve::bind_type(return_type, &body_te, &mut bindings.ty, model);

    // The lambda body's actual multiplicity is intentionally NOT
    // bound into `ft_return_mult`. Java parity: Java's
    // `bind_from_lambda_body` analog only binds the type slot.
    // Empirically, binding `m := body's mult` regresses the QP body
    // shape `func():Float[1] { if(true, |$this->map(...), |1.0) }`,
    // where map returns `Float[0..1]` and the LUB widens the if's
    // return to `Float[0..1]`, mismatching the QP's declared
    // `Float[1]`. Keep `m` unbound here — `is_multiplicity_compatible`'s
    // permissive `Variable(_)` arm handles the rest.
    let _ = ft_return_mult;
    let _ = last_expr;
}

/// Walks a `TypeExpr` collecting every `Generic(name)` it carries (at
/// the top level, inside `Named.type_arguments`, inside
/// `FunctionType.parameters` / `return_type`, inside
/// `AlgebraUnion`, inside `Relation` column types). Invokes `sink` once
/// per name.
fn collect_generic_names_in_typeexpr<F: FnMut(&smol_str::SmolStr)>(
    te: &crate::types::TypeExpr,
    sink: &mut F,
) {
    use crate::types::TypeExpr;
    match te {
        TypeExpr::Generic(name) => sink(name),
        TypeExpr::Named { type_arguments, .. } => {
            for ta in type_arguments {
                collect_generic_names_in_typeexpr(ta, sink);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (t, _) in parameters {
                collect_generic_names_in_typeexpr(t, sink);
            }
            collect_generic_names_in_typeexpr(return_type, sink);
        }
        TypeExpr::AlgebraUnion(left, right) => {
            collect_generic_names_in_typeexpr(left, sink);
            collect_generic_names_in_typeexpr(right, sink);
        }
        TypeExpr::Relation(cols) => {
            for c in cols {
                collect_generic_names_in_typeexpr(&c.type_expr, sink);
            }
        }
        _ => {}
    }
}

/// Walks a lambda-body expression and patches `FunctionCall.function`
/// fields that lower-time dispatch couldn't resolve. The freshly-extended
/// `var_types` (carrying the bound lambda-param types pulled from the
/// surrounding `Function<{T->V}>` slot) may now allow narrowing to
/// pick a unique candidate.
///
/// Returns `Some(patched_clone)` when at least one FunctionCall was
/// re-narrowed to a unique candidate; `None` when nothing changed
/// (caller can keep using the original `last_expr` without paying a
/// clone). Recursive — handles nested calls (`$y->plus(1)->times(2)`).
///
/// Global-by-name lookup is intentional. The original import scope
/// isn't reachable from `bind_from_lambda_body`; falling back to a
/// model-wide walk gives the right answer for the platform's
/// canonical overload sets (where simple names like `average` /
/// `sort` / `sum` collide across at most one or two packages, and the
/// narrower picks by type). Documented O(model size) for one
/// expression; if a future profile shows this in the hot path we can
/// add a simple-name index alongside `Element::Function`.
fn redispatch_unresolved_calls(
    vs: &ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Option<ValueSpec> {
    use crate::types::{ExprKind, FunctionCallData};
    let ExprKind::FunctionCall(data) = vs.kind.as_ref() else {
        return None;
    };
    // Recursively patch nested calls first — `$x->f()->g()` lowers to
    // `g(f(x))`, and our caller hands us the outer (g). If `f`'s
    // function id is unresolved, we need to patch it before re-narrowing
    // `g` (which inspects arg types).
    let mut new_args: Vec<ValueSpec> = Vec::with_capacity(data.arguments.len());
    let mut any_changed = false;
    for a in &data.arguments {
        if let Some(patched) = redispatch_unresolved_calls(a, model, var_types) {
            new_args.push(patched);
            any_changed = true;
        } else {
            new_args.push(a.clone());
        }
    }
    // If the outer call already resolved, propagate any inner patches and stop.
    if data.function.is_some() {
        if !any_changed {
            return None;
        }
        let new_data = FunctionCallData {
            function: data.function,
            function_name: data.function_name.clone(),
            arguments: new_args,
        };
        return Some(ValueSpec {
            kind: Box::new(ExprKind::FunctionCall(new_data)),
            source_info: vs.source_info.clone(),
            type_info: vs.type_info.clone(),
        });
    }
    // function is None — try to re-narrow with extended var_types.
    let candidates: Vec<_> = model
        .resolve_functions_by_simple_name_globally(&data.function_name)
        .into_iter()
        .filter(|&eid| {
            if let crate::model::Element::Function(f) = model.get_element(eid) {
                f.parameters.len() == new_args.len()
            } else {
                false
            }
        })
        .collect();
    let narrowed = resolve::narrow_candidates_by_type(&candidates, &new_args, model, var_types);
    if narrowed.len() == 1 {
        let new_data = FunctionCallData {
            function: Some(narrowed[0]),
            function_name: data.function_name.clone(),
            arguments: new_args,
        };
        Some(ValueSpec {
            kind: Box::new(ExprKind::FunctionCall(new_data)),
            source_info: vs.source_info.clone(),
            type_info: vs.type_info.clone(),
        })
    } else if any_changed {
        let new_data = FunctionCallData {
            function: data.function,
            function_name: data.function_name.clone(),
            arguments: new_args,
        };
        Some(ValueSpec {
            kind: Box::new(ExprKind::FunctionCall(new_data)),
            source_info: vs.source_info.clone(),
            type_info: vs.type_info.clone(),
        })
    } else {
        None
    }
}
