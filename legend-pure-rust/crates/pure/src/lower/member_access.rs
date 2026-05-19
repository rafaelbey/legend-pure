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

//! Member-access lowering: `$x.name` and `$x.qp(args)` dispatch.
//!
//! Step 4 of the lowering encapsulation plan; sibling slice to
//! `lower/literal.rs`, `lower/collection.rs`, and `lower/operator.rs`.
//!
//! Includes the `.all` desugaring path that rewrites both `$x.all` and
//! `Class.all()` to `FunctionCall("getAll", [target])` — mirroring
//! Java Pure's parser-level `AntlrContextToM3CoreInstance.allOrFunction`
//! transform so the runtime sees a single entry point for the `.all`
//! accessor.

use legend_pure_parser_ast::expression as ast_expr;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{ExprKind, FunctionCallData, ValueSpec};

use super::function_app::lower_qp_call_args;
use super::{lower_expression, operator, untyped};

/// Lowers member access (dot): `$x.name` or `$x.derived('arg')`.
///
/// **`.all` desugaring.** Both `$x.all` and `Class.all()` rewrite to a
/// `FunctionCall("getAll", [target])`. Mirrors Java Pure's parser-level
/// `allOrFunction` desugaring (`AntlrContextToM3CoreInstance.allOrFunction`)
/// so the runtime has exactly one entry point — the `getAll` native — for
/// the `.all` accessor. Without this, the call form `Class.all()` would
/// fall through to function dispatch and fail to find an `all` native.
pub(super) fn lower_member_access(
    e: &ast_expr::MemberAccess,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    if let Some(getall) = desugar_all_to_getall(e, ctx, errors) {
        return Some(getall);
    }
    // Lower into the unified `FunctionCall { kind: Property|QualifiedProperty }`
    // shape — the receiver is `arguments[0]`, the property name lives in
    // `function_name`, and (for QPs) any qualifier args follow the
    // receiver. Mirrors Java's `propertyExpression` in
    // `AntlrContextToM3CoreInstance` which builds a `SimpleFunctionExpression`
    // with `_propertyName` / `_qualifiedPropertyName` set and the receiver
    // as the first parameter value.
    match e {
        ast_expr::MemberAccess::Simple(s) => {
            let target = lower_expression(&s.target, ctx, errors)?;
            // Use the member identifier's own span as the PropertyCall
            // ValueSpec's `source_info` — that's the clickable region
            // the goto-def index emits for the property/enum-value
            // name. The dot's span (`s.source_info`) and the
            // receiver's span are recoverable separately when needed.
            Some(untyped(
                ExprKind::PropertyCall(FunctionCallData {
                    function: None,
                    function_name: SmolStr::new(s.member.as_str()),
                    arguments: vec![target],
                }),
                s.member_source_info.clone(),
            ))
        }
        ast_expr::MemberAccess::Qualified(q) => {
            let target = lower_expression(&q.target, ctx, errors)?;
            // Two-phase QP-arg lowering with lambda-parameter type
            // inference, mirroring `lower_args_with_lambda_inference`'s
            // shape but resolving the candidate by walking the receiver
            // type's `qualified_properties` (QPs aren't free-name —
            // they're class-scoped). A lambda whose matching QP param
            // is `Function<{T[m]→V[n]}>` is lowered with that
            // expectation, so e.g. `^M_ThisMapHolder().func(a | 2.0)`
            // types `a` as `M_ThisMapHolder[1]` instead of falling into
            // the type-hole guard.
            let arguments =
                lower_qp_call_args(&target, q.member.as_str(), &q.arguments, ctx, errors);
            // QP call: use the member identifier's span — same
            // rationale as the `Simple` arm above.
            Some(untyped(
                ExprKind::QualifiedPropertyCall(FunctionCallData {
                    function: None,
                    function_name: SmolStr::new(q.member.as_str()),
                    arguments,
                }),
                q.member_source_info.clone(),
            ))
        }
    }
}

/// Rewrite milestoning grammar shortcuts on a class reference:
///
/// - `Class.all` / `Class.all()` → `getAll(Class)`
/// - `Class.all($d)` → `getAll(Class, $d)` (single-temporal milestoning)
/// - `Class.all($pd, $bd)` → `getAll(Class, $pd, $bd)` (bitemporal)
/// - `Class.allVersions()` → `getAllVersions(Class)` (every version, no filter)
/// - `Class.allVersionsInRange($s, $e)` → `getAllVersionsInRange(Class, $s, $e)`
///
/// Returns `Some(value_spec)` when a shortcut fires; `None` for everything
/// else so the regular member-access lowering runs unchanged. Java parity:
/// `AntlrContextToM3CoreInstance.allOrFunction` plus the milestoning
/// grammar block in the same file.
fn desugar_all_to_getall(
    e: &ast_expr::MemberAccess,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let (target_ast, member, source_info, extra_args): (_, _, _, &[ast_expr::Expression]) = match e
    {
        ast_expr::MemberAccess::Simple(s) => (&s.target, &s.member, &s.source_info, &[]),
        ast_expr::MemberAccess::Qualified(q) => {
            (&q.target, &q.member, &q.source_info, q.arguments.as_slice())
        }
    };

    // Map shortcut name → desugared native name + acceptable arity range
    // (counting the receiver as arg 0). The receiver is always the class
    // reference; user-supplied args go after.
    let (native_name, expected_user_args): (&'static str, &[usize]) = match member.as_str() {
        // `.all` covers `getAll(Class)`, `getAll(Class, Date)`, and
        // `getAll(Class, Date, Date)`. Accept 0, 1, or 2 user args.
        "all" => ("getAll", &[0, 1, 2]),
        // `.allVersions()` — no args; returns every version.
        "allVersions" => ("getAllVersions", &[0]),
        // `.allVersionsInRange(start, end)` — exactly 2 user args.
        "allVersionsInRange" => ("getAllVersionsInRange", &[2]),
        _ => return None,
    };

    if !expected_user_args.contains(&extra_args.len()) {
        errors.push(CompilationError {
            message: format!(
                "{}() does not accept {} argument(s); expected one of {:?}",
                member.as_str(),
                extra_args.len(),
                expected_user_args,
            ),
            source_info: source_info.clone(),
            kind: crate::error::CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new(format!(
                    "MemberAccess::Qualified(\"{}\", {} args)",
                    member.as_str(),
                    extra_args.len()
                )),
            },
        });
        return None;
    }

    let target_val = lower_expression(target_ast, ctx, errors)?;
    let mut arguments = vec![target_val];
    for a in extra_args {
        let lowered = lower_expression(a, ctx, errors)?;
        arguments.push(lowered);
    }
    let arity = arguments.len();
    let ptr = operator::synthetic_unqualified_ptr(native_name, source_info);
    let mut scratch_errors: Vec<CompilationError> = Vec::new();
    let function_id = resolve::resolve_function_call(
        &ptr,
        arity,
        &arguments,
        source_info,
        ctx,
        &mut scratch_errors,
    );
    if function_id.is_some() {
        errors.extend(scratch_errors);
    }
    Some(untyped(
        ExprKind::FunctionCall(FunctionCallData {
            function: function_id,
            function_name: SmolStr::new_static(native_name),
            arguments,
        }),
        source_info.clone(),
    ))
}
