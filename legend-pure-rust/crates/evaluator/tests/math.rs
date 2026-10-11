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

//! The math, arithmetic and comparison natives fished from the archive,
//! driven through a real `Evaluator` over the bootstrap model: each call
//! is a hand-built node dispatched by its mangled key, the same shape the
//! PCT lambdas reach the evaluator in.

use std::str::FromStr;

use legend_pure_evaluator::builders::{ModelBuilder, expr};
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_evaluator::value::Value;
use legend_pure_metamodel::types::ValueSpec;
use rust_decimal::Decimal;

const ROUND: &str = "round_Number_1__Integer_1_";
const ROUND_DECIMAL: &str = "round_Decimal_1__Integer_1__Decimal_1_";
const ROUND_FLOAT: &str = "round_Float_1__Integer_1__Float_1_";
const LESS_THAN: &str = "lessThan_Number_1__Number_1__Boolean_1_";

/// Evaluate one call node against the bootstrap model with every fished native.
fn run(call: &ValueSpec) -> Value {
    let model = ModelBuilder::new().finish();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    eval.eval(call).unwrap()
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn round_rounds_half_to_even_through_the_evaluator() {
    // testPositiveFloatRoundHalfEvenDown / …HalfEvenUp / testNegativeFloatRoundHalfEvenDown
    assert_eq!(
        run(&expr::call(ROUND, vec![expr::float(2.5)])),
        Value::Integer(2)
    );
    assert_eq!(
        run(&expr::call(ROUND, vec![expr::float(16.5)])),
        Value::Integer(16)
    );
    assert_eq!(
        run(&expr::call(ROUND, vec![expr::float(-17.5)])),
        Value::Integer(-18)
    );
    assert_eq!(
        run(&expr::call(ROUND, vec![expr::int(17)])),
        Value::Integer(17)
    );
    // testDecimalRound: `round(3.14159d)` and `3.14159d->round(2)`.
    assert_eq!(
        run(&expr::call(ROUND, vec![expr::decimal(dec("-3.14159"))])),
        Value::Integer(-3)
    );
    assert_eq!(
        run(&expr::call(
            ROUND_DECIMAL,
            vec![expr::decimal(dec("3.14159")), expr::int(3)]
        )),
        Value::Decimal(dec("3.142"))
    );
    // testFloatRoundWithScale: `round(-17.36, 1)`.
    assert_eq!(
        run(&expr::call(
            ROUND_FLOAT,
            vec![expr::float(-17.36), expr::int(1)]
        )),
        Value::Float(-17.4)
    );
}

#[test]
fn less_than_orders_numbers_and_strings_through_the_evaluator() {
    // testLessThan_Number: `3 < 4`, `4 < 3`, `3 < 3`.
    let lt = |a: ValueSpec, b: ValueSpec| run(&expr::call(LESS_THAN, vec![a, b]));
    assert_eq!(lt(expr::int(1), expr::int(2)), Value::Boolean(true));
    assert_eq!(lt(expr::int(4), expr::int(3)), Value::Boolean(false));
    assert_eq!(lt(expr::int(3), expr::int(3)), Value::Boolean(false));
    // Mixed Number subtypes promote: `1 < 2.2`, `0.5d < 2.5`.
    assert_eq!(lt(expr::int(1), expr::float(2.2)), Value::Boolean(true));
    assert_eq!(
        lt(expr::decimal(dec("0.5")), expr::float(2.5)),
        Value::Boolean(true)
    );
    // The platform's `lessThan(String, String)` reaches the same native.
    assert_eq!(
        lt(expr::string("a"), expr::string("b")),
        Value::Boolean(true)
    );
}
