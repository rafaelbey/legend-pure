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
