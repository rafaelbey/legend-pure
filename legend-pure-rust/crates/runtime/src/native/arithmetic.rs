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

//! Arithmetic native functions: `plus`, `minus`, `times`, `divide`, `abs`, `mod`, `rem`.

use rust_decimal::prelude::ToPrimitive;

use crate::error::PureRuntimeError;
use crate::native::{NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// plus — polymorphic addition (Integer, Float, Decimal)
// ---------------------------------------------------------------------------

/// Pure `plus(Number[1], Number[1]): Number[1]` — addition.
///
/// Type promotion rules (matching Java):
/// - Integer + Integer → Integer
/// - Float + Float → Float
/// - Integer + Float | Float + Integer → Float
/// - Decimal + Decimal → Decimal
/// - Decimal + Integer | Integer + Decimal → Decimal
#[derive(Debug)]
pub struct Plus;

impl NativeFunction for Plus {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("plus", args, 2)?;
        match (&args[0], &args[1]) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_add(*b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            #[allow(clippy::cast_precision_loss)]
            (Value::Integer(a), Value::Float(b)) | (Value::Float(b), Value::Integer(a)) => {
                Ok(Value::Float(*a as f64 + b))
            }
            (Value::Decimal(a), Value::Decimal(b)) => Ok(Value::Decimal(*a + *b)),
            (Value::Decimal(a), Value::Integer(b)) | (Value::Integer(b), Value::Decimal(a)) => {
                Ok(Value::Decimal(*a + rust_decimal::Decimal::from(*b)))
            }
            _ => Err(PureRuntimeError::EvaluationError(format!(
                "plus: unsupported types {} and {}",
                args[0].type_name(),
                args[1].type_name()
            ))),
        }
    }

    fn signature(&self) -> &'static str {
        "plus(Number[1], Number[1]): Number[1]"
    }
}

// ---------------------------------------------------------------------------
// minus
// ---------------------------------------------------------------------------

/// Pure `minus(Number[1], Number[1]): Number[1]` — subtraction.
#[derive(Debug)]
pub struct Minus;

impl NativeFunction for Minus {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("minus", args, 2)?;
        match (&args[0], &args[1]) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_sub(*b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            #[allow(clippy::cast_precision_loss)]
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
            #[allow(clippy::cast_precision_loss)]
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a - *b as f64)),
            (Value::Decimal(a), Value::Decimal(b)) => Ok(Value::Decimal(*a - *b)),
            (Value::Decimal(a), Value::Integer(b)) => {
                Ok(Value::Decimal(*a - rust_decimal::Decimal::from(*b)))
            }
            (Value::Integer(a), Value::Decimal(b)) => {
                Ok(Value::Decimal(rust_decimal::Decimal::from(*a) - *b))
            }
            _ => Err(PureRuntimeError::EvaluationError(format!(
                "minus: unsupported types {} and {}",
                args[0].type_name(),
                args[1].type_name()
            ))),
        }
    }

    fn signature(&self) -> &'static str {
        "minus(Number[1], Number[1]): Number[1]"
    }
}

// ---------------------------------------------------------------------------
// times
// ---------------------------------------------------------------------------

/// Pure `times(Number[1], Number[1]): Number[1]` — multiplication.
#[derive(Debug)]
pub struct Times;

impl NativeFunction for Times {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("times", args, 2)?;
        match (&args[0], &args[1]) {
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_mul(*b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            #[allow(clippy::cast_precision_loss)]
            (Value::Integer(a), Value::Float(b)) | (Value::Float(b), Value::Integer(a)) => {
                Ok(Value::Float(*a as f64 * b))
            }
            (Value::Decimal(a), Value::Decimal(b)) => Ok(Value::Decimal(*a * *b)),
            (Value::Decimal(a), Value::Integer(b)) | (Value::Integer(b), Value::Decimal(a)) => {
                Ok(Value::Decimal(*a * rust_decimal::Decimal::from(*b)))
            }
            _ => Err(PureRuntimeError::EvaluationError(format!(
                "times: unsupported types {} and {}",
                args[0].type_name(),
                args[1].type_name()
            ))),
        }
    }

    fn signature(&self) -> &'static str {
        "times(Number[1], Number[1]): Number[1]"
    }
}

// ---------------------------------------------------------------------------
// divide
// ---------------------------------------------------------------------------

/// Pure `divide(Number[1], Number[1]): Float[1]` — division.
///
/// Pure division always returns Float (even for Integer / Integer),
/// matching Java behavior.
#[derive(Debug)]
pub struct Divide;

impl NativeFunction for Divide {
    #[allow(clippy::cast_precision_loss)]
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("divide", args, 2)?;
        match (&args[0], &args[1]) {
            (Value::Integer(_), Value::Integer(0)) => Err(PureRuntimeError::DivisionByZero),
            (Value::Float(_), Value::Float(b)) if *b == 0.0 => {
                Err(PureRuntimeError::DivisionByZero)
            }
            (Value::Integer(a), Value::Integer(b)) => Ok(Value::Float(*a as f64 / *b as f64)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 / b)),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a / *b as f64)),
            (Value::Decimal(a), Value::Decimal(b)) => {
                if b.is_zero() {
                    return Err(PureRuntimeError::DivisionByZero);
                }
                // Decimal division → Float to match Pure semantics
                Ok(Value::Float(
                    a.to_f64().unwrap_or(f64::NAN) / b.to_f64().unwrap_or(f64::NAN),
                ))
            }
            _ => Err(PureRuntimeError::EvaluationError(format!(
                "divide: unsupported types {} and {}",
                args[0].type_name(),
                args[1].type_name()
            ))),
        }
    }

    fn signature(&self) -> &'static str {
        "divide(Number[1], Number[1]): Float[1]"
    }
}

// ---------------------------------------------------------------------------
// abs
// ---------------------------------------------------------------------------

/// Pure `abs(Number[1]): Number[1]` — absolute value.
#[derive(Debug)]
pub struct Abs;

impl NativeFunction for Abs {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("abs", args, 1)?;
        match &args[0] {
            Value::Integer(i) => Ok(Value::Integer(i.wrapping_abs())),
            Value::Float(f) => Ok(Value::Float(f.abs())),
            Value::Decimal(d) => Ok(Value::Decimal(d.abs())),
            _ => Err(PureRuntimeError::type_mismatch("Number", &args[0])),
        }
    }

    fn signature(&self) -> &'static str {
        "abs(Number[1]): Number[1]"
    }
}

// ---------------------------------------------------------------------------
// mod / rem
// ---------------------------------------------------------------------------

/// Pure `mod(Integer[1], Integer[1]): Integer[1]` — modulus (always non-negative).
#[derive(Debug)]
pub struct Mod;

impl NativeFunction for Mod {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("mod", args, 2)?;
        let a = args[0].as_integer()?;
        let b = args[1].as_integer()?;
        if b == 0 {
            return Err(PureRuntimeError::DivisionByZero);
        }
        Ok(Value::Integer(a.rem_euclid(b)))
    }

    fn signature(&self) -> &'static str {
        "mod(Integer[1], Integer[1]): Integer[1]"
    }
}

/// Pure `rem(Number[1], Number[1]): Number[1]` — remainder (preserves sign of dividend).
#[derive(Debug)]
pub struct Rem;

impl NativeFunction for Rem {
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError> {
        expect_args("rem", args, 2)?;
        match (&args[0], &args[1]) {
            (Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    return Err(PureRuntimeError::DivisionByZero);
                }
                Ok(Value::Integer(a % b))
            }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
            _ => Err(PureRuntimeError::EvaluationError(
                "rem: both arguments must be the same numeric type".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "rem(Number[1], Number[1]): Number[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all arithmetic native functions into the registry.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("plus", Plus);
    registry.register("minus", Minus);
    registry.register("times", Times);
    registry.register("divide", Divide);
    registry.register("abs", Abs);
    registry.register("mod", Mod);
    registry.register("rem", Rem);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn plus_integers() {
        let r = Plus
            .execute(&[Value::Integer(2), Value::Integer(3)])
            .unwrap();
        assert_eq!(r, Value::Integer(5));
    }

    #[test]
    fn plus_floats() {
        let r = Plus
            .execute(&[Value::Float(1.5), Value::Float(2.5)])
            .unwrap();
        assert_eq!(r, Value::Float(4.0));
    }

    #[test]
    fn plus_integer_float_promotion() {
        let r = Plus
            .execute(&[Value::Integer(1), Value::Float(2.5)])
            .unwrap();
        assert_eq!(r, Value::Float(3.5));
    }

    #[test]
    fn plus_decimals() {
        let a = Decimal::from_str("10.50").unwrap();
        let b = Decimal::from_str("3.25").unwrap();
        let r = Plus
            .execute(&[Value::Decimal(a), Value::Decimal(b)])
            .unwrap();
        assert_eq!(r, Value::Decimal(Decimal::from_str("13.75").unwrap()));
    }

    #[test]
    fn minus_integers() {
        let r = Minus
            .execute(&[Value::Integer(10), Value::Integer(3)])
            .unwrap();
        assert_eq!(r, Value::Integer(7));
    }

    #[test]
    fn times_integers() {
        let r = Times
            .execute(&[Value::Integer(4), Value::Integer(5)])
            .unwrap();
        assert_eq!(r, Value::Integer(20));
    }

    #[test]
    fn divide_integers_returns_float() {
        let r = Divide
            .execute(&[Value::Integer(7), Value::Integer(2)])
            .unwrap();
        assert_eq!(r, Value::Float(3.5));
    }

    #[test]
    fn divide_by_zero_errors() {
        let r = Divide.execute(&[Value::Integer(1), Value::Integer(0)]);
        assert!(r.is_err());
    }

    #[test]
    fn abs_negative() {
        assert_eq!(
            Abs.execute(&[Value::Integer(-5)]).unwrap(),
            Value::Integer(5)
        );
    }

    #[test]
    fn mod_positive() {
        assert_eq!(
            Mod.execute(&[Value::Integer(7), Value::Integer(3)])
                .unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn mod_negative_dividend() {
        // rem_euclid: -7 mod 3 = 2 (always non-negative)
        assert_eq!(
            Mod.execute(&[Value::Integer(-7), Value::Integer(3)])
                .unwrap(),
            Value::Integer(2)
        );
    }

    #[test]
    fn rem_negative_dividend() {
        // Rust remainder: -7 % 3 = -1 (preserves sign)
        assert_eq!(
            Rem.execute(&[Value::Integer(-7), Value::Integer(3)])
                .unwrap(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn wrong_arg_count_errors() {
        assert!(Plus.execute(&[Value::Integer(1)]).is_err());
        assert!(Abs.execute(&[]).is_err());
    }
}
