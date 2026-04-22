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

// ===========================================================================
// 8. Meta-model natives: pathToElement / elementToPath / element properties
// ===========================================================================

#[test]
fn eval_path_to_element_resolves_package() {
    // Resolve a package from its qualified path and then round-trip via
    // elementToPath — the 1-arg Pure wrapper forwards to the 3-arg native.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure".into()));
}

#[test]
fn eval_path_to_element_resolves_nested_package() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure::metamodel::type', '::')->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure::metamodel::type".into()));
}

#[test]
fn eval_element_name_package() {
    // $pkg.name returns the simple package name (scalar in the M3 metamodel).
    let result = eval_pure(
        r"
        function test::f(): String[*] {
            pathToElement('meta::pure', '::').name;
        }
        ",
        "f__String_MANY_",
    );
    // Check that we got either a single-value or a collection containing 'pure'.
    match result {
        Value::String(s) => assert_eq!(s.as_str(), "pure"),
        Value::Collection(v) => {
            assert_eq!(v.len(), 1, "expected 1 element, got {}", v.len());
            assert_eq!(v[0], Value::String("pure".into()));
        }
        other => panic!("Expected String or Collection, got {other:?}"),
    }
}

#[test]
fn eval_package_children_includes_sub_packages() {
    // meta::pure has many sub-packages (metamodel, functions, test, ...).
    // Just assert the collection is non-empty.
    let result = eval_pure(
        r"
        function test::f(): Integer[1] {
            pathToElement('meta::pure', '::').children->size();
        }
        ",
        "f__Integer_1_",
    );
    match result {
        Value::Integer(n) => assert!(n > 0, "expected non-empty children, got {n}"),
        other => panic!("Expected Integer, got {other:?}"),
    }
}

#[test]
fn eval_element_to_path_with_custom_separator() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            elementToPath(pathToElement('meta::pure', '::'), '.', false);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta.pure".into()));
}

#[test]
fn eval_element_to_path_include_root() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            elementToPath(pathToElement('meta::pure', '::'), '::', true);
        }
        ",
        "f__String_1_",
    );
    // includeRoot=true prepends the literal "Root" segment (matches the
    // Java Pure runtime's rendering of the unnamed root package).
    assert_eq!(result, Value::String("Root::meta::pure".into()));
}

#[test]
fn eval_surveyor_entry_point_runs_to_completion() {
    // End-to-end proof that the Pure-native surveyor can walk the compiled
    // platform: discovery → match/instanceOf dispatch → ^Class(...) object
    // construction → flatten → aggregate → `^TestReport(...)` return.
    //
    // The collection package has `<<test.Test>>`-stereotyped functions that
    // rely on `assertEquals`/`joinStrings`/etc. (not all implemented yet), so
    // many will bucket as ERROR. The regression lock is structural: the
    // surveyor must return a `TestReport` with all five counter fields
    // populated to non-negative integers that sum to the total.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::collection".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let report_id = match report {
        Value::Object(id) => id,
        other => panic!("surveyor returned non-object: {other:?}"),
    };

    let heap = evaluator.heap();
    assert_eq!(
        heap.classifier(report_id).unwrap(),
        "meta::pure::test::surveyor::TestReport"
    );

    // Each counter is a single Integer >= 0.
    let read_counter = |name: &str| -> i64 {
        let values = heap.get_property_values(report_id, name).unwrap();
        let collected: Vec<_> = values.iter().cloned().collect();
        assert_eq!(
            collected.len(),
            1,
            "expected one Integer for {name}, got {collected:?}"
        );
        match collected[0] {
            Value::Integer(n) => n,
            ref other => panic!("expected Integer for {name}, got {other:?}"),
        }
    };
    let pass = read_counter("passCount");
    let fail = read_counter("failCount");
    let error = read_counter("errorCount");
    let skip = read_counter("skipCount");
    for (name, v) in [
        ("passCount", pass),
        ("failCount", fail),
        ("errorCount", error),
        ("skipCount", skip),
    ] {
        assert!(v >= 0, "{name} should be non-negative, got {v}");
    }

    // `results` length must equal pass + fail + error + skip — every test
    // outcome is classified into exactly one bucket.
    let results = heap.get_property_values(report_id, "results").unwrap();
    let total = i64::try_from(results.len()).expect("results length fits in i64");
    assert_eq!(
        total,
        pass + fail + error + skip,
        "results length ({total}) must equal pass+fail+error+skip"
    );
}

#[test]
fn eval_cast_routes_through_prefix_fallback_despite_name_mangle_drift() {
    // The `cast` native is registered as `cast_Any_m__V_1__V_m_` but the
    // compiler mangles Pure-level `cast` call sites differently (e.g., T vs V
    // type-variable naming). This must still route via the simple-name
    // prefix fallback. Regressing here would silently break `cast` in any
    // platform function that calls it.
    let result = eval_pure(
        r"
        function test::f(): Integer[1] {
            42->cast(@Integer);
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_instance_of_with_type_reference() {
    // @X evaluates to Value::Element after the TypeReference change —
    // instanceOf must accept it as its type argument.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            42->instanceOf(@Integer);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_match_dispatches_to_first_matching_lambda() {
    // A simple match that distinguishes Integer from String. We feed
    // an integer literal via a trivial wrapping — the lambda's declared
    // parameter type drives dispatch.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->match([
                i: Integer[1] | 'int',
                s: String[1] | 'str'
            ]);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("int".into()));
}
