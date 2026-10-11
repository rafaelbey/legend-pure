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

//! The string natives that need a real evaluator (heap objects, the
//! model, qualified properties), driven through hand-built nodes. The
//! literal-only cases stay inline in `native/string.rs` over `MockCtx`.

use legend_pure_evaluator::builders::{
    ModelBuilder, class, expr, named, property, qualified_property,
};
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_evaluator::value::Value;
use legend_pure_metamodel::bootstrap::{INTEGER_ID, STRING_ID};
use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::model::PureModel;
use legend_pure_metamodel::types::{DateValue, Multiplicity, ValueSpec};

const NEW: &str = "new_Class_1__String_1__Any_MANY__T_1_";
const TO_STRING: &str = "toString_Any_1__String_1_";

/// The elements `toString` is exercised against.
struct Fixture {
    model: PureModel,
    /// `my::Person { name: String[1] }` — no `toString()`.
    person: ElementId,
    /// `my::Named { name: String[1]; toString() { $this.name }: String[1] }`.
    named_class: ElementId,
    /// `my::Child extends my::Named` — inherits `toString()`.
    child: ElementId,
    /// `my::Malformed { name: String[1]; toString() { 1 }: Integer[1] }`.
    malformed: ElementId,
    /// `my::Kind { CITY, COUNTRY }`.
    kind: ElementId,
}

fn fixture() -> Fixture {
    let mut b = ModelBuilder::new();
    let name = || property("name", named(STRING_ID), Multiplicity::PureOne);
    let person = b.class("my::Person", class(Vec::new(), vec![name()]));
    let mut named_def = class(Vec::new(), vec![name()]);
    named_def.qualified_properties.push(qualified_property(
        "toString",
        Vec::new(),
        named(STRING_ID),
        Multiplicity::PureOne,
        vec![expr::prop(expr::var("this"), "name")],
    ));
    let named_class = b.class("my::Named", named_def);
    let child = b.class("my::Child", class(vec![named(named_class)], Vec::new()));
    let mut malformed_def = class(Vec::new(), vec![name()]);
    malformed_def.qualified_properties.push(qualified_property(
        "toString",
        Vec::new(),
        named(INTEGER_ID),
        Multiplicity::PureOne,
        vec![expr::int(1)],
    ));
    let malformed = b.class("my::Malformed", malformed_def);
    let kind = b.enumeration("my::Kind", &["CITY", "COUNTRY"]);
    Fixture {
        model: b.finish(),
        person,
        named_class,
        child,
        malformed,
        kind,
    }
}

/// The compiler-emitted shape of `^Class(name=$name)`.
fn new_named(class: ElementId, name: &str) -> ValueSpec {
    expr::call(
        NEW,
        vec![
            expr::type_ref(named(class)),
            expr::string(""),
            expr::collection(Vec::new()),
            expr::collection(Vec::new()),
            expr::string("name"),
            expr::string(name),
            expr::boolean(false),
        ],
    )
}

fn to_string(arg: ValueSpec) -> ValueSpec {
    expr::call(TO_STRING, vec![arg])
}

/// Evaluate `toString(arg)` and unwrap the string.
fn render(model: &PureModel, arg: ValueSpec) -> String {
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(model, &registry);
    match &eval.eval(&to_string(arg)).unwrap() {
        Value::String(s) => s.to_string(),
        other => panic!("toString returned a non-string: {other:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn date_time(
    (year, month, day): (i16, i8, i8),
    (hour, minute, second): (i8, i8, i8),
    subsecond_nanos: i32,
    subsecond_digits: u8,
    tz_offset_minutes: Option<i16>,
) -> ValueSpec {
    expr::date(DateValue::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        subsecond_nanos,
        subsecond_digits,
        has_minutes: true,
        has_seconds: true,
        tz_offset_minutes,
    })
}

#[test]
fn to_string_renders_primitives_in_the_java_shape() {
    let model = ModelBuilder::new().finish();
    assert_eq!(render(&model, expr::int(1)), "1");
    assert_eq!(render(&model, expr::int(-1)), "-1");
    assert_eq!(render(&model, expr::boolean(true)), "true");
    assert_eq!(render(&model, expr::string("")), "");
    assert_eq!(render(&model, expr::string("the quick")), "the quick");
    assert_eq!(
        render(&model, expr::decimal(rust_decimal::Decimal::new(314, 2))),
        "3.14"
    );
}

/// `testFloatToString*`: integer-valued floats keep their `.0`, no
/// E-notation either way.
#[test]
fn to_string_renders_floats_like_java_double_to_string() {
    let model = ModelBuilder::new().finish();
    assert_eq!(render(&model, expr::float(2.0)), "2.0");
    assert_eq!(render(&model, expr::float(3.25)), "3.25");
    assert_eq!(render(&model, expr::float(0.01)), "0.01");
    assert_eq!(render(&model, expr::float(134_210_000.0)), "134210000.0");
    assert_eq!(
        render(&model, expr::float(0.000_000_013_421)),
        "0.000000013421"
    );
}

/// A collection renders bracketed, comma-separated; a nested collection
/// literal flattens on evaluation, as Pure collections do.
#[test]
fn to_string_renders_a_collection_bracketed() {
    let model = ModelBuilder::new().finish();
    let coll = expr::collection(vec![
        expr::int(1),
        expr::string("a"),
        expr::float(2.0),
        expr::collection(vec![expr::boolean(false)]),
    ]);
    assert_eq!(render(&model, coll), "[1, a, 2.0, false]");
}

/// `testDateToString`, `testDateTimeToString`, `testSimpleDateToString`:
/// the Pure date text without the `%`, date-times in UTC with `+0000`.
#[test]
fn to_string_renders_dates_as_pure_date_text() {
    let model = ModelBuilder::new().finish();
    let strict = |year, month, day| expr::date(DateValue::StrictDate { year, month, day });
    assert_eq!(render(&model, strict(2014, Some(1), Some(1))), "2014-01-01");
    assert_eq!(render(&model, strict(2014, Some(1), None)), "2014-01");
    assert_eq!(render(&model, strict(2014, None, None)), "2014");
    assert_eq!(
        render(&model, date_time((2014, 1, 1), (0, 0, 0), 0, 3, None)),
        "2014-01-01T00:00:00.000+0000"
    );
    assert_eq!(
        render(
            &model,
            date_time((2014, 1, 1), (20, 54, 27), 352_000_000, 3, Some(-300))
        ),
        "2014-01-02T01:54:27.352+0000"
    );
}

/// `testEnumerationToString`, `testClassToString`: an enum value prints
/// its member name, an element its simple name.
#[test]
fn to_string_renders_enum_values_and_elements_by_name() {
    let f = fixture();
    assert_eq!(render(&f.model, expr::enum_value(f.kind, "CITY")), "CITY");
    assert_eq!(render(&f.model, expr::element_ref(f.kind)), "Kind");
    assert_eq!(render(&f.model, expr::element_ref(f.person)), "Person");
}

/// `testPersonToString`: an instance whose class declares no
/// `toString()` falls back to Java's anonymous-instance name.
#[test]
fn to_string_of_an_instance_without_to_string_is_anonymous() {
    let f = fixture();
    let s = render(&f.model, new_named(f.person, "Pierre"));
    assert!(s.starts_with("Anonymous_"), "{s}");
}

/// `testComplexClassToString`: a class's own `toString()` qualified
/// property is what `toString` returns.
#[test]
fn to_string_dispatches_to_the_class_to_string_qualified_property() {
    let f = fixture();
    assert_eq!(render(&f.model, new_named(f.named_class, "Ann")), "Ann");
}

#[test]
fn to_string_finds_the_qualified_property_on_a_generalization() {
    let f = fixture();
    assert_eq!(render(&f.model, new_named(f.child, "Bob")), "Bob");
}

/// A `toString()` not declared `String[1]` is not honoured.
#[test]
fn to_string_skips_a_qualified_property_not_declared_string_one() {
    let f = fixture();
    let s = render(&f.model, new_named(f.malformed, "Eve"));
    assert!(s.starts_with("Anonymous_"), "{s}");
}

#[test]
fn to_string_renders_instances_inside_a_collection() {
    let f = fixture();
    let coll = expr::collection(vec![
        new_named(f.named_class, "Ann"),
        expr::int(7),
        new_named(f.child, "Bob"),
    ]);
    assert_eq!(render(&f.model, coll), "[Ann, 7, Bob]");
}
