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

//! Lambda lowering: `{x: String[1] | $x + 'hello'}` → `ExprKind::Lambda`.
//!
//! Step 4 of the lowering encapsulation plan. The
//! caller-driven `lower_lambda_with_expected_types` entry point is
//! re-exported from `lower/mod.rs` because the function-application
//! lowering paths still drive lambda-with-expectations calls.
//!
//! Note: this module owns the *lowering* side of the lambda. The
//! mirror inference-side helpers (`compute_lambda_param_expectations`
//! and `bind_from_lambda_body`) are slated for consolidation into
//! `crate::inference::lambda` per Step 3e of the plan; once that
//! lands, this module's expectation-aware path will read from there.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, ValueSpec};

use super::{lower_expression_body, untyped};

/// Lowers a lambda: `{x: String[1] | $x + 'hello'}` → `ValueSpec::Lambda`.
///
/// Lambda parameters are lowered the same way as function parameters.
/// Untyped parameters (inferred lambdas like `x | $x + 1`) are kept with
/// best-effort type information — full inference is deferred.
#[allow(clippy::unnecessary_wraps)] // consistent signature with other lower_* fns
pub(super) fn lower_lambda(
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
    let strict = crate::strict_mode::is_enabled();

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

            // Default mode (Java parity): "uninferred" only when there's
            // neither a declaration nor any caller-side expectation.
            // `Generic(T)` expectations stay silent because they may
            // bind at the call site if the enclosing fn is parametric.
            //
            // Strict mode adds: an expectation containing ANY
            // `Generic(name)` whose `name` isn't a transitive parameter
            // of the enclosing element (`ctx.type_parameters`) — at any
            // depth — is also a failure. Walks the full type via
            // `unresolved_type_params` so `List<T>`, `Function<{T->X}>`,
            // and arbitrarily nested cases all fail when T (or X) isn't
            // in the enclosing fn's scope. Catches `needsPred(x | …)`,
            // `mapToList(x | …)`, and `compose(f, g)`-style chains where
            // the propagating Generic can't bind from any sibling arg.
            let expected_unbindable_in_strict = strict
                && match expected {
                    Some((te, _)) => {
                        let unresolved = crate::inference::context::unresolved_type_params(te);
                        unresolved
                            .iter()
                            .any(|name| !ctx.type_parameters.iter().any(|t| t == name))
                    }
                    None => false,
                };
            let is_uninferred = (declared_type.is_none() && expected.is_none())
                || (declared_type.is_none() && expected_unbindable_in_strict);
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
        // A `Named { element: X, … }` is concrete enough to use as a
        // lambda-param expectation as long as the head element is
        // resolved (not `Any`). The type_arguments may still contain
        // `Generic(_)` nodes — those are typically transitive
        // generics from the enclosing function (e.g.,
        // `condList: Pair<Function<{->Boolean[1]}>, Function<{->T[m]}>>[*]`
        // where `T`/`m` are the outer fn's params). Property access
        // and function dispatch work fine with the concrete head;
        // any contained Generic substitutes naturally at the
        // enclosing scope's binding pass.
        //
        // The previous `type_arguments.iter().all(is_concrete_type)`
        // gate falsely rejected such types, falling back to
        // `Unresolved` — which then propagated through the lambda
        // body's property accesses and erased the type info needed
        // for downstream `eval`/`map`/`filter` to bind their own T/V.
        TypeExpr::Named { element, .. } => *element != crate::bootstrap::ANY_ID,
        // FunctionType is concrete when its outer shape is known —
        // same logic. Type-arguments and inner generics can survive.
        TypeExpr::FunctionType { .. } => true,
        TypeExpr::Relation(_) => true,
        // `Generic` and `AlgebraUnion` are not concrete *at the head*
        // — the lambda lowering can't produce a useful expected type
        // from them (the enclosing scope may bind later, but the
        // lambda's own dispatch needs a concrete head). `Unresolved`
        // is the type-hole marker.
        TypeExpr::Generic(_) | TypeExpr::AlgebraUnion(_, _) | TypeExpr::Unresolved => false,
    }
}
