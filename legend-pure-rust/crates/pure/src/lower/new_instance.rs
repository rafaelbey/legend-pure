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

//! `^Class<TypeArgs>(...)` lowering: produces `FunctionCall("new", [class,
//! name, [type_arg_refs], [type_var_vals], (key, value, augmented_bool)*])`.
//!
//! Step 4 of the lowering encapsulation plan. The lowered ValueSpec
//! pre-sets `type_info = Class<TypeArgs>[1]` so downstream consumers
//! (Pass 2.5, runtime `New::execute`, the resolver) read parametric
//! type information from a single canonical slot instead of
//! re-deriving it from the runtime-shaped arg stream.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::type_ref as ast_type;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::{build_packageable_element_ref, lower_expression, untyped};

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
pub(super) fn lower_new_instance(
    e: &ast_expr::NewInstanceExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let class_id = resolve::resolve_element_ptr(&e.class, &e.source_info, ctx, errors)?;

    let mut arguments = Vec::with_capacity(4 + e.assignments.len() * 3);
    // Use the CLASS reference's span (the `abc::Class1` part of
    // `^abc::Class1()`), not the whole NewInstanceExpr's span (which
    // covers only the `^` token by parser convention). The reference
    // index walks this argument as a `PackageableElementRef` and uses
    // its `source_info` as the clickable region. Without this fix
    // Cmd-click anywhere inside `^abc::Class1()` lands outside the
    // recorded ref and goto-def silently no-ops.
    arguments.push(build_packageable_element_ref(
        class_id,
        legend_pure_parser_ast::source_info::Spanned::source_info(&e.class).clone(),
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
            source_info: None,
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
