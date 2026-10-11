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
use legend_pure_evaluator::error::{PureExceptionKind, PureRuntimeError};
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
const FIRST: &str = "first_T_MANY__T_$0_1$_";
const REMOVE_DUPLICATES: &str = "removeDuplicates_T_MANY__Function_$0_1$__Function_$0_1$__T_MANY_";
const SORT: &str = "sort_T_m__Function_$0_1$__Function_$0_1$__T_m_";
const SIZE: &str = "size_Any_MANY__Integer_1_";
const RANGE: &str = "range_Integer_1__Integer_1__Integer_1__Integer_MANY_";
const MAP_M: &str = "map_T_m__Function_1__V_m_";
const MAP_MANY: &str = "map_T_MANY__Function_1__V_MANY_";
const MAP_ZERO_ONE: &str = "map_T_$0_1$__Function_1__V_$0_1$_";
const TIMES: &str = "times_Number_MANY__Number_1_";
const EQ: &str = "eq_Any_1__Any_1__Boolean_1_";
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

/// Evaluate `spec` on a fresh evaluator and return the message of the
/// `EvaluationError` it raises.
fn run_err(spec: &ValueSpec) -> String {
    let model = model();
    let registry = NativeRegistry::standard();
    let err = Evaluator::new(&model, &registry)
        .eval(spec)
        .expect_err("expected an error");
    match err.kind {
        PureExceptionKind::ExecutionError(PureRuntimeError::EvaluationError(m)) => m,
        other => panic!("expected an EvaluationError, got {other:?}"),
    }
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

#[test]
fn first_returns_the_head_or_nothing() {
    assert_eq!(
        run(&expr::call(FIRST, vec![strings(&["a", "b"])])),
        Value::String("a".into())
    );
    assert_eq!(
        run(&expr::call(FIRST, vec![expr::int(7)])),
        Value::Integer(7)
    );
    assert_eq!(run(&expr::call(FIRST, vec![ints(&[])])), Value::Unit);
}

/// `{x | $x * $x}` over integers.
fn square_lambda() -> ValueSpec {
    expr::lambda(
        vec![param("x", named(INTEGER_ID), Multiplicity::PureOne)],
        vec![expr::call(
            TIMES,
            vec![expr::collection(vec![expr::var("x"), expr::var("x")])],
        )],
    )
}

#[test]
fn remove_duplicates_honours_the_key_and_the_equality_lambdas() {
    let dedup = |col: ValueSpec, key: ValueSpec, eql: ValueSpec| {
        as_ints(&run(&expr::call(REMOVE_DUPLICATES, vec![col, key, eql])))
    };
    let none = || expr::collection(Vec::new());
    // Default equality, first occurrence wins.
    assert_eq!(dedup(ints(&[1, 2, 1, 3, 2]), none(), none()), [1, 2, 3]);
    // Key lambda: -1 and 1 share the key 1.
    assert_eq!(
        dedup(ints(&[1, -1, 2, -2, 3]), square_lambda(), none()),
        [1, 2, 3]
    );
    // Equality lambda over the elements: {a, b | eq($a, $b)}.
    let eql = expr::lambda(
        vec![
            param("a", named(INTEGER_ID), Multiplicity::PureOne),
            param("b", named(INTEGER_ID), Multiplicity::PureOne),
        ],
        vec![expr::call(EQ, vec![expr::var("a"), expr::var("b")])],
    );
    assert_eq!(dedup(ints(&[3, 3, 4, 3]), none(), eql), [3, 4]);
    assert_eq!(dedup(ints(&[]), none(), none()), Vec::<i64>::new());
}

#[test]
fn sort_orders_by_natural_order_key_or_comparator() {
    let sort = |col: ValueSpec, key: ValueSpec, comp: ValueSpec| {
        as_ints(&run(&expr::call(SORT, vec![col, key, comp])))
    };
    let none = || expr::collection(Vec::new());
    assert_eq!(
        sort(ints(&[342, 5, 2, 171, 1]), none(), none()),
        [1, 2, 5, 171, 342]
    );
    // Key lambda: order by the square, the elements stay themselves.
    assert_eq!(
        sort(ints(&[3, -1, -2]), square_lambda(), none()),
        [-1, -2, 3]
    );
    // Comparator lambda: {x, y | $y - $x} sorts descending.
    let descending = expr::lambda(
        vec![
            param("x", named(INTEGER_ID), Multiplicity::PureOne),
            param("y", named(INTEGER_ID), Multiplicity::PureOne),
        ],
        vec![expr::call(
            MINUS,
            vec![expr::collection(vec![expr::var("y"), expr::var("x")])],
        )],
    );
    assert_eq!(
        sort(ints(&[342, 5, 2, 171, 1]), none(), descending),
        [342, 171, 5, 2, 1]
    );
    assert_eq!(sort(ints(&[]), none(), none()), Vec::<i64>::new());
}

#[test]
fn size_counts_elements_of_any_multiplicity() {
    let size = |v: ValueSpec| run(&expr::call(SIZE, vec![v]));
    assert_eq!(size(strings(&["a", "b", "c"])), Value::Integer(3));
    assert_eq!(size(expr::int(9)), Value::Integer(1));
    assert_eq!(size(ints(&[])), Value::Integer(0));
    // size of a zip result: heap objects count like any element.
    assert_eq!(
        size(expr::call(ZIP, vec![ints(&[1, 2]), strings(&["a", "b"])])),
        Value::Integer(2)
    );
}

#[test]
fn range_follows_the_sign_of_the_step_and_rejects_zero() {
    let range = |start: i64, stop: i64, step: i64| {
        expr::call(
            RANGE,
            vec![expr::int(start), expr::int(stop), expr::int(step)],
        )
    };
    assert_eq!(as_ints(&run(&range(0, 10, 3))), [0, 3, 6, 9]);
    assert_eq!(as_ints(&run(&range(5, 0, -2))), [5, 3, 1]);
    assert_eq!(as_ints(&run(&range(2, 2, 1))), Vec::<i64>::new());
    assert_eq!(as_ints(&run(&range(5, 0, 1))), Vec::<i64>::new());
    assert_eq!(run_err(&range(0, 5, 0)), "range step must not be 0");
}

#[test]
fn map_applies_the_lambda_and_flattens_the_results() {
    // [1, 2, 3]->map(x | $x * $x)
    assert_eq!(
        as_ints(&run(&expr::call(
            MAP_M,
            vec![ints(&[1, 2, 3]), square_lambda()]
        ))),
        [1, 4, 9]
    );
    // [1, 2]->map(x | [$x, $x]): a V[*] lambda contributes every element.
    let twice = expr::lambda(
        vec![param("x", named(INTEGER_ID), Multiplicity::PureOne)],
        vec![expr::collection(vec![expr::var("x"), expr::var("x")])],
    );
    assert_eq!(
        as_ints(&run(&expr::call(MAP_MANY, vec![ints(&[1, 2]), twice]))),
        [1, 1, 2, 2]
    );
    // [1, 2]->map(x | []): an empty result contributes nothing.
    let nothing = expr::lambda(
        vec![param("x", named(INTEGER_ID), Multiplicity::PureOne)],
        vec![expr::collection(Vec::new())],
    );
    assert_eq!(
        run(&expr::call(MAP_MANY, vec![ints(&[1, 2]), nothing])),
        Value::Unit
    );
    // The [0..1] overload over an empty input.
    assert_eq!(
        run(&expr::call(MAP_ZERO_ONE, vec![ints(&[]), square_lambda()])),
        Value::Unit
    );
}
