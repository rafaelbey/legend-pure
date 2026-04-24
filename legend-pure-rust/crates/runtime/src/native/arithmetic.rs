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
//!
//! All arithmetic operators are **eager** — the native forces every argument
//! up front via `ctx.evaluate`. Helpers return `PureRuntimeError`; `?` on
//! each call promotes that into the `PureException` the trait requires.

use legend_pure_parser_pure::types::ValueSpec;
use rust_decimal::prelude::ToPrimitive;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

// ---------------------------------------------------------------------------
// plus — polymorphic addition (Integer, Float, Decimal, String)
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
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Pure's sole numeric `plus` signature is `(Number[*]):Number[1]`
        // (plus `(Decimal[*]):Decimal[1]`). The compiler lowers `a + b`
        // to `plus([a, b])` so the native always receives a single
        // argument. String concatenation is a distinct native
        // (`StringPlus`, mangled `plus_String_MANY__String_1_`) —
        // compile-time dispatch must route String operands there; this
        // native strictly handles numerics and errors on String input.
        expect_args("plus", &values, 1)?;
        let items: Vec<Value> = values[0].to_collection().iter().cloned().collect();
        Ok(Evaluated::new(plus_fold(items)?))
    }

    fn signature(&self) -> &'static str {
        "plus(Number[*]): Number[1]"
    }
}

/// Pairwise `plus` — the promotion matrix for the numeric `plus` shapes
/// (Integer / Float / Decimal). String concatenation has its own
/// dedicated native (`StringPlus`, registered as
/// `plus_String_MANY__String_1_`), so this fold never sees strings.
fn plus_pair(a: &Value, b: &Value) -> Result<Value, PureRuntimeError> {
    match (a, b) {
        (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_add(*b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        #[allow(clippy::cast_precision_loss)]
        (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
        #[allow(clippy::cast_precision_loss)]
        (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a + *b as f64)),
        (Value::Decimal(a), Value::Decimal(b)) => Ok(Value::Decimal(*a + *b)),
        (Value::Decimal(a), Value::Integer(b)) => {
            Ok(Value::Decimal(*a + rust_decimal::Decimal::from(*b)))
        }
        (Value::Integer(a), Value::Decimal(b)) => {
            Ok(Value::Decimal(rust_decimal::Decimal::from(*a) + *b))
        }
        _ => Err(PureRuntimeError::EvaluationError(format!(
            "plus: unsupported types {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// Fold a collection of numerics (or strings) through pairwise `plus`.
///
/// Empty input → `Integer(0)`. Non-empty reduces left-to-right so types
/// follow Pure's promotion rules (Integer → Float → Decimal). Strings concat.
fn plus_fold(values: Vec<Value>) -> Result<Value, PureRuntimeError> {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return Ok(Value::Integer(0));
    };
    iter.try_fold(first, |acc, next| plus_pair(&acc, &next))
}

// ---------------------------------------------------------------------------
// minus
// ---------------------------------------------------------------------------

/// Pure `minus(Number[1], Number[1]): Number[1]` — subtraction.
#[derive(Debug)]
pub struct Minus;

impl NativeFunction for Minus {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Pure's sole `minus` signature is `(Number[*]):Number[1]`. The
        // compiler lowers `a - b` / `-x` to `minus([a, b])` / `minus([x])`
        // so the native always receives a single argument. Normalise via
        // `to_collection()` — this also handles the legacy direct-scalar
        // call shape some tests still emit from `->eval(x)`.
        expect_args("minus", &values, 1)?;
        let items: Vec<Value> = values[0].to_collection().iter().cloned().collect();
        Ok(Evaluated::new(minus_fold(items)?))
    }

    fn signature(&self) -> &'static str {
        "minus(Number[*]): Number[1]"
    }
}

/// Unary negation on a single numeric value. Unit-tagged values
/// negate the payload and keep the unit tag intact.
fn minus_unary(v: &Value) -> Result<Value, PureRuntimeError> {
    match v {
        Value::Integer(a) => Ok(Value::Integer(-a)),
        Value::Float(a) => Ok(Value::Float(-a)),
        Value::Decimal(a) => Ok(Value::Decimal(-*a)),
        Value::UnitInstance { unit_id, inner } => {
            let negated = minus_unary(inner)?;
            Ok(Value::UnitInstance {
                unit_id: *unit_id,
                inner: Box::new(negated),
            })
        }
        other => Err(PureRuntimeError::EvaluationError(format!(
            "minus: unsupported type {}",
            other.type_name()
        ))),
    }
}

/// Pairwise subtraction with Integer → Float → Decimal promotion, mirroring
/// `plus_pair`'s matrix.
fn minus_pair(a: &Value, b: &Value) -> Result<Value, PureRuntimeError> {
    match (a, b) {
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
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// Fold a collection of numerics through pairwise subtraction, mirroring
/// Java Pure's `Minus.execute` switch on size:
/// - empty → `Integer(0)`
/// - size 1 → unary negate (`0 - x = -x`)
/// - size ≥ 2 → left-fold subtract: `[x, y, z, …]` → `x - y - z - …`
fn minus_fold(values: Vec<Value>) -> Result<Value, PureRuntimeError> {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return Ok(Value::Integer(0));
    };
    match iter.next() {
        None => minus_unary(&first),
        Some(second) => {
            let initial = minus_pair(&first, &second)?;
            iter.try_fold(initial, |acc, next| minus_pair(&acc, &next))
        }
    }
}

// ---------------------------------------------------------------------------
// times
// ---------------------------------------------------------------------------

/// Pure `times(Number[1], Number[1]): Number[1]` — multiplication.
#[derive(Debug)]
pub struct Times;

impl NativeFunction for Times {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Pure's sole `times` signature is `(Number[*]):Number[1]`. The
        // compiler lowers `a * b` to `times([a, b])`; this native always
        // receives a single argument.
        expect_args("times", &values, 1)?;
        let items: Vec<Value> = values[0].to_collection().iter().cloned().collect();
        Ok(Evaluated::new(times_fold(items)?))
    }

    fn signature(&self) -> &'static str {
        "times(Number[*]): Number[1]"
    }
}

/// Pairwise multiplication with Integer → Float → Decimal promotion,
/// mirroring `plus_pair`'s matrix.
fn times_pair(a: &Value, b: &Value) -> Result<Value, PureRuntimeError> {
    match (a, b) {
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
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// Fold a collection of numerics through pairwise multiplication, mirroring
/// Java Pure's `Times.execute` switch on size:
/// - empty → `Integer(1)` (multiplicative identity)
/// - size 1 → unchanged
/// - size ≥ 2 → left-fold multiply
fn times_fold(values: Vec<Value>) -> Result<Value, PureRuntimeError> {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return Ok(Value::Integer(1));
    };
    iter.try_fold(first, |acc, next| times_pair(&acc, &next))
}

// ---------------------------------------------------------------------------
// divide
// ---------------------------------------------------------------------------

/// Pure `divide(Number[1], Number[1]): Float[1]` — division.
///
/// Pure division always returns Float (even for Integer / Integer), matching Java.
#[derive(Debug)]
pub struct Divide;

impl NativeFunction for Divide {
    #[allow(clippy::cast_precision_loss)]
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("divide", &values, 2)?;
        let v = match (&values[0], &values[1]) {
            (Value::Integer(_), Value::Integer(0)) => {
                return Err(PureRuntimeError::DivisionByZero.into());
            }
            (Value::Float(_), Value::Float(b)) if *b == 0.0 => {
                return Err(PureRuntimeError::DivisionByZero.into());
            }
            (Value::Integer(a), Value::Integer(b)) => Value::Float(*a as f64 / *b as f64),
            (Value::Float(a), Value::Float(b)) => Value::Float(a / b),
            (Value::Integer(a), Value::Float(b)) => Value::Float(*a as f64 / b),
            (Value::Float(a), Value::Integer(b)) => Value::Float(a / *b as f64),
            (Value::Decimal(a), Value::Decimal(b)) => {
                if b.is_zero() {
                    return Err(PureRuntimeError::DivisionByZero.into());
                }
                // Decimal division → Float to match Pure semantics
                Value::Float(a.to_f64().unwrap_or(f64::NAN) / b.to_f64().unwrap_or(f64::NAN))
            }
            _ => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "divide: unsupported types {} and {}",
                    values[0].type_name(),
                    values[1].type_name()
                ))
                .into());
            }
        };
        Ok(Evaluated::new(v))
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
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("abs", &values, 1)?;
        let v = match &values[0] {
            Value::Integer(i) => Value::Integer(i.wrapping_abs()),
            Value::Float(f) => Value::Float(f.abs()),
            Value::Decimal(d) => Value::Decimal(d.abs()),
            other => return Err(PureRuntimeError::type_mismatch("Number", other).into()),
        };
        Ok(Evaluated::new(v))
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
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("mod", &values, 2)?;
        let a = values[0].as_integer()?;
        let b = values[1].as_integer()?;
        if b == 0 {
            return Err(PureRuntimeError::DivisionByZero.into());
        }
        Ok(Evaluated::new(Value::Integer(a.rem_euclid(b))))
    }

    fn signature(&self) -> &'static str {
        "mod(Integer[1], Integer[1]): Integer[1]"
    }
}

/// Pure `rem(Number[1], Number[1]): Number[1]` — remainder (preserves sign of dividend).
#[derive(Debug)]
pub struct Rem;

impl NativeFunction for Rem {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("rem", &values, 2)?;
        let v = match (&values[0], &values[1]) {
            (Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    return Err(PureRuntimeError::DivisionByZero.into());
                }
                Value::Integer(a % b)
            }
            (Value::Float(a), Value::Float(b)) => Value::Float(a % b),
            _ => {
                return Err(PureRuntimeError::EvaluationError(
                    "rem: both arguments must be the same numeric type".into(),
                )
                .into());
            }
        };
        Ok(Evaluated::new(v))
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
    // plus — one shared implementation handles all numeric types
    registry.register("plus_Integer_MANY__Integer_1_", Plus);
    registry.register("plus_Float_MANY__Float_1_", Plus);
    registry.register("plus_Decimal_MANY__Decimal_1_", Plus);
    registry.register("plus_Number_MANY__Number_1_", Plus);

    // minus
    registry.register("minus_Integer_MANY__Integer_1_", Minus);
    registry.register("minus_Float_MANY__Float_1_", Minus);
    registry.register("minus_Decimal_MANY__Decimal_1_", Minus);
    registry.register("minus_Number_MANY__Number_1_", Minus);

    // times
    registry.register("times_Integer_MANY__Integer_1_", Times);
    registry.register("times_Float_MANY__Float_1_", Times);
    registry.register("times_Decimal_MANY__Decimal_1_", Times);
    registry.register("times_Number_MANY__Number_1_", Times);

    // divide (two signatures in Java)
    registry.register("divide_Number_1__Number_1__Float_1_", Divide);
    registry.register("divide_Decimal_1__Decimal_1__Integer_1__Decimal_1_", Divide);

    // abs
    registry.register("abs_Integer_1__Integer_1_", Abs);
    registry.register("abs_Float_1__Float_1_", Abs);
    registry.register("abs_Decimal_1__Decimal_1_", Abs);
    registry.register("abs_Number_1__Number_1_", Abs);

    // mod
    registry.register("mod_Integer_1__Integer_1__Integer_1_", Mod);

    // rem
    registry.register("rem_Number_1__Number_1__Number_1_", Rem);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;

    use super::*;
    use crate::native::{
        MockCtx, force_all, lit_bool, lit_collection, lit_float, lit_int, lit_str,
    };

    /// Decimal-literal shim — `MockCtx` supports only bare `DecimalLiteral`
    /// specs, so tests that need a Decimal value go through this helper
    /// rather than fabricating an ad-hoc `ValueSpec`.
    fn ld(d: Decimal) -> legend_pure_parser_pure::types::ValueSpec {
        crate::native::lit_decimal(d)
    }

    #[test]
    fn plus_integers() {
        let r = Plus
            .execute(
                &[crate::native::lit_collection(vec![lit_int(2), lit_int(3)])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::Integer(5));
    }

    #[test]
    fn plus_floats() {
        let r = Plus
            .execute(
                &[crate::native::lit_collection(vec![
                    lit_float(1.5),
                    lit_float(2.5),
                ])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::Float(4.0));
    }

    #[test]
    fn plus_integer_float_promotion() {
        let r = Plus
            .execute(
                &[crate::native::lit_collection(vec![
                    lit_int(1),
                    lit_float(2.5),
                ])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::Float(3.5));
    }

    #[test]
    fn plus_decimals() {
        let a = Decimal::from_str("10.50").unwrap();
        let b = Decimal::from_str("3.25").unwrap();
        let r = Plus
            .execute(
                &[crate::native::lit_collection(vec![ld(a), ld(b)])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(
            r.into_value(),
            Value::Decimal(Decimal::from_str("13.75").unwrap())
        );
    }

    #[test]
    fn minus_integers() {
        let r = Minus
            .execute(
                &[crate::native::lit_collection(vec![lit_int(10), lit_int(3)])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::Integer(7));
    }

    #[test]
    fn times_integers() {
        let r = Times
            .execute(
                &[crate::native::lit_collection(vec![lit_int(4), lit_int(5)])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::Integer(20));
    }

    #[test]
    fn divide_integers_returns_float() {
        let r = Divide
            .execute(&[lit_int(7), lit_int(2)], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::Float(3.5));
    }

    #[test]
    fn divide_by_zero_errors() {
        let r = Divide.execute(&[lit_int(1), lit_int(0)], &mut MockCtx);
        assert!(r.is_err());
    }

    #[test]
    fn abs_negative() {
        assert_eq!(
            Abs.execute(&[lit_int(-5)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(5)
        );
    }

    #[test]
    fn mod_positive() {
        assert_eq!(
            Mod.execute(&[lit_int(7), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(1)
        );
    }

    #[test]
    fn mod_negative_dividend() {
        // rem_euclid: -7 mod 3 = 2 (always non-negative)
        assert_eq!(
            Mod.execute(&[lit_int(-7), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(2)
        );
    }

    #[test]
    fn rem_negative_dividend() {
        // Rust remainder: -7 % 3 = -1 (preserves sign)
        assert_eq!(
            Rem.execute(&[lit_int(-7), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn wrong_arg_count_errors() {
        // `plus` takes exactly one `Number[*]` argument. A lone Integer
        // literal is normalised to a singleton collection and returns
        // unchanged. An empty collection returns the identity `Integer(0)`.
        // Zero arguments at the native-call level is an arity violation.
        assert_eq!(
            Plus.execute(&[lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(1)
        );
        assert_eq!(
            Plus.execute(&[crate::native::lit_collection(Vec::new())], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(0)
        );
        assert!(Plus.execute(&[], &mut MockCtx).is_err());
        assert!(Abs.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn plus_folds_integer_collection() {
        // `plus([1, 2, 3])` sums the collection — this is the surveyor path
        // (`$results->map(r | $r.elapsed)->plus()`).
        let spec = lit_collection(vec![lit_int(1), lit_int(2), lit_int(3)]);
        assert_eq!(
            Plus.execute(&[spec], &mut MockCtx).unwrap().into_value(),
            Value::Integer(6)
        );
    }

    #[test]
    fn type_mismatch_errors() {
        // String instead of Number
        assert!(
            Plus.execute(&[lit_str("1"), lit_int(1)], &mut MockCtx)
                .is_err()
        );
        assert!(Abs.execute(&[lit_bool(true)], &mut MockCtx).is_err());
        // One float, one string
        assert!(
            Plus.execute(&[lit_float(1.0), lit_str("2")], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn division_by_zero_errors() {
        assert!(
            Divide
                .execute(&[lit_int(5), lit_int(0)], &mut MockCtx)
                .is_err()
        );
        assert!(
            Divide
                .execute(&[lit_float(5.0), lit_float(0.0)], &mut MockCtx)
                .is_err()
        );
        assert!(
            Mod.execute(&[lit_int(5), lit_int(0)], &mut MockCtx)
                .is_err()
        );
        assert!(
            Rem.execute(&[lit_int(5), lit_int(0)], &mut MockCtx)
                .is_err()
        );
    }
}
