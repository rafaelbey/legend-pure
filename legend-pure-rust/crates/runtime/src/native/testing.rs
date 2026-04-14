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

//! Test assertion native functions: `assert`.
//!
//! These support the PCT (Pure Compatibility Tests) and user-authored
//! Pure test suites.

use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

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
// Registration
// ---------------------------------------------------------------------------

/// Register test assertion native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("assert_Boolean_1__Function_1__Boolean_1_", Assert);
}
