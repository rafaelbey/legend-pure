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

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
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

/// Pure `eq(Any[1], Any[1]): Boolean[1]` — identity / primitive equality.
///
/// For primitive values: compares by value.
/// For objects: compares identity (`ObjectId == ObjectId`), not properties.
/// This is NOT the same as `equal` — two objects with identical properties
/// are `equal` but not `eq`.
#[derive(Debug)]
pub struct Eq;

impl NativeFunction for Eq {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("eq", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(values[0] == values[1])))
    }

    fn signature(&self) -> &'static str {
        "eq(Any[1], Any[1]): Boolean[1]"
    }
}

/// Pure `equal(Any[*], Any[*]): Boolean[1]` — structural / value equality.
///
/// For primitives: compares by value (same as `eq`).
/// For objects: key-property structural equality (deep); currently deferred —
/// falls back to identity (`ObjectId == ObjectId`) for object comparisons.
/// For collections: element-wise equality via `Value::PartialEq`.
#[derive(Debug)]
pub struct Equal;

impl NativeFunction for Equal {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("equal", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(values[0] == values[1])))
    }

    fn signature(&self) -> &'static str {
        "equal(Any[*], Any[*]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// lessThan / lessThanEqual / greaterThan / greaterThanEqual
// ---------------------------------------------------------------------------

/// Pure `lessThan(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct LessThan;

impl NativeFunction for LessThan {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("lessThan", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(
            numeric_cmp(&values[0], &values[1])?.is_lt(),
        )))
    }

    fn signature(&self) -> &'static str {
        "lessThan(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `lessThanEqual(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct LessThanEqual;

impl NativeFunction for LessThanEqual {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("lessThanEqual", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(
            !numeric_cmp(&values[0], &values[1])?.is_gt(),
        )))
    }

    fn signature(&self) -> &'static str {
        "lessThanEqual(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `greaterThan(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct GreaterThan;

impl NativeFunction for GreaterThan {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("greaterThan", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(
            numeric_cmp(&values[0], &values[1])?.is_gt(),
        )))
    }

    fn signature(&self) -> &'static str {
        "greaterThan(Number[1], Number[1]): Boolean[1]"
    }
}

/// Pure `greaterThanEqual(Number[1], Number[1]): Boolean[1]`
#[derive(Debug)]
pub struct GreaterThanEqual;

impl NativeFunction for GreaterThanEqual {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("greaterThanEqual", &values, 2)?;
        Ok(Evaluated::new(Value::Boolean(
            !numeric_cmp(&values[0], &values[1])?.is_lt(),
        )))
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
    registry.register("eq_Any_1__Any_1__Boolean_1_", Eq);
    registry.register("equal_Any_MANY__Any_MANY__Boolean_1_", Equal);
    registry.register("lessThan_Number_1__Number_1__Boolean_1_", LessThan);
    registry.register(
        "lessThanEqual_Number_1__Number_1__Boolean_1_",
        LessThanEqual,
    );
    registry.register("greaterThan_Number_1__Number_1__Boolean_1_", GreaterThan);
    registry.register(
        "greaterThanEqual_Number_1__Number_1__Boolean_1_",
        GreaterThanEqual,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, force_all, lit_float, lit_int, lit_str};

    #[test]
    fn equal_same_type() {
        assert_eq!(
            Equal
                .execute(&[lit_int(1), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
        assert_eq!(
            Equal
                .execute(&[lit_int(1), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn equal_different_types() {
        assert_eq!(
            Equal
                .execute(&[lit_int(1), lit_str("1")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn less_than_integers() {
        assert_eq!(
            LessThan
                .execute(&[lit_int(1), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
        assert_eq!(
            LessThan
                .execute(&[lit_int(2), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn less_than_mixed_numeric() {
        assert_eq!(
            LessThan
                .execute(&[lit_int(1), lit_float(1.5)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn greater_than_equal_boundary() {
        assert_eq!(
            GreaterThanEqual
                .execute(&[lit_int(3), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn string_comparison() {
        assert_eq!(
            LessThan
                .execute(&[lit_str("a"), lit_str("b")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn wrong_arg_count_errors() {
        assert!(Equal.execute(&[lit_int(1)], &mut MockCtx).is_err());
        assert!(LessThan.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    #[ignore = "needs Evaluator; migrate to eval_tests.rs"]
    fn invalid_type_comparisons() {}
}
