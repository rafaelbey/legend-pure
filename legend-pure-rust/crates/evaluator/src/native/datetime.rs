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

//! Date and time natives over [`crate::date`]: the constructors (`date`,
//! `parseDate`), the arithmetic (`adjust`, `dateDiff`, `datePart`), the
//! component accessors (`year`, `monthNumber`, `dayOfMonth`, `hour`,
//! `minute`, `second`) and the precision predicates (`hasDay`, `hasHour`,
//! `hasSubsecond`, …).
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.
//! All arithmetic and precision rules flow through `PureDate`.
//! `DurationUnit` values arrive as `Value::EnumValue` and are read by
//! their member name.

use legend_pure_metamodel::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

/// The time-granularity unit accepted by `adjust` and `dateDiff`.
///
/// Pure's `DurationUnit` enum values reach the runtime as
/// `Value::EnumValue`; the historical `Value::String("DurationUnit.<VARIANT>")`
/// shape is still accepted and parsed by suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    /// Gregorian years.
    Years,
    /// Calendar months.
    Months,
    /// 7-day weeks.
    Weeks,
    /// Calendar days.
    Days,
    /// Hours.
    Hours,
    /// Minutes.
    Minutes,
    /// Seconds.
    Seconds,
    /// Milliseconds.
    Milliseconds,
    /// Microseconds.
    Microseconds,
    /// Nanoseconds.
    Nanoseconds,
}

/// Parse a `DurationUnit` from a Pure enum value (either typed
/// `Value::EnumValue` reading off `.member`, or a fallback string of
/// the form `"DurationUnit.DAYS"` / `"DAYS"`).
///
/// Java Pure passes the typed enum at runtime; PCT tests like
/// testAdjustByYears do `DurationUnit.YEARS`, which lowers to a
/// `Value::EnumValue { enum_id, member: "YEARS" }`. The `as_string`
/// path was only hit by historical synthetic test code; it stays as a
/// fallback so the matcher keeps tolerating both shapes.
fn duration_unit(v: &Value) -> Result<DurationUnit, PureRuntimeError> {
    let suffix_owned;
    let suffix: &str = match v {
        Value::EnumValue { member, .. } => member.as_str(),
        Value::String(s) => {
            suffix_owned = s.rsplit('.').next().unwrap_or(s.as_str()).to_string();
            &suffix_owned[..]
        }
        other => {
            return Err(PureRuntimeError::type_mismatch("DurationUnit", other));
        }
    };
    match suffix {
        "YEARS" => Ok(DurationUnit::Years),
        "MONTHS" => Ok(DurationUnit::Months),
        "WEEKS" => Ok(DurationUnit::Weeks),
        "DAYS" => Ok(DurationUnit::Days),
        "HOURS" => Ok(DurationUnit::Hours),
        "MINUTES" => Ok(DurationUnit::Minutes),
        "SECONDS" => Ok(DurationUnit::Seconds),
        "MILLISECONDS" => Ok(DurationUnit::Milliseconds),
        "MICROSECONDS" => Ok(DurationUnit::Microseconds),
        "NANOSECONDS" => Ok(DurationUnit::Nanoseconds),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "unknown DurationUnit: {other}"
        ))),
    }
}

/// Count Sunday boundaries crossed between two civil dates.
///
/// Returns a positive integer when `b > a`, negative when `b < a`, and
/// 0 when they're equal. Forward intervals count Sundays in `(a, b]`;
/// backward intervals count Sundays in `[b, a)` and negate. Matches
/// Java Pure's `dateDiff(WEEKS)` contract — see `testDateDiffWeeks`.
///
/// Direction asymmetry note: the half-open intervals differ at the
/// endpoints — forward includes the latest date, backward includes the
/// earliest. This matches the test fixtures where Sat → Sun = 1 (Sun
/// included) while Sun → Sat = 0 (Sun excluded going backward).
fn sunday_boundaries_between(a: jiff::civil::Date, b: jiff::civil::Date, days: i64) -> i64 {
    use jiff::civil::Weekday;
    /// Days until the *next* Sunday strictly after this weekday.
    /// Sunday → 7 (a full week to the next Sunday).
    fn to_next_sun(w: Weekday) -> i64 {
        match w {
            Weekday::Sunday => 7,
            Weekday::Monday => 6,
            Weekday::Tuesday => 5,
            Weekday::Wednesday => 4,
            Weekday::Thursday => 3,
            Weekday::Friday => 2,
            Weekday::Saturday => 1,
        }
    }
    /// Days from this weekday to the next Sunday on/after it (Sunday
    /// itself → 0).
    fn to_this_or_next_sun(w: Weekday) -> i64 {
        match w {
            Weekday::Sunday => 0,
            other => to_next_sun(other),
        }
    }
    if days == 0 {
        return 0;
    }
    if days > 0 {
        // Forward (a, b]: count Sundays strictly after a, on/before b.
        let span = days;
        let to_first = to_next_sun(a.weekday());
        if to_first > span {
            0
        } else {
            (span - to_first) / 7 + 1
        }
    } else {
        // Backward [b, a): count Sundays on/after b, strictly before a.
        let span = -days;
        let to_first = to_this_or_next_sun(b.weekday());
        if to_first >= span {
            0
        } else {
            -(((span - to_first - 1) / 7) + 1)
        }
    }
}

/// Pure `dateDiff(Date[1], Date[1], DurationUnit[1]): Integer[1]`.
///
/// Returns the signed span `d2 - d1` in the requested unit. Sub-second
/// units require both inputs to have at least second precision; larger
/// units require the corresponding component on both sides.
#[derive(Debug)]
pub struct DateDiff;

impl NativeFunction for DateDiff {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("dateDiff", &values, 3)?;
        let d1 = values[0].as_date()?;
        let d2 = values[1].as_date()?;
        let unit = duration_unit(&values[2])?;

        // Time-based units require datetime precision on both sides; date-
        // based units (Year/Month/Week/Day) only need day precision.
        let needs_time = matches!(
            unit,
            DurationUnit::Hours
                | DurationUnit::Minutes
                | DurationUnit::Seconds
                | DurationUnit::Milliseconds
                | DurationUnit::Microseconds
                | DurationUnit::Nanoseconds
        );

        if needs_time {
            let a = d1.to_civil_datetime()?;
            let b = d2.to_civil_datetime()?;
            let jiff_unit = match unit {
                DurationUnit::Hours => jiff::Unit::Hour,
                DurationUnit::Minutes => jiff::Unit::Minute,
                DurationUnit::Seconds => jiff::Unit::Second,
                DurationUnit::Milliseconds => jiff::Unit::Millisecond,
                DurationUnit::Microseconds => jiff::Unit::Microsecond,
                DurationUnit::Nanoseconds => jiff::Unit::Nanosecond,
                _ => unreachable!(),
            };
            let span = a
                .until((jiff_unit, b))
                .map_err(|e| PureRuntimeError::EvaluationError(format!("dateDiff: {e}")))?;
            let v = match unit {
                DurationUnit::Hours => i64::from(span.get_hours()),
                DurationUnit::Minutes => span.get_minutes(),
                DurationUnit::Seconds => span.get_seconds(),
                DurationUnit::Milliseconds => span.get_milliseconds(),
                DurationUnit::Microseconds => span.get_microseconds(),
                DurationUnit::Nanoseconds => span.get_nanoseconds(),
                _ => unreachable!(),
            };
            Ok(Evaluated::new(Value::Integer(v)))
        } else {
            // Calendar-component math for date-based units. Java Pure's
            // `dateDiff(a, b, YEARS)` returns `b.year - a.year` regardless
            // of the day/time within each year — so `dateDiff(2015-12-31,
            // 2016-01-01, YEARS) == 1`. jiff's `Span.years` measures
            // *elapsed* years (the same call would yield 0). Same shape
            // for Months: `(b.year - a.year) * 12 + (b.month - a.month)`.
            // Days/Weeks use the calendar-day delta (Days = epoch diff;
            // Weeks = Sunday boundaries crossed, see below).
            let a = d1.to_civil_date()?;
            let b = d2.to_civil_date()?;
            let v = match unit {
                DurationUnit::Years => i64::from(b.year()) - i64::from(a.year()),
                DurationUnit::Months => {
                    (i64::from(b.year()) - i64::from(a.year())) * 12
                        + (i64::from(b.month()) - i64::from(a.month()))
                }
                DurationUnit::Days => {
                    let span = a
                        .until(b)
                        .map_err(|e| PureRuntimeError::EvaluationError(format!("dateDiff: {e}")))?;
                    i64::from(span.get_days())
                }
                DurationUnit::Weeks => {
                    // Java Pure semantics: count of Sunday boundaries
                    // crossed, NOT raw `days / 7`. Forward (a < b):
                    // # Sundays in (a, b]; Backward (a > b): negative #
                    // Sundays in [b, a). Concretely Sat → Sun = 1 even
                    // though it's only 1 day, because one Sunday boundary
                    // is crossed; Sun → Sat = 0 because no Sunday is
                    // included. Tested by testDateDiffWeeks in
                    // essential/date/operation/dateDiff.pure.
                    let span = a
                        .until(b)
                        .map_err(|e| PureRuntimeError::EvaluationError(format!("dateDiff: {e}")))?;
                    let days = i64::from(span.get_days());
                    sunday_boundaries_between(a, b, days)
                }
                _ => unreachable!(),
            };
            Ok(Evaluated::new(Value::Integer(v)))
        }
    }
}

/// Pure `adjust(Date[1], Integer[1], DurationUnit[1]): Date[1]`.
///
/// Every unit goes through the matching `PureDate::add_*`; a week is
/// seven days. Sub-second units require at least second precision on
/// the input (see `PureDate::add_milliseconds`).
#[derive(Debug)]
pub struct Adjust;

impl NativeFunction for Adjust {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("adjust", &values, 3)?;
        let d = values[0].as_date()?;
        let n = values[1].as_integer()?;
        let unit = duration_unit(&values[2])?;

        let new_date = match unit {
            DurationUnit::Years => d.add_years(n),
            DurationUnit::Months => d.add_months(n),
            DurationUnit::Weeks => d.add_days(n.checked_mul(7).ok_or_else(|| {
                PureRuntimeError::EvaluationError("adjust: weeks overflow".into())
            })?),
            DurationUnit::Days => d.add_days(n),
            DurationUnit::Hours => d.add_hours(n),
            DurationUnit::Minutes => d.add_minutes(n),
            DurationUnit::Seconds => d.add_seconds(n),
            DurationUnit::Milliseconds => d.add_milliseconds(n),
            DurationUnit::Microseconds => d.add_microseconds(n),
            DurationUnit::Nanoseconds => d.add_nanoseconds(n),
        }?;
        Ok(Evaluated::new(Value::Date(new_date)))
    }
}

/// Register the fished date and time natives under their platform keys.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "dateDiff_Date_1__Date_1__DurationUnit_1__Integer_1_",
        DateDiff,
    );
    registry.register("adjust_Date_1__Integer_1__DurationUnit_1__Date_1_", Adjust);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::MockCtx;

    // ---- duration_unit ----

    #[test]
    fn duration_unit_fqn_form() {
        assert_eq!(
            duration_unit(&Value::String("DurationUnit.DAYS".into())).unwrap(),
            DurationUnit::Days
        );
    }

    #[test]
    fn duration_unit_bare_form() {
        assert_eq!(
            duration_unit(&Value::String("YEARS".into())).unwrap(),
            DurationUnit::Years
        );
    }

    #[test]
    fn duration_unit_unknown_errors() {
        assert!(duration_unit(&Value::String("WEEKS_AND_DAYS".into())).is_err());
    }

    #[test]
    fn duration_unit_type_mismatch_errors() {
        assert!(duration_unit(&Value::Integer(5)).is_err());
    }

    // ---- defensive checks (arity + type) ----
    //
    // Pure-level behaviour (happy paths, semantic errors) is driven through
    // a real `Evaluator` in `tests/datetime.rs`. The tests below only
    // exercise the natives' own defensive checks — `expect_args` for
    // arity, `as_date()` type assertion — which need no evaluator.

    #[test]
    fn date_diff_wrong_arg_count() {
        assert!(DateDiff.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn adjust_wrong_arg_count() {
        assert!(Adjust.execute(&[], &mut MockCtx).is_err());
    }

    // ---- registration ----

    #[test]
    fn register_populates_registry() {
        let mut reg = NativeRegistry::new();
        register(&mut reg);
        assert!(
            reg.get("adjust_Date_1__Integer_1__DurationUnit_1__Date_1_")
                .is_some()
        );
        assert!(
            reg.get("dateDiff_Date_1__Date_1__DurationUnit_1__Integer_1_")
                .is_some()
        );
    }
}
