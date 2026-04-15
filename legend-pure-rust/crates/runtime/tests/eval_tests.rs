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

//! End-to-end evaluation tests: compile Pure → evaluate → assert Value.
//!
//! These tests exercise the full pipeline: parse platform + user Pure →
//! compile → evaluate. The platform Pure files provide the standard library
//! (plus, equal, if, map, etc.), so the compiler resolves operator calls
//! to their platform Function `ElementId`s, and FQN mangling produces the
//! correct native registry keys.
//!
//! # Test progression
//!
//! 1. Literal evaluation (int, float, string, bool)
//! 2. Arithmetic and comparison operators
//! 3. Variables (`let`)
//! 4. Control flow (`if`)
//! 5. Collections and lambdas (`map`, `filter`, `fold`)
//! 6. User-defined functions
//! 7. Assertions (prerequisite for PCT)

use std::sync::OnceLock;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// Cached pre-parsed platform ASTs + auto-imports.
///
/// Compiling platform sources is expensive (~200 Pure files), so we parse
/// them once and cache the ASTs. Each test then clones the ASTs and combines
/// them with user code for a fresh compilation.
fn platform_model() -> &'static PlatformFixture {
    static FIXTURE: OnceLock<PlatformFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let raw = legend_pure_core_platform::sources::platform_sources();
        let mut parsed_files = Vec::new();
        for s in raw {
            match legend_pure_parser_parser::parse(s.content, s.path) {
                Ok(sf) => parsed_files.push(sf),
                Err(partial) => parsed_files.push(partial.source_file),
            }
        }
        let auto_imports = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect();
        PlatformFixture {
            parsed_files,
            auto_imports,
        }
    })
}

struct PlatformFixture {
    parsed_files: Vec<legend_pure_parser_ast::SourceFile>,
    auto_imports: Vec<SmolStr>,
}

/// Compile platform + user Pure source and evaluate a function by its
/// mangled FQN within the `test` package.
///
/// The `fqn` parameter is the mangled function name as produced by Pass 2.1,
/// e.g. `"f__Integer_1_"` for `function test::f(): Integer[1]`.
fn eval_pure(source: &str, fqn: &str) -> Value {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);

    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));

    eval.call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("Evaluation error: {e}"))
}

/// Like `eval_pure` but expects an evaluation error.
#[allow(dead_code)]
fn eval_pure_err(source: &str, fqn: &str) -> String {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);

    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));

    eval.call_user_function_by_id(fn_id)
        .expect_err("Expected evaluation to fail")
        .to_string()
}

/// Compile user test source together with the platform model.
fn compile_with_platform(user_source: &str) -> PureModel {
    let fixture = platform_model();

    // Parse user source — user test code should parse cleanly
    let user_ast = legend_pure_parser_parser::parse(user_source, "<test>").unwrap_or_else(|e| {
        panic!(
            "Parse error: {:?}",
            e.errors
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        )
    });

    // Combine platform + user files
    let mut all_files: Vec<_> = fixture.parsed_files.clone();
    all_files.push(user_ast);

    // Compile everything together — the compiler resolves operator calls
    // to platform Function elements, and Pass 2.1 mangles their names
    match legend_pure_parser_pure::pipeline::compile(&all_files, &fixture.auto_imports) {
        Ok(model) => model,
        Err(partial) => {
            // Accept compilation with errors for now — platform files may
            // have unsupported constructs. As long as our test function
            // compiled successfully, evaluation will work.
            partial.model
        }
    }
}

// ===========================================================================
// 1. Literals
// ===========================================================================

#[test]
fn eval_integer_literal() {
    let result = eval_pure("function test::f(): Integer[1] { 42 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_negative_integer() {
    let result = eval_pure("function test::f(): Integer[1] { -7 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(-7));
}

#[test]
fn eval_string_literal() {
    let result = eval_pure(
        "function test::f(): String[1] { 'hello world' }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn eval_boolean_true() {
    let result = eval_pure("function test::f(): Boolean[1] { true }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_boolean_false() {
    let result = eval_pure("function test::f(): Boolean[1] { false }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_float_literal() {
    let result = eval_pure("function test::f(): Float[1] { 2.5 }", "f__Float_1_");
    assert_eq!(result, Value::Float(2.5));
}

// ===========================================================================
// 2. Arithmetic operators
// ===========================================================================

#[test]
fn eval_addition() {
    let result = eval_pure("function test::f(): Integer[1] { 1 + 2 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn eval_subtraction() {
    let result = eval_pure("function test::f(): Integer[1] { 10 - 3 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(7));
}

#[test]
fn eval_multiplication() {
    let result = eval_pure("function test::f(): Integer[1] { 6 * 7 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_complex_arithmetic() {
    let result = eval_pure(
        "function test::f(): Integer[1] { 2 + 3 * 4 }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(14));
}

// ===========================================================================
// 3. Comparison operators
// ===========================================================================

#[test]
fn eval_equal_true() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 == 1 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_equal_false() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 == 2 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_less_than() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 < 2 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

// ===========================================================================
// 4. Boolean operators
// ===========================================================================

#[test]
fn eval_and() {
    let result = eval_pure(
        "function test::f(): Boolean[1] { true && false }",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_or() {
    let result = eval_pure(
        "function test::f(): Boolean[1] { false || true }",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_not() {
    let result = eval_pure("function test::f(): Boolean[1] { !true }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

// ===========================================================================
// 5. String operations
// ===========================================================================

#[test]
fn eval_string_concat() {
    let result = eval_pure(
        "function test::f(): String[1] { 'hello' + ' ' + 'world' }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("hello world".into()));
}

// ===========================================================================
// 6. Variables (let)
// ===========================================================================

#[test]
fn eval_let_binding() {
    let result = eval_pure(
        "function test::f(): Integer[1] { let x = 42; $x; }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_let_with_arithmetic() {
    let result = eval_pure(
        "function test::f(): Integer[1] { let x = 10; let y = 20; $x + $y; }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(30));
}

// ===========================================================================
// 7. Control flow (if)
// ===========================================================================

#[test]
fn eval_if_true_branch() {
    let result = eval_pure(
        "function test::f(): Integer[1] { if(true, |1, |2) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn eval_if_false_branch() {
    let result = eval_pure(
        "function test::f(): Integer[1] { if(false, |1, |2) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn eval_if_with_expression() {
    let result = eval_pure(
        "function test::f(): String[1] { if(1 == 1, |'yes', |'no') }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("yes".into()));
}

// ===========================================================================
// 8. Collections
// ===========================================================================

#[test]
fn eval_collection_literal() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3] }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(1));
            assert_eq!(v[1], Value::Integer(2));
            assert_eq!(v[2], Value::Integer(3));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

// ===========================================================================
// 9. User-defined function calls
// ===========================================================================

#[test]
fn eval_call_user_function() {
    let result = eval_pure(
        r"
        function test::double(x: Integer[1]): Integer[1] { $x * 2 }
        function test::f(): Integer[1] { test::double(21) }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_recursive_function() {
    let result = eval_pure(
        r"
        function test::factorial(n: Integer[1]): Integer[1] {
            if($n == 0, |1, |$n * test::factorial($n - 1))
        }
        function test::f(): Integer[1] { test::factorial(5) }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(120));
}

// ===========================================================================
// 10. Lambda + higher-order (map, filter, fold)
// ===========================================================================

#[test]
fn eval_map_lambda() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3]->map(x | $x * 2) }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(2));
            assert_eq!(v[1], Value::Integer(4));
            assert_eq!(v[2], Value::Integer(6));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_filter_lambda() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3, 4, 5]->filter(x | $x > 3) }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 2);
            assert_eq!(v[0], Value::Integer(4));
            assert_eq!(v[1], Value::Integer(5));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_fold_sum() {
    let result = eval_pure(
        "function test::f(): Integer[1] { [1, 2, 3, 4, 5]->fold({x, acc | $acc + $x}, 0) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(15));
}

// ===========================================================================
// 11. Combined: approaching Pure test complexity
// ===========================================================================

#[test]
fn eval_let_with_if_and_arithmetic() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            let x = 10;
            let y = 20;
            if($x + $y == 30, |'correct', |'wrong');
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("correct".into()));
}

// ===========================================================================
// 12. Advanced Variable Shadowing & Closures (TDD)
// ===========================================================================

#[test]

fn eval_lambda_let_shadows_outer_let() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 10;
            [1, 2, 3]->map(y | 
                let x = 20;
                $x + $y
            );
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(21));
            assert_eq!(v[1], Value::Integer(22));
            assert_eq!(v[2], Value::Integer(23));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]

fn eval_variable_shadowing_lambda() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 10;
            [1, 2, 3]->map(x | $x * 2);
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(2));
            assert_eq!(v[1], Value::Integer(4));
            assert_eq!(v[2], Value::Integer(6));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
#[ignore = "TDD: capturing outer scopes correctly across deep function calls"]
fn eval_closure_captures_outer_scope() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let multiplier = 10;
            let base = 5;
            [1, 2, 3]->map(x | ($x * $multiplier) + $base);
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(15));
            assert_eq!(v[1], Value::Integer(25));
            assert_eq!(v[2], Value::Integer(35));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
#[ignore = "TDD: awaiting evaluator support for closure variable escaping/capturing"]
fn eval_closure_captures_and_binds_inner_let() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 100;
            [1, 2]->map(y | 
                let inner = $x;
                $inner + $y
            );
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 2);
            assert_eq!(v[0], Value::Integer(101));
            assert_eq!(v[1], Value::Integer(102));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}
