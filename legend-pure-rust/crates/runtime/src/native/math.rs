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

//! Mathematical native functions: rounding (`floor`, `ceiling`, `round`,
//! `sign`), analytic (`sqrt`, `cbrt`, `exp`, `log`, `log10`, `pow`),
//! trigonometric (`sin`, `cos`, `tan`, `cot`, `asin`, `acos`, `atan`,
//! `atan2`), numeric conversion (`toFloat`, `toDecimal`), and string
//! parsing (`parseInteger`, `parseFloat`, `parseBoolean`).
//!
//! `Value::Integer`, `Value::Float`, and `Value::Decimal` inputs are all
//! promoted to `f64` for the transcendental operators. Integer → Float
//! promotion is handled by [`Value::as_float`]; Decimal → Float uses
//! [`rust_decimal::prelude::ToPrimitive::to_f64`] with a graceful fallback
//! to an `EvaluationError` when the conversion fails.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// Shared helper — promote any numeric Value (including Decimal) to f64
// ---------------------------------------------------------------------------

/// Coerce a numeric `Value` (Integer, Float, or Decimal) to `f64`.
///
/// `Value::as_float` handles Integer→Float already but rejects `Decimal`.
/// The math natives accept `Number`, so this wrapper adds the Decimal case
/// without mutating `Value::as_float`'s narrower contract.
fn number_to_f64(func_name: &str, v: &Value) -> Result<f64, PureRuntimeError> {
    match v {
        Value::Decimal(d) => d.to_f64().ok_or_else(|| {
            PureRuntimeError::EvaluationError(format!(
                "{func_name}: cannot convert Decimal {d} to Float"
            ))
        }),
        _ => v.as_float(),
    }
}

// ---------------------------------------------------------------------------
// floor
// ---------------------------------------------------------------------------

/// Pure `floor(Number[1]):Integer[1]` — largest integer ≤ x.
#[derive(Debug)]
pub struct Floor;

impl NativeFunction for Floor {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("floor", args, 1)?;
        let x = number_to_f64("floor", &args[0])?;
        // cast_possible_truncation acceptable here — Pure's floor returns Integer and
        // values outside i64 range are genuinely out of spec.
        #[allow(clippy::cast_possible_truncation)]
        Ok(Value::Integer(x.floor() as i64))
    }

    fn signature(&self) -> &'static str {
        "floor(Number[1]):Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// ceiling
// ---------------------------------------------------------------------------

/// Pure `ceiling(Number[1]):Integer[1]` — smallest integer ≥ x.
#[derive(Debug)]
pub struct Ceiling;

impl NativeFunction for Ceiling {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("ceiling", args, 1)?;
        let x = number_to_f64("ceiling", &args[0])?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(Value::Integer(x.ceil() as i64))
    }

    fn signature(&self) -> &'static str {
        "ceiling(Number[1]):Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// round
// ---------------------------------------------------------------------------

/// Pure `round(Number[1]):Integer[1]` — half-away-from-zero rounding.
#[derive(Debug)]
pub struct Round;

impl NativeFunction for Round {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("round", args, 1)?;
        let x = number_to_f64("round", &args[0])?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(Value::Integer(x.round() as i64))
    }

    fn signature(&self) -> &'static str {
        "round(Number[1]):Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// sign
// ---------------------------------------------------------------------------

/// Pure `sign(Number[1]):Integer[1]` — returns -1, 0, or 1.
#[derive(Debug)]
pub struct Sign;

impl NativeFunction for Sign {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("sign", args, 1)?;
        let x = number_to_f64("sign", &args[0])?;
        let s = if x > 0.0 {
            1
        } else if x < 0.0 {
            -1
        } else {
            0
        };
        Ok(Value::Integer(s))
    }

    fn signature(&self) -> &'static str {
        "sign(Number[1]):Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// sqrt
// ---------------------------------------------------------------------------

/// Pure `sqrt(Number[1]):Float[1]` — square root.
///
/// Negative inputs yield `NaN` per IEEE-754, matching Java's
/// `Math.sqrt` semantics.
#[derive(Debug)]
pub struct Sqrt;

impl NativeFunction for Sqrt {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("sqrt", args, 1)?;
        let x = number_to_f64("sqrt", &args[0])?;
        Ok(Value::Float(x.sqrt()))
    }

    fn signature(&self) -> &'static str {
        "sqrt(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// cbrt
// ---------------------------------------------------------------------------

/// Pure `cbrt(Number[1]):Float[1]` — cube root.
#[derive(Debug)]
pub struct Cbrt;

impl NativeFunction for Cbrt {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("cbrt", args, 1)?;
        let x = number_to_f64("cbrt", &args[0])?;
        Ok(Value::Float(x.cbrt()))
    }

    fn signature(&self) -> &'static str {
        "cbrt(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// exp
// ---------------------------------------------------------------------------

/// Pure `exp(Number[1]):Float[1]` — natural exponential, `e^x`.
#[derive(Debug)]
pub struct Exp;

impl NativeFunction for Exp {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("exp", args, 1)?;
        let x = number_to_f64("exp", &args[0])?;
        Ok(Value::Float(x.exp()))
    }

    fn signature(&self) -> &'static str {
        "exp(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// log (natural log)
// ---------------------------------------------------------------------------

/// Pure `log(Number[1]):Float[1]` — natural logarithm (base `e`).
///
/// `log(0)` yields `-inf`; `log(negative)` yields `NaN` per IEEE-754.
#[derive(Debug)]
pub struct Log;

impl NativeFunction for Log {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("log", args, 1)?;
        let x = number_to_f64("log", &args[0])?;
        Ok(Value::Float(x.ln()))
    }

    fn signature(&self) -> &'static str {
        "log(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// log10
// ---------------------------------------------------------------------------

/// Pure `log10(Number[1]):Float[1]` — base-10 logarithm.
#[derive(Debug)]
pub struct Log10;

impl NativeFunction for Log10 {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("log10", args, 1)?;
        let x = number_to_f64("log10", &args[0])?;
        Ok(Value::Float(x.log10()))
    }

    fn signature(&self) -> &'static str {
        "log10(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// pow
// ---------------------------------------------------------------------------

/// Pure `pow(Number[1], Number[1]):Float[1]` — `base^exponent`.
#[derive(Debug)]
pub struct Pow;

impl NativeFunction for Pow {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("pow", args, 2)?;
        let base = number_to_f64("pow", &args[0])?;
        let exp = number_to_f64("pow", &args[1])?;
        Ok(Value::Float(base.powf(exp)))
    }

    fn signature(&self) -> &'static str {
        "pow(Number[1], Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// sin / cos / tan / cot
// ---------------------------------------------------------------------------

/// Pure `sin(Number[1]):Float[1]` — sine of `x` in radians.
#[derive(Debug)]
pub struct Sin;

impl NativeFunction for Sin {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("sin", args, 1)?;
        let x = number_to_f64("sin", &args[0])?;
        Ok(Value::Float(x.sin()))
    }

    fn signature(&self) -> &'static str {
        "sin(Number[1]):Float[1]"
    }
}

/// Pure `cos(Number[1]):Float[1]` — cosine of `x` in radians.
#[derive(Debug)]
pub struct Cos;

impl NativeFunction for Cos {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("cos", args, 1)?;
        let x = number_to_f64("cos", &args[0])?;
        Ok(Value::Float(x.cos()))
    }

    fn signature(&self) -> &'static str {
        "cos(Number[1]):Float[1]"
    }
}

/// Pure `tan(Number[1]):Float[1]` — tangent of `x` in radians.
#[derive(Debug)]
pub struct Tan;

impl NativeFunction for Tan {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("tan", args, 1)?;
        let x = number_to_f64("tan", &args[0])?;
        Ok(Value::Float(x.tan()))
    }

    fn signature(&self) -> &'static str {
        "tan(Number[1]):Float[1]"
    }
}

/// Pure `cot(Number[1]):Float[1]` — cotangent (`1 / tan(x)`) of `x` in radians.
#[derive(Debug)]
pub struct Cot;

impl NativeFunction for Cot {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("cot", args, 1)?;
        let x = number_to_f64("cot", &args[0])?;
        Ok(Value::Float(1.0 / x.tan()))
    }

    fn signature(&self) -> &'static str {
        "cot(Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// asin / acos / atan / atan2
// ---------------------------------------------------------------------------

/// Pure `asin(Number[1]):Float[1]` — inverse sine (radians). Inputs
/// outside [-1, 1] yield `NaN` per IEEE-754.
#[derive(Debug)]
pub struct Asin;

impl NativeFunction for Asin {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("asin", args, 1)?;
        let x = number_to_f64("asin", &args[0])?;
        Ok(Value::Float(x.asin()))
    }

    fn signature(&self) -> &'static str {
        "asin(Number[1]):Float[1]"
    }
}

/// Pure `acos(Number[1]):Float[1]` — inverse cosine (radians). Inputs
/// outside [-1, 1] yield `NaN` per IEEE-754.
#[derive(Debug)]
pub struct Acos;

impl NativeFunction for Acos {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("acos", args, 1)?;
        let x = number_to_f64("acos", &args[0])?;
        Ok(Value::Float(x.acos()))
    }

    fn signature(&self) -> &'static str {
        "acos(Number[1]):Float[1]"
    }
}

/// Pure `atan(Number[1]):Float[1]` — inverse tangent (radians).
#[derive(Debug)]
pub struct Atan;

impl NativeFunction for Atan {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("atan", args, 1)?;
        let x = number_to_f64("atan", &args[0])?;
        Ok(Value::Float(x.atan()))
    }

    fn signature(&self) -> &'static str {
        "atan(Number[1]):Float[1]"
    }
}

/// Pure `atan2(Number[1], Number[1]):Float[1]` — four-quadrant inverse
/// tangent of `y / x`, returning the angle in radians.
#[derive(Debug)]
pub struct Atan2;

impl NativeFunction for Atan2 {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("atan2", args, 2)?;
        let y = number_to_f64("atan2", &args[0])?;
        let x = number_to_f64("atan2", &args[1])?;
        Ok(Value::Float(y.atan2(x)))
    }

    fn signature(&self) -> &'static str {
        "atan2(Number[1], Number[1]):Float[1]"
    }
}

// ---------------------------------------------------------------------------
// toFloat / toDecimal
// ---------------------------------------------------------------------------

/// Pure `toFloat(Number[1]):Float[1]` — numeric → Float coercion.
#[derive(Debug)]
pub struct ToFloat;

impl NativeFunction for ToFloat {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toFloat", args, 1)?;
        let x = number_to_f64("toFloat", &args[0])?;
        Ok(Value::Float(x))
    }

    fn signature(&self) -> &'static str {
        "toFloat(Number[1]):Float[1]"
    }
}

/// Pure `toDecimal(Number[1]):Decimal[1]` — numeric → Decimal coercion.
///
/// `Decimal` inputs pass through unchanged. `Integer` / `Float` inputs are
/// first promoted to `f64` and then routed through
/// [`Decimal::from_f64_retain`], which can fail for `NaN` / infinity —
/// in that case an `EvaluationError` is returned.
#[derive(Debug)]
pub struct ToDecimal;

impl NativeFunction for ToDecimal {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toDecimal", args, 1)?;
        match &args[0] {
            Value::Decimal(d) => Ok(Value::Decimal(*d)),
            Value::Integer(i) => Ok(Value::Decimal(Decimal::from(*i))),
            Value::Float(f) => Decimal::from_f64_retain(*f)
                .map(Value::Decimal)
                .ok_or_else(|| {
                    PureRuntimeError::EvaluationError(format!(
                        "toDecimal: cannot convert Float {f} to Decimal"
                    ))
                }),
            other => Err(PureRuntimeError::type_mismatch("Number", other)),
        }
    }

    fn signature(&self) -> &'static str {
        "toDecimal(Number[1]):Decimal[1]"
    }
}

// ---------------------------------------------------------------------------
// parseInteger / parseFloat / parseBoolean
// ---------------------------------------------------------------------------

/// Pure `parseInteger(String[1]):Integer[1]` — decimal string → `Integer`.
///
/// Returns an `EvaluationError` if the string is not a valid `i64`.
#[derive(Debug)]
pub struct ParseInteger;

impl NativeFunction for ParseInteger {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("parseInteger", args, 1)?;
        let s = args[0].as_string()?;
        s.parse::<i64>().map(Value::Integer).map_err(|e| {
            PureRuntimeError::EvaluationError(format!("parseInteger: cannot parse {s:?}: {e}"))
        })
    }

    fn signature(&self) -> &'static str {
        "parseInteger(String[1]):Integer[1]"
    }
}

/// Pure `parseFloat(String[1]):Float[1]` — decimal string → `Float`.
///
/// Returns an `EvaluationError` if the string is not a valid `f64`.
#[derive(Debug)]
pub struct ParseFloat;

impl NativeFunction for ParseFloat {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("parseFloat", args, 1)?;
        let s = args[0].as_string()?;
        s.parse::<f64>().map(Value::Float).map_err(|e| {
            PureRuntimeError::EvaluationError(format!("parseFloat: cannot parse {s:?}: {e}"))
        })
    }

    fn signature(&self) -> &'static str {
        "parseFloat(String[1]):Float[1]"
    }
}

/// Pure `parseBoolean(String[1]):Boolean[1]` — `"true"` / `"false"`
/// (case-sensitive per Pure spec).
///
/// Returns an `EvaluationError` for any other input.
#[derive(Debug)]
pub struct ParseBoolean;

impl NativeFunction for ParseBoolean {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("parseBoolean", args, 1)?;
        let s = args[0].as_string()?;
        match s.as_str() {
            "true" => Ok(Value::Boolean(true)),
            "false" => Ok(Value::Boolean(false)),
            other => Err(PureRuntimeError::EvaluationError(format!(
                "parseBoolean: expected \"true\" or \"false\", got {other:?}"
            ))),
        }
    }

    fn signature(&self) -> &'static str {
        "parseBoolean(String[1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all math native functions into the registry under their
/// mangled Pure FQNs.
pub fn register(registry: &mut NativeRegistry) {
    // rounding
    registry.register("floor_Number_1__Integer_1_", Floor);
    registry.register("ceiling_Number_1__Integer_1_", Ceiling);
    registry.register("round_Number_1__Integer_1_", Round);
    registry.register("sign_Number_1__Integer_1_", Sign);

    // analytic
    registry.register("sqrt_Number_1__Float_1_", Sqrt);
    registry.register("cbrt_Number_1__Float_1_", Cbrt);
    registry.register("exp_Number_1__Float_1_", Exp);
    registry.register("log_Number_1__Float_1_", Log);
    registry.register("log10_Number_1__Float_1_", Log10);
    registry.register("pow_Number_1__Number_1__Float_1_", Pow);

    // trigonometric
    registry.register("sin_Number_1__Float_1_", Sin);
    registry.register("cos_Number_1__Float_1_", Cos);
    registry.register("tan_Number_1__Float_1_", Tan);
    registry.register("cot_Number_1__Float_1_", Cot);
    registry.register("asin_Number_1__Float_1_", Asin);
    registry.register("acos_Number_1__Float_1_", Acos);
    registry.register("atan_Number_1__Float_1_", Atan);
    registry.register("atan2_Number_1__Number_1__Float_1_", Atan2);

    // numeric conversion
    registry.register("toFloat_Number_1__Float_1_", ToFloat);
    registry.register("toDecimal_Number_1__Decimal_1_", ToDecimal);

    // string parsing
    registry.register("parseInteger_String_1__Integer_1_", ParseInteger);
    registry.register("parseFloat_String_1__Float_1_", ParseFloat);
    registry.register("parseBoolean_String_1__Boolean_1_", ParseBoolean);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;

    use super::*;
    use crate::native::NoOpEvalCtx;

    // Small helper — abs-difference comparison for floats. Avoids pulling in
    // `approx` or hand-rolling tolerances throughout the test body.
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    // -----------------------------------------------------------------------
    // floor
    // -----------------------------------------------------------------------

    #[test]
    fn floor_positive_float() {
        let r = Floor
            .execute(&[Value::Float(3.7)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(3));
    }

    #[test]
    fn floor_negative_float() {
        let r = Floor
            .execute(&[Value::Float(-3.2)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(-4));
    }

    #[test]
    fn floor_integer_passthrough() {
        let r = Floor
            .execute(&[Value::Integer(5)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(5));
    }

    #[test]
    fn floor_decimal() {
        let d = Decimal::from_str("4.9").unwrap();
        let r = Floor
            .execute(&[Value::Decimal(d)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(4));
    }

    #[test]
    fn floor_wrong_arg_count() {
        assert!(Floor.execute(&[], &mut NoOpEvalCtx).is_err());
        assert!(
            Floor
                .execute(&[Value::Float(1.0), Value::Float(2.0)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn floor_type_mismatch() {
        assert!(
            Floor
                .execute(&[Value::String("hi".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // ceiling
    // -----------------------------------------------------------------------

    #[test]
    fn ceiling_positive_float() {
        let r = Ceiling
            .execute(&[Value::Float(3.2)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(4));
    }

    #[test]
    fn ceiling_negative_float() {
        let r = Ceiling
            .execute(&[Value::Float(-3.7)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(-3));
    }

    #[test]
    fn ceiling_wrong_arg_count() {
        assert!(Ceiling.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn ceiling_type_mismatch() {
        assert!(
            Ceiling
                .execute(&[Value::Boolean(true)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // round
    // -----------------------------------------------------------------------

    #[test]
    fn round_half_away_from_zero() {
        assert_eq!(
            Round
                .execute(&[Value::Float(2.5)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(3)
        );
        assert_eq!(
            Round
                .execute(&[Value::Float(-2.5)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(-3)
        );
    }

    #[test]
    fn round_below_half() {
        assert_eq!(
            Round
                .execute(&[Value::Float(2.4)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(2)
        );
    }

    #[test]
    fn round_wrong_arg_count() {
        assert!(Round.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn round_type_mismatch() {
        assert!(
            Round
                .execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // sign
    // -----------------------------------------------------------------------

    #[test]
    fn sign_positive() {
        assert_eq!(
            Sign.execute(&[Value::Integer(7)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn sign_negative() {
        assert_eq!(
            Sign.execute(&[Value::Float(-0.1)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn sign_zero() {
        assert_eq!(
            Sign.execute(&[Value::Integer(0)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(0)
        );
    }

    #[test]
    fn sign_wrong_arg_count() {
        assert!(Sign.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn sign_type_mismatch() {
        assert!(
            Sign.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // sqrt
    // -----------------------------------------------------------------------

    #[test]
    fn sqrt_positive() {
        let r = Sqrt
            .execute(&[Value::Integer(16)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(4.0));
    }

    #[test]
    fn sqrt_zero() {
        let r = Sqrt
            .execute(&[Value::Float(0.0)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(0.0));
    }

    #[test]
    fn sqrt_negative_is_nan() {
        // IEEE-754 sqrt of a negative yields NaN.
        let r = Sqrt
            .execute(&[Value::Float(-1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(f.is_nan(), "expected NaN, got {f}"),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn sqrt_wrong_arg_count() {
        assert!(Sqrt.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn sqrt_type_mismatch() {
        assert!(
            Sqrt.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // cbrt
    // -----------------------------------------------------------------------

    #[test]
    fn cbrt_positive() {
        let r = Cbrt
            .execute(&[Value::Float(27.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 3.0), "expected ~3.0, got {f}"),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cbrt_negative() {
        let r = Cbrt
            .execute(&[Value::Float(-8.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, -2.0), "expected ~-2.0, got {f}"),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cbrt_wrong_arg_count() {
        assert!(Cbrt.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn cbrt_type_mismatch() {
        assert!(
            Cbrt.execute(&[Value::Boolean(false)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // exp / log / log10 / pow
    // -----------------------------------------------------------------------

    #[test]
    fn exp_zero() {
        let r = Exp.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        assert_eq!(r, Value::Float(1.0));
    }

    #[test]
    fn exp_one_is_e() {
        let r = Exp.execute(&[Value::Float(1.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, std::f64::consts::E)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn exp_wrong_arg_count() {
        assert!(Exp.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn exp_type_mismatch() {
        assert!(
            Exp.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn log_one_is_zero() {
        let r = Log.execute(&[Value::Float(1.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn log_e_is_one() {
        let r = Log
            .execute(&[Value::Float(std::f64::consts::E)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn log_zero_is_neg_inf() {
        let r = Log.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(f.is_infinite() && f.is_sign_negative()),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn log_wrong_arg_count() {
        assert!(Log.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn log_type_mismatch() {
        assert!(
            Log.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn log10_100_is_two() {
        let r = Log10
            .execute(&[Value::Float(100.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 2.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn log10_one_is_zero() {
        let r = Log10
            .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn log10_wrong_arg_count() {
        assert!(Log10.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn log10_type_mismatch() {
        assert!(
            Log10
                .execute(&[Value::Boolean(true)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn pow_squares() {
        let r = Pow
            .execute(&[Value::Float(3.0), Value::Float(2.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 9.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn pow_integer_exponent() {
        let r = Pow
            .execute(&[Value::Integer(2), Value::Integer(10)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1024.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn pow_wrong_arg_count() {
        assert!(Pow.execute(&[Value::Float(1.0)], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn pow_type_mismatch() {
        assert!(
            Pow.execute(
                &[Value::Float(1.0), Value::String("x".into())],
                &mut NoOpEvalCtx
            )
            .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // sin / cos / tan / cot
    // -----------------------------------------------------------------------

    #[test]
    fn sin_zero() {
        let r = Sin.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn sin_half_pi() {
        let r = Sin
            .execute(
                &[Value::Float(std::f64::consts::FRAC_PI_2)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn sin_wrong_arg_count() {
        assert!(Sin.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn sin_type_mismatch() {
        assert!(
            Sin.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn cos_zero() {
        let r = Cos.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cos_pi() {
        let r = Cos
            .execute(&[Value::Float(std::f64::consts::PI)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, -1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cos_wrong_arg_count() {
        assert!(Cos.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn cos_type_mismatch() {
        assert!(
            Cos.execute(&[Value::Boolean(true)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn tan_zero() {
        let r = Tan.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn tan_pi_over_four() {
        let r = Tan
            .execute(
                &[Value::Float(std::f64::consts::FRAC_PI_4)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn tan_wrong_arg_count() {
        assert!(Tan.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn tan_type_mismatch() {
        assert!(
            Tan.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn cot_pi_over_four() {
        let r = Cot
            .execute(
                &[Value::Float(std::f64::consts::FRAC_PI_4)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 1.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cot_zero_is_infinite() {
        // 1 / tan(0) = 1 / 0 = +inf (IEEE-754).
        let r = Cot.execute(&[Value::Float(0.0)], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Float(f) => assert!(f.is_infinite()),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn cot_wrong_arg_count() {
        assert!(Cot.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn cot_type_mismatch() {
        assert!(
            Cot.execute(&[Value::Boolean(true)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // asin / acos / atan / atan2
    // -----------------------------------------------------------------------

    #[test]
    fn asin_zero() {
        let r = Asin
            .execute(&[Value::Float(0.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn asin_one_is_half_pi() {
        let r = Asin
            .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, std::f64::consts::FRAC_PI_2)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn asin_out_of_range_is_nan() {
        let r = Asin
            .execute(&[Value::Float(2.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(f.is_nan()),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn asin_wrong_arg_count() {
        assert!(Asin.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn asin_type_mismatch() {
        assert!(
            Asin.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn acos_one_is_zero() {
        let r = Acos
            .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn acos_neg_one_is_pi() {
        let r = Acos
            .execute(&[Value::Float(-1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, std::f64::consts::PI)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn acos_wrong_arg_count() {
        assert!(Acos.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn acos_type_mismatch() {
        assert!(
            Acos.execute(&[Value::Boolean(false)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn atan_zero() {
        let r = Atan
            .execute(&[Value::Float(0.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn atan_one_is_pi_over_four() {
        let r = Atan
            .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, std::f64::consts::FRAC_PI_4)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn atan_wrong_arg_count() {
        assert!(Atan.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn atan_type_mismatch() {
        assert!(
            Atan.execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn atan2_quadrants() {
        // atan2(1, 1) = pi/4
        let r = Atan2
            .execute(&[Value::Float(1.0), Value::Float(1.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, std::f64::consts::FRAC_PI_4)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn atan2_zero_zero_is_zero() {
        // atan2(0, 0) is defined as 0 by IEEE-754.
        let r = Atan2
            .execute(&[Value::Float(0.0), Value::Float(0.0)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 0.0)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn atan2_wrong_arg_count() {
        assert!(
            Atan2
                .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn atan2_type_mismatch() {
        assert!(
            Atan2
                .execute(
                    &[Value::Float(1.0), Value::String("x".into())],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // toFloat / toDecimal
    // -----------------------------------------------------------------------

    #[test]
    fn to_float_from_integer() {
        let r = ToFloat
            .execute(&[Value::Integer(7)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(7.0));
    }

    #[test]
    fn to_float_from_float() {
        let r = ToFloat
            .execute(&[Value::Float(2.5)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(2.5));
    }

    #[test]
    fn to_float_from_decimal() {
        let d = Decimal::from_str("3.14").unwrap();
        let r = ToFloat
            .execute(&[Value::Decimal(d)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Float(f) => assert!(approx_eq(f, 3.14)),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn to_float_wrong_arg_count() {
        assert!(ToFloat.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn to_float_type_mismatch() {
        assert!(
            ToFloat
                .execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn to_decimal_from_integer() {
        let r = ToDecimal
            .execute(&[Value::Integer(5)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Decimal(Decimal::from(5)));
    }

    #[test]
    fn to_decimal_from_float() {
        let r = ToDecimal
            .execute(&[Value::Float(1.5)], &mut NoOpEvalCtx)
            .unwrap();
        match r {
            Value::Decimal(d) => assert_eq!(d, Decimal::from_str("1.5").unwrap()),
            other => panic!("expected Decimal, got {other:?}"),
        }
    }

    #[test]
    fn to_decimal_nan_errors() {
        let r = ToDecimal.execute(&[Value::Float(f64::NAN)], &mut NoOpEvalCtx);
        assert!(r.is_err());
    }

    #[test]
    fn to_decimal_wrong_arg_count() {
        assert!(ToDecimal.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn to_decimal_type_mismatch() {
        assert!(
            ToDecimal
                .execute(&[Value::String("x".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // parseInteger / parseFloat / parseBoolean
    // -----------------------------------------------------------------------

    #[test]
    fn parse_integer_positive() {
        let r = ParseInteger
            .execute(&[Value::String("123".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(123));
    }

    #[test]
    fn parse_integer_negative() {
        let r = ParseInteger
            .execute(&[Value::String("-42".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Integer(-42));
    }

    #[test]
    fn parse_integer_invalid_errors() {
        assert!(
            ParseInteger
                .execute(&[Value::String("abc".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_integer_wrong_arg_count() {
        assert!(ParseInteger.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn parse_integer_type_mismatch() {
        assert!(
            ParseInteger
                .execute(&[Value::Integer(1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_float_decimal_form() {
        let r = ParseFloat
            .execute(&[Value::String("3.14".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(3.14));
    }

    #[test]
    fn parse_float_integer_form() {
        let r = ParseFloat
            .execute(&[Value::String("7".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Float(7.0));
    }

    #[test]
    fn parse_float_invalid_errors() {
        assert!(
            ParseFloat
                .execute(&[Value::String("not-a-number".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_float_wrong_arg_count() {
        assert!(ParseFloat.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn parse_float_type_mismatch() {
        assert!(
            ParseFloat
                .execute(&[Value::Float(1.0)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_boolean_true() {
        let r = ParseBoolean
            .execute(&[Value::String("true".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Boolean(true));
    }

    #[test]
    fn parse_boolean_false() {
        let r = ParseBoolean
            .execute(&[Value::String("false".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::Boolean(false));
    }

    #[test]
    fn parse_boolean_case_sensitive() {
        // "True" / "TRUE" / "False" are NOT accepted per Pure spec.
        assert!(
            ParseBoolean
                .execute(&[Value::String("True".into())], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(
            ParseBoolean
                .execute(&[Value::String("FALSE".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_boolean_invalid_errors() {
        assert!(
            ParseBoolean
                .execute(&[Value::String("yes".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_boolean_wrong_arg_count() {
        assert!(ParseBoolean.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn parse_boolean_type_mismatch() {
        assert!(
            ParseBoolean
                .execute(&[Value::Boolean(true)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // Registration sanity check
    // -----------------------------------------------------------------------

    #[test]
    fn register_adds_all_natives() {
        let mut reg = NativeRegistry::new();
        register(&mut reg);
        // 23 natives in the plan.
        assert_eq!(reg.len(), 23, "expected 23 math natives registered");
        assert!(reg.get("floor_Number_1__Integer_1_").is_some());
        assert!(reg.get("atan2_Number_1__Number_1__Float_1_").is_some());
        assert!(reg.get("parseBoolean_String_1__Boolean_1_").is_some());
    }
}
