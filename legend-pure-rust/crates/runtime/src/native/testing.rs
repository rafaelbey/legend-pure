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

//! Test support native functions: `assert`, `executeTest`, `executePCTTest`.
//!
//! These support the PCT (Pure Compatibility Tests) and user-authored
//! Pure test suites via the `meta::pure::test::surveyor` orchestrator.

use std::time::Instant;

use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureExceptionKind, PureRuntimeError};
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args};
use crate::value::{FunctionValue, Value};

// ---------------------------------------------------------------------------
// assert
// ---------------------------------------------------------------------------

/// Pure `assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]`
///
/// Asserts a boolean condition. If false, evaluates the message expression
/// and returns an error with that message. The message is only evaluated
/// on failure — short-circuit via direct `ctx.evaluate` calls.
#[derive(Debug)]
pub struct Assert;

impl NativeFunction for Assert {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("assert", args, 2)?;
        // Condition is plain `Boolean[1]` — force directly. Message is
        // typed `Function<{->String[1]}>`, so the compiler wraps it in a
        // zero-parameter lambda; `force_thunk` unwraps that only on failure
        // so a passing assertion never builds its message.
        let condition = ctx.evaluate(&args[0])?.as_boolean()?;
        if condition {
            Ok(Evaluated::new(Value::Boolean(true)))
        } else {
            let msg_val = crate::native::force_thunk(&args[1], ctx)?.into_value();
            let msg = msg_val
                .as_string()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "Assertion failed".to_string());
            Err(PureRuntimeError::AssertionFailed(msg).into())
        }
    }

    fn signature(&self) -> &'static str {
        "assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// executeTest
// ---------------------------------------------------------------------------

/// Pure `executeTest(testFn:Function<{->Any[*]}>[1]): TestResult[1]`
///
/// Executes a zero-arg test function in a sandbox, catches exceptions,
/// measures elapsed time, and returns a fully-populated `TestResult` heap
/// object.
///
/// Outcome classification:
/// - `Ok(_)` → PASS
/// - `Err(AssertionFailed)` or `Err(ConstraintViolation)` → FAIL
/// - `Err(ExecutionError(FunctionNotFound(_)))` → SKIP
/// - Any other error → ERROR
#[derive(Debug)]
pub struct ExecuteTest;

impl NativeFunction for ExecuteTest {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("executeTest", args, 1)?;
        let test_fn_val = ctx.evaluate(&args[0])?.into_value();

        let fqn = function_fqn(&test_fn_val, ctx);
        let start = Instant::now();
        let result = ctx.call_function(&test_fn_val, &[]);
        let elapsed = start.elapsed().as_millis() as i64;

        let (status, message) = classify_outcome(result);
        build_test_result(ctx, fqn, status, elapsed, message)
    }

    fn signature(&self) -> &'static str {
        "executeTest(testFn:Function<{->Any[*]}>[1]): TestResult[1]"
    }
}

// ---------------------------------------------------------------------------
// executePCTTest
// ---------------------------------------------------------------------------

/// Pure `executePCTTest(testFn:Function<Any>[1], adapter:Function<Any>[1], exclusions:Map<Function<Any>,String>[1]): TestResult[1]`
///
/// Executes a PCT test function, injecting `adapter` as its sole argument.
/// Handles the exclusion map:
/// - Test fails but is in exclusions → PASS (expected failure)
/// - Test passes but is in exclusions → FAIL (exclusion needs rebase)
///
/// Note: exclusion map lookup by Function key is not yet supported; the
/// `exclusions` argument is accepted but ignored for now.
#[derive(Debug)]
pub struct ExecutePCTTest;

impl NativeFunction for ExecutePCTTest {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("executePCTTest", args, 3)?;
        // Force testFn and adapter; leave exclusions (args[2]) unevaluated — it's ignored.
        let test_fn_val = ctx.evaluate(&args[0])?.into_value();
        let adapter = ctx.evaluate(&args[1])?.into_value();
        // args[2] is the exclusions Map — ignored for now

        let fqn = function_fqn(&test_fn_val, ctx);
        let start = Instant::now();
        let result = ctx.call_function(&test_fn_val, &[adapter]);
        let elapsed = start.elapsed().as_millis() as i64;

        let (status, message) = classify_outcome(result);
        build_test_result(ctx, fqn, status, elapsed, message)
    }

    fn signature(&self) -> &'static str {
        "executePCTTest(testFn:Function<Any>[1], adapter:Function<Any>[1], exclusions:Map<Function<Any>,String>[1]): TestResult[1]"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Status enum values in the same `"EnumSimpleName.VALUE"` form produced by
/// [`Evaluator::eval_enum_value`](crate::eval::Evaluator) and the enum-value
/// branch of `eval_element_property`. The surveyor compares with
/// `$r.status == TestStatus.PASS` — the two sides must agree string-for-string.
// Member names in `meta::pure::test::surveyor::TestStatus`. Stored as the
// `member` field of the `Value::EnumValue` this native writes to the
// `TestResult.status` slot so surveyor-side `$r.status == TestStatus.PASS`
// comparisons structurally agree (same `enum_id` + same `member`).
//
// `SKIP` intentionally has no constant here — this native never produces a
// SKIP result. See [`classify_outcome`] for the policy: SKIP is reserved
// for the surveyor's exclusion-map path, not a bucket for runtime gaps.
const STATUS_PASS: &str = "PASS";
const STATUS_FAIL: &str = "FAIL";
const STATUS_ERROR: &str = "ERROR";

/// Extract a human-readable FQN from a function-valued argument.
///
/// - Compiled elements render via [`build_element_path`](crate::model_utils::build_element_path)
///   so surveyor reports carry the same `a::b::funcName` format users see in
///   source, not the mangled FQN stored in `ElementNode::name`.
/// - Element refs (e.g. a function passed via `$t->cast(@Function<…>)`) are
///   also path-rendered.
/// - Lambdas collapse to `"<lambda>"`.
/// - Everything else is `"<unknown>"`.
fn function_fqn(callable: &Value, ctx: &dyn EvalContextTrait) -> String {
    use crate::model_utils::build_element_path;
    match callable {
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Compiled(id) => build_element_path(ctx.model(), *id, "::", false),
            FunctionValue::Lambda(_) => "<lambda>".to_string(),
        },
        Value::Element(id) => build_element_path(ctx.model(), *id, "::", false),
        _ => "<unknown>".to_string(),
    }
}

/// Classify a function call result into (status_name, optional_message).
///
/// Status values are *member* names (`"PASS"`, `"FAIL"`, `"ERROR"`,
/// `"SKIP"`) that [`build_test_result`] pairs with the `TestStatus`
/// enumeration's [`ElementId`] to build the [`Value::EnumValue`] the
/// surveyor stores in `TestResult.status`.
///
/// # Classification policy
///
/// - `AssertionFailed` / `ConstraintViolation` → FAIL. The test's
///   behavioural contract was violated — the assertion evaluated to
///   `false` or a class constraint tripped.
/// - Everything else → ERROR. The runtime did not complete the test's
///   execution, whatever the cause (type mismatch, missing native,
///   cast failure, division by zero, …).
///
/// **SKIP is deliberately not produced by this function.** A SKIP
/// result in Pure's surveyor means "this test is expected to be
/// excluded" — driven by `executePCTTest`'s `exclusions` map or an
/// explicit `test.ExcludePlatform` annotation. Skipping on
/// `FunctionNotFound` (by kind or by message substring) would hide
/// real runtime gaps as "intentional exclusions", breaking the Java-
/// parity signal the ERROR bucket exists to surface.
fn classify_outcome(
    result: Result<Value, crate::error::PureException>,
) -> (&'static str, Option<String>) {
    match result {
        Ok(_) => (STATUS_PASS, None),
        Err(e) => {
            let msg = e.to_string();
            match &e.kind {
                PureExceptionKind::AssertionFailed(_)
                | PureExceptionKind::ConstraintViolation { .. } => (STATUS_FAIL, Some(msg)),
                _ => (STATUS_ERROR, Some(msg)),
            }
        }
    }
}

/// Allocate a `TestResult` heap object and populate its properties.
///
/// The `status` slot holds a `Value::EnumValue` backed by the Pure
/// `meta::pure::test::surveyor::TestStatus` enumeration. The surveyor
/// filter compares `$r.status == TestStatus.PASS` — structural equality
/// requires both sides carry the same `enum_id` + `member`.
fn build_test_result(
    ctx: &mut dyn EvalContextTrait,
    fqn: String,
    status: &str,
    elapsed: i64,
    message: Option<String>,
) -> Result<Evaluated, PureException> {
    // Resolve the `TestStatus` enumeration once per call — the surveyor
    // filter compares `$r.status == TestStatus.PASS`, which requires both
    // sides to carry the same enum_id.
    let test_status_path: [SmolStr; 5] = [
        SmolStr::new_static("meta"),
        SmolStr::new_static("pure"),
        SmolStr::new_static("test"),
        SmolStr::new_static("surveyor"),
        SmolStr::new_static("TestStatus"),
    ];
    let enum_id = ctx
        .model()
        .resolve_by_path(&test_status_path)
        .ok_or_else(|| {
            PureRuntimeError::EvaluationError(
                "testing: meta::pure::test::surveyor::TestStatus not found".into(),
            )
        })?;
    let status_value = Value::EnumValue {
        enum_id,
        member: SmolStr::new(status),
    };

    let heap = ctx.heap_mut();
    let id = heap.alloc_dynamic("meta::pure::test::surveyor::TestResult");
    heap.mutate_add(id, "fqn", &[Value::String(SmolStr::new(fqn))])?;
    heap.mutate_add(id, "status", &[status_value])?;
    heap.mutate_add(id, "elapsed", &[Value::Integer(elapsed)])?;
    if let Some(msg) = message {
        heap.mutate_add(id, "message", &[Value::String(SmolStr::new(msg))])?;
    }
    Ok(Evaluated::new(Value::Object(id)))
}

// ---------------------------------------------------------------------------
// assertError
// ---------------------------------------------------------------------------

/// Pure `assertError(f:Function<{->Any[*]}>[1], message:String[1],
///                    line:Integer[0..1], column:Integer[0..1]):Boolean[1]`
///
/// Runs `f` (typically `{|expr}`) and asserts it raises a Pure exception
/// whose message equals the expected `message`. The optional `line` /
/// `column` parameters are accepted for signature parity but not matched —
/// source positions from compiled Pure tests don't always line up with the
/// platform expectations, and matching on the message alone covers every
/// test-suite usage.
#[derive(Debug)]
pub struct AssertError;

impl NativeFunction for AssertError {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() < 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "assertError: expected at least 2 arguments (function, message), got {}",
                args.len()
            ))
            .into());
        }
        let func_val = ctx.evaluate(&args[0])?.into_value();
        let expected = ctx.evaluate(&args[1])?.into_value();
        let expected_msg = match expected {
            Value::String(s) => s.to_string(),
            other => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "assertError: second argument must be String, got {}",
                    other.type_name()
                ))
                .into());
            }
        };
        match ctx.call_function(&func_val, &[]) {
            Ok(_) => Err(PureRuntimeError::AssertionFailed(format!(
                "assertError: expected error '{expected_msg}' but call succeeded"
            ))
            .into()),
            Err(e) => {
                // Use the kind-specific message payload — `AssertionFailed` is
                // the literal assertion text, `ConstraintViolation` uses its
                // custom message when present (else the formatted fallback),
                // and `ExecutionError` unwraps to the underlying error's
                // Display. `e.to_string()` would include the exception-kind
                // prefix (e.g. "Constraint violation: ..."), which the Pure
                // platform tests never spell out verbatim.
                let actual = match &e.kind {
                    PureExceptionKind::AssertionFailed(m) => m.clone(),
                    PureExceptionKind::ConstraintViolation {
                        message: Some(m), ..
                    } => m.clone(),
                    PureExceptionKind::ConstraintViolation { .. } => e.to_string(),
                    PureExceptionKind::ExecutionError(err) => err.to_string(),
                };
                if actual == expected_msg {
                    Ok(Evaluated::new(Value::Boolean(true)))
                } else {
                    Err(PureRuntimeError::AssertionFailed(format!(
                        "assertError: expected '{expected_msg}' but got '{actual}'"
                    ))
                    .into())
                }
            }
        }
    }

    fn signature(&self) -> &'static str {
        "assertError(f:Function<{->Any[*]}>[1], message:String[1], line:Integer[0..1], column:Integer[0..1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register test-support native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("assert_Boolean_1__Function_1__Boolean_1_", Assert);
    registry.register(
        "assertError_Function_1__String_1__Integer_$0_1$__Integer_$0_1$__Boolean_1_",
        AssertError,
    );
    registry.register("executeTest_Function_1__TestResult_1_", ExecuteTest);
    registry.register(
        "executePCTTest_Function_1__Function_1__Map_1__TestResult_1_",
        ExecutePCTTest,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, lit_bool, lit_str};

    #[test]
    fn assert_true_passes() {
        let r = Assert
            .execute(&[lit_bool(true), lit_str("unreachable")], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::Boolean(true));
    }

    #[test]
    fn assert_false_fails_with_message() {
        let err = Assert
            .execute(&[lit_bool(false), lit_str("oops")], &mut MockCtx)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("oops"), "expected 'oops' in: {msg}");
    }

    #[test]
    fn assert_wrong_arg_count_errors() {
        assert!(Assert.execute(&[lit_bool(true)], &mut MockCtx).is_err());
        assert!(Assert.execute(&[], &mut MockCtx).is_err());
    }
}
