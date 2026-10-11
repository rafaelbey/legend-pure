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

//! The date and time natives driven through a real `Evaluator`: date
//! literals and `DurationUnit` enum literals in, `PureDate` values out.
//! The expected values are the platform PCT assertions
//! (`essential/date/operation/*.pure`).

use legend_pure_evaluator::builders::{ModelBuilder, expr};
use legend_pure_evaluator::date::{PureDate, TimePrecision};
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_evaluator::value::Value;
use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::model::PureModel;
use legend_pure_metamodel::types::{DateValue, ValueSpec};

const ADJUST: &str = "adjust_Date_1__Integer_1__DurationUnit_1__Date_1_";

/// The platform's `meta::pure::functions::date::DurationUnit` (`_structures.pure`).
fn duration_unit_model() -> (PureModel, ElementId) {
    let mut b = ModelBuilder::new();
    let unit = b.enumeration(
        "meta::pure::functions::date::DurationUnit",
        &[
            "YEARS",
            "MONTHS",
            "WEEKS",
            "DAYS",
            "HOURS",
            "MINUTES",
            "SECONDS",
            "MILLISECONDS",
            "MICROSECONDS",
            "NANOSECONDS",
        ],
    );
    (b.finish(), unit)
}

/// `%y-m-d`.
fn strict(year: i16, month: i8, day: i8) -> ValueSpec {
    expr::date(DateValue::StrictDate {
        year,
        month: Some(month),
        day: Some(day),
    })
}

/// `%y-m-dTh:mi:s`.
fn second_precision(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> ValueSpec {
    expr::date(DateValue::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        subsecond_nanos: 0,
        subsecond_digits: 0,
        has_minutes: true,
        has_seconds: true,
        tz_offset_minutes: None,
    })
}

fn strict_value(year: i16, month: i8, day: i8) -> Value {
    Value::Date(PureDate::strict_date(year, month, day).unwrap())
}

fn second_value(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> Value {
    Value::Date(
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
        .unwrap(),
    )
}

#[test]
fn adjust_moves_a_date_by_each_calendar_unit() {
    let (model, unit) = duration_unit_model();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let adjust = |date: ValueSpec, n: i64, member: &str| {
        expr::call(
            ADJUST,
            vec![date, expr::int(n), expr::enum_value(unit, member)],
        )
    };
    // testAdjustByDays, testAdjustByWeeks, testAdjustByMonths, testAdjustByYears
    assert_eq!(
        eval.eval(&adjust(strict(2015, 4, 15), 1, "DAYS")).unwrap(),
        strict_value(2015, 4, 16)
    );
    assert_eq!(
        eval.eval(&adjust(strict(2015, 4, 15), -2, "WEEKS"))
            .unwrap(),
        strict_value(2015, 4, 1)
    );
    assert_eq!(
        eval.eval(&adjust(strict(2016, 1, 31), 1, "MONTHS"))
            .unwrap(),
        strict_value(2016, 2, 29)
    );
    assert_eq!(
        eval.eval(&adjust(strict(2016, 2, 29), 1, "YEARS")).unwrap(),
        strict_value(2017, 2, 28)
    );
    // testAdjustByHours
    assert_eq!(
        eval.eval(&adjust(
            second_precision(2015, 4, 15, 12, 12, 11),
            36,
            "HOURS"
        ))
        .unwrap(),
        second_value(2015, 4, 17, 0, 12, 11)
    );
}

#[test]
fn adjust_rejects_an_argument_that_is_not_a_duration_unit() {
    let (model, _) = duration_unit_model();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let call = expr::call(
        ADJUST,
        vec![strict(2015, 4, 15), expr::int(1), expr::int(3)],
    );
    assert!(eval.eval(&call).is_err());
}
