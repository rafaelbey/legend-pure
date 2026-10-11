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

//! The collection natives driven through a real `Evaluator` over
//! hand-built nodes: the lambda-driven iterators, the natives that
//! allocate heap objects (`zip`) and the `Map` natives. Literal-only
//! cases stay inline in `native/collection.rs`.

use legend_pure_evaluator::builders::{ModelBuilder, expr, named, param};
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_evaluator::value::Value;
use legend_pure_metamodel::bootstrap::INTEGER_ID;
use legend_pure_metamodel::model::PureModel;
use legend_pure_metamodel::types::{Multiplicity, ValueSpec};

const FOLD: &str = "fold_T_MANY__Function_1__V_m__V_m_";
const PLUS: &str = "plus_Number_MANY__Number_1_";
const MINUS: &str = "minus_Number_MANY__Number_1_";

fn model() -> PureModel {
    ModelBuilder::new().finish()
}

fn ints(items: &[i64]) -> ValueSpec {
    expr::collection(items.iter().map(|&i| expr::int(i)).collect())
}

/// `{a, b | $a + $b}` over integers (Pure `plus` takes its operands as
/// one collection).
fn plus_lambda(a: &str, b: &str) -> ValueSpec {
    expr::lambda(
        vec![
            param(a, named(INTEGER_ID), Multiplicity::PureOne),
            param(b, named(INTEGER_ID), Multiplicity::PureOne),
        ],
        vec![expr::call(
            PLUS,
            vec![expr::collection(vec![expr::var(a), expr::var(b)])],
        )],
    )
}

#[test]
fn fold_threads_the_accumulator_through_the_lambda() {
    let model = model();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    // [1, 2, 3, 4]->fold({x, y | $x + $y}, 7)
    let sum = expr::call(
        FOLD,
        vec![ints(&[1, 2, 3, 4]), plus_lambda("x", "y"), expr::int(7)],
    );
    assert_eq!(eval.eval(&sum).unwrap(), Value::Integer(17));
    // [1, 2, 3]->fold({x, acc | $x - $acc}, 10): the lambda's parameters
    // are (element, accumulator): 1-10 = -9, 2-(-9) = 11, 3-11 = -8.
    let ordered = expr::call(
        FOLD,
        vec![
            ints(&[1, 2, 3]),
            expr::lambda(
                vec![
                    param("x", named(INTEGER_ID), Multiplicity::PureOne),
                    param("acc", named(INTEGER_ID), Multiplicity::PureOne),
                ],
                vec![expr::call(
                    MINUS,
                    vec![expr::collection(vec![expr::var("x"), expr::var("acc")])],
                )],
            ),
            expr::int(10),
        ],
    );
    assert_eq!(eval.eval(&ordered).unwrap(), Value::Integer(-8));
}

#[test]
fn fold_over_an_empty_collection_returns_the_identity() {
    let model = model();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let folded = expr::call(FOLD, vec![ints(&[]), plus_lambda("x", "y"), expr::int(1)]);
    assert_eq!(eval.eval(&folded).unwrap(), Value::Integer(1));
}
