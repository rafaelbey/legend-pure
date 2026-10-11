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

//! The lowering over hand-built M3 nodes: the shapes the Java compiler
//! produces (see `lower::node_builders::m3`) lowered into the evaluator's
//! tree and model, then run. No JVM.

use legend_pure_evaluator::builders::ModelBuilder;
use legend_pure_evaluator::eval::Evaluator;
use legend_pure_evaluator::lower::node_builders::{TestNode, m3};
use legend_pure_evaluator::lower::{LowerError, Lowering};
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_evaluator::value::{FunctionValue, LambdaClosure, Value};
use legend_pure_metamodel::bootstrap::{BOOLEAN_ID, INTEGER_ID, STRING_ID};
use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::model::{Element, PureModel};
use legend_pure_metamodel::types::{ExprKind, Multiplicity};

const PLUS: &str = "meta::pure::functions::math::plus_Integer_MANY__Integer_1_";
const IF: &str = "meta::pure::functions::lang::if_Boolean_1__Function_1__Function_1__T_m_";
const LET: &str = "meta::pure::functions::lang::letFunction_String_1__T_m__T_m_";
const EQ: &str = "meta::pure::functions::boolean::eq_Any_1__Any_1__Boolean_1_";

fn integer() -> Value {
    m3::generic(Some(Value::Element(INTEGER_ID)), Vec::new())
}

fn one() -> Value {
    m3::mult(1, Some(1))
}

fn many() -> Value {
    m3::mult(0, None)
}

fn literal(v: Value) -> Value {
    m3::iv(vec![v], Some(integer()), Some(one()))
}

fn int(i: i64) -> Value {
    literal(Value::Integer(i))
}

/// `plus([a, b, …])`: one collection argument, as the Java compiler emits.
fn plus(items: Vec<Value>) -> Value {
    let plus = m3::native_function(
        PLUS,
        "plus",
        m3::function_type(
            vec![m3::param("values", integer(), many())],
            integer(),
            one(),
        ),
    );
    m3::sfe(
        plus,
        "plus",
        vec![m3::iv(items, Some(integer()), Some(many()))],
    )
}

fn root(body: Vec<Value>) -> Value {
    m3::lambda(Vec::new(), body, integer(), one()).build()
}

/// Lower a root lambda over `model`, apply what it added, run it.
fn lower_and_run(model: &mut PureModel, root: &Value) -> Result<Value, LowerError> {
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(model, chunk, model.chunk_len(chunk));
    let closure = lowering.lower_root_lambda(root)?;
    lowering.into_extension().apply(model);
    Ok(run(model, closure))
}

fn run(model: &PureModel, closure: LambdaClosure) -> Value {
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(model, &registry);
    eval.context_mut().push_scope();
    eval.apply_callable(
        &Value::Function(Box::new(FunctionValue::Lambda(closure))),
        &[],
    )
    .unwrap()
}

#[test]
fn a_native_call_lowers_to_an_element_named_by_its_mangled_signature() {
    let mut model = PureModel::bootstrapped();
    let before = model.chunks.len();
    let lambda = root(vec![plus(vec![int(1), int(2)])]);

    let chunk = u16::try_from(before).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    assert!(closure.parameters.is_empty());
    assert_eq!(closure.body.len(), 1);
    let ExprKind::FunctionCall(call) = &*closure.body[0].kind else {
        panic!("expected a call, got {:?}", closure.body[0].kind);
    };
    assert_eq!(call.function_name, "plus");
    let function = call.function.expect("resolved to the lowered element");
    assert_eq!(call.arguments.len(), 1);
    assert!(
        matches!(&*call.arguments[0].kind, ExprKind::Collection { elements } if elements.len() == 2)
    );

    let extension = lowering.into_extension();
    assert_eq!(extension.len(), 1);
    extension.apply(&mut model);
    assert_eq!(model.chunks.len(), before + 1);
    assert_eq!(
        model.get_node(function).name,
        "plus_Integer_MANY__Integer_1_"
    );
    assert!(
        matches!(model.get_element(function), Element::Function(f) if f.is_native && f.parameters.len() == 1)
    );
    assert_eq!(model.resolve_fqn_str(PLUS), Some(function));

    assert_eq!(run(&model, closure), Value::Integer(3));
}

#[test]
fn the_bridge_chunk_is_whatever_comes_after_the_model_s_chunks() {
    // A builder model already holds chunk 1, so the lowering lands in 2;
    // a second lowering appends to the same chunk.
    let mut model = ModelBuilder::new().finish();
    assert_eq!(model.chunks.len(), 2);
    assert_eq!(
        lower_and_run(&mut model, &root(vec![plus(vec![int(2), int(3)])])),
        Ok(Value::Integer(5))
    );
    assert_eq!(model.chunks.len(), 3);
    assert_eq!(model.chunk_len(2), 1);

    let eq = m3::native_function(
        EQ,
        "eq",
        m3::function_type(
            vec![
                m3::param("left", integer(), one()),
                m3::param("right", integer(), one()),
            ],
            m3::generic(Some(Value::Element(BOOLEAN_ID)), Vec::new()),
            one(),
        ),
    );
    let body = vec![m3::sfe(eq, "eq", vec![plus(vec![int(1), int(1)]), int(2)])];
    let chunk = 2;
    let mut lowering = Lowering::new(&model, chunk, model.chunk_len(chunk));
    let closure = lowering.lower_root_lambda(&root(body)).unwrap();
    let extension = lowering.into_extension();
    // `plus` already exists in the model: only `eq` is new.
    assert_eq!(extension.len(), 1);
    extension.apply(&mut model);
    assert_eq!(model.chunk_len(2), 2);
    assert_eq!(
        model.resolve_fqn_str(EQ),
        Some(ElementId::InstanceId {
            chunk_id: 2,
            local_idx: 1
        })
    );
    assert_eq!(run(&model, closure), Value::Boolean(true));
}

#[test]
fn nested_lambdas_lower_bare_so_if_takes_a_branch() {
    let mut model = PureModel::bootstrapped();
    let string = m3::generic(Some(Value::Element(STRING_ID)), Vec::new());
    let branch = |s: &str| {
        m3::iv(
            vec![
                m3::lambda(
                    Vec::new(),
                    vec![m3::iv(
                        vec![Value::String(s.into())],
                        Some(string.clone()),
                        Some(one()),
                    )],
                    string.clone(),
                    one(),
                )
                .build(),
            ],
            None,
            None,
        )
    };
    let if_fn = m3::native_function(
        IF,
        "if",
        m3::function_type(Vec::new(), m3::type_parameter("T"), many()),
    );
    let call = m3::sfe(
        if_fn,
        "if",
        vec![
            m3::iv(vec![Value::Boolean(true)], None, None),
            branch("yes"),
            branch("no"),
        ],
    );
    let lambda = root(vec![call]);
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    let ExprKind::FunctionCall(call) = &*closure.body[0].kind else {
        panic!("expected a call");
    };
    assert!(
        matches!(&*call.arguments[1].kind, ExprKind::Lambda { parameters, body } if parameters.is_empty() && body.len() == 1)
    );
    lowering.into_extension().apply(&mut model);
    assert_eq!(run(&model, closure), Value::String("yes".into()));
}

#[test]
fn let_keeps_its_simple_name_for_the_free_variable_walk() {
    let mut model = PureModel::bootstrapped();
    let let_fn = m3::native_function(
        LET,
        "letFunction",
        m3::function_type(Vec::new(), m3::type_parameter("T"), many()),
    );
    let body = vec![
        m3::sfe(
            let_fn,
            "letFunction",
            vec![m3::iv(vec![Value::String("x".into())], None, None), int(7)],
        ),
        m3::var("x"),
    ];
    let lambda = root(body);
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    assert!(
        matches!(&*closure.body[0].kind, ExprKind::FunctionCall(c) if c.function_name == "letFunction")
    );
    assert!(matches!(&*closure.body[1].kind, ExprKind::Variable { name } if name == "x"));
    lowering.into_extension().apply(&mut model);
    assert_eq!(run(&model, closure), Value::Integer(7));
}

#[test]
fn every_literal_kind_lowers() {
    let mut model = PureModel::bootstrapped();
    let date = legend_pure_evaluator::date::PureDate::strict_date(2014, 2, 1).unwrap();
    let values = vec![
        Value::Float(1.5),
        Value::Decimal(rust_decimal::Decimal::new(25, 1)),
        Value::String("s".into()),
        Value::Boolean(false),
        Value::Date(date),
    ];
    let mut items: Vec<Value> = values
        .iter()
        .cloned()
        .map(|v| m3::iv(vec![v], None, None))
        .collect();
    items.push(m3::iv(vec![Value::Latest], None, None));
    let lambda = root(vec![m3::iv(items, None, Some(many()))]);
    let result = lower_and_run(&mut model, &lambda).unwrap();
    let Value::Collection(items) = &result else {
        panic!("expected a collection, got {result:?}");
    };
    // `%latest` is compared by shape: `Value::Latest` is not equal to
    // itself under the archived `PartialEq` (a Step 1 finding).
    assert_eq!(items.len(), 6);
    assert_eq!(items.iter().take(5).cloned().collect::<Vec<_>>(), values);
    assert!(matches!(items.last(), Some(Value::Latest)));
}

#[test]
fn an_unknown_node_kind_is_unsupported_and_adds_nothing() {
    let model = PureModel::bootstrapped();
    let odd = TestNode::new("meta::x::Y").build();
    let lambda = root(vec![plus(vec![int(1), odd])]);
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let err = lowering.lower_root_lambda(&lambda).unwrap_err();
    assert_eq!(
        err,
        LowerError::Unsupported("lowering meta::x::Y as a value".into())
    );
    // Nothing reached the model: the extension is dropped, not applied.
    drop(lowering);
    assert_eq!(model.chunk_len(chunk), 0);
}

#[test]
fn a_lambda_with_open_variables_but_no_context_is_unsupported() {
    let model = PureModel::bootstrapped();
    let lambda = m3::lambda(Vec::new(), vec![m3::var("k")], integer(), one())
        .prop("openVariables", vec![Value::String("k".into())])
        .build();
    let mut lowering = Lowering::new(&model, 1, 0);
    let err = lowering.lower_root_lambda(&lambda).unwrap_err();
    assert!(
        matches!(err, LowerError::Unsupported(m) if m.starts_with("lowering captured variables"))
    );
}

#[test]
fn specs_carry_the_node_s_span() {
    let model = PureModel::bootstrapped();
    let lit = TestNode::new(legend_pure_evaluator::m3_paths::INSTANCE_VALUE)
        .prop("values", vec![Value::Integer(4)])
        .at("f.pure", 3, 9)
        .build();
    let lambda = root(vec![lit]);
    let mut lowering = Lowering::new(&model, 1, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    let span = &closure.body[0].source_info;
    assert_eq!(
        (span.source.as_str(), span.start_line, span.start_column),
        ("f.pure", 3, 9)
    );
    // A node without a span gets the placeholder, never a panic.
    assert_eq!(
        lower_and_run(&mut PureModel::bootstrapped(), &root(vec![int(1)])),
        Ok(Value::Integer(1))
    );
    let _ = Multiplicity::PureOne;
}

fn string_type() -> Value {
    m3::generic(Some(Value::Element(STRING_ID)), Vec::new())
}

fn str_lit(s: &str) -> Value {
    m3::iv(
        vec![Value::String(s.into())],
        Some(string_type()),
        Some(one()),
    )
}

#[test]
fn a_pure_bodied_function_lowers_with_its_body_and_runs() {
    // `function twice(x:Integer[1]):Integer[1] { plus([$x, $x]) }`
    let mut model = PureModel::bootstrapped();
    let twice = m3::concrete_function(
        "my::pkg::twice_Integer_1__Integer_1_",
        "twice",
        m3::function_type(vec![m3::param("x", integer(), one())], integer(), one()),
        vec![plus(vec![m3::var("x"), m3::var("x")])],
    );
    let lambda = root(vec![m3::sfe(twice.clone(), "twice", vec![int(21)])]);
    assert_eq!(lower_and_run(&mut model, &lambda), Ok(Value::Integer(42)));
    let id = model
        .resolve_fqn_str("my::pkg::twice_Integer_1__Integer_1_")
        .expect("lowered");
    assert!(
        matches!(model.get_element(id), Element::Function(f) if !f.is_native && f.body.len() == 1 && f.parameters[0].name == "x")
    );
}

#[test]
fn mutually_recursive_functions_lower_once_each() {
    // `isEven(n) = if(n == 0, |true, |isOdd(n - 1))`, `isOdd(n) = if(n == 0, |false, |isEven(n - 1))`.
    let mut model = PureModel::bootstrapped();
    let boolean = || m3::generic(Some(Value::Element(BOOLEAN_ID)), Vec::new());
    let eq = || {
        m3::native_function(
            EQ,
            "eq",
            m3::function_type(
                vec![
                    m3::param("l", integer(), one()),
                    m3::param("r", integer(), one()),
                ],
                boolean(),
                one(),
            ),
        )
    };
    let if_fn = || {
        m3::native_function(
            IF,
            "if",
            m3::function_type(Vec::new(), m3::type_parameter("T"), many()),
        )
    };
    let minus = || {
        m3::native_function(
            "meta::pure::functions::math::minus_Integer_MANY__Integer_1_",
            "minus",
            m3::function_type(
                vec![m3::param("values", integer(), many())],
                integer(),
                one(),
            ),
        )
    };
    let branch = |body: Value| {
        m3::iv(
            vec![m3::lambda(Vec::new(), vec![body], boolean(), one()).build()],
            None,
            None,
        )
    };
    let bool_lit = |b: bool| m3::iv(vec![Value::Boolean(b)], Some(boolean()), Some(one()));
    let n_minus_1 = || {
        m3::sfe(
            minus(),
            "minus",
            vec![m3::iv(
                vec![m3::var("n"), int(1)],
                Some(integer()),
                Some(many()),
            )],
        )
    };
    // Build the two function nodes so each body references the other node
    // (the Java graph is a cycle; `TestNode` values are shared handles).
    let signature = || m3::function_type(vec![m3::param("n", integer(), one())], boolean(), one());
    let is_odd_shell = TestNode::new(legend_pure_evaluator::m3_paths::CONCRETE_FUNCTION_DEFINITION)
        .path("my::pkg::isOdd_Integer_1__Boolean_1_")
        .one("functionName", Value::String("isOdd".into()))
        .one(
            "classifierGenericType",
            m3::classifier_generic_type(signature()),
        )
        .build();
    let is_even = m3::concrete_function(
        "my::pkg::isEven_Integer_1__Boolean_1_",
        "isEven",
        signature(),
        vec![m3::sfe(
            if_fn(),
            "if",
            vec![
                m3::sfe(eq(), "eq", vec![m3::var("n"), int(0)]),
                branch(bool_lit(true)),
                branch(m3::sfe(is_odd_shell.clone(), "isOdd", vec![n_minus_1()])),
            ],
        )],
    );
    // Fill isOdd's body now that isEven exists: a TestNode is a heap
    // object, so set the property through the heap view.
    let Value::Object(h) = &is_odd_shell else {
        unreachable!()
    };
    h.borrow_mut()
        .mutate_set(
            "expressionSequence",
            &[m3::sfe(
                if_fn(),
                "if",
                vec![
                    m3::sfe(eq(), "eq", vec![m3::var("n"), int(0)]),
                    branch(bool_lit(false)),
                    branch(m3::sfe(is_even.clone(), "isEven", vec![n_minus_1()])),
                ],
            )],
        )
        .unwrap();
    let lambda = root(vec![m3::sfe(is_even, "isEven", vec![int(6)])]);
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    let extension = lowering.into_extension();
    // isEven, isOdd, if, eq, minus: five elements, each once.
    assert_eq!(extension.len(), 5);
    extension.apply(&mut model);
    assert_eq!(run(&model, closure), Value::Boolean(true));
}

#[test]
fn a_function_referenced_as_a_value_becomes_an_element_reference() {
    let model = PureModel::bootstrapped();
    let plus_fn = m3::native_function(
        PLUS,
        "plus",
        m3::function_type(
            vec![m3::param("values", integer(), many())],
            integer(),
            one(),
        ),
    );
    let lambda = root(vec![m3::iv(vec![plus_fn], None, Some(one()))]);
    let mut lowering = Lowering::new(&model, 1, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    assert!(matches!(
        &*closure.body[0].kind,
        ExprKind::PackageableElementRef { .. }
    ));
    let extension = lowering.into_extension();
    assert_eq!(extension.len(), 1);
    assert!(extension.lookup(PLUS).is_some());
}

#[test]
fn captured_variables_become_the_closure_s_captures() {
    let mut model = PureModel::bootstrapped();
    let lambda = m3::lambda(
        Vec::new(),
        vec![plus(vec![m3::var("k"), int(1)])],
        integer(),
        one(),
    )
    .captured("k", vec![Value::Integer(41)])
    .build();
    let chunk = u16::try_from(model.chunks.len()).unwrap();
    let mut lowering = Lowering::new(&model, chunk, 0);
    let closure = lowering.lower_root_lambda(&lambda).unwrap();
    assert_eq!(closure.captures.get("k"), Some(&Value::Integer(41)));
    lowering.into_extension().apply(&mut model);
    assert_eq!(run(&model, closure), Value::Integer(42));
}

#[test]
fn a_captured_lambda_is_lowered_into_a_closure() {
    // `let g = {x | $x + 1}; |$g->eval(5)` — the captured lambda arrives
    // as a node and must become a function value; `eval` is not fished,
    // so apply it directly.
    let mut model = PureModel::bootstrapped();
    let g = m3::lambda(
        vec![m3::param("x", integer(), one())],
        vec![plus(vec![m3::var("x"), int(1)])],
        integer(),
        one(),
    )
    .build();
    let lambda = m3::lambda(Vec::new(), vec![str_lit("unused")], string_type(), one())
        .captured("g", vec![g])
        .build();
    let (f, extension) = {
        let mut lowering = Lowering::new(&model, 1, 0);
        let closure = lowering.lower_root_lambda(&lambda).unwrap();
        let Some(Value::Function(f)) = closure.captures.get("g") else {
            panic!(
                "expected a function value, got {:?}",
                closure.captures.get("g")
            );
        };
        let FunctionValue::Lambda(g) = &**f else {
            panic!("expected a lambda closure");
        };
        assert_eq!(g.parameters.len(), 1);
        assert_eq!(g.parameters[0].name, "x");
        (f.clone(), lowering.into_extension())
    };
    extension.apply(&mut model);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    eval.context_mut().push_scope();
    let applied = eval
        .apply_callable(&Value::Function(f), &[Value::Integer(5)])
        .unwrap();
    assert_eq!(applied, Value::Integer(6));
}
