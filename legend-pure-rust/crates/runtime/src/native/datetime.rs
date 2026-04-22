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

//! Date and time native functions.
//!
//! This module covers Pure date/time built-ins: `now`, `today`, component
//! accessors (`year`, `monthNumber`, `dayOfMonth`, `hour`, `minute`,
//! `second`), precision probes (`hasDay`, `hasMonth`, `hasHour`,
//! `hasMinute`, `hasSecond`, `hasSubsecond`,
//! `hasSubsecondWithAtLeastPrecision`), date construction (`date` with
//! 1–6 Integer overloads), `datePart`, `parseDate`, `dateDiff`, and
//! `adjust`.
//!
//! All arithmetic and precision rules flow through [`PureDate`] — we do
//! not reach into `date.rs` to add new methods. `DurationUnit` values
//! arrive as `Value::String("DurationUnit.DAYS")` (the evaluator's
//! enum-value shape) and are parsed by their suffix.

use std::str::FromStr;

use crate::date::{DatePrecision, PureDate, TimePrecision};
use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// DurationUnit — the unit argument for adjust/dateDiff
// ---------------------------------------------------------------------------

/// The time-granularity unit accepted by `adjust` and `dateDiff`.
///
/// Pure's `DurationUnit` enum values reach the runtime as
/// `Value::String("DurationUnit.<VARIANT>")` via the evaluator's generic
/// enum-value fallback. We parse by suffix.
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
    /// Milliseconds — not supported by `PureDate` arithmetic; returns
    /// `EvaluationError` from `adjust`.
    Milliseconds,
    /// Microseconds — not supported by `PureDate` arithmetic; returns
    /// `EvaluationError` from `adjust`.
    Microseconds,
    /// Nanoseconds — not supported by `PureDate` arithmetic; returns
    /// `EvaluationError` from `adjust`.
    Nanoseconds,
}

/// Parse a `DurationUnit` from a Pure enum-value string.
///
/// Accepts both the fully-qualified form (`"DurationUnit.DAYS"`) and the
/// bare variant (`"DAYS"`). The `rsplit('.').next()` call yields
/// `Some("DAYS")` in both cases; the `unwrap_or` branch defends against
/// future changes to the format.
fn duration_unit(v: &Value) -> Result<DurationUnit, PureRuntimeError> {
    let s = v.as_string()?;
    let suffix = s.rsplit('.').next().unwrap_or(s.as_str());
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

// ---------------------------------------------------------------------------
// Small i64 → i8/i16/i32 helper
// ---------------------------------------------------------------------------

fn i64_to_i16_arg(name: &str, n: i64) -> Result<i16, PureRuntimeError> {
    i16::try_from(n)
        .map_err(|_| PureRuntimeError::EvaluationError(format!("{name}: year {n} out of range")))
}

fn i64_to_i8_arg(name: &str, n: i64) -> Result<i8, PureRuntimeError> {
    i8::try_from(n).map_err(|_| {
        PureRuntimeError::EvaluationError(format!("{name}: component {n} out of range"))
    })
}

// ---------------------------------------------------------------------------
// now / today
// ---------------------------------------------------------------------------

/// Pure `now(): DateTime[1]` — current instant, to-second precision, UTC.
#[derive(Debug)]
pub struct Now;

impl NativeFunction for Now {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("now", args, 0)?;
        let ts = jiff::Timestamp::now();
        let zoned = ts.in_tz("UTC").map_err(|e| {
            PureRuntimeError::EvaluationError(format!("now: timezone lookup failed: {e}"))
        })?;
        let dt = zoned.datetime();
        PureDate::datetime(
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second(),
            0,
            TimePrecision::Second,
        )
        .map(Value::Date)
    }

    fn signature(&self) -> &'static str {
        "now(): DateTime[1]"
    }
}

/// Pure `today(): StrictDate[1]` — today's calendar date, UTC.
#[derive(Debug)]
pub struct Today;

impl NativeFunction for Today {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("today", args, 0)?;
        let ts = jiff::Timestamp::now();
        let zoned = ts.in_tz("UTC").map_err(|e| {
            PureRuntimeError::EvaluationError(format!("today: timezone lookup failed: {e}"))
        })?;
        let d = zoned.date();
        PureDate::strict_date(d.year(), d.month(), d.day()).map(Value::Date)
    }

    fn signature(&self) -> &'static str {
        "today(): StrictDate[1]"
    }
}

// ---------------------------------------------------------------------------
// Component accessors
// ---------------------------------------------------------------------------

/// Pure `year(Date[1]): Integer[1]`.
#[derive(Debug)]
pub struct Year;

impl NativeFunction for Year {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("year", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Integer(i64::from(d.get_year())))
    }

    fn signature(&self) -> &'static str {
        "year(Date[1]): Integer[1]"
    }
}

/// Pure `monthNumber(Date[1]): Integer[1]`.
#[derive(Debug)]
pub struct MonthNumber;

impl NativeFunction for MonthNumber {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("monthNumber", args, 1)?;
        let d = args[0].as_date()?;
        match d.get_month() {
            Some(m) => Ok(Value::Integer(i64::from(m))),
            None => Err(PureRuntimeError::EvaluationError(
                "monthNumber: date has no month component".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "monthNumber(Date[1]): Integer[1]"
    }
}

/// Pure `dayOfMonth(Date[1]): Integer[1]`.
#[derive(Debug)]
pub struct DayOfMonth;

impl NativeFunction for DayOfMonth {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("dayOfMonth", args, 1)?;
        let d = args[0].as_date()?;
        match d.get_day() {
            Some(day) => Ok(Value::Integer(i64::from(day))),
            None => Err(PureRuntimeError::EvaluationError(
                "dayOfMonth: date has no day component".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "dayOfMonth(Date[1]): Integer[1]"
    }
}

/// Pure `hour(DateTime[1]): Integer[1]`.
#[derive(Debug)]
pub struct Hour;

impl NativeFunction for Hour {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hour", args, 1)?;
        let d = args[0].as_date()?;
        match d.get_hour() {
            Some(h) => Ok(Value::Integer(i64::from(h))),
            None => Err(PureRuntimeError::EvaluationError(
                "hour: date has no time component".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "hour(DateTime[1]): Integer[1]"
    }
}

/// Pure `minute(DateTime[1]): Integer[1]`.
#[derive(Debug)]
pub struct Minute;

impl NativeFunction for Minute {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("minute", args, 1)?;
        let d = args[0].as_date()?;
        match d.get_minute() {
            Some(m) => Ok(Value::Integer(i64::from(m))),
            None => Err(PureRuntimeError::EvaluationError(
                "minute: date has no minute component".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "minute(DateTime[1]): Integer[1]"
    }
}

/// Pure `second(DateTime[1]): Integer[1]`.
#[derive(Debug)]
pub struct Second;

impl NativeFunction for Second {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("second", args, 1)?;
        let d = args[0].as_date()?;
        match d.get_second() {
            Some(s) => Ok(Value::Integer(i64::from(s))),
            None => Err(PureRuntimeError::EvaluationError(
                "second: date has no second component".into(),
            )),
        }
    }

    fn signature(&self) -> &'static str {
        "second(DateTime[1]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// datePart
// ---------------------------------------------------------------------------

/// Pure `datePart(Date[1]): StrictDate[1]` — drop the time component.
///
/// Requires at least day precision. A year- or year-month-only date is
/// rejected rather than being silently padded with `month=1`/`day=1`.
#[derive(Debug)]
pub struct DatePart;

impl NativeFunction for DatePart {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("datePart", args, 1)?;
        let d = args[0].as_date()?;
        let year = d.get_year();
        let month = d.get_month().ok_or_else(|| {
            PureRuntimeError::EvaluationError("datePart: date has no month component".into())
        })?;
        let day = d.get_day().ok_or_else(|| {
            PureRuntimeError::EvaluationError("datePart: date has no day component".into())
        })?;
        PureDate::strict_date(year, month, day).map(Value::Date)
    }

    fn signature(&self) -> &'static str {
        "datePart(Date[1]): StrictDate[1]"
    }
}

// ---------------------------------------------------------------------------
// dateDiff
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("dateDiff", args, 3)?;
        let d1 = args[0].as_date()?;
        let d2 = args[1].as_date()?;
        let unit = duration_unit(&args[2])?;

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
            Ok(Value::Integer(v))
        } else {
            let a = d1.to_civil_date()?;
            let b = d2.to_civil_date()?;
            let jiff_unit = match unit {
                DurationUnit::Years => jiff::Unit::Year,
                DurationUnit::Months => jiff::Unit::Month,
                DurationUnit::Weeks => jiff::Unit::Week,
                DurationUnit::Days => jiff::Unit::Day,
                _ => unreachable!(),
            };
            let span = a
                .until((jiff_unit, b))
                .map_err(|e| PureRuntimeError::EvaluationError(format!("dateDiff: {e}")))?;
            let v = match unit {
                DurationUnit::Years => i64::from(span.get_years()),
                DurationUnit::Months => i64::from(span.get_months()),
                DurationUnit::Weeks => i64::from(span.get_weeks()),
                DurationUnit::Days => i64::from(span.get_days()),
                _ => unreachable!(),
            };
            Ok(Value::Integer(v))
        }
    }

    fn signature(&self) -> &'static str {
        "dateDiff(Date[1], Date[1], DurationUnit[1]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// adjust
// ---------------------------------------------------------------------------

/// Pure `adjust(Date[1], Integer[1], DurationUnit[1]): Date[1]`.
///
/// Sub-second units (milli/micro/nano) are not supported by `PureDate`'s
/// `add_*` API and return `EvaluationError`.
#[derive(Debug)]
pub struct Adjust;

impl NativeFunction for Adjust {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("adjust", args, 3)?;
        let d = args[0].as_date()?;
        let n = args[1].as_integer()?;
        let unit = duration_unit(&args[2])?;

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
            DurationUnit::Milliseconds | DurationUnit::Microseconds | DurationUnit::Nanoseconds => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "adjust: DurationUnit {unit:?} not supported by PureDate arithmetic"
                )));
            }
        }?;
        Ok(Value::Date(new_date))
    }

    fn signature(&self) -> &'static str {
        "adjust(Date[1], Integer[1], DurationUnit[1]): Date[1]"
    }
}

// ---------------------------------------------------------------------------
// Precision probes
// ---------------------------------------------------------------------------

fn has_precision_at_least(d: &PureDate, at: DatePrecision) -> bool {
    match (d.precision(), at) {
        // Non-time-vs-non-time comparison falls back to enum Ord.
        (DatePrecision::Year, DatePrecision::Year) => true,
        (p, DatePrecision::Year) => p >= DatePrecision::Year || matches!(p, DatePrecision::Time(_)),
        (p, DatePrecision::Month) => {
            p >= DatePrecision::Month || matches!(p, DatePrecision::Time(_))
        }
        (p, DatePrecision::Day) => p >= DatePrecision::Day || matches!(p, DatePrecision::Time(_)),
        // Time comparison: Time(a) >= Time(b) iff a >= b on the nested enum.
        (DatePrecision::Time(a), DatePrecision::Time(b)) => a >= b,
        (_, DatePrecision::Time(_)) => false,
    }
}

/// Pure `hasMonth(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasMonth;

impl NativeFunction for HasMonth {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasMonth", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(has_precision_at_least(
            &d,
            DatePrecision::Month,
        )))
    }

    fn signature(&self) -> &'static str {
        "hasMonth(Date[1]): Boolean[1]"
    }
}

/// Pure `hasDay(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasDay;

impl NativeFunction for HasDay {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasDay", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(has_precision_at_least(
            &d,
            DatePrecision::Day,
        )))
    }

    fn signature(&self) -> &'static str {
        "hasDay(Date[1]): Boolean[1]"
    }
}

/// Pure `hasHour(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasHour;

impl NativeFunction for HasHour {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasHour", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(d.has_time()))
    }

    fn signature(&self) -> &'static str {
        "hasHour(Date[1]): Boolean[1]"
    }
}

/// Pure `hasMinute(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasMinute;

impl NativeFunction for HasMinute {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasMinute", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(matches!(
            d.precision(),
            DatePrecision::Time(tp) if tp >= TimePrecision::Minute
        )))
    }

    fn signature(&self) -> &'static str {
        "hasMinute(Date[1]): Boolean[1]"
    }
}

/// Pure `hasSecond(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasSecond;

impl NativeFunction for HasSecond {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasSecond", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(matches!(
            d.precision(),
            DatePrecision::Time(tp) if tp >= TimePrecision::Second
        )))
    }

    fn signature(&self) -> &'static str {
        "hasSecond(Date[1]): Boolean[1]"
    }
}

/// Pure `hasSubsecond(Date[1]): Boolean[1]`.
#[derive(Debug)]
pub struct HasSubsecond;

impl NativeFunction for HasSubsecond {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasSubsecond", args, 1)?;
        let d = args[0].as_date()?;
        Ok(Value::Boolean(matches!(
            d.precision(),
            DatePrecision::Time(TimePrecision::Subsecond(_))
        )))
    }

    fn signature(&self) -> &'static str {
        "hasSubsecond(Date[1]): Boolean[1]"
    }
}

/// Pure `hasSubsecondWithAtLeastPrecision(Date[1], Integer[1]): Boolean[1]`.
///
/// The second argument is the required number of subsecond digits (1–9).
/// Returns `true` iff the date carries subsecond precision whose digit
/// count is at least that value.
#[derive(Debug)]
pub struct HasSubsecondWithAtLeastPrecision;

impl NativeFunction for HasSubsecondWithAtLeastPrecision {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("hasSubsecondWithAtLeastPrecision", args, 2)?;
        let d = args[0].as_date()?;
        let required = args[1].as_integer()?;
        let have = match d.precision() {
            DatePrecision::Time(TimePrecision::Subsecond(digits)) => i64::from(digits),
            _ => return Ok(Value::Boolean(false)),
        };
        Ok(Value::Boolean(have >= required))
    }

    fn signature(&self) -> &'static str {
        "hasSubsecondWithAtLeastPrecision(Date[1], Integer[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// parseDate
// ---------------------------------------------------------------------------

/// Pure `parseDate(String[1]): Date[1]`.
///
/// Accepts a small family of ISO-like shapes (delegated to `jiff::civil`):
/// * `"YYYY-MM-DDTHH:MM:SS[.fffffffff]"` → `DateTime`
/// * `"YYYY-MM-DD"`                       → `StrictDate`
///
/// Returns `EvaluationError` if neither shape parses.
#[derive(Debug)]
pub struct ParseDate;

impl NativeFunction for ParseDate {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("parseDate", args, 1)?;
        let s = args[0].as_string()?.as_str();

        // DateTime first — has the 'T' separator when present.
        if s.contains('T') {
            let dt = jiff::civil::DateTime::from_str(s).map_err(|e| {
                PureRuntimeError::EvaluationError(format!("parseDate: invalid datetime: {e}"))
            })?;
            return Ok(Value::Date(PureDate::from_civil_datetime(dt)));
        }

        // Fall back to date.
        let date = jiff::civil::Date::from_str(s).map_err(|e| {
            PureRuntimeError::EvaluationError(format!("parseDate: invalid date: {e}"))
        })?;
        Ok(Value::Date(PureDate::from_civil_date(date)))
    }

    fn signature(&self) -> &'static str {
        "parseDate(String[1]): Date[1]"
    }
}

// ---------------------------------------------------------------------------
// date(...) — six Integer-arity overloads sharing one implementation
// ---------------------------------------------------------------------------

/// Pure `date(Integer[1], ...): Date[1]` — construct a date from 1–6
/// Integer components (year; year+month; year+month+day; …; +hour; +min;
/// +sec). Arity selects precision.
#[derive(Debug)]
pub struct DateConstruct;

impl NativeFunction for DateConstruct {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        if args.is_empty() || args.len() > 6 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "date: expected 1..=6 arguments, got {}",
                args.len()
            )));
        }

        let year = i64_to_i16_arg("date", args[0].as_integer()?)?;
        if args.len() == 1 {
            return PureDate::year(year).map(Value::Date);
        }

        let month = i64_to_i8_arg("date", args[1].as_integer()?)?;
        if args.len() == 2 {
            return PureDate::year_month(year, month).map(Value::Date);
        }

        let day = i64_to_i8_arg("date", args[2].as_integer()?)?;
        if args.len() == 3 {
            return PureDate::strict_date(year, month, day).map(Value::Date);
        }

        let hour = i64_to_i8_arg("date", args[3].as_integer()?)?;
        if args.len() == 4 {
            return PureDate::datetime(year, month, day, hour, 0, 0, 0, TimePrecision::Hour)
                .map(Value::Date);
        }

        let minute = i64_to_i8_arg("date", args[4].as_integer()?)?;
        if args.len() == 5 {
            return PureDate::datetime(year, month, day, hour, minute, 0, 0, TimePrecision::Minute)
                .map(Value::Date);
        }

        let second = i64_to_i8_arg("date", args[5].as_integer()?)?;
        PureDate::datetime(
            year,
            month,
            day,
            hour,
            minute,
            second,
            0,
            TimePrecision::Second,
        )
        .map(Value::Date)
    }

    fn signature(&self) -> &'static str {
        "date(Integer[1], ...): Date[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all datetime native functions into the registry under their
/// fully qualified mangled names.
///
/// Not currently wired into `NativeRegistry::standard()` — integration
/// happens in a separate track.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("now__DateTime_1_", Now);
    registry.register("today__StrictDate_1_", Today);

    registry.register("year_Date_1__Integer_1_", Year);
    registry.register("monthNumber_Date_1__Integer_1_", MonthNumber);
    registry.register("dayOfMonth_Date_1__Integer_1_", DayOfMonth);
    registry.register("hour_DateTime_1__Integer_1_", Hour);
    registry.register("minute_DateTime_1__Integer_1_", Minute);
    registry.register("second_DateTime_1__Integer_1_", Second);

    registry.register("datePart_Date_1__StrictDate_1_", DatePart);

    registry.register(
        "dateDiff_Date_1__Date_1__DurationUnit_1__Integer_1_",
        DateDiff,
    );
    registry.register("adjust_Date_1__Integer_1__DurationUnit_1__Date_1_", Adjust);

    registry.register("hasMonth_Date_1__Boolean_1_", HasMonth);
    registry.register("hasDay_Date_1__Boolean_1_", HasDay);
    registry.register("hasHour_Date_1__Boolean_1_", HasHour);
    registry.register("hasMinute_Date_1__Boolean_1_", HasMinute);
    registry.register("hasSecond_Date_1__Boolean_1_", HasSecond);
    registry.register("hasSubsecond_Date_1__Boolean_1_", HasSubsecond);
    registry.register(
        "hasSubsecondWithAtLeastPrecision_Date_1__Integer_1__Boolean_1_",
        HasSubsecondWithAtLeastPrecision,
    );

    registry.register("parseDate_String_1__Date_1_", ParseDate);

    // date(...) overloads — six mangled keys pointing at one impl.
    registry.register("date_Integer_1__Date_1_", DateConstruct);
    registry.register("date_Integer_1__Integer_1__Date_1_", DateConstruct);
    registry.register(
        "date_Integer_1__Integer_1__Integer_1__Date_1_",
        DateConstruct,
    );
    registry.register(
        "date_Integer_1__Integer_1__Integer_1__Integer_1__Date_1_",
        DateConstruct,
    );
    registry.register(
        "date_Integer_1__Integer_1__Integer_1__Integer_1__Integer_1__Date_1_",
        DateConstruct,
    );
    registry.register(
        "date_Integer_1__Integer_1__Integer_1__Integer_1__Integer_1__Integer_1__Date_1_",
        DateConstruct,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NoOpEvalCtx;

    // ---- helpers ----

    fn date(y: i16, m: i8, d: i8) -> Value {
        Value::Date(PureDate::strict_date(y, m, d).unwrap())
    }

    fn datetime_s(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        Value::Date(PureDate::datetime(y, m, d, h, mi, s, 0, TimePrecision::Second).unwrap())
    }

    fn subsec(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8, nanos: i32, digits: u8) -> Value {
        Value::Date(
            PureDate::datetime(y, m, d, h, mi, s, nanos, TimePrecision::Subsecond(digits)).unwrap(),
        )
    }

    fn dunit(name: &str) -> Value {
        Value::String(format!("DurationUnit.{name}").into())
    }

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

    // ---- now / today ----

    #[test]
    fn now_returns_datetime() {
        let r = Now.execute(&[], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Date(d) => {
                assert!(d.has_time());
                assert!(d.get_year() >= 2024);
            }
            other => panic!("expected Date, got {other:?}"),
        }
    }

    #[test]
    fn now_rejects_args() {
        assert!(Now.execute(&[Value::Integer(1)], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn today_returns_strict_date() {
        let r = Today.execute(&[], &mut NoOpEvalCtx).unwrap();
        match r {
            Value::Date(d) => {
                assert!(!d.has_time());
                assert_eq!(d.precision(), DatePrecision::Day);
            }
            other => panic!("expected Date, got {other:?}"),
        }
    }

    #[test]
    fn today_rejects_args() {
        assert!(
            Today
                .execute(&[Value::Integer(1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // ---- year / monthNumber / dayOfMonth ----

    #[test]
    fn year_happy() {
        assert_eq!(
            Year.execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(2024)
        );
    }

    #[test]
    fn year_year_only() {
        let d = Value::Date(PureDate::year(2024).unwrap());
        assert_eq!(
            Year.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Integer(2024)
        );
    }

    #[test]
    fn year_wrong_arg_count() {
        assert!(Year.execute(&[], &mut NoOpEvalCtx).is_err());
        assert!(
            Year.execute(&[date(2024, 1, 1), date(2024, 1, 1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn year_type_mismatch() {
        assert!(
            Year.execute(&[Value::Integer(2024)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn month_number_happy() {
        assert_eq!(
            MonthNumber
                .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(3)
        );
    }

    #[test]
    fn month_number_year_only_errors() {
        let d = Value::Date(PureDate::year(2024).unwrap());
        assert!(MonthNumber.execute(&[d], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn day_of_month_happy() {
        assert_eq!(
            DayOfMonth
                .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(15)
        );
    }

    #[test]
    fn day_of_month_no_day_errors() {
        let d = Value::Date(PureDate::year_month(2024, 3).unwrap());
        assert!(DayOfMonth.execute(&[d], &mut NoOpEvalCtx).is_err());
    }

    // ---- hour / minute / second ----

    #[test]
    fn hour_happy() {
        assert_eq!(
            Hour.execute(&[datetime_s(2024, 3, 15, 10, 30, 45)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(10)
        );
    }

    #[test]
    fn hour_no_time_errors() {
        assert!(
            Hour.execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn minute_happy() {
        assert_eq!(
            Minute
                .execute(&[datetime_s(2024, 3, 15, 10, 30, 45)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(30)
        );
    }

    #[test]
    fn minute_only_hour_errors() {
        let d =
            Value::Date(PureDate::datetime(2024, 3, 15, 10, 0, 0, 0, TimePrecision::Hour).unwrap());
        assert!(Minute.execute(&[d], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn second_happy() {
        assert_eq!(
            Second
                .execute(&[datetime_s(2024, 3, 15, 10, 30, 45)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(45)
        );
    }

    #[test]
    fn second_only_minute_errors() {
        let d = Value::Date(
            PureDate::datetime(2024, 3, 15, 10, 30, 0, 0, TimePrecision::Minute).unwrap(),
        );
        assert!(Second.execute(&[d], &mut NoOpEvalCtx).is_err());
    }

    // ---- datePart ----

    #[test]
    fn date_part_strips_time() {
        let d = datetime_s(2024, 3, 15, 10, 30, 45);
        let r = DatePart.execute(&[d], &mut NoOpEvalCtx).unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date");
        };
        assert_eq!(pd.precision(), DatePrecision::Day);
        assert_eq!(pd.get_year(), 2024);
        assert_eq!(pd.get_month(), Some(3));
        assert_eq!(pd.get_day(), Some(15));
    }

    #[test]
    fn date_part_of_strict_date_is_identity() {
        let r = DatePart
            .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, date(2024, 3, 15));
    }

    #[test]
    fn date_part_rejects_year_only() {
        let d = Value::Date(PureDate::year(2024).unwrap());
        assert!(DatePart.execute(&[d], &mut NoOpEvalCtx).is_err());
    }

    // ---- dateDiff ----

    #[test]
    fn date_diff_days() {
        let r = DateDiff
            .execute(
                &[date(2024, 3, 15), date(2024, 3, 20), dunit("DAYS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(5));
    }

    #[test]
    fn date_diff_days_negative() {
        let r = DateDiff
            .execute(
                &[date(2024, 3, 20), date(2024, 3, 15), dunit("DAYS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(-5));
    }

    #[test]
    fn date_diff_years() {
        let r = DateDiff
            .execute(
                &[date(2020, 1, 1), date(2024, 1, 1), dunit("YEARS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(4));
    }

    #[test]
    fn date_diff_months() {
        let r = DateDiff
            .execute(
                &[date(2024, 1, 1), date(2024, 5, 1), dunit("MONTHS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(4));
    }

    #[test]
    fn date_diff_weeks() {
        let r = DateDiff
            .execute(
                &[date(2024, 3, 1), date(2024, 3, 22), dunit("WEEKS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(3));
    }

    #[test]
    fn date_diff_hours() {
        let r = DateDiff
            .execute(
                &[
                    datetime_s(2024, 3, 15, 10, 0, 0),
                    datetime_s(2024, 3, 15, 13, 0, 0),
                    dunit("HOURS"),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(3));
    }

    #[test]
    fn date_diff_seconds() {
        let r = DateDiff
            .execute(
                &[
                    datetime_s(2024, 3, 15, 10, 0, 0),
                    datetime_s(2024, 3, 15, 10, 0, 30),
                    dunit("SECONDS"),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, Value::Integer(30));
    }

    #[test]
    fn date_diff_time_unit_needs_time_precision() {
        let r = DateDiff.execute(
            &[date(2024, 3, 15), date(2024, 3, 16), dunit("SECONDS")],
            &mut NoOpEvalCtx,
        );
        assert!(r.is_err());
    }

    #[test]
    fn date_diff_wrong_arg_count() {
        assert!(
            DateDiff
                .execute(&[date(2024, 1, 1), date(2024, 1, 1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn date_diff_unknown_unit() {
        assert!(
            DateDiff
                .execute(
                    &[
                        date(2024, 1, 1),
                        date(2024, 1, 2),
                        Value::String("DurationUnit.FORTNIGHTS".into())
                    ],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    // ---- adjust ----

    #[test]
    fn adjust_days() {
        let r = Adjust
            .execute(
                &[date(2024, 3, 15), Value::Integer(5), dunit("DAYS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, date(2024, 3, 20));
    }

    #[test]
    fn adjust_weeks() {
        let r = Adjust
            .execute(
                &[date(2024, 3, 1), Value::Integer(2), dunit("WEEKS")],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, date(2024, 3, 15));
    }

    #[test]
    fn adjust_years() {
        let d = Value::Date(PureDate::year(2020).unwrap());
        let r = Adjust
            .execute(&[d, Value::Integer(4), dunit("YEARS")], &mut NoOpEvalCtx)
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date");
        };
        assert_eq!(pd.get_year(), 2024);
    }

    #[test]
    fn adjust_hours_on_datetime() {
        let r = Adjust
            .execute(
                &[
                    datetime_s(2024, 3, 15, 10, 0, 0),
                    Value::Integer(3),
                    dunit("HOURS"),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, datetime_s(2024, 3, 15, 13, 0, 0));
    }

    #[test]
    fn adjust_hours_on_year_only_errors() {
        let d = Value::Date(PureDate::year(2024).unwrap());
        assert!(
            Adjust
                .execute(&[d, Value::Integer(1), dunit("HOURS")], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn adjust_milliseconds_unsupported() {
        assert!(
            Adjust
                .execute(
                    &[
                        datetime_s(2024, 3, 15, 10, 0, 0),
                        Value::Integer(1),
                        dunit("MILLISECONDS")
                    ],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    #[test]
    fn adjust_wrong_arg_count() {
        assert!(
            Adjust
                .execute(&[date(2024, 1, 1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // ---- hasXxx probes ----

    #[test]
    fn has_month_true() {
        assert_eq!(
            HasMonth
                .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_month_false_on_year_only() {
        let d = Value::Date(PureDate::year(2024).unwrap());
        assert_eq!(
            HasMonth.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_day_true() {
        assert_eq!(
            HasDay
                .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_day_false_on_year_month() {
        let d = Value::Date(PureDate::year_month(2024, 3).unwrap());
        assert_eq!(
            HasDay.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_hour_true_on_datetime() {
        assert_eq!(
            HasHour
                .execute(&[datetime_s(2024, 3, 15, 10, 0, 0)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_hour_false_on_date() {
        assert_eq!(
            HasHour
                .execute(&[date(2024, 3, 15)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_minute_true_on_datetime_with_minute() {
        let d = Value::Date(
            PureDate::datetime(2024, 3, 15, 10, 30, 0, 0, TimePrecision::Minute).unwrap(),
        );
        assert_eq!(
            HasMinute.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_minute_false_on_hour_only() {
        let d =
            Value::Date(PureDate::datetime(2024, 3, 15, 10, 0, 0, 0, TimePrecision::Hour).unwrap());
        assert_eq!(
            HasMinute.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_second_true_on_seconds() {
        assert_eq!(
            HasSecond
                .execute(&[datetime_s(2024, 3, 15, 10, 30, 0)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_second_false_on_minute_only() {
        let d = Value::Date(
            PureDate::datetime(2024, 3, 15, 10, 30, 0, 0, TimePrecision::Minute).unwrap(),
        );
        assert_eq!(
            HasSecond.execute(&[d], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_subsecond_true() {
        assert_eq!(
            HasSubsecond
                .execute(
                    &[subsec(2024, 3, 15, 10, 30, 0, 123_000_000, 3)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn has_subsecond_false_on_seconds_only() {
        assert_eq!(
            HasSubsecond
                .execute(&[datetime_s(2024, 3, 15, 10, 30, 0)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_subsecond_with_at_least_precision_ok() {
        let d = subsec(2024, 3, 15, 10, 30, 0, 123_000_000, 3);
        assert_eq!(
            HasSubsecondWithAtLeastPrecision
                .execute(&[d.clone(), Value::Integer(3)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            HasSubsecondWithAtLeastPrecision
                .execute(&[d, Value::Integer(6)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_subsecond_with_at_least_precision_on_seconds_is_false() {
        assert_eq!(
            HasSubsecondWithAtLeastPrecision
                .execute(
                    &[datetime_s(2024, 3, 15, 10, 30, 0), Value::Integer(1)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn has_subsecond_with_at_least_precision_wrong_args() {
        assert!(
            HasSubsecondWithAtLeastPrecision
                .execute(&[date(2024, 1, 1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // ---- parseDate ----

    #[test]
    fn parse_date_strict() {
        let r = ParseDate
            .execute(&[Value::String("2024-03-15".into())], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(r, date(2024, 3, 15));
    }

    #[test]
    fn parse_date_datetime() {
        let r = ParseDate
            .execute(
                &[Value::String("2024-03-15T10:30:45".into())],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date");
        };
        assert_eq!(pd.get_hour(), Some(10));
        assert_eq!(pd.get_minute(), Some(30));
        assert_eq!(pd.get_second(), Some(45));
    }

    #[test]
    fn parse_date_invalid() {
        assert!(
            ParseDate
                .execute(&[Value::String("not-a-date".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_date_type_mismatch() {
        assert!(
            ParseDate
                .execute(&[Value::Integer(20240315)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn parse_date_wrong_arg_count() {
        assert!(ParseDate.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    // ---- date(...) construct ----

    #[test]
    fn date_construct_year() {
        let r = DateConstruct
            .execute(&[Value::Integer(2024)], &mut NoOpEvalCtx)
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date")
        };
        assert_eq!(pd.precision(), DatePrecision::Year);
        assert_eq!(pd.get_year(), 2024);
    }

    #[test]
    fn date_construct_year_month() {
        let r = DateConstruct
            .execute(&[Value::Integer(2024), Value::Integer(3)], &mut NoOpEvalCtx)
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date")
        };
        assert_eq!(pd.precision(), DatePrecision::Month);
        assert_eq!(pd.get_month(), Some(3));
    }

    #[test]
    fn date_construct_strict_date() {
        let r = DateConstruct
            .execute(
                &[Value::Integer(2024), Value::Integer(3), Value::Integer(15)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, date(2024, 3, 15));
    }

    #[test]
    fn date_construct_hour() {
        let r = DateConstruct
            .execute(
                &[
                    Value::Integer(2024),
                    Value::Integer(3),
                    Value::Integer(15),
                    Value::Integer(10),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date")
        };
        assert_eq!(pd.precision(), DatePrecision::Time(TimePrecision::Hour));
        assert_eq!(pd.get_hour(), Some(10));
    }

    #[test]
    fn date_construct_minute() {
        let r = DateConstruct
            .execute(
                &[
                    Value::Integer(2024),
                    Value::Integer(3),
                    Value::Integer(15),
                    Value::Integer(10),
                    Value::Integer(30),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        let Value::Date(pd) = r else {
            panic!("expected Date")
        };
        assert_eq!(pd.precision(), DatePrecision::Time(TimePrecision::Minute));
        assert_eq!(pd.get_minute(), Some(30));
    }

    #[test]
    fn date_construct_second() {
        let r = DateConstruct
            .execute(
                &[
                    Value::Integer(2024),
                    Value::Integer(3),
                    Value::Integer(15),
                    Value::Integer(10),
                    Value::Integer(30),
                    Value::Integer(45),
                ],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, datetime_s(2024, 3, 15, 10, 30, 45));
    }

    #[test]
    fn date_construct_empty_errors() {
        assert!(DateConstruct.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn date_construct_too_many_errors() {
        let args: Vec<Value> = (0..7).map(|_| Value::Integer(1)).collect();
        assert!(DateConstruct.execute(&args, &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn date_construct_invalid_month_errors() {
        assert!(
            DateConstruct
                .execute(
                    &[Value::Integer(2024), Value::Integer(13)],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    #[test]
    fn date_construct_year_out_of_range() {
        assert!(
            DateConstruct
                .execute(&[Value::Integer(10_000_000)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn date_construct_type_mismatch() {
        assert!(
            DateConstruct
                .execute(&[Value::String("2024".into())], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // ---- registration ----

    #[test]
    fn register_populates_registry() {
        let mut reg = NativeRegistry::new();
        register(&mut reg);
        assert!(reg.get("now__DateTime_1_").is_some());
        assert!(reg.get("today__StrictDate_1_").is_some());
        assert!(reg.get("year_Date_1__Integer_1_").is_some());
        assert!(reg.get("parseDate_String_1__Date_1_").is_some());
        assert!(reg.get("date_Integer_1__Date_1_").is_some());
        assert!(
            reg.get(
                "date_Integer_1__Integer_1__Integer_1__Integer_1__Integer_1__Integer_1__Date_1_"
            )
            .is_some()
        );
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
