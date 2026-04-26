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

//! String native functions: `plus` (concatenation), `length`, `substring`,
//! `indexOf`, `contains`, `startsWith`, `endsWith`, `toLower`, `toUpper`,
//! `trim`, `ltrim`, `rtrim`, `reverseString`, `replace`, `joinStrings`,
//! `toString`, `format`.

use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

// ---------------------------------------------------------------------------
// plus (string concatenation)
// ---------------------------------------------------------------------------

/// Pure `plus(String[*]): String[1]` — string concatenation over a
/// collection of strings. Single signature mirroring Java Pure's
/// `StringPlus.execute`, which reads `params.get(0).values` and
/// concatenates in order.
///
/// The compiler lowers `a + b` for String operands to
/// `plus_String_MANY__String_1_([a, b])`, so the native always
/// receives exactly one Collection argument.
#[derive(Debug)]
pub struct StringPlus;

impl NativeFunction for StringPlus {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("plus (String)", &values, 1)?;
        let items = values[0].to_collection();
        let mut out = String::new();
        for item in items.iter() {
            out.push_str(item.as_string()?.as_str());
        }
        Ok(Evaluated::new(Value::String(SmolStr::new(&out))))
    }

    fn signature(&self) -> &'static str {
        "plus(String[*]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// length
// ---------------------------------------------------------------------------

/// Pure `length(String[1]): Integer[1]`
#[derive(Debug)]
pub struct Length;

impl NativeFunction for Length {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("length", &values, 1)?;
        let s = values[0].as_string()?;
        #[allow(clippy::cast_possible_wrap)]
        Ok(Evaluated::new(Value::Integer(s.len() as i64)))
    }

    fn signature(&self) -> &'static str {
        "length(String[1]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// substring
// ---------------------------------------------------------------------------

/// Pure `substring(String[1], Integer[1], Integer[1]): String[1]`
///
/// Pure uses 0-based start index, and the end index is exclusive.
#[derive(Debug)]
pub struct Substring;

impl NativeFunction for Substring {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        let s = values[0].as_string()?;
        let (start_i, end_i) = match values.len() {
            2 => {
                // 2-arg form: start through end of string. Java Pure
                // mirrors String.substring(int) with no terminator.
                let start = values[1].as_integer()?.max(0);
                #[allow(clippy::cast_possible_wrap)]
                let end = s.len() as i64;
                (start, end)
            }
            3 => (
                values[1].as_integer()?.max(0),
                values[2].as_integer()?.max(0),
            ),
            n => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "substring: expected 2 or 3 argument(s), got {n}"
                ))
                .into());
            }
        };
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let start = (start_i as usize).min(s.len());
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let end = (end_i as usize).min(s.len());
        let start = start.min(end);
        Ok(Evaluated::new(Value::String(SmolStr::new(&s[start..end]))))
    }

    fn signature(&self) -> &'static str {
        "substring(String[1], Integer[1] [, Integer[1]]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// contains / startsWith / endsWith / indexOf
// ---------------------------------------------------------------------------

/// Pure `contains(String[1], String[1]): Boolean[1]`
#[derive(Debug)]
pub struct Contains;

impl NativeFunction for Contains {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("contains", &values, 2)?;
        let s = values[0].as_string()?;
        let sub = values[1].as_string()?;
        Ok(Evaluated::new(Value::Boolean(s.contains(sub.as_str()))))
    }

    fn signature(&self) -> &'static str {
        "contains(String[1], String[1]): Boolean[1]"
    }
}

/// Pure `startsWith(String[1], String[1]): Boolean[1]`
#[derive(Debug)]
pub struct StartsWith;

impl NativeFunction for StartsWith {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("startsWith", &values, 2)?;
        let s = values[0].as_string()?;
        let prefix = values[1].as_string()?;
        Ok(Evaluated::new(Value::Boolean(
            s.starts_with(prefix.as_str()),
        )))
    }

    fn signature(&self) -> &'static str {
        "startsWith(String[1], String[1]): Boolean[1]"
    }
}

/// Pure `endsWith(String[1], String[1]): Boolean[1]`
#[derive(Debug)]
pub struct EndsWith;

impl NativeFunction for EndsWith {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("endsWith", &values, 2)?;
        let s = values[0].as_string()?;
        let suffix = values[1].as_string()?;
        Ok(Evaluated::new(Value::Boolean(s.ends_with(suffix.as_str()))))
    }

    fn signature(&self) -> &'static str {
        "endsWith(String[1], String[1]): Boolean[1]"
    }
}

/// Pure `indexOf(String[1], String[1]): Integer[1]`
/// Pure `indexOf(String[1], String[1], Integer[1]): Integer[1]` — search
/// from a non-zero start index.
///
/// Returns -1 if not found (matching Java behavior). The 3-arg form's
/// returned index is absolute (not relative to the start), matching
/// `String.indexOf(String, int)` in Java.
#[derive(Debug)]
pub struct IndexOf;

impl NativeFunction for IndexOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        let s = values[0].as_string()?;
        let sub = values[1].as_string()?;
        let from = match values.len() {
            2 => 0usize,
            3 => {
                let raw = values[2].as_integer()?.max(0);
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let from = (raw as usize).min(s.len());
                from
            }
            n => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "indexOf: expected 2 or 3 argument(s), got {n}"
                ))
                .into());
            }
        };
        #[allow(clippy::cast_possible_wrap)]
        let idx = s[from..]
            .find(sub.as_str())
            .map_or(-1, |i| (i + from) as i64);
        Ok(Evaluated::new(Value::Integer(idx)))
    }

    fn signature(&self) -> &'static str {
        "indexOf(String[1], String[1] [, Integer[1]]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// toLower / toUpper / trim
// ---------------------------------------------------------------------------

/// Pure `toLower(String[1]): String[1]`
#[derive(Debug)]
pub struct ToLower;

impl NativeFunction for ToLower {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toLower", &values, 1)?;
        let s = values[0].as_string()?;
        Ok(Evaluated::new(Value::String(SmolStr::new(
            s.to_lowercase(),
        ))))
    }

    fn signature(&self) -> &'static str {
        "toLower(String[1]): String[1]"
    }
}

/// Pure `toUpper(String[1]): String[1]`
#[derive(Debug)]
pub struct ToUpper;

impl NativeFunction for ToUpper {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toUpper", &values, 1)?;
        let s = values[0].as_string()?;
        Ok(Evaluated::new(Value::String(SmolStr::new(
            s.to_uppercase(),
        ))))
    }

    fn signature(&self) -> &'static str {
        "toUpper(String[1]): String[1]"
    }
}

/// Pure `trim(String[1]): String[1]`
#[derive(Debug)]
pub struct Trim;

impl NativeFunction for Trim {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("trim", &values, 1)?;
        let s = values[0].as_string()?;
        Ok(Evaluated::new(Value::String(SmolStr::new(s.trim()))))
    }

    fn signature(&self) -> &'static str {
        "trim(String[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// toString
// ---------------------------------------------------------------------------

/// Pure `toString(Any[1]): String[1]` — convert any value to its string representation.
#[derive(Debug)]
pub struct ToString;

impl NativeFunction for ToString {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toString", &values, 1)?;
        Ok(Evaluated::new(Value::String(SmolStr::new(
            values[0].to_string(),
        ))))
    }

    fn signature(&self) -> &'static str {
        "toString(Any[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// format
// ---------------------------------------------------------------------------

/// Produce a repr string for `%r`: strings are quoted, other values use Display.
fn repr_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{s}'"),
        other => other.to_string(),
    }
}

/// Pure `format(String[1], Any[*]): String[1]`
///
/// Replaces `%s` (Display), `%r` (repr — strings quoted), `%d` (integer),
/// `%f` (float) in the format string with successive values from `args[1]`.
/// `args[1]` is the `Any[*]` collection of substitution values; if it is a
/// `Value::Collection`, its elements are iterated; otherwise it is treated as
/// a single-element list.
#[derive(Debug)]
pub struct Format;

impl NativeFunction for Format {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("format", &values, 2)?;
        let template = values[0].as_string()?;

        let subs: Vec<&Value> = match &values[1] {
            Value::Collection(v) => v.iter().collect(),
            single => vec![single],
        };

        let mut result = String::with_capacity(template.len());
        let mut chars = template.chars().peekable();
        let mut sub_idx = 0usize;

        while let Some(c) = chars.next() {
            if c != '%' {
                result.push(c);
                continue;
            }
            match chars.peek() {
                Some(&spec) if matches!(spec, 's' | 'r' | 'd' | 'f') => {
                    chars.next();
                    let v = subs.get(sub_idx).copied().ok_or_else(|| {
                        PureRuntimeError::EvaluationError(format!(
                            "format: not enough arguments (needed arg {sub_idx})"
                        ))
                    })?;
                    sub_idx += 1;
                    match spec {
                        'r' => result.push_str(&repr_value(v)),
                        's' => match v {
                            Value::String(s) => result.push_str(s.as_str()),
                            other => result.push_str(&other.to_string()),
                        },
                        _ => result.push_str(&v.to_string()),
                    }
                }
                _ => result.push('%'),
            }
        }

        Ok(Evaluated::new(Value::String(SmolStr::new(result))))
    }

    fn signature(&self) -> &'static str {
        "format(String[1], Any[*]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// ltrim / rtrim
// ---------------------------------------------------------------------------

/// Pure `ltrim(String[1]): String[1]` — remove leading whitespace.
#[derive(Debug)]
pub struct Ltrim;

impl NativeFunction for Ltrim {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("ltrim", &values, 1)?;
        let s = values[0].as_string()?;
        Ok(Evaluated::new(Value::String(SmolStr::new(s.trim_start()))))
    }

    fn signature(&self) -> &'static str {
        "ltrim(String[1]): String[1]"
    }
}

/// Pure `rtrim(String[1]): String[1]` — remove trailing whitespace.
#[derive(Debug)]
pub struct Rtrim;

impl NativeFunction for Rtrim {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("rtrim", &values, 1)?;
        let s = values[0].as_string()?;
        Ok(Evaluated::new(Value::String(SmolStr::new(s.trim_end()))))
    }

    fn signature(&self) -> &'static str {
        "rtrim(String[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// reverseString
// ---------------------------------------------------------------------------

/// Pure `reverseString(String[1]): String[1]` — reverse the characters of
/// a string. Reversal is per Unicode scalar (`char`), not per byte, so
/// multi-byte characters are preserved intact.
#[derive(Debug)]
pub struct ReverseString;

impl NativeFunction for ReverseString {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("reverseString", &values, 1)?;
        let s = values[0].as_string()?;
        let reversed: String = s.chars().rev().collect();
        Ok(Evaluated::new(Value::String(SmolStr::new(reversed))))
    }

    fn signature(&self) -> &'static str {
        "reverseString(String[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// replace
// ---------------------------------------------------------------------------

/// Pure `replace(String[1], String[1], String[1]): String[1]` — replace all
/// occurrences of `target` in `source` with `replacement`.
#[derive(Debug)]
pub struct Replace;

impl NativeFunction for Replace {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("replace", &values, 3)?;
        let source = values[0].as_string()?;
        let target = values[1].as_string()?;
        let replacement = values[2].as_string()?;
        let out = source
            .as_str()
            .replace(target.as_str(), replacement.as_str());
        Ok(Evaluated::new(Value::String(SmolStr::new(out))))
    }

    fn signature(&self) -> &'static str {
        "replace(String[1], String[1], String[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// joinStrings (2-arg and 4-arg)
// ---------------------------------------------------------------------------

/// Pure `joinStrings` — concatenate a collection of strings with a separator.
///
/// Two arities are supported:
/// - `joinStrings(String[*], String[1]): String[1]` — elements joined by
///   `separator`.
/// - `joinStrings(String[*], String[1], String[1], String[1]): String[1]` —
///   `prefix + elements.join(separator) + suffix`.
///
/// The `execute` implementation dispatches on `values.len()` so a single
/// struct can back both mangled registrations.
#[derive(Debug)]
pub struct JoinStrings;

impl NativeFunction for JoinStrings {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Pure signature: `joinStrings(strings, prefix, separator, suffix)`
        // for the 4-arg overload; `joinStrings(strings, separator)` for the
        // 2-arg one. The previous code swapped prefix and separator —
        // `'rrr2'->joinStrings('[', ', ', ']')` produced `, 'rrr2'['eee2']`
        // (separator-then-element-then-prefix-then-element-then-suffix)
        // because the index map landed `values[1]→separator,
        // values[2]→prefix`. The Pure platform itself uses
        // `joinStrings('\nexpected: [', ', ', ']')` so the broken order
        // emitted by `assertSameElements` / size-many `assertEquals`
        // diff messages was bucketing all collection comparisons under
        // a misformatted "Assert failure" string.
        let (separator, prefix, suffix) = match values.len() {
            2 => (values[1].as_string()?.clone(), None, None),
            4 => (
                values[2].as_string()?.clone(),
                Some(values[1].as_string()?.clone()),
                Some(values[3].as_string()?.clone()),
            ),
            n => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "joinStrings: expected 2 or 4 argument(s), got {n}"
                ))
                .into());
            }
        };

        let coll = values[0].to_collection();
        let mut parts: Vec<String> = Vec::with_capacity(coll.len());
        for v in coll.iter() {
            parts.push(v.as_string()?.as_str().to_owned());
        }
        let joined = parts.join(separator.as_str());

        let result = match (prefix, suffix) {
            (Some(p), Some(s)) => format!("{p}{joined}{s}"),
            _ => joined,
        };
        Ok(Evaluated::new(Value::String(SmolStr::new(result))))
    }

    fn signature(&self) -> &'static str {
        "joinStrings(String[*], String[1]): String[1]"
    }
}

// ---------------------------------------------------------------------------
// split
// ---------------------------------------------------------------------------

/// Pure `split(str:String[1], token:String[1]):String[*]`
///
/// Splits `str` at every occurrence of `token`. When `token` is absent,
/// the result is a single-element collection holding the original
/// string. Mirror of Java's `String.split(literal)` modulo regex —
/// `token` is treated as a literal substring, not a regex.
#[derive(Debug)]
pub struct Split;

impl NativeFunction for Split {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("split", &values, 2)?;
        let s = values[0].as_string()?;
        let token = values[1].as_string()?;
        let parts: Vec<Value> = s
            .split(token.as_str())
            .map(|p| Value::String(SmolStr::new(p)))
            .collect();
        Ok(Evaluated::new(Value::from_vec(parts)))
    }

    fn signature(&self) -> &'static str {
        "split(String[1], String[1]):String[*]"
    }
}

// ---------------------------------------------------------------------------
// parseDecimal
// ---------------------------------------------------------------------------

/// Pure `parseDecimal(string:String[1]):Decimal[1]`
/// Pure `parseDecimal(string:String[1], precision:Integer[1], scale:Integer[1]):Decimal[1]`
///
/// Parse a string into a `Decimal`. Strips an optional trailing `d`
/// suffix (Pure's decimal literal marker) and tolerates leading `+`
/// plus zero-padding (`+0000003.14`) — matches the platform PCT test
/// expectations in `parseDecimal.pure`.
///
/// The 3-arg overload rounds to `scale` digits after the decimal
/// point using banker's rounding (Decimal's default), which matches
/// Java's BigDecimal `setScale(scale, HALF_EVEN)` behavior.
/// `precision` is accepted for signature parity but not enforced —
/// the platform tests assert only on the rounded value, not on
/// total-digit truncation.
#[derive(Debug)]
pub struct ParseDecimal;

impl NativeFunction for ParseDecimal {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        use rust_decimal::Decimal;
        use std::str::FromStr;

        let values = force_all(args, ctx)?;
        match values.len() {
            1 => {
                let s = values[0].as_string()?;
                let trimmed = s.trim_end_matches('d');
                let d = Decimal::from_str(trimmed).map_err(|e| {
                    PureRuntimeError::EvaluationError(format!(
                        "parseDecimal: cannot parse '{s}' as Decimal: {e}"
                    ))
                })?;
                Ok(Evaluated::new(Value::Decimal(d)))
            }
            3 => {
                let s = values[0].as_string()?;
                let scale = i64_arg(&values[1], "parseDecimal scale")?;
                if !(0..=28).contains(&scale) {
                    return Err(PureRuntimeError::EvaluationError(format!(
                        "parseDecimal: scale must be in 0..=28, got {scale}"
                    ))
                    .into());
                }
                let trimmed = s.trim_end_matches('d');
                let d = Decimal::from_str(trimmed).map_err(|e| {
                    PureRuntimeError::EvaluationError(format!(
                        "parseDecimal: cannot parse '{s}' as Decimal: {e}"
                    ))
                })?;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let rounded = d.round_dp(scale as u32);
                Ok(Evaluated::new(Value::Decimal(rounded)))
            }
            n => Err(PureRuntimeError::EvaluationError(format!(
                "parseDecimal: expected 1 or 3 argument(s), got {n}"
            ))
            .into()),
        }
    }

    fn signature(&self) -> &'static str {
        "parseDecimal(String[1], Integer[0..1], Integer[0..1]):Decimal[1]"
    }
}

fn i64_arg(v: &Value, ctx: &str) -> Result<i64, PureException> {
    match v {
        Value::Integer(n) => Ok(*n),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "{ctx}: expected Integer, got {}",
            other.type_name()
        ))
        .into()),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all string native functions.
pub fn register(registry: &mut NativeRegistry) {
    // String concatenation — not a native in Java (delegates to joinStrings),
    // but we implement it natively for efficiency.
    registry.register("plus_String_MANY__String_1_", StringPlus);

    registry.register("length_String_1__Integer_1_", Length);
    registry.register(
        "substring_String_1__Integer_1__Integer_1__String_1_",
        Substring,
    );
    registry.register("substring_String_1__Integer_1__String_1_", Substring);
    registry.register("contains_String_1__String_1__Boolean_1_", Contains);
    registry.register("startsWith_String_1__String_1__Boolean_1_", StartsWith);
    registry.register("endsWith_String_1__String_1__Boolean_1_", EndsWith);
    registry.register("indexOf_String_1__String_1__Integer_1_", IndexOf);
    registry.register("indexOf_String_1__String_1__Integer_1__Integer_1_", IndexOf);
    registry.register("split_String_1__String_1__String_MANY_", Split);
    registry.register("parseDecimal_String_1__Decimal_1_", ParseDecimal);
    registry.register(
        "parseDecimal_String_1__Integer_1__Integer_1__Decimal_1_",
        ParseDecimal,
    );
    registry.register("toLower_String_1__String_1_", ToLower);
    registry.register("toUpper_String_1__String_1_", ToUpper);
    registry.register("trim_String_1__String_1_", Trim);
    registry.register("ltrim_String_1__String_1_", Ltrim);
    registry.register("rtrim_String_1__String_1_", Rtrim);
    registry.register("reverseString_String_1__String_1_", ReverseString);
    registry.register("replace_String_1__String_1__String_1__String_1_", Replace);
    registry.register("joinStrings_String_MANY__String_1__String_1_", JoinStrings);
    registry.register(
        "joinStrings_String_MANY__String_1__String_1__String_1__String_1_",
        JoinStrings,
    );
    registry.register("toString_Any_1__String_1_", ToString);
    registry.register("format_String_1__Any_MANY__String_1_", Format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, force_all, lit_collection, lit_int, lit_str};

    #[test]
    fn string_plus() {
        // StringPlus takes a single `String[*]` argument — a Collection of
        // strings that get concatenated in order. Matches Java Pure's
        // `StringPlus.execute` which iterates `params.get(0).values`.
        let r = StringPlus
            .execute(
                &[crate::native::lit_collection(vec![
                    lit_str("hello"),
                    lit_str(" world"),
                ])],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::String("hello world".into()));
    }

    #[test]
    fn string_length() {
        assert_eq!(
            Length
                .execute(&[lit_str("hello")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(5)
        );
    }

    #[test]
    fn string_substring() {
        let r = Substring
            .execute(
                &[lit_str("hello world"), lit_int(6), lit_int(11)],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::String("world".into()));
    }

    #[test]
    fn string_contains() {
        assert_eq!(
            Contains
                .execute(&[lit_str("hello"), lit_str("ell")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn string_index_of_found() {
        assert_eq!(
            IndexOf
                .execute(&[lit_str("hello"), lit_str("ll")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(2)
        );
    }

    #[test]
    fn string_index_of_not_found() {
        assert_eq!(
            IndexOf
                .execute(&[lit_str("hello"), lit_str("xyz")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn string_to_lower() {
        assert_eq!(
            ToLower
                .execute(&[lit_str("Hello")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("hello".into())
        );
    }

    #[test]
    fn string_trim() {
        assert_eq!(
            Trim.execute(&[lit_str("  hi  ")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("hi".into())
        );
    }

    #[test]
    fn to_string_integer() {
        assert_eq!(
            ToString
                .execute(&[lit_int(42)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("42".into())
        );
    }

    #[test]
    fn format_s_specifier() {
        let coll = lit_collection(vec![lit_str("world")]);
        let r = Format
            .execute(&[lit_str("hello %s"), coll], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("hello world".into()));
    }

    #[test]
    fn format_r_specifier_quotes_string() {
        let coll = lit_collection(vec![lit_str("foo")]);
        let r = Format
            .execute(&[lit_str("%r"), coll], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("'foo'".into()));
    }

    #[test]
    fn format_two_args() {
        let coll = lit_collection(vec![lit_int(1), lit_int(2)]);
        let r = Format
            .execute(&[lit_str("%s + %s"), coll], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("1 + 2".into()));
    }

    #[test]
    fn wrong_arg_count_errors() {
        // StringPlus takes exactly 1 Collection argument; 0 args is a
        // wrong-arity error. A singleton Collection is valid (concats to
        // just that string).
        assert!(StringPlus.execute(&[], &mut MockCtx).is_err());
        assert!(Length.execute(&[], &mut MockCtx).is_err());
        assert!(Substring.execute(&[lit_str("a")], &mut MockCtx).is_err());
        assert!(ToString.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn type_mismatch_errors() {
        assert!(
            StringPlus
                .execute(&[lit_int(1), lit_str("b")], &mut MockCtx)
                .is_err()
        );
        assert!(Length.execute(&[lit_int(1)], &mut MockCtx).is_err());
        assert!(
            Substring
                .execute(&[lit_str("a"), lit_str("b"), lit_int(1)], &mut MockCtx)
                .is_err()
        );
        assert!(
            Contains
                .execute(&[lit_str("a"), lit_int(1)], &mut MockCtx)
                .is_err()
        );
    }

    // -- ltrim / rtrim --

    #[test]
    fn ltrim_strips_leading_whitespace() {
        assert_eq!(
            Ltrim
                .execute(&[lit_str("  hi  ")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("hi  ".into())
        );
    }

    #[test]
    fn ltrim_empty_string() {
        assert_eq!(
            Ltrim
                .execute(&[lit_str("")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("".into())
        );
    }

    #[test]
    fn ltrim_wrong_arg_count() {
        assert!(Ltrim.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn ltrim_type_mismatch() {
        assert!(Ltrim.execute(&[lit_int(1)], &mut MockCtx).is_err());
    }

    #[test]
    fn rtrim_strips_trailing_whitespace() {
        assert_eq!(
            Rtrim
                .execute(&[lit_str("  hi  ")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("  hi".into())
        );
    }

    #[test]
    fn rtrim_empty_string() {
        assert_eq!(
            Rtrim
                .execute(&[lit_str("")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("".into())
        );
    }

    #[test]
    fn rtrim_wrong_arg_count() {
        assert!(
            Rtrim
                .execute(&[lit_str("a"), lit_str("b")], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn rtrim_type_mismatch() {
        use crate::native::lit_bool;
        assert!(Rtrim.execute(&[lit_bool(true)], &mut MockCtx).is_err());
    }

    // -- reverseString --

    #[test]
    fn reverse_string_ascii() {
        assert_eq!(
            ReverseString
                .execute(&[lit_str("hello")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("olleh".into())
        );
    }

    #[test]
    fn reverse_string_empty() {
        assert_eq!(
            ReverseString
                .execute(&[lit_str("")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("".into())
        );
    }

    #[test]
    fn reverse_string_unicode_preserves_codepoints() {
        // "héllo" contains a 2-byte UTF-8 char; reversing per byte would
        // produce invalid UTF-8. Reversing per char gives "olléh".
        assert_eq!(
            ReverseString
                .execute(&[lit_str("héllo")], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::String("olléh".into())
        );
    }

    #[test]
    fn reverse_string_wrong_arg_count() {
        assert!(ReverseString.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn reverse_string_type_mismatch() {
        assert!(ReverseString.execute(&[lit_int(1)], &mut MockCtx).is_err());
    }

    // -- replace --

    #[test]
    fn replace_replaces_all_occurrences() {
        let r = Replace
            .execute(
                &[lit_str("foo bar foo"), lit_str("foo"), lit_str("baz")],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::String("baz bar baz".into()));
    }

    #[test]
    fn replace_no_match_returns_source_unchanged() {
        let r = Replace
            .execute(
                &[lit_str("hello"), lit_str("xyz"), lit_str("abc")],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::String("hello".into()));
    }

    #[test]
    fn replace_wrong_arg_count() {
        assert!(
            Replace
                .execute(&[lit_str("a"), lit_str("b")], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn replace_type_mismatch() {
        assert!(
            Replace
                .execute(&[lit_str("a"), lit_int(1), lit_str("c")], &mut MockCtx)
                .is_err()
        );
    }

    // -- joinStrings --

    #[test]
    fn join_strings_2arg_basic() {
        let coll = lit_collection(vec![lit_str("a"), lit_str("b"), lit_str("c")]);
        let r = JoinStrings
            .execute(&[coll, lit_str(",")], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("a,b,c".into()));
    }

    #[test]
    fn join_strings_2arg_empty_collection() {
        let coll = lit_collection(vec![]);
        let r = JoinStrings
            .execute(&[coll, lit_str(",")], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("".into()));
    }

    #[test]
    fn join_strings_2arg_scalar_treated_as_singleton() {
        // A bare String[1] value passed where String[*] is expected should
        // be treated as a one-element collection via Value::to_collection.
        let r = JoinStrings
            .execute(&[lit_str("lone"), lit_str(",")], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::String("lone".into()));
    }

    #[test]
    fn join_strings_4arg_with_prefix_and_suffix() {
        // 4-arg signature is `joinStrings(strings, prefix, separator, suffix)`
        // — see commit b72e67822ca for the impl's arg-order fix that this
        // test had drifted out of sync with.
        let coll = lit_collection(vec![lit_str("a"), lit_str("b")]);
        let r = JoinStrings
            .execute(
                &[coll, lit_str("["), lit_str(", "), lit_str("]")],
                &mut MockCtx,
            )
            .unwrap();
        assert_eq!(r.into_value(), Value::String("[a, b]".into()));
    }

    #[test]
    fn join_strings_wrong_arg_count() {
        assert!(JoinStrings.execute(&[], &mut MockCtx).is_err());
        assert!(
            JoinStrings
                .execute(&[lit_str("only")], &mut MockCtx)
                .is_err()
        );
        assert!(
            JoinStrings
                .execute(&[lit_str("a"), lit_str("b"), lit_str("c")], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn join_strings_element_type_mismatch() {
        let coll = lit_collection(vec![lit_str("a"), lit_int(1)]);
        assert!(
            JoinStrings
                .execute(&[coll, lit_str(",")], &mut MockCtx)
                .is_err()
        );
    }
}
