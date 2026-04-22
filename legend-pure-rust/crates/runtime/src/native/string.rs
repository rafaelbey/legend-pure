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
//! `trim`, `toString`, `format`.

use smol_str::SmolStr;

use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// plus (string concatenation)
// ---------------------------------------------------------------------------

/// Pure `plus(String[1], String[1]): String[1]` — string concatenation.
///
/// Note: This is registered as `stringPlus` to avoid collision with
/// numeric `plus`. The evaluator dispatches based on argument types.
#[derive(Debug)]
pub struct StringPlus;

impl NativeFunction for StringPlus {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("plus (String)", args, 2)?;
        let a = args[0].as_string()?;
        let b = args[1].as_string()?;
        Ok(Value::String(SmolStr::new(format!("{a}{b}"))))
    }

    fn signature(&self) -> &'static str {
        "plus(String[1], String[1]): String[1]"
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("length", args, 1)?;
        let s = args[0].as_string()?;
        #[allow(clippy::cast_possible_wrap)]
        Ok(Value::Integer(s.len() as i64))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("substring", args, 3)?;
        let s = args[0].as_string()?;
        let start_i = args[1].as_integer()?.max(0);
        let end_i = args[2].as_integer()?.max(0);
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let start = (start_i as usize).min(s.len());
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let end = (end_i as usize).min(s.len());
        let start = start.min(end);
        Ok(Value::String(SmolStr::new(&s[start..end])))
    }

    fn signature(&self) -> &'static str {
        "substring(String[1], Integer[1], Integer[1]): String[1]"
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("contains", args, 2)?;
        let s = args[0].as_string()?;
        let sub = args[1].as_string()?;
        Ok(Value::Boolean(s.contains(sub.as_str())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("startsWith", args, 2)?;
        let s = args[0].as_string()?;
        let prefix = args[1].as_string()?;
        Ok(Value::Boolean(s.starts_with(prefix.as_str())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("endsWith", args, 2)?;
        let s = args[0].as_string()?;
        let suffix = args[1].as_string()?;
        Ok(Value::Boolean(s.ends_with(suffix.as_str())))
    }

    fn signature(&self) -> &'static str {
        "endsWith(String[1], String[1]): Boolean[1]"
    }
}

/// Pure `indexOf(String[1], String[1]): Integer[1]`
///
/// Returns -1 if not found (matching Java behavior).
#[derive(Debug)]
pub struct IndexOf;

impl NativeFunction for IndexOf {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("indexOf", args, 2)?;
        let s = args[0].as_string()?;
        let sub = args[1].as_string()?;
        #[allow(clippy::cast_possible_wrap)]
        let idx = s.find(sub.as_str()).map_or(-1, |i| i as i64);
        Ok(Value::Integer(idx))
    }

    fn signature(&self) -> &'static str {
        "indexOf(String[1], String[1]): Integer[1]"
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toLower", args, 1)?;
        let s = args[0].as_string()?;
        Ok(Value::String(SmolStr::new(s.to_lowercase())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toUpper", args, 1)?;
        let s = args[0].as_string()?;
        Ok(Value::String(SmolStr::new(s.to_uppercase())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("trim", args, 1)?;
        let s = args[0].as_string()?;
        Ok(Value::String(SmolStr::new(s.trim())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toString", args, 1)?;
        Ok(Value::String(SmolStr::new(args[0].to_string())))
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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("format", args, 2)?;
        let template = args[0].as_string()?;

        let subs: Vec<&Value> = match &args[1] {
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

        Ok(Value::String(SmolStr::new(result)))
    }

    fn signature(&self) -> &'static str {
        "format(String[1], Any[*]): String[1]"
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
    registry.register("toLower_String_1__String_1_", ToLower);
    registry.register("toUpper_String_1__String_1_", ToUpper);
    registry.register("trim_String_1__String_1_", Trim);
    registry.register("toString_Any_1__String_1_", ToString);
    registry.register("format_String_1__Any_MANY__String_1_", Format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NoOpEvalCtx;

    #[test]
    fn string_plus() {
        let r = StringPlus
            .execute(
                &[
                    Value::String("hello".into()),
                    Value::String(" world".into()),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::String("hello world".into()));
    }

    #[test]
    fn string_length() {
        assert_eq!(
            Length
                .execute(&[Value::String("hello".into())], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(5)
        );
    }

    #[test]
    fn string_substring() {
        let r = Substring
            .execute(
                &[
                    Value::String("hello world".into()),
                    Value::Integer(6),
                    Value::Integer(11),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::String("world".into()));
    }

    #[test]
    fn string_contains() {
        assert_eq!(
            Contains
                .execute(
                    &[Value::String("hello".into()), Value::String("ell".into())],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn string_index_of_found() {
        assert_eq!(
            IndexOf
                .execute(
                    &[Value::String("hello".into()), Value::String("ll".into())],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Integer(2)
        );
    }

    #[test]
    fn string_index_of_not_found() {
        assert_eq!(
            IndexOf
                .execute(
                    &[Value::String("hello".into()), Value::String("xyz".into())],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn string_to_lower() {
        assert_eq!(
            ToLower
                .execute(&[Value::String("Hello".into())], &mut NoOpEvalCtx)
                .unwrap(),
            Value::String("hello".into())
        );
    }

    #[test]
    fn string_trim() {
        assert_eq!(
            Trim.execute(&[Value::String("  hi  ".into())], &mut NoOpEvalCtx)
                .unwrap(),
            Value::String("hi".into())
        );
    }

    #[test]
    fn to_string_integer() {
        assert_eq!(
            ToString
                .execute(&[Value::Integer(42)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::String("42".into())
        );
    }

    #[test]
    fn format_s_specifier() {
        use crate::value::Value;
        use im_rc::Vector;
        let coll = Value::Collection(Box::new(Vector::from_iter([Value::String("world".into())])));
        let r = Format
            .execute(&[Value::String("hello %s".into()), coll], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::String("hello world".into()));
    }

    #[test]
    fn format_r_specifier_quotes_string() {
        use crate::value::Value;
        use im_rc::Vector;
        let coll = Value::Collection(Box::new(Vector::from_iter([Value::String("foo".into())])));
        let r = Format
            .execute(&[Value::String("%r".into()), coll], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::String("'foo'".into()));
    }

    #[test]
    fn format_two_args() {
        use crate::value::Value;
        use im_rc::Vector;
        let coll = Value::Collection(Box::new(Vector::from_iter([
            Value::Integer(1),
            Value::Integer(2),
        ])));
        let r = Format
            .execute(&[Value::String("%s + %s".into()), coll], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, Value::String("1 + 2".into()));
    }

    #[test]
    fn wrong_arg_count_errors() {
        assert!(
            StringPlus
                .execute(&[Value::String("a".into())], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(Length.execute(&[], &mut NoOpEvalCtx).is_err());
        assert!(
            Substring
                .execute(&[Value::String("a".into())], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(ToString.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn type_mismatch_errors() {
        assert!(
            StringPlus
                .execute(
                    &[Value::Integer(1), Value::String("b".into())],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
        assert!(
            Length
                .execute(&[Value::Integer(1)], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(
            Substring
                .execute(
                    &[
                        Value::String("a".into()),
                        Value::String("b".into()),
                        Value::Integer(1)
                    ],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
        assert!(
            Contains
                .execute(
                    &[Value::String("a".into()), Value::Integer(1)],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }
}
