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
    }
}
