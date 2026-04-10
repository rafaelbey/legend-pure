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

//! Boolean native functions: `and`, `or`, `not`.

use crate::error::PureRuntimeError;
use crate::native::{NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

/// Pure `and(Boolean[1], Boolean[1]): Boolean[1]`
#[derive(Debug)]
pub struct And;

impl NativeFunction for And {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("and", args, 2)?;
        let a = args[0].as_boolean()?;
        let b = args[1].as_boolean()?;
        Ok(Value::Boolean(a && b))
    }

    fn signature(&self) -> &'static str {
        "and(Boolean[1], Boolean[1]): Boolean[1]"
    }
}

/// Pure `or(Boolean[1], Boolean[1]): Boolean[1]`
#[derive(Debug)]
pub struct Or;

impl NativeFunction for Or {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("or", args, 2)?;
        let a = args[0].as_boolean()?;
        let b = args[1].as_boolean()?;
        Ok(Value::Boolean(a || b))
    }

    fn signature(&self) -> &'static str {
        "or(Boolean[1], Boolean[1]): Boolean[1]"
    }
}

/// Pure `not(Boolean[1]): Boolean[1]`
#[derive(Debug)]
pub struct Not;

impl NativeFunction for Not {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("not", args, 1)?;
        let a = args[0].as_boolean()?;
        Ok(Value::Boolean(!a))
    }

    fn signature(&self) -> &'static str {
        "not(Boolean[1]): Boolean[1]"
    }
}

/// Register all boolean native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("and", And);
    registry.register("or", Or);
    registry.register("not", Not);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn and_true_true() {
        assert_eq!(
            And.execute(&[Value::Boolean(true), Value::Boolean(true)])
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn and_true_false() {
        assert_eq!(
            And.execute(&[Value::Boolean(true), Value::Boolean(false)])
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn or_false_true() {
        assert_eq!(
            Or.execute(&[Value::Boolean(false), Value::Boolean(true)])
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn not_true() {
        assert_eq!(
            Not.execute(&[Value::Boolean(true)]).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn type_error_on_non_boolean() {
        assert!(
            And.execute(&[Value::Integer(1), Value::Boolean(true)])
                .is_err()
        );
    }
}
