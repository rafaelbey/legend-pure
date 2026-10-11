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

//! Mathematical natives: rounding (`floor`, `ceiling`, `round`, `sign`),
//! analytic (`sqrt`, `cbrt`, `exp`, `log`, `log10`, `pow`), trigonometric
//! (`sin`, `cos`, `tan`, `cot`, `asin`, `acos`, `atan`, `atan2`), numeric
//! conversion (`toFloat`, `toDecimal`) and string parsing (`parseInteger`,
//! `parseFloat`, `parseBoolean`).
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.
//! [`java_number_string`](crate::native::math::java_number_string) lives here because the string natives
//! (`toString`, `format`) and the math error messages share it.

use crate::native::NativeRegistry;
use crate::value::Value;

/// Render a numeric value using Java's `toString` rules — used in the
/// error-message text the platform PCT tests pin (`assertError` against
/// e.g. "Unable to compute sqrt of -1.0") and by `toString`/`format`.
///
/// The key divergence from Rust's default `{f64}`/`{i64}` Display is
/// that Java's `Double.toString` always emits at least one decimal place
/// for integer-valued doubles (`2.0`, not `2`); integer values format as
/// the bare number. NaN / infinity surface verbatim.
#[must_use]
pub fn java_number_string(v: &Value) -> String {
    match v {
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => {
            if f.is_nan() {
                "NaN".to_string()
            } else if f.is_infinite() {
                if *f > 0.0 {
                    "Infinity".into()
                } else {
                    "-Infinity".into()
                }
            } else if (*f - f.trunc()).abs() < f64::EPSILON && f.is_finite() {
                format!("{f:.1}")
            } else {
                f.to_string()
            }
        }
        Value::Decimal(d) => d.to_string(),
        other => other.to_string(),
    }
}

/// Register the fished mathematical natives.
pub fn register(_registry: &mut NativeRegistry) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_number_string_matches_java_to_string() {
        assert_eq!(java_number_string(&Value::Integer(42)), "42");
        assert_eq!(java_number_string(&Value::Integer(-7)), "-7");
        assert_eq!(java_number_string(&Value::Float(2.0)), "2.0");
        assert_eq!(java_number_string(&Value::Float(-1.0)), "-1.0");
        assert_eq!(java_number_string(&Value::Float(2.5)), "2.5");
        assert_eq!(java_number_string(&Value::Float(f64::NAN)), "NaN");
        assert_eq!(java_number_string(&Value::Float(f64::INFINITY)), "Infinity");
        assert_eq!(
            java_number_string(&Value::Float(f64::NEG_INFINITY)),
            "-Infinity"
        );
        assert_eq!(
            java_number_string(&Value::Decimal(rust_decimal::Decimal::new(125, 2))),
            "1.25"
        );
    }
}
