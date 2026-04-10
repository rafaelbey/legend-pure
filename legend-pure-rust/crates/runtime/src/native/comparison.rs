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

//! Comparison native functions: `equal`, `lessThan`, `lessThanEqual`,
//! `greaterThan`, `greaterThanEqual`.

use crate::error::PureRuntimeError;
use crate::native::{NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compare two numeric values, returning an `Ordering`.
/// Promotes Integer→Float when mixed.
#[allow(clippy::cast_precision_loss)]
fn numeric_cmp(a: &Value, b: &Value) -> Result<std::cmp::Ordering, PureRuntimeError> {
    match (a, b) {
        (Value::Integer(a), Value::Integer(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Integer(a), Value::Float(b)) => {
            let af = *a as f64;
            Ok(af.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Float(a), Value::Integer(b)) => {
            let bf = *b as f64;
            Ok(a.partial_cmp(&bf).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Decimal(a), Value::Decimal(b)) => Ok(a.cmp(b)),
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        _ => Err(PureRuntimeError::EvaluationError(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

// ---------------------------------------------------------------------------
// equal
// ---------------------------------------------------------------------------

/// Pure `equal(Any[1], Any[1]): Boolean[1]` — structural equality.
#[derive(Debug)]
pub struct Equal;

impl NativeFunction for Equal {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("equal", args, 2)?;
        Ok(Value::Boolean(args[0] == args[1]))
    }

    fn signature(&self) -> &'static str {
        "equal(Any[1], Any[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// lessThan / lessThanEqual / greaterThan / greaterThanEqual
// ---------------------------------------------------------------------------

/// Pure `lessThan(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct LessThan;

impl NativeFunction for LessThan {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("lessThan", args, 2)?;
        Ok(Value::Boolean(numeric_cmp(&args[0], &args[1])?.is_lt()))
    }

    fn signature(&self) -> &'static str {
        "lessThan(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `lessThanEqual(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct LessThanEqual;

impl NativeFunction for LessThanEqual {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("lessThanEqual", args, 2)?;
        Ok(Value::Boolean(!numeric_cmp(&args[0], &args[1])?.is_gt()))
    }

    fn signature(&self) -> &'static str {
        "lessThanEqual(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `greaterThan(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct GreaterThan;

impl NativeFunction for GreaterThan {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("greaterThan", args, 2)?;
        Ok(Value::Boolean(numeric_cmp(&args[0], &args[1])?.is_gt()))
    }

    fn signature(&self) -> &'static str {
        "greaterThan(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `greaterThanEqual(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct GreaterThanEqual;

impl NativeFunction for GreaterThanEqual {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("greaterThanEqual", args, 2)?;
        Ok(Value::Boolean(!numeric_cmp(&args[0], &args[1])?.is_lt()))
    }

    fn signature(&self) -> &'static str {
        "greaterThanEqual(Number[1], Number[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all comparison native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("equal", Equal);
    registry.register("lessThan", LessThan);
    registry.register("lessThanEqual", LessThanEqual);
    registry.register("greaterThan", GreaterThan);
    registry.register("greaterThanEqual", GreaterThanEqual);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_same_type() {
        assert_eq!(
            Equal
                .execute(&[Value::Integer(1), Value::Integer(1)])
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            Equal
                .execute(&[Value::Integer(1), Value::Integer(2)])
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn equal_different_types() {
        assert_eq!(
            Equal
                .execute(&[Value::Integer(1), Value::String("1".into())])
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn less_than_integers() {
        assert_eq!(
            LessThan
                .execute(&[Value::Integer(1), Value::Integer(2)])
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            LessThan
                .execute(&[Value::Integer(2), Value::Integer(1)])
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn less_than_mixed_numeric() {
        assert_eq!(
            LessThan
                .execute(&[Value::Integer(1), Value::Float(1.5)])
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn greater_than_equal_boundary() {
        assert_eq!(
            GreaterThanEqual
                .execute(&[Value::Integer(3), Value::Integer(3)])
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn string_comparison() {
        assert_eq!(
            LessThan
                .execute(&[Value::String("a".into()), Value::String("b".into())])
                .unwrap(),
            Value::Boolean(true)
        );
    }
}
