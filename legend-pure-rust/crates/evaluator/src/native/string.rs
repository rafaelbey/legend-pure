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

//! String natives: `toString` (the generic renderer, dispatching to a
//! class's own `toString()` qualified property when it declares one),
//! `format`, `joinStrings`, the predicates (`contains`, `startsWith`,
//! `endsWith`), the slicers (`substring`, `indexOf`, `length`, `split`,
//! `replace`, `reverseString`), case and trimming, and `parseDecimal`.
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.

use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

/// Pure `toString(Any[1]): String[1]` — convert any value to its
/// human-readable Pure string form. Distinct from `Value::Display`
/// (which is a debug-leaning shape with quoted strings, `%`-prefixed
/// dates, and `<Object@id>` placeholders): toString matches Java
/// Pure's `toString` shape, which is what platform tests pin in
/// `essential/string/toString/toString.pure`.
///
/// Heap-object dispatch is delegated to whatever `toString()` qualified
/// property the receiver's class (or any of its generalizations) defines
/// — `Pair`, `List`, and any user class with a `toString()` qualifier
/// flow through the same path. When no such QP exists, the runtime
/// falls back to `Anonymous_<obj_id>`, matching Java
/// `ToString.execute`'s `value.getName()` branch. See
/// `legend-pure-runtime-java-engine-interpreted/.../ToString.java:50-70`.
#[derive(Debug)]
pub struct ToString;

impl NativeFunction for ToString {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toString", &values, 1)?;
        let s = pure_to_string(&values[0], ctx)?;
        Ok(Evaluated::new(Value::String(SmolStr::new(s))))
    }
}

/// Locate the `toString()` qualified property on the receiver's class
/// (walking generalizations) via the same `find_qp_with_generalization`
/// the evaluator uses for dispatch. Returns the
/// [`FoundQp`](crate::eval::FoundQp) so callers can hand it back to
/// [`EvalContextTrait::invoke_qualified_property_found`] without a
/// second lookup.
fn lookup_to_string_qp(
    ctx: &dyn EvalContextTrait,
    obj_id: &crate::heap::ObjectHandle,
) -> Option<crate::eval::FoundQp> {
    let classifier = ctx.heap().classifier(obj_id).ok()?;
    let class_id = ctx.model().resolve_fqn_str(classifier.as_str())?;
    crate::eval::find_qp_with_generalization(ctx.model(), class_id, "toString", 0)
}

/// `true` iff the QP's declared return is exactly `String[1]`. Pure's
/// `toString()` contract requires this; any other declared shape is
/// treated as a malformed QP and skipped.
fn qp_returns_string_one(found: &crate::eval::FoundQp) -> bool {
    use legend_pure_metamodel::bootstrap::STRING_ID;
    use legend_pure_metamodel::types::{Multiplicity, TypeExpr};
    matches!(
        (&found.return_type, &found.return_multiplicity),
        (TypeExpr::Named { element, .. }, Multiplicity::PureOne)
            if *element == STRING_ID
    )
}

/// Render a value to its Pure-toString form.
///
/// For primitives, this returns the literal source form (mirrors Java
/// Pure's `value.getName()` for primitive types). For heap objects this
/// invokes any `toString()` qualified property visible on the receiver's
/// class (with generalization walk) — *but only if the declared return
/// is `String[1]`*; malformed QPs are skipped via
/// `qp_returns_string_one`, and the value formats as
/// `Anonymous_<id>` (exactly how Java's `ToString.execute` falls back
/// when `findBestToStringFunction` returns null).
///
/// # Errors
/// Propagates any [`PureException`] raised while evaluating a class's
/// `toString` body.
pub(crate) fn pure_to_string(
    value: &Value,
    ctx: &mut dyn EvalContextTrait,
) -> Result<String, PureException> {
    // Iterative explicit-frame DFS so deep `Collection(Collection(...))`
    // and `UnitInstance` chains can't overflow the Rust thread stack.
    // The naive recursion was unbounded in syntactic nesting; the
    // iterative form scales with heap allocations only. Mirrors the
    // shape of `render_representation` and the iterative `Value::Clone`
    // / `Value::Drop` in `crates/evaluator/src/value.rs`.
    //
    // `Value::Object` dispatches through
    // `ctx.invoke_qualified_property("toString", …)`. A user-defined
    // `toString` is required by Pure's contract to return `String[1]`,
    // so we **gate the invocation on the declared signature**: query
    // `qualified_property_signature` first and skip the invoke entirely
    // when the declared return isn't `String[1]`. Avoids running a
    // malformed `toString` body that happens to be expensive or
    // side-effectful before discovering the shape mismatch.
    //
    // The fallback for "no String[1] QP" matches Java Pure's
    // `ToString.execute` path: render as `Anonymous_<id>`.
    enum Op<'a> {
        Visit(&'a Value),
        AssembleCollection(usize),
        AssembleUnitInstance(ElementId),
    }

    let mut ops: Vec<Op<'_>> = vec![Op::Visit(value)];
    let mut built: Vec<String> = Vec::new();

    while let Some(op) = ops.pop() {
        match op {
            Op::Visit(v) => match v {
                Value::Collection(items) => {
                    ops.push(Op::AssembleCollection(items.len()));
                    for item in items.iter().rev() {
                        ops.push(Op::Visit(item));
                    }
                }
                Value::UnitInstance { unit_id, inner } => {
                    ops.push(Op::AssembleUnitInstance(*unit_id));
                    ops.push(Op::Visit(inner));
                }
                Value::Object(obj_id) => {
                    // Find the `toString()` QP *once* via the same model
                    // walk the evaluator uses for dispatch, gate on the
                    // declared `String[1]` return, then invoke the
                    // already-found QP directly so the trait doesn't
                    // redo the classifier → class → QP lookup.
                    let invoked = lookup_to_string_qp(ctx, obj_id)
                        .filter(qp_returns_string_one)
                        .map(|found| ctx.invoke_qualified_property_found(v, &found, &[]))
                        .transpose()?;
                    match invoked.as_ref() {
                        Some(Value::String(s)) => built.push(s.to_string()),
                        // No QP, declared return wasn't `String[1]`, or
                        // the body diverged from its declared shape —
                        // all collapse to the Java-Pure fallback form.
                        _ => built.push(format!("Anonymous_{:p}", std::rc::Rc::as_ptr(obj_id))),
                    }
                }
                leaf => built.push(pure_to_string_leaf(leaf, ctx)),
            },
            Op::AssembleCollection(len) => {
                let start = built.len() - len;
                let parts: Vec<String> = built.drain(start..).collect();
                built.push(format!("[{}]", parts.join(", ")));
            }
            Op::AssembleUnitInstance(unit_id) => {
                let inner_s = built.pop().ok_or_else(|| {
                    PureRuntimeError::EvaluationError(
                        "pure_to_string: internal driver bug — AssembleUnitInstance reached with empty built stack".into(),
                    )
                })?;
                let unit_name = ctx.model().element_name(unit_id).to_string();
                built.push(format!("{inner_s} {unit_name}"));
            }
        }
    }

    built.pop().ok_or_else(|| {
        PureRuntimeError::EvaluationError(
            "pure_to_string: internal driver bug — built stack empty after traversal".into(),
        )
        .into()
    })
}

/// `toString` rendering for the non-recursive (`Collection` / `UnitInstance` /
/// `Object` excluded) `Value` variants. The full driver is `pure_to_string`;
/// this helper handles only leaves so the driver's match can stay tight.
fn pure_to_string_leaf(value: &Value, ctx: &dyn EvalContextTrait) -> String {
    use crate::value::FunctionValue;
    match value {
        Value::String(s) => s.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        // Float: route through the Java-style formatter so
        // integer-valued doubles render with the trailing `.0`
        // platform tests pin (testFloatToStringWithExcessTrailingZeros,
        // testFloatToStringWithPositiveExponent). Rust's native
        // `{f64}` Display strips the `.0`; everything else
        // (including the avoidance of E-notation for normal-range
        // values, and 0.000000013421-style expanded form for very
        // small values) already matches Java's expected output.
        Value::Float(_) => crate::native::math::java_number_string(value),
        Value::Decimal(d) => d.to_string(),
        // Date/DateTime: no `%` prefix (PureDate's Display already
        // formats with TZ for time-precision dates).
        Value::Date(d) => d.to_string(),
        Value::Latest => "%latest".to_string(),
        Value::StrictTime(t) => t.to_string(),
        Value::Unit => String::new(),
        // Element ref (Class, Function, Enumeration, …): simple-leaf
        // name. testClassToString and testEnumerationToString assert
        // `STR_Person->toString() == 'STR_Person'`.
        Value::Element(id) => crate::model_utils::element_simple_name(ctx.model(), *id).to_string(),
        // Enum value: just the member (Java parity — `CITY` not
        // `STR_GeographicEntityType.CITY`).
        Value::EnumValue { member, .. } => member.to_string(),
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Lambda(_) => "<Lambda>".to_string(),
            FunctionValue::Compiled(id) => {
                crate::model_utils::element_simple_name(ctx.model(), *id).to_string()
            }
            FunctionValue::Path(p) => format!(
                "<Path:{}{}>",
                p.steps.len(),
                p.name.as_ref().map(|n| format!("!{n}")).unwrap_or_default()
            ),
        },
        Value::Map(m) => format!("<Map size={}>", m.borrow().entries.len()),
        // Recursive variants are driven by the iterative loop;
        // reaching them here means the dispatch went wrong.
        Value::Collection(_) | Value::UnitInstance { .. } | Value::Object(_) => unreachable!(
            "pure_to_string_leaf called on recursive Value variant; the iterative driver should have queued these"
        ),
    }
}

/// Register the fished string natives.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("toString_Any_1__String_1_", ToString);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, lit_int};

    #[test]
    fn to_string_integer() {
        assert_eq!(
            ToString
                .execute(&[lit_int(42)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("42".into())
        );
    }

    #[test]
    fn to_string_wrong_arg_count_errors() {
        assert!(ToString.execute(&[], &mut MockCtx).is_err());
    }

    /// Build `Collection(Collection(... Integer(0) ...))` `depth`
    /// levels deep. Primitive leaves keep the renderer off any code
    /// paths that would need a real model.
    fn deep_nested_collection(depth: usize) -> Value {
        let mut v = Value::Integer(0);
        for _ in 0..depth {
            let mut inner = im_rc::Vector::new();
            inner.push_back(v);
            v = Value::Collection(Box::new(inner));
        }
        v
    }

    /// Locks the iterative `pure_to_string` contract: rendering a
    /// 10 000-level-deep `Collection(...)` chain must not overflow
    /// the Rust thread stack. The previous recursive implementation
    /// overflowed in the same `Collection`-nesting band as
    /// `render_representation` did.
    #[test]
    fn pure_to_string_deep_collection_no_overflow() {
        let v = deep_nested_collection(10_000);
        let s = super::pure_to_string(&v, &mut MockCtx).expect("render must succeed");
        assert_eq!(s.matches('[').count(), 10_000);
        assert_eq!(s.matches(']').count(), 10_000);
        assert!(s.contains('0'));
    }

    /// Sanity: shape produced for a small list matches `toString`
    /// form (`[1, 2, 3]`, no quotes around integers).
    #[test]
    fn pure_to_string_shallow_collection_shape() {
        let mut pv = im_rc::Vector::new();
        pv.push_back(Value::Integer(1));
        pv.push_back(Value::Integer(2));
        pv.push_back(Value::Integer(3));
        let v = Value::Collection(Box::new(pv));
        assert_eq!(
            super::pure_to_string(&v, &mut MockCtx).unwrap(),
            "[1, 2, 3]"
        );
    }
}
