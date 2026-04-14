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

//! Core language native functions: `letFunction`, `if`.
//!
//! These are the fundamental control-flow primitives that the Pure
//! language desugars into. `letFunction` binds variables; `if` provides
//! short-circuiting conditionals.

use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// letFunction
// ---------------------------------------------------------------------------

/// Pure `letFunction(String[1], T[m]): T[m]`
///
/// The compiler desugars `let x = expr` → `FunctionCall("letFunction", [name, value])`.
/// This native binds the value to the variable name in the current scope
/// and returns the value.
#[derive(Debug)]
pub struct LetFunction;

impl NativeFunction for LetFunction {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("letFunction", args, 2)?;
        let name = args[0].as_string()?;
        let value = args[1].clone();
        ctx.context_mut().set(name.clone(), value.clone());
        Ok(value)
    }

    fn signature(&self) -> &'static str {
        "letFunction(String[1], T[m]): T[m]"
    }
}

// ---------------------------------------------------------------------------
// if
// ---------------------------------------------------------------------------

/// Pure `if(Boolean[1], Function<{->T[m]}>[1], Function<{->T[m]}>[1]): T[m]`
///
/// Short-circuiting conditional. Uses `defer_execution()` so the evaluator
/// wraps both branches as zero-parameter lambdas. Only the taken branch
/// is evaluated.
#[derive(Debug)]
pub struct If;

impl NativeFunction for If {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("if", args, 3)?;

        // With defer_execution, arg[0] is a lambda wrapping the boolean condition.
        // Evaluate it to get the actual boolean.
        let condition_val = ctx.eval_lambda(&args[0], &[])?;
        let condition = condition_val.as_boolean()?;

        println!("If native condition = {} args[1] = {:?}", condition, args[1]);
        if condition {
            ctx.eval_lambda(&args[1], &[])
        } else {
            ctx.eval_lambda(&args[2], &[])
        }
    }

    fn signature(&self) -> &'static str {
        "if(Boolean[1], Function<{->T[m]}>[1], Function<{->T[m]}>[1]): T[m]"
    }

    fn defer_execution(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register core language native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("letFunction_String_1__T_m__T_m_", LetFunction);
    registry.register("if_Boolean_1__Function_1__Function_1__T_m_", If);
}
