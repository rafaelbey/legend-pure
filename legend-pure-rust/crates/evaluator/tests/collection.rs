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
const DROP: &str = "drop_T_MANY__Integer_1__T_MANY_";
const TAKE: &str = "take_T_MANY__Integer_1__T_MANY_";
const ZIP: &str = "zip_T_MANY__U_MANY__Pair_MANY_";
const PLUS: &str = "plus_Number_MANY__Number_1_";
const MINUS: &str = "minus_Number_MANY__Number_1_";

fn model() -> PureModel {
    ModelBuilder::new().finish()
}

/// Evaluate `spec` on a fresh evaluator over the bootstrap.
fn run(spec: &ValueSpec) -> Value {
    let model = model();
    let registry = NativeRegistry::standard();
    Evaluator::new(&model, &registry)
        .eval(spec)
        .unwrap_or_else(|e| panic!("evaluation failed: {e:?}"))
}

fn ints(items: &[i64]) -> ValueSpec {
    expr::collection(items.iter().map(|&i| expr::int(i)).collect())
}

/// The integers of a result, whatever its multiplicity shape (`Unit`,
/// scalar or `Collection`).
fn as_ints(value: &Value) -> Vec<i64> {
    value
        .to_collection()
        .iter()
        .map(|v| v.as_integer().expect("an Integer"))
        .collect()
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

#[test]
fn drop_skips_the_first_n_and_clamps_the_count() {
    let drop = |items: &[i64], n: i64| run(&expr::call(DROP, vec![ints(items), expr::int(n)]));
    assert_eq!(as_ints(&drop(&[1, 2, 3], 1)), [2, 3]);
    assert_eq!(as_ints(&drop(&[1, 2, 3], 5)), Vec::<i64>::new());
    assert_eq!(as_ints(&drop(&[1, 2, 3], -1)), [1, 2, 3]);
    assert_eq!(as_ints(&drop(&[], 1)), Vec::<i64>::new());
}

#[test]
fn take_keeps_the_first_n_and_clamps_the_count() {
    let take = |items: &[i64], n: i64| run(&expr::call(TAKE, vec![ints(items), expr::int(n)]));
    assert_eq!(as_ints(&take(&[1, 2, 3], 2)), [1, 2]);
    assert_eq!(as_ints(&take(&[1, 2, 3], 5)), [1, 2, 3]);
    assert_eq!(as_ints(&take(&[1, 2, 3], -1)), Vec::<i64>::new());
    assert_eq!(as_ints(&take(&[], 1)), Vec::<i64>::new());
}

fn strings(items: &[&str]) -> ValueSpec {
    expr::collection(items.iter().map(|s| expr::string(s)).collect())
}

#[test]
fn zip_pairs_up_to_the_shorter_side_and_the_pairs_read_back() {
    let model = model();
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let zipped = eval
        .eval(&expr::call(
            ZIP,
            vec![ints(&[1, 2, 3]), strings(&["a", "b"])],
        ))
        .unwrap();
    let pairs = zipped.to_collection();
    assert_eq!(pairs.len(), 2);
    for pair in &pairs {
        let Value::Object(handle) = pair else {
            panic!("expected a Pair object, got {pair:?}");
        };
        assert_eq!(
            eval.heap().classifier(handle).unwrap(),
            "meta::pure::functions::collection::Pair"
        );
    }
    // $zipped->at(1).first / .second, through property access on the heap object.
    eval.context_mut().push_scope();
    eval.context_mut().set("p", pairs[1].clone());
    assert_eq!(
        eval.eval(&expr::prop(expr::var("p"), "first")).unwrap(),
        Value::Integer(2)
    );
    assert_eq!(
        eval.eval(&expr::prop(expr::var("p"), "second")).unwrap(),
        Value::String("b".into())
    );
    // Either side empty: no pairs.
    assert!(run(&expr::call(ZIP, vec![ints(&[]), strings(&["a"])])).is_empty());
    assert!(run(&expr::call(ZIP, vec![ints(&[1]), strings(&[])])).is_empty());
}
