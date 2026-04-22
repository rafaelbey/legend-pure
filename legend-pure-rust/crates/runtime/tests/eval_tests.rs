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

// ===========================================================================
// 9. Meta / Reflection (id, type, genericType, rawType, enumName,
//    enumValues, toRepresentation, subTypeOf)
// ===========================================================================

#[test]
fn eval_type_of_integer_resolves_to_integer_primitive() {
    // type(42) must return the bootstrap `Integer` primitive. elementToPath
    // gives us a stable string comparison despite the primitive's generated
    // ElementId drifting between runs.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn eval_type_of_string_resolves_to_string_primitive() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            'hi'->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("String".into()));
}

#[test]
fn eval_type_of_element_is_itself() {
    // type(<element>) returns the element — round-trip through elementToPath
    // proves the same path is preserved.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure".into()));
}

#[test]
fn eval_generic_type_wraps_rawtype() {
    // genericType(42).rawType->elementToPath() must round-trip back to
    // Integer. This exercises rawType's read of the heap-stored property.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->genericType()->rawType()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn eval_id_of_integer_returns_display() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->id();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("42".into()));
}

#[test]
fn eval_id_of_element_returns_fqn() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->id();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure".into()));
}

#[test]
fn eval_to_representation_quotes_strings() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            'hello'->toRepresentation();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("'hello'".into()));
}

#[test]
fn eval_to_representation_integer_is_unquoted() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->toRepresentation();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("42".into()));
}

#[test]
fn eval_sub_type_of_integer_number_is_true() {
    // Integer extends Number — walking the primitive super_type chain must
    // find it.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @Number);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_sub_type_of_is_reflexive() {
    // Reflexivity: every type is a subtype of itself.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @Integer);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_sub_type_of_unrelated_is_false() {
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @String);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_enum_name_of_user_enum() {
    // Define a tiny enumeration, then ask for its simple name via the
    // metamodel reference.
    let result = eval_pure(
        r"
        Enum test::Color { RED, GREEN, BLUE }

        function test::f(): String[1] {
            enumName(@test::Color);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Color".into()));
}

#[test]
fn eval_enum_values_expands_to_each_member() {
    // enumValues returns one "EnumName.Member" entry per declared value in
    // declaration order.
    let result = eval_pure(
        r"
        Enum test::Color { RED, GREEN, BLUE }

        function test::f(): Color[*] {
            enumValues(@test::Color);
        }
        ",
        "f__Color_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::String("Color.RED".into()));
            assert_eq!(v[1], Value::String("Color.GREEN".into()));
            assert_eq!(v[2], Value::String("Color.BLUE".into()));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

// ===========================================================================
// 10. Coordinator regression — contains_ prefix disambiguation + find lambda
// ===========================================================================

#[test]
fn eval_contains_collection_variant_dispatches_correctly() {
    // Collection `contains(T[*], Any[1]):Boolean[1]` mangles to
    // `contains_T_MANY__Any_1__Boolean_1_`; string
    // `contains(String[1], String[1]):Boolean[1]` mangles to
    // `contains_String_1__String_1__Boolean_1_`. Both keys share the
    // `contains_` prefix — exact-FQN dispatch must win so the collection
    // form gets its own native.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            [1, 2, 3]->contains(2);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_contains_string_variant_still_routes_to_string_impl() {
    // Sanity: the string form continues to work after the collection form
    // was added.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            'hello world'->contains('world');
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_find_returns_first_matching_element() {
    // Closes out the `find_returns_first_match` placeholder that was
    // ignored at unit-test level (lambda-dependent — needs real evaluator).
    let result = eval_pure(
        r"
        function test::f(): Integer[0..1] {
            [1, 2, 3, 4]->find(x | $x > 2);
        }
        ",
        "f__Integer_$0_1$_",
    );
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn eval_find_returns_unit_when_no_match() {
    let result = eval_pure(
        r"
        function test::f(): Integer[0..1] {
            [1, 2, 3]->find(x | $x > 10);
        }
        ",
        "f__Integer_$0_1$_",
    );
    assert_eq!(result, Value::Unit);
}

#[test]
fn eval_element_to_path_ephemeral_element() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Root')
                )
            )->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_element_include_root_true() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Root')
                )
            )->elementToPath(true);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Root::pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_element_non_root_outermost() {
    // Outermost package is named "Other" (not "Root"). With include_root=true
    // the outermost name is still used as the prefix.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Other')
                )
            )->elementToPath(true);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Other::pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_nameless() {
    // `^PackageableElement()` with no `name` renders as the empty string.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("".into()));
}

#[test]
#[ignore = "broad surveyor canary across meta::pure::functions — prints full bucket breakdown"]
fn eval_surveyor_broad_canary() {
    use legend_pure_runtime::eval::Evaluator;

    let packages = [
        "meta::pure::functions::meta::tests",
        "meta::pure::functions::collection::tests",
        "meta::pure::functions::string::tests",
        "meta::pure::functions::math::tests",
        "meta::pure::functions::date::tests",
        "meta::pure::functions::boolean::tests",
        "meta::pure::functions::lang::tests",
    ];

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();

    for pkg in &packages {
        let mut evaluator = Evaluator::new(&model, &registry);
        let report_result = evaluator.call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String(SmolStr::new(pkg)),
                Value::String("".into()),
            ],
        );
        let Ok(Value::Object(report_id)) = report_result else {
            eprintln!("[{pkg}] surveyor failed: {report_result:?}");
            continue;
        };
        let heap = evaluator.heap();
        let read = |name: &str| -> i64 {
            let values = heap.get_property_values(report_id, name).unwrap();
            match values.iter().next() {
                Some(Value::Integer(n)) => *n,
                _ => -1,
            }
        };
        let pass = read("passCount");
        let fail = read("failCount");
        let error = read("errorCount");
        let skip = read("skipCount");
        eprintln!(
            "[{pkg}] pass={pass} fail={fail} error={error} skip={skip} total={}",
            pass + fail + error + skip
        );
    }
}

#[test]
fn eval_surveyor_element_to_path_all_tests_pass() {
    // After the compiler's inherited-property-type-resolution fix, every
    // `<<test.Test>>` function under `meta::pure::functions::meta::tests::elementToPath`
    // should PASS. If this regresses, the surveyor has a real problem —
    // diagnose before relaxing the assertion.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::meta::tests::elementToPath".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let Value::Object(report_id) = report else {
        panic!("surveyor returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    let read = |name: &str| -> i64 {
        let values = heap.get_property_values(report_id, name).unwrap();
        match values.iter().next() {
            Some(Value::Integer(n)) => *n,
            _ => -1,
        }
    };
    let pass = read("passCount");
    let fail = read("failCount");
    let error = read("errorCount");
    let skip = read("skipCount");
    assert_eq!(
        error, 0,
        "expected 0 ERROR'd tests, got {error} (pass={pass}, fail={fail}, skip={skip})"
    );
    assert!(pass >= 7, "expected >=7 PASS, got {pass}");
}

#[test]
fn eval_surveyor_on_element_to_path_tests_has_nonzero_runs() {
    // Canary: after the Track 1–5 native rollout, surveyor should be able to
    // bucket real platform `<<test.Test>>` functions as PASS/FAIL/ERROR
    // (not all-ERROR). We don't assert specific counts — the assertion chain
    // still reaches into many natives we haven't implemented — but at least
    // *one* test in this package must flip out of ERROR.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::meta::tests::elementToPath".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let Value::Object(report_id) = report else {
        panic!("surveyor returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    let read = |name: &str| -> i64 {
        let values = heap.get_property_values(report_id, name).unwrap();
        match values.iter().next() {
            Some(Value::Integer(n)) => *n,
            _ => -1,
        }
    };
    let pass = read("passCount");
    let fail = read("failCount");
    let error = read("errorCount");
    let skip = read("skipCount");
    let total = pass + fail + error + skip;
    assert!(total > 0, "expected at least one test, got 0");
    assert!(
        pass + fail + skip > 0,
        "expected at least one non-error outcome after native rollout (pass={pass}, fail={fail}, error={error}, skip={skip})"
    );
}
