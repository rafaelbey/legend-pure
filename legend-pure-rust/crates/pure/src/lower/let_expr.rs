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

//! Let-expression lowering: `let x = expr` →
//! `FunctionCall("letFunction", [name, value])`.
//!
//! Step 4 of the lowering encapsulation plan. Includes the
//! `infer_let_type` helper that registers the let-bound variable's
//! type+multiplicity into `ResolutionContext::variable_types` so
//! downstream dispatch can narrow on `$x`.

use legend_pure_parser_ast::expression as ast_expr;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::ResolutionContext;
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::{lower_expression, untyped};

/// Lowers `let x = expr` → `FunctionCall("letFunction", [name, value])`.
///
/// Matches the Java M3 desugaring: the variable name becomes a string
/// literal, and the value is the lowered RHS expression.
pub(super) fn lower_let(
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
    use crate::types::{Multiplicity, TypeExpr};

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
                //
                // Note on `copy`: `^$x(field=val)` lowers to
                // `FunctionCall { function: None, function_name: "copy" }`.
                // Its type comes from `lower_copy`'s pre-set `type_info`
                // (read via the early-return at the top of
                // `infer_let_type`), not through this arm.
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
        // Lambda: build `Named<LambdaFunction>{type_args=[FunctionType{...}]}`
        // mirroring `build_packageable_element_ref`'s function-ref
        // shape. The Named<LambdaFunction> wrapper is what makes
        // `bind_type`'s subtype walk fire (LambdaFunction → Function),
        // exposing the FunctionType slot to `eval<V|m>(func:Function<{->V[m]}>)`
        // for V/m extraction.
        //
        // Without this, $z stayed type-less and `$z->eval(...)` saw a
        // bare TypeExpr that bind_type's Named arm couldn't recurse
        // into (the param is Named<Function>{[FunctionType]} but the
        // arg was a bare FunctionType — different arms).
        ExprKind::Lambda { parameters, body } => {
            let ft_params: Vec<(crate::types::TypeExpr, crate::types::Multiplicity)> = parameters
                .iter()
                .map(|p| (p.type_expr.clone(), p.multiplicity.clone()))
                .collect();
            let (return_type, return_multiplicity) =
                if let Some(last) = body.last() {
                    let ret_te = crate::resolve::infer_typeexpr_from_valuespec(
                        last,
                        ctx.model,
                        &ctx.variable_types,
                    )
                    .unwrap_or(crate::types::TypeExpr::Unresolved);
                    let ret_mult = crate::resolve::infer_multiplicity_from_valuespec(
                        last,
                        ctx.model,
                        &ctx.variable_types,
                    )
                    .unwrap_or(crate::types::Multiplicity::PureOne);
                    (ret_te, ret_mult)
                } else {
                    (crate::types::TypeExpr::Unresolved, Multiplicity::PureOne)
                };
            let function_type = crate::types::TypeExpr::FunctionType {
                parameters: ft_params,
                return_type: Box::new(return_type),
                return_multiplicity,
            };
            // Wrap in Named<LambdaFunction>{[FunctionType]} so subtype-walk
            // to Function lifts the FunctionType slot out for binding.
            let lambda_metaclass = ctx.model.resolve_by_path(&[
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("function"),
                SmolStr::new("LambdaFunction"),
            ]);
            let wrapped = if let Some(eid) = lambda_metaclass {
                crate::types::TypeExpr::Named {
                    element: eid,
                    type_arguments: vec![function_type],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                }
            } else {
                function_type
            };
            Some((wrapped, Multiplicity::PureOne))
        }
        _ => None,
    }
}
