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

//! Collection native functions (non-lambda): `size`, `isEmpty`, `at`,
//! `first`, `last`, `range`, `take`, `drop`, `concatenate`.
//!
//! Lambda-dependent functions (`map`, `filter`, `fold`) are handled
//! directly by the evaluator, not through this registry.

use im_rc::Vector as PVector;

use crate::error::PureRuntimeError;
use crate::native::{NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// size
// ---------------------------------------------------------------------------

/// Pure `size(Any[*]): Integer[1]` — number of elements in a collection.
#[derive(Debug)]
pub struct Size;

impl NativeFunction for Size {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("size", args, 1)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_possible_wrap)]
        Ok(Value::Integer(coll.len() as i64))
    }

    fn signature(&self) -> &'static str {
        "size(Any[*]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// isEmpty
// ---------------------------------------------------------------------------

/// Pure `isEmpty(Any[*]): Boolean[1]`
#[derive(Debug)]
pub struct IsEmpty;

impl NativeFunction for IsEmpty {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("isEmpty", args, 1)?;
        Ok(Value::Boolean(args[0].is_empty()))
    }

    fn signature(&self) -> &'static str {
        "isEmpty(Any[*]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// at
// ---------------------------------------------------------------------------

/// Pure `at(Any[*], Integer[1]): Any[1]` — element at index.
///
/// Pure uses 0-based indexing.
#[derive(Debug)]
pub struct At;

impl NativeFunction for At {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("at", args, 2)?;
        let coll = args[0].to_collection();
        let idx = args[1].as_integer()?;
        if idx < 0 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "at: index cannot be negative, got {idx}"
            )));
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let idx = idx as usize;
        coll.get(idx).cloned().ok_or_else(|| {
            PureRuntimeError::EvaluationError(format!(
                "at: index {idx} out of bounds for collection of size {}",
                coll.len()
            ))
        })
    }

    fn signature(&self) -> &'static str {
        "at(Any[*], Integer[1]): Any[1]"
    }
}

// ---------------------------------------------------------------------------
// first / last
// ---------------------------------------------------------------------------

/// Pure `first(Any[*]): Any[0..1]` — first element or Unit.
#[derive(Debug)]
pub struct First;

impl NativeFunction for First {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("first", args, 1)?;
        match &args[0] {
            Value::Collection(v) => Ok(v.front().cloned().unwrap_or(Value::Unit)),
            Value::Unit => Ok(Value::Unit),
            other => Ok(other.clone()), // scalar is its own first
        }
    }

    fn signature(&self) -> &'static str {
        "first(Any[*]): Any[0..1]"
    }
}

/// Pure `last(Any[*]): Any[0..1]` — last element or Unit.
#[derive(Debug)]
pub struct Last;

impl NativeFunction for Last {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("last", args, 1)?;
        match &args[0] {
            Value::Collection(v) => Ok(v.back().cloned().unwrap_or(Value::Unit)),
            Value::Unit => Ok(Value::Unit),
            other => Ok(other.clone()),
        }
    }

    fn signature(&self) -> &'static str {
        "last(Any[*]): Any[0..1]"
    }
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

/// Pure `range(Integer[1], Integer[1], Integer[1]): Integer[*]`
///
/// `range(start, stop, step)` — generates integers from start (inclusive)
/// to stop (exclusive) with the given step.
#[derive(Debug)]
pub struct Range;

impl NativeFunction for Range {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("range", args, 3)?;
        let start = args[0].as_integer()?;
        let end_exclusive = args[1].as_integer()?;
        let increment = args[2].as_integer()?;
        if increment == 0 {
            return Err(PureRuntimeError::EvaluationError(
                "range: step cannot be zero".into(),
            ));
        }
        let mut result = PVector::new();
        let mut i = start;
        if increment > 0 {
            while i < end_exclusive {
                result.push_back(Value::Integer(i));
                i += increment;
            }
        } else {
            while i > end_exclusive {
                result.push_back(Value::Integer(i));
                i += increment;
            }
        }
        Ok(Value::Collection(result))
    }

    fn signature(&self) -> &'static str {
        "range(Integer[1], Integer[1], Integer[1]): Integer[*]"
    }
}

// ---------------------------------------------------------------------------
// take / drop
// ---------------------------------------------------------------------------

/// Pure `take(Any[*], Integer[1]): Any[*]` — first N elements.
#[derive(Debug)]
pub struct Take;

impl NativeFunction for Take {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("take", args, 2)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = args[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Value::Collection(coll.take(n)))
    }

    fn signature(&self) -> &'static str {
        "take(Any[*], Integer[1]): Any[*]"
    }
}

/// Pure `drop(Any[*], Integer[1]): Any[*]` — all elements after first N.
#[derive(Debug)]
pub struct Drop;

impl NativeFunction for Drop {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("drop", args, 2)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = args[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Value::Collection(coll.skip(n)))
    }

    fn signature(&self) -> &'static str {
        "drop(Any[*], Integer[1]): Any[*]"
    }
}

// ---------------------------------------------------------------------------
// concatenate
// ---------------------------------------------------------------------------

/// Pure `concatenate(Any[*], Any[*]): Any[*]` — concatenate two collections.
#[derive(Debug)]
pub struct Concatenate;

impl NativeFunction for Concatenate {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("concatenate", args, 2)?;
        let mut a = args[0].to_collection();
        let b = args[1].to_collection();
        a.append(b);
        Ok(Value::Collection(a))
    }

    fn signature(&self) -> &'static str {
        "concatenate(Any[*], Any[*]): Any[*]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all collection native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("size", Size);
    registry.register("isEmpty", IsEmpty);
    registry.register("at", At);
    registry.register("first", First);
    registry.register("last", Last);
    registry.register("range", Range);
    registry.register("take", Take);
    registry.register("drop", Drop);
    registry.register("concatenate", Concatenate);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn int_collection(items: &[i64]) -> Value {
        let mut v = PVector::new();
        for &i in items {
            v.push_back(Value::Integer(i));
        }
        Value::Collection(v)
    }

    #[test]
    fn size_of_collection() {
        assert_eq!(
            Size.execute(&[int_collection(&[1, 2, 3])]).unwrap(),
            Value::Integer(3)
        );
    }

    #[test]
    fn size_of_scalar() {
        assert_eq!(
            Size.execute(&[Value::Integer(42)]).unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn size_of_unit() {
        assert_eq!(Size.execute(&[Value::Unit]).unwrap(), Value::Integer(0));
    }

    #[test]
    fn is_empty_true() {
        assert_eq!(
            IsEmpty.execute(&[Value::Unit]).unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn is_empty_false() {
        assert_eq!(
            IsEmpty.execute(&[Value::Integer(1)]).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn at_valid_index() {
        assert_eq!(
            At.execute(&[int_collection(&[10, 20, 30]), Value::Integer(1)])
                .unwrap(),
            Value::Integer(20)
        );
    }

    #[test]
    fn at_out_of_bounds() {
        assert!(
            At.execute(&[int_collection(&[10, 20]), Value::Integer(5)])
                .is_err()
        );
    }

    #[test]
    fn first_of_collection() {
        assert_eq!(
            First.execute(&[int_collection(&[10, 20, 30])]).unwrap(),
            Value::Integer(10)
        );
    }

    #[test]
    fn first_of_empty() {
        assert_eq!(First.execute(&[Value::Unit]).unwrap(), Value::Unit);
    }

    #[test]
    fn last_of_collection() {
        assert_eq!(
            Last.execute(&[int_collection(&[10, 20, 30])]).unwrap(),
            Value::Integer(30)
        );
    }

    #[test]
    fn range_ascending() {
        let r = Range
            .execute(&[Value::Integer(0), Value::Integer(5), Value::Integer(1)])
            .unwrap();
        assert_eq!(r, int_collection(&[0, 1, 2, 3, 4]));
    }

    #[test]
    fn range_with_step() {
        let r = Range
            .execute(&[Value::Integer(0), Value::Integer(10), Value::Integer(3)])
            .unwrap();
        assert_eq!(r, int_collection(&[0, 3, 6, 9]));
    }

    #[test]
    fn range_descending() {
        let r = Range
            .execute(&[Value::Integer(5), Value::Integer(0), Value::Integer(-1)])
            .unwrap();
        assert_eq!(r, int_collection(&[5, 4, 3, 2, 1]));
    }

    #[test]
    fn take_elements() {
        assert_eq!(
            Take.execute(&[int_collection(&[1, 2, 3, 4, 5]), Value::Integer(3)])
                .unwrap(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn drop_elements() {
        assert_eq!(
            Drop.execute(&[int_collection(&[1, 2, 3, 4, 5]), Value::Integer(2)])
                .unwrap(),
            int_collection(&[3, 4, 5])
        );
    }

    #[test]
    fn concatenate_collections() {
        assert_eq!(
            Concatenate
                .execute(&[int_collection(&[1, 2]), int_collection(&[3, 4])])
                .unwrap(),
            int_collection(&[1, 2, 3, 4])
        );
    }
}
