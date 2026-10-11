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

//! The meta natives driven through a real `Evaluator` over hand-built
//! classes, enumerations and lambdas. The cases mirror the platform PCT
//! tests (`essential/lang/flow/match.pure`, …).
//!
//! `NativeRegistry::standard()` does not register the meta natives yet,
//! so every test adds them with `native::meta::register`.

use legend_pure_evaluator::builders::{ModelBuilder, class, expr, named, param, property};
use legend_pure_evaluator::error::{PureException, PureExceptionKind, PureRuntimeError};
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::native::{NativeRegistry, meta};
use legend_pure_evaluator::value::Value;
use legend_pure_metamodel::bootstrap::{ANY_ID, DATE_ID, INTEGER_ID, STRING_ID};
use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::model::PureModel;
use legend_pure_metamodel::types::{DateValue, Multiplicity, TypeExpr, ValueSpec};

const NEW: &str = "new_Class_1__String_1__Any_MANY__T_1_";
const MATCH: &str = "match_Any_MANY__Function_$1_MANY$__T_m_";
const MATCH_WITH: &str = "match_Any_MANY__Function_$1_MANY$__P_o__T_m_";

fn registry() -> NativeRegistry {
    let mut registry = NativeRegistry::standard();
    meta::register(&mut registry);
    registry
}

fn evaluation_error(err: &PureException) -> Option<&str> {
    match &err.kind {
        PureExceptionKind::ExecutionError(PureRuntimeError::EvaluationError(m)) => Some(m),
        _ => None,
    }
}

/// The classes of `match.pure`: `Geo`, `MA_Address extends Geo`,
/// `MA_Location extends Geo`, `MA_Person`.
struct Geography {
    model: PureModel,
    address: ElementId,
    location: ElementId,
    person: ElementId,
}

fn geography() -> Geography {
    let mut b = ModelBuilder::new();
    let geo = b.class(
        "meta::pure::functions::lang::tests::match::Geo",
        class(Vec::new(), Vec::new()),
    );
    let address = b.class(
        "meta::pure::functions::lang::tests::match::MA_Address",
        class(
            vec![named(geo)],
            vec![property("name", named(STRING_ID), Multiplicity::PureOne)],
        ),
    );
    let location = b.class(
        "meta::pure::functions::lang::tests::match::MA_Location",
        class(
            vec![named(geo)],
            vec![property("place", named(STRING_ID), Multiplicity::PureOne)],
        ),
    );
    let person = b.class(
        "meta::pure::functions::lang::tests::match::MA_Person",
        class(
            Vec::new(),
            vec![property(
                "firstName",
                named(STRING_ID),
                Multiplicity::PureOne,
            )],
        ),
    );
    Geography {
        model: b.finish(),
        address,
        location,
        person,
    }
}

/// `^Class(key = value)` in the compiler-emitted positional shape.
fn new_instance(class: ElementId, key: &str, value: &str) -> ValueSpec {
    expr::call(
        NEW,
        vec![
            expr::type_ref(named(class)),
            expr::string("i"),
            expr::collection(Vec::new()),
            expr::collection(Vec::new()),
            expr::string(key),
            expr::string(value),
            expr::boolean(false),
        ],
    )
}

/// `{name:Type[mult] | body}`.
fn branch(name: &str, ty: TypeExpr, mult: Multiplicity, body: ValueSpec) -> ValueSpec {
    expr::lambda(vec![param(name, ty, mult)], vec![body])
}

/// `{name:Type[mult], extra:String[extra_mult] | body}`.
fn branch_with(ty: TypeExpr, extra_mult: Multiplicity, body: ValueSpec) -> ValueSpec {
    expr::lambda(
        vec![
            param("a", ty, Multiplicity::PureOne),
            param("b", named(STRING_ID), extra_mult),
        ],
        vec![body],
    )
}

fn match_call(subject: ValueSpec, branches: Vec<ValueSpec>) -> ValueSpec {
    expr::call(MATCH, vec![subject, expr::collection(branches)])
}

fn strings(values: &[&str]) -> Value {
    Value::Collection(Box::new(
        values.iter().map(|s| Value::String((*s).into())).collect(),
    ))
}

fn ints(values: &[i64]) -> Value {
    Value::Collection(Box::new(
        values.iter().map(|n| Value::Integer(*n)).collect(),
    ))
}

#[test]
fn match_dispatches_an_instance_on_its_class_and_falls_back_to_any() {
    let g = geography();
    let registry = registry();
    let mut eval = Evaluator::new(&g.model, &registry);
    let branches = |any_mult: Multiplicity, any_label: &str| {
        vec![
            branch(
                "a",
                named(g.address),
                Multiplicity::PureOne,
                expr::string("address"),
            ),
            branch(
                "l",
                named(g.location),
                Multiplicity::PureOne,
                expr::string("location"),
            ),
            branch("a", named(ANY_ID), any_mult, expr::string(any_label)),
        ]
    };
    let one = |subject: ValueSpec| match_call(subject, branches(Multiplicity::PureOne, "Any1"));
    let many = |subject: ValueSpec| match_call(subject, branches(Multiplicity::ZeroOrMany, "Any*"));

    // testMatch
    assert_eq!(
        eval.eval(&one(new_instance(g.address, "name", "Hoboken, NJ")))
            .unwrap(),
        Value::String("address".into())
    );
    assert_eq!(
        eval.eval(&one(new_instance(g.location, "place", "Hoboken, NJ")))
            .unwrap(),
        Value::String("location".into())
    );
    assert_eq!(
        eval.eval(&one(new_instance(g.person, "firstName", "Pierre")))
            .unwrap(),
        Value::String("Any1".into())
    );
    assert_eq!(
        eval.eval(&many(new_instance(g.person, "firstName", "Pierre")))
            .unwrap(),
        Value::String("Any*".into())
    );
    let two_people = expr::collection(vec![
        new_instance(g.person, "firstName", "Pierre"),
        new_instance(g.person, "firstName", "Fabrice"),
    ]);
    assert_eq!(
        eval.eval(&many(two_people)).unwrap(),
        Value::String("Any*".into())
    );
}

#[test]
fn match_gates_each_branch_on_the_subject_count() {
    let model = ModelBuilder::new().finish();
    let registry = registry();
    let mut eval = Evaluator::new(&model, &registry);
    let up_to_four = Multiplicity::Range {
        lower: 1,
        upper: Some(4),
    };

    // testMatchManyWithMany: three strings skip `Integer[1..4]` and land
    // on `String[*]`, which receives the whole collection.
    let three = expr::collection(vec![
        expr::string("w"),
        expr::string("w"),
        expr::string("w"),
    ]);
    let call = match_call(
        three,
        vec![
            branch(
                "i",
                named(INTEGER_ID),
                up_to_four.clone(),
                expr::string("z"),
            ),
            branch(
                "s",
                named(STRING_ID),
                Multiplicity::ZeroOrMany,
                expr::var("s"),
            ),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), strings(&["w", "w", "w"]));

    // `[x:Integer[*] | $x]` takes a collection `Integer[1]` refuses.
    let call = match_call(
        expr::collection(vec![expr::int(1), expr::int(2), expr::int(3)]),
        vec![
            branch(
                "i",
                named(INTEGER_ID),
                Multiplicity::PureOne,
                expr::string("one"),
            ),
            branch(
                "x",
                named(INTEGER_ID),
                Multiplicity::ZeroOrMany,
                expr::var("x"),
            ),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), ints(&[1, 2, 3]));

    // testMatchOneWithZeroOne
    let call = match_call(
        expr::collection(vec![expr::int(1)]),
        vec![
            branch(
                "i",
                named(INTEGER_ID),
                Multiplicity::ZeroOrOne,
                expr::collection(vec![expr::int(1)]),
            ),
            branch(
                "s",
                named(STRING_ID),
                Multiplicity::PureOne,
                expr::string("address:"),
            ),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), Value::Integer(1));

    // testMatchZeroWithZero: an empty subject matches the first branch
    // whose lower bound is zero, whatever its type.
    let call = match_call(
        expr::collection(Vec::new()),
        vec![
            branch("i", named(INTEGER_ID), up_to_four, expr::string("ints")),
            branch(
                "s",
                named(STRING_ID),
                Multiplicity::Range {
                    lower: 0,
                    upper: Some(0),
                },
                expr::string("address"),
            ),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), Value::String("address".into()));
}

#[test]
fn match_picks_the_branch_whose_type_fits_a_primitive_subject() {
    let model = ModelBuilder::new().finish();
    let registry = registry();
    let mut eval = Evaluator::new(&model, &registry);
    // testMatchWithFunctions: `[{a:Integer[1]|1}, {a:String[1]|[1,2]}, {a:Date[1]|[4,5,6]}]`
    let branches = || {
        vec![
            branch("a", named(INTEGER_ID), Multiplicity::PureOne, expr::int(1)),
            branch(
                "a",
                named(STRING_ID),
                Multiplicity::PureOne,
                expr::collection(vec![expr::int(1), expr::int(2)]),
            ),
            branch(
                "a",
                named(DATE_ID),
                Multiplicity::PureOne,
                expr::collection(vec![expr::int(4), expr::int(5), expr::int(6)]),
            ),
        ]
    };
    assert_eq!(
        eval.eval(&match_call(expr::int(1), branches())).unwrap(),
        Value::Integer(1)
    );
    assert_eq!(
        eval.eval(&match_call(expr::string("1"), branches()))
            .unwrap(),
        ints(&[1, 2])
    );
    let year_2005 = expr::date(DateValue::StrictDate {
        year: 2005,
        month: None,
        day: None,
    });
    assert_eq!(
        eval.eval(&match_call(year_2005, branches())).unwrap(),
        ints(&[4, 5, 6])
    );
}

#[test]
fn match_with_an_extra_parameter_passes_it_to_the_chosen_branch() {
    let model = ModelBuilder::new().finish();
    let registry = registry();
    let mut eval = Evaluator::new(&model, &registry);
    // testMatchWithExtraParam
    let call = expr::call(
        MATCH_WITH,
        vec![
            expr::collection(vec![expr::int(1)]),
            expr::collection(vec![
                branch_with(named(INTEGER_ID), Multiplicity::PureOne, expr::var("b")),
                branch_with(named(STRING_ID), Multiplicity::PureOne, expr::string("bad")),
            ]),
            expr::string("other"),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), Value::String("other".into()));

    let call = expr::call(
        MATCH_WITH,
        vec![
            expr::collection(vec![expr::int(1)]),
            expr::collection(vec![
                branch_with(named(INTEGER_ID), Multiplicity::ZeroOrMany, expr::var("b")),
                branch_with(
                    named(STRING_ID),
                    Multiplicity::ZeroOrMany,
                    expr::string("bad"),
                ),
            ]),
            expr::collection(vec![expr::string("other"), expr::string("yo")]),
        ],
    );
    assert_eq!(eval.eval(&call).unwrap(), strings(&["other", "yo"]));
}

#[test]
fn match_without_a_fitting_branch_is_an_evaluation_error() {
    let model = ModelBuilder::new().finish();
    let registry = registry();
    let mut eval = Evaluator::new(&model, &registry);
    let call = match_call(
        expr::string("Digby"),
        vec![branch(
            "i",
            named(INTEGER_ID),
            Multiplicity::PureOne,
            expr::int(1),
        )],
    );
    let err = eval.eval(&call).unwrap_err();
    let msg = evaluation_error(&err).expect("an evaluation error");
    assert!(
        msg.starts_with("match: no branch matched for value"),
        "{msg}"
    );
}
