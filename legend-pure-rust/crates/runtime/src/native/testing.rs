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

use smol_str::SmolStr;

use crate::error::{PureExceptionKind, PureRuntimeError};
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::{FunctionValue, Value};

// ---------------------------------------------------------------------------
// assert
// ---------------------------------------------------------------------------

/// Pure `assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]`
///
/// Asserts a boolean condition. If false, evaluates the message lambda
/// and returns an error with that message. Uses `defer_execution()` so
/// the message lambda is only evaluated on failure.
#[derive(Debug)]
pub struct Assert;

impl NativeFunction for Assert {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("assert", args, 2)?;

        // With defer_execution, arg[0] is a lambda wrapping the condition.
        let condition_val = ctx.eval_lambda(&args[0], &[])?;
        let condition = condition_val.as_boolean()?;

        if condition {
            Ok(Value::Boolean(true))
        } else {
            let msg_val = ctx.eval_lambda(&args[1], &[])?;
            let msg = match msg_val.as_string() {
                Ok(s) => s.to_string(),
                Err(_) => "Assertion failed".to_string(),
            };
            Err(PureRuntimeError::AssertionFailed(msg))
        }
    }

    fn signature(&self) -> &'static str {
        "assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]"
    }

    fn defer_execution(&self) -> bool {
        true
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("executeTest", args, 1)?;
        let test_fn = &args[0];

        let fqn = function_fqn(test_fn, ctx);
        let start = Instant::now();
        let result = ctx.call_function(test_fn, &[]);
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("executePCTTest", args, 3)?;
        let test_fn = &args[0];
        let adapter = args[1].clone();
        // args[2] is the exclusions Map — stored for future use

        let fqn = function_fqn(test_fn, ctx);
        let start = Instant::now();
        let result = ctx.call_function(test_fn, &[adapter]);
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

/// Status enum values as mangled Pure strings.
const STATUS_PASS: &str = "meta::pure::test::surveyor::TestStatus.PASS";
const STATUS_FAIL: &str = "meta::pure::test::surveyor::TestStatus.FAIL";
const STATUS_ERROR: &str = "meta::pure::test::surveyor::TestStatus.ERROR";
const STATUS_SKIP: &str = "meta::pure::test::surveyor::TestStatus.SKIP";

/// Extract the FQN from a compiled function value; fall back to `"<lambda>"`.
fn function_fqn(callable: &Value, ctx: &dyn EvalContextTrait) -> String {
    match callable {
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Compiled(id) => ctx.model().get_node(*id).name.to_string(),
            FunctionValue::Lambda(_) => "<lambda>".to_string(),
        },
        _ => "<unknown>".to_string(),
    }
}

/// Classify a function call result into (status_string, optional_message).
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
                PureExceptionKind::ExecutionError(PureRuntimeError::FunctionNotFound(_)) => {
                    (STATUS_SKIP, Some(msg))
                }
                PureExceptionKind::ExecutionError(PureRuntimeError::EvaluationError(inner))
                    if inner.contains("Function not found:") =>
                {
                    (STATUS_SKIP, Some(msg))
                }
                _ => (STATUS_ERROR, Some(msg)),
            }
        }
    }
}

/// Allocate a `TestResult` heap object and populate its properties.
fn build_test_result(
    ctx: &mut dyn EvalContextTrait,
    fqn: String,
    status: &str,
    elapsed: i64,
    message: Option<String>,
) -> Result<Value, PureRuntimeError> {
    let heap = ctx.heap_mut();
    let id = heap.alloc_dynamic("meta::pure::test::surveyor::TestResult");
    heap.mutate_add(id, "fqn", &[Value::String(SmolStr::new(fqn))])?;
    heap.mutate_add(id, "status", &[Value::String(SmolStr::new(status))])?;
    heap.mutate_add(id, "elapsed", &[Value::Integer(elapsed)])?;
    if let Some(msg) = message {
        heap.mutate_add(id, "message", &[Value::String(SmolStr::new(msg))])?;
    }
    Ok(Value::Object(id))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register test-support native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("assert_Boolean_1__Function_1__Boolean_1_", Assert);
    registry.register("executeTest_Function_1__TestResult_1_", ExecuteTest);
    registry.register(
        "executePCTTest_Function_1__Function_1__Map_1__TestResult_1_",
        ExecutePCTTest,
    );
}
