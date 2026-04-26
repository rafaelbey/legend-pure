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

/// Initialise `tracing-subscriber` once per test process so the
/// `tracing::debug!` / `tracing::warn!` events emitted by the
/// compiler (`resolve_function_call`, `narrow_candidates_by_type`,
/// `reactivate_value`, etc.) surface in test output when the user
/// sets `RUST_LOG`. Default filter is `error` (silent); override
/// with e.g.
///     RUST_LOG=legend_pure_parser_pure::resolve=debug cargo test ...
/// to trace overload resolution for a specific failing test.
fn init_test_tracing() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

/// Cached pre-parsed platform ASTs + auto-imports.
///
/// Compiling platform sources is expensive (~200 Pure files), so we parse
/// them once and cache the ASTs. Each test then clones the ASTs and combines
/// them with user code for a fresh compilation.
fn platform_model() -> &'static PlatformFixture {
    init_test_tracing();
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
fn eval_id_of_element_returns_simple_name() {
    // `id()` on a model-element reference returns the **simple name**, not
    // the qualified path — matches Java Pure and the `testId`/`testPrimitives`
    // PCT tests (`CC_Person->id() == 'CC_Person'`, not the full
    // `meta::pure::...::CC_Person` path). `elementToPath` remains the native
    // for qualified-path rendering.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->id();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("pure".into()));
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
    // enumValues returns one `Value::EnumValue { enum_id, member }` entry
    // per declared value in declaration order. The result's enum_id field
    // points at the same Color Enumeration element across every member.
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
            let members: Vec<(SmolStr, SmolStr)> = v
                .iter()
                .filter_map(|val| match val {
                    Value::EnumValue { enum_id: _, member } => {
                        Some((SmolStr::new("Color"), member.clone()))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(members.len(), 3, "all entries must be EnumValue, got {v:?}");
            assert_eq!(members[0].1, "RED");
            assert_eq!(members[1].1, "GREEN");
            assert_eq!(members[2].1, "BLUE");
            if let (
                Value::EnumValue { enum_id: e0, .. },
                Value::EnumValue { enum_id: e1, .. },
                Value::EnumValue { enum_id: e2, .. },
            ) = (&v[0], &v[1], &v[2])
            {
                assert_eq!(e0, e1);
                assert_eq!(e1, e2);
            }
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
fn eval_partial_date_literals_round_trip_toRepresentation() {
    // A year-only and a year-month StrictDate literal must preserve
    // precision through lowering and `toRepresentation`. Before the
    // partial-precision fix, the parser accepted `%2014` / `%2014-01`
    // but `parse_strict_date` in lower.rs required 3 segments and
    // silently returned `None`, dropping the expression during
    // lowering and corrupting argument counts for the surrounding call.
    assert_eq!(
        eval_pure(
            r"function test::f(): String[1] { %2014->toRepresentation() }",
            "f__String_1_",
        ),
        Value::String("%2014".into()),
    );
    assert_eq!(
        eval_pure(
            r"function test::g(): String[1] { %2014-01->toRepresentation() }",
            "g__String_1_",
        ),
        Value::String("%2014-01".into()),
    );
}

#[test]
fn eval_multiple_assertEq_mixed_date_precision() {
    // Regression: multiple assertEq calls with mixed date precisions —
    // previously crashed with "Variable 'actual' not found" because
    // the year-month date literal failed to lower and `filter_map`
    // silently dropped the second argument of `assertEq`.
    assert_eq!(
        eval_pure(
            r"
            function test::f(): Boolean[1] {
                assertEq('%2014-01-01', %2014-01-01->toRepresentation());
                assertEq('%2014-01', %2014-01->toRepresentation());
                assertEq('%2014', %2014->toRepresentation());
            }
            ",
            "f__Boolean_1_",
        ),
        Value::Boolean(true),
    );
}

#[test]
#[ignore = "diagnostic: bucket meta::tests errors by first line of message"]
fn eval_surveyor_meta_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::meta::tests");
}

#[test]
fn eval_string_plus_inside_fold_lambda() {
    // Mirrors the platform plus.pure:66 pattern:
    // `$people->fold({p1, p2 | $p2.lastName + ' ' + $p1.lastName}, init)`
    // — `+` between two narrowed-by-fold lambda params and a string
    // literal. Today this fails to compile against the platform with
    // "Ambiguous function call 'plus': found 5 overloads with 1 args
    // (narrowed from 5 candidates)" because the lambda-param narrowing
    // doesn't reach `+` operands inside `fold`.
    let result = eval_pure(
        r"
        function test::f(): String[1]
        {
            ['a', 'b', 'c']->fold({p, s | $s + '/' + $p}, 'init')
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("init/a/b/c".into()));
}

#[test]
fn eval_class_let_then_lambda_param_typed_via_pkg_ref_typeinfo() {
    // Locks the platform-AmbiguousImport fix from the same series as
    // `eval_lambda_param_inference_narrows_overloads` but on the
    // type-info-flow side. Pre-fix, `^test::P(...)` lowering produced
    // a `new(@P, ...)` ValueSpec whose `function` was None and whose
    // `type_info` carried `P[1]` — but `infer_let_type` didn't read
    // `type_info`, so `var_types["people"]` was never populated,
    // causing `$people->map(p | …)` to leave `p` typed as `Any` and
    // killing every `+` overload narrowing inside the lambda body.
    // Post-fix:
    //   1. `lower_packageable_element_ref` pre-sets `type_info` to
    //      `Class<P>[1]` via the new `build_packageable_element_ref`
    //      helper (used by all three PackageableElementRef call
    //      sites).
    //   2. `infer_typeexpr_from_valuespec` and `infer_let_type` both
    //      honour `vs.type_info` first, mirroring the canonical
    //      `reference_type_info_capture` pattern.
    // Together this lets `$people->map(p | $p.lastName + 'X')` flow
    // `Person` from the let through to the lambda body's `+` operand
    // narrowing.
    let result = eval_pure(
        r"
        Class test::P { lastName: String[1]; }

        function test::f(): String[1]
        {
            let people = [^test::P(lastName='Doe')];
            $people->map(p | $p.lastName + 'X')->joinStrings(',')
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("DoeX".into()));
}

#[test]
fn eval_string_plus_string_variable() {
    // Mirrors what stress generators emit: `function f(name: String[1]) { 'hello ' + $name }`.
    use legend_pure_runtime::eval::Evaluator;
    let model = compile_with_platform(
        r#"
        function test::greet(name: String[1]): String[1]
        {
            'hello ' + $name
        }
        "#,
    );
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let result = eval
        .call("test::greet", &[Value::String("world".into())])
        .expect("call should succeed");
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn eval_lambda_param_inference_narrows_overloads() {
    // Locks the bug fixed by the type-narrowing step in
    // `lower_args_with_lambda_inference`. Pure's platform `map` declares
    // three arity-2 overloads; without narrowing, the orchestrator gives
    // up and types `s` as `Any[1]`, which causes `+` to dispatch to a
    // non-string overload and the lambda to silently return nothing.
    // After the fix, narrowing by `['a','b']` (the concrete first arg)
    // collapses the overload set to one and feeds `String[1]` into the
    // lambda body's type info.
    let result = eval_pure(
        r#"
        function test::f(): String[1]
        {
            ['a','b']->map(s| $s + 'X')->joinStrings(',')
        }
        "#,
        "f__String_1_",
    );
    assert_eq!(result, Value::String("aX,bX".into()));
}

#[test]
fn eval_relation_at_chain_returns_relation_type() {
    // Locks in the `@(x:String)->genericType().rawType->cast(@RelationType<Any>)`
    // chain that `meta::pure::functions::meta::tests::addColumns` needs as
    // its source argument. Returns the column's element name reached via
    // `.columns->at(0).classifierGenericType.typeArguments[1].rawType.name`.
    let result = eval_pure(
        r#"
        function test::f(): String[1]
        {
            let rt = @(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne();
            let col = $rt.columns->at(0);
            $col.classifierGenericType->toOne().typeArguments->at(1).rawType.name->toOne()
        }
        "#,
        "f__String_1_",
    );
    assert_eq!(result, Value::String("String".into()));
}

#[test]
fn eval_relation_add_columns_against_at_chain_source() {
    // End-to-end: `@(x:String)->genericType().rawType->cast(@RelationType<Any>)
    // ->toOne()->addColumns(~[ab:String[1], z:Integer])` materialises a
    // RelationType whose merged columns are reachable through the
    // `.classifierGenericType.multiplicityArguments[0].lowerBound.value`
    // chain (P0's MultiplicityValue shape fix).
    let result = eval_pure(
        r#"
        function test::f(): String[1]
        {
            let rt = addColumns(
                @(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne(),
                ~[ab:String[1], z:Integer]);
            let col = $rt.columns->at(2);
            let mult = $col.classifierGenericType.multiplicityArguments->at(0);
            $col.name->toOne()
                + ':'
                + $col.classifierGenericType->toOne().typeArguments->at(1).rawType.name->toOne()
                + '['
                + $mult.lowerBound.value->toOne()->toString()
                + '..'
                + $mult.upperBound.value->toOne()->toString()
                + ']'
        }
        "#,
        "f__String_1_",
    );
    assert_eq!(result, Value::String("z:Integer[0..1]".into()));
}

#[test]
fn eval_relation_add_columns_after_evaluate_and_deactivate() {
    // Drives the platform-defined
    // `meta::pure::functions::relation::tests::testAddColumnsAfterEvaluateAndDeactivate`
    // in isolation. Source is `^RelationType<Any>()->evaluateAndDeactivate()`,
    // so this test exercises the addColumns native end-to-end without
    // depending on the `@(x:String)->genericType().rawType->cast(...)` chain
    // that the sister test `testAddColumns` requires.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let result = evaluator
        .call(
            "meta::pure::functions::relation::tests::testAddColumnsAfterEvaluateAndDeactivate",
            &[],
        )
        .expect("test should evaluate without error");

    assert_eq!(result, Value::Boolean(true));
}

#[test]
#[ignore = "diagnostic: bucket lang::tests errors by first line of message"]
fn eval_surveyor_lang_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::lang::tests");
}

#[test]
#[ignore = "diagnostic: bucket boolean::tests errors by first line of message"]
fn eval_surveyor_boolean_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::boolean::tests");
}

#[test]
#[ignore = "diagnostic: bucket collection::tests errors by first line of message"]
fn eval_surveyor_collection_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::collection::tests");
}

#[test]
#[ignore = "diagnostic: bucket string::tests errors by first line of message"]
fn eval_surveyor_string_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::string::tests");
}

#[test]
#[ignore = "diagnostic: bucket math::tests errors by first line of message"]
fn eval_surveyor_math_tests_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::math::tests");
}

#[test]
#[ignore = "diagnostic: bucket meta::tests FAILs by first line of message"]
fn eval_surveyor_meta_tests_fail_histogram() {
    surveyor_fail_histogram("meta::pure::functions::meta::tests");
}

#[test]
#[ignore = "diagnostic: bucket lang::tests FAILs by first line of message"]
fn eval_surveyor_lang_tests_fail_histogram() {
    surveyor_fail_histogram("meta::pure::functions::lang::tests");
}

#[test]
#[ignore = "diagnostic: bucket string::tests FAILs by first line of message"]
fn eval_surveyor_string_tests_fail_histogram() {
    surveyor_fail_histogram("meta::pure::functions::string::tests");
}

#[test]
#[ignore = "diagnostic: bucket collection::tests FAILs by first line of message"]
fn eval_surveyor_collection_tests_fail_histogram() {
    surveyor_fail_histogram("meta::pure::functions::collection::tests");
}

fn surveyor_error_histogram(package: &str) {
    surveyor_outcome_histogram(package, "ERROR");
}

fn surveyor_fail_histogram(package: &str) {
    surveyor_outcome_histogram(package, "FAIL");
}

fn surveyor_outcome_histogram(package: &str, target_status: &str) {
    use legend_pure_runtime::eval::Evaluator;
    use std::collections::BTreeMap;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String(SmolStr::new(package)),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");
    let Value::Object(report_id) = report else {
        panic!();
    };
    let heap = evaluator.heap();
    let results = heap.get_property_values(report_id, "results").unwrap();

    let mut histogram: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut outcome_count = 0;
    for val in results.iter() {
        let Value::Object(res_id) = val else {
            continue;
        };
        let status = heap
            .get_property_values(*res_id, "status")
            .ok()
            .and_then(|v| v.iter().next().cloned());
        let matches_target = matches!(&status, Some(Value::EnumValue { member, .. }) if member.as_str() == target_status);
        if !matches_target {
            continue;
        }
        outcome_count += 1;
        let fqn = heap
            .get_property_values(*res_id, "fqn")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "<unknown>".into());
        let msg = heap
            .get_property_values(*res_id, "message")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "<no message>".into());
        // Bucket by "first useful line after Execution error" — the actual cause.
        let bucket = msg
            .lines()
            .map(|l| l.trim_matches('"').trim())
            .find(|l| {
                !l.is_empty()
                    && !l.starts_with("Execution error")
                    && !l.starts_with("Assert failure")
                    && !l.starts_with("Full Stack")
            })
            .unwrap_or("<no bucket line>")
            .to_string();
        histogram
            .entry(bucket)
            .or_default()
            .push(format!("{fqn} || {msg}"));
    }
    eprintln!("\n=== {package} {target_status} histogram ({outcome_count} results) ===",);
    let mut rows: Vec<_> = histogram.iter().collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (bucket, tests) in rows {
        eprintln!("\n[{}] {}", tests.len(), bucket);
        for (i, t) in tests.iter().enumerate().take(40) {
            eprintln!("    {}. {}", i + 1, t);
        }
        if tests.len() > 40 {
            eprintln!("    ... and {} more", tests.len() - 40);
        }
    }
}

/// Harvest every distinct `Function not found: FQN` signalled by an
/// ERROR-bucket result across all surveyor packages. Groups by simple
/// name (the part before the first mangled-type segment) so we can
/// spot families of missing overloads the Rust registry hasn't wired
/// up yet. Previously targeted the SKIP bucket when `classify_outcome`
/// downgraded FunctionNotFound; now that classification policy is
/// strict (no message-based SKIP), those same results surface in the
/// ERROR bucket.
#[test]
#[ignore = "diagnostic: list missing-native FQNs across all surveyor packages"]
fn eval_surveyor_missing_natives_harvest() {
    use legend_pure_runtime::eval::Evaluator;
    use std::collections::{BTreeMap, BTreeSet};

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
    let mut evaluator = Evaluator::new(&model, &registry);
    let mut by_simple: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for pkg in packages {
        let report = evaluator
            .call(
                "meta::pure::test::surveyor::runTestsFromPath",
                &[Value::String(SmolStr::new(pkg)), Value::String("".into())],
            )
            .expect("runTestsFromPath returns a TestReport");
        let Value::Object(report_id) = report else {
            continue;
        };
        let results = evaluator
            .heap()
            .get_property_values(report_id, "results")
            .unwrap_or_else(|_| im_rc::Vector::new());
        for val in results.iter() {
            let Value::Object(res_id) = val else {
                continue;
            };
            let status = evaluator
                .heap()
                .get_property_values(*res_id, "status")
                .ok()
                .and_then(|v| v.iter().next().cloned());
            if !matches!(&status, Some(Value::EnumValue { member, .. }) if member.as_str() == "ERROR")
            {
                continue;
            }
            let msg = evaluator
                .heap()
                .get_property_values(*res_id, "message")
                .ok()
                .and_then(|v| v.iter().next().cloned())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            // Lines that actually name the missing function — accept both
            // "Function not found: X" and error wrapping variants.
            for line in msg.lines() {
                let trimmed = line.trim_matches('"').trim();
                if let Some(rest) = trimmed.strip_prefix("Function not found: ") {
                    let fqn = rest.trim().trim_end_matches('"').to_string();
                    let simple = fqn.split('_').next().unwrap_or(&fqn).to_string();
                    by_simple.entry(simple).or_default().insert(fqn);
                }
            }
        }
    }

    eprintln!(
        "\n=== Missing-native harvest: {} distinct simple names, {} distinct mangled FQNs ===",
        by_simple.len(),
        by_simple.values().map(BTreeSet::len).sum::<usize>()
    );
    for (simple, fqns) in &by_simple {
        eprintln!("\n[{}] {} ({} variants)", fqns.len(), simple, fqns.len());
        for fqn in fqns {
            eprintln!("    {fqn}");
        }
    }
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
            &[Value::String(SmolStr::new(pkg)), Value::String("".into())],
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

// ===========================================================================
// PCT (Pure Compatibility Tests) — adapter-driven test execution
// ===========================================================================
//
// PCT tests differ from `<<test.Test>>` tests in two ways:
// 1. They are annotated `<<PCT.test>>` and live alongside the function they
//    exercise (e.g. `boolean/operation/not.pure` defines both `not` and the
//    8 `<<PCT.test>>` functions for it).
// 2. They take an *adapter* as their sole parameter — a function-of-function
//    that the runtime injects so the same test body can run against multiple
//    execution back-ends. The in-memory adapter
//    `meta::pure::test::pct::testAdapterForInMemoryExecution<X|o>` is
//    just `$f->eval()`.
//
// The Pure-side surveyor (`meta::pure::test::surveyor::runPCTTests`) walks
// a package, filters `<<PCT.test>>` functions, and invokes each via
// `executePCTTest($t, $adapter, $exclusions)`.
//
// These canaries de-risk that pipeline end-to-end *before* the
// `loadPCTManifest` native lands — they pass the adapter and an empty
// exclusion map directly so the test exercises only discovery → adapter
// dispatch → classify, not JSON manifest parsing.

/// Build the in-memory adapter `Value::Element` and an empty exclusions
/// `Value::Map` for direct `runPCTTests` invocation.
fn pct_canary_args(model: &PureModel) -> (Value, Value) {
    use legend_pure_runtime::value::MapState;
    use std::cell::RefCell;
    use std::rc::Rc;

    // The adapter's mangled FQN as it appears in the shipped manifest
    // `pct_essential_native.json`. Resolves against the platform model.
    let adapter_path: [SmolStr; 5] = [
        "meta".into(),
        "pure".into(),
        "test".into(),
        "pct".into(),
        "testAdapterForInMemoryExecution_Function_1__X_o_".into(),
    ];
    let adapter_id = model
        .resolve_by_path(&adapter_path)
        .expect("in-memory adapter must resolve in the platform model");
    let adapter = Value::Element(adapter_id);

    let exclusions = Value::Map(Rc::new(RefCell::new(MapState::default())));
    (adapter, exclusions)
}

/// PCT manifest for the Rust port — colocated JSON file mirroring
/// the Java `pct_*_native.json` schema (`adapter` + `exclusions`)
/// from `legend-pure-core/.../platform/pure/{essential,grammar/...}/
/// pct_*_native.json`.
///
/// `exclusions` is a `Map<test_fqn → expected_message>` where each
/// entry pins a Rust-port-specific failure: tests whose expected
/// output exceeds a representational limit of our runtime, not a
/// fixable behaviour gap. [`apply_exclusion`] flips matching
/// FAIL/ERROR results to PASS; if a test ever stops failing, the
/// helper flips PASS back to FAIL with "PCT exclusion needs rebase"
/// so stale entries can't go quietly.
///
/// **Exclusion categories (review quarterly):**
///
/// `testAdjust*BigNumber` (5 tests) — assert the result of adding
/// extreme spans (`9_600_000_000` months, `12_345_678_912` hours, …)
/// to dates and expect years like `800002016` or `-1406373`. Our
/// `PureDate` carries the year as `i16` (the `jiff::civil::DateTime`
/// field), so any year outside `[-32768, 32767]` overflows. Java
/// Pure carries year as `int`, giving it roughly
/// `[-2_147_483_648, 2_147_483_647]`. The Rust port makes a
/// smaller-but-correct trade — reject extreme years instead of
/// silently truncating. The non-`BigNumber` siblings in the same
/// packages cover the same arithmetic for in-range inputs.
///
/// `testLarge{Times,Minus,Plus}` (3 tests) — assert i64-overflowing
/// arithmetic with literals like `9223372036854775898` (i64::MAX +
/// 91) and expected results like `18446744073709551614` (2^64 - 2)
/// that don't fit in i64. The platform marks these
/// `{test.excludePlatform = 'Java compiled'}` because Java's Long
/// arithmetic wraps the same way ours does — this is a parity
/// statement, not a bug. Our parser silently parses out-of-i64
/// literals as 0; both expected and actual produce wrong but
/// stable values.
///
/// To remove an exclusion: fix the underlying representational
/// limit (widen year to i32, promote to Decimal on i64 overflow,
/// …). The next canary run will report "PCT exclusion needs
/// rebase" — drop the entry from the JSON.
const PCT_RUST_PORT_MANIFEST_JSON: &str =
    include_str!("pct_rust_port.json");

/// Lazy view onto the parsed Rust-port PCT manifest.
fn rust_port_manifest() -> &'static serde_json::Value {
    use std::sync::OnceLock;
    static CELL: OnceLock<serde_json::Value> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(PCT_RUST_PORT_MANIFEST_JSON)
            .expect("pct_rust_port.json: invalid JSON")
    })
}

/// Build a Rust-port-specific exclusions Map for PCT runs.
///
/// Reads the manifest at `crates/runtime/tests/pct_rust_port.json` —
/// the manifest's `adapter` field is the same value `pct_canary_args`
/// resolves, so we don't override it here; we only build the
/// `Map<Function<Any>, String>` exclusions argument from the
/// manifest's `exclusions` object.
///
/// Use from canary diagnostics where we want the broad pass-count
/// to reflect runtime gaps (yes/no), not representational bounds
/// the platform tests pin extra-aggressively.
fn pct_canary_args_with_rust_exclusions(model: &PureModel) -> (Value, Value) {
    use legend_pure_runtime::value::{MapState, ValueKey};
    use std::cell::RefCell;
    use std::rc::Rc;

    let adapter_path: [SmolStr; 5] = [
        "meta".into(),
        "pure".into(),
        "test".into(),
        "pct".into(),
        "testAdapterForInMemoryExecution_Function_1__X_o_".into(),
    ];
    let adapter = Value::Element(
        model
            .resolve_by_path(&adapter_path)
            .expect("in-memory adapter must resolve"),
    );

    let manifest = rust_port_manifest();
    let exclusions_obj = manifest
        .get("exclusions")
        .and_then(|v| v.as_object())
        .expect("pct_rust_port.json: missing or non-object 'exclusions'");

    let mut state = MapState::default();
    for (fqn, msg) in exclusions_obj {
        let msg = msg.as_str().unwrap_or_else(|| {
            panic!("pct_rust_port.json: exclusion value for {fqn} must be a string")
        });
        state.entries.insert(
            ValueKey::String(SmolStr::new(fqn)),
            Value::String(SmolStr::new(msg)),
        );
    }
    let exclusions = Value::Map(Rc::new(RefCell::new(state)));
    (adapter, exclusions)
}

/// Read a non-negative integer counter from a heap-allocated `TestReport`.
fn read_report_counter(
    evaluator: &Evaluator,
    report_id: legend_pure_runtime::heap::ObjectId,
    name: &str,
) -> i64 {
    let values = evaluator
        .heap()
        .get_property_values(report_id, name)
        .unwrap_or_else(|e| panic!("get {name}: {e}"));
    match values.iter().next() {
        Some(Value::Integer(n)) => *n,
        _ => -1,
    }
}

#[test]
fn eval_pct_canary_boolean_not_runs() {
    // Phase 1 de-risk: PCT tests have generic signatures
    // (`testNotTrue<Z|y>(f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1])`).
    // The adapter is bound to `$f`, then `$f->eval(|true->not())` must
    // dispatch correctly with `Z=Boolean, y=1` inferred at the call site.
    //
    // We don't assert PASS counts — the goal is that *at least one* test
    // in the package flips out of ERROR, proving discovery + adapter
    // dispatch + classify all work end-to-end. Specific counts are
    // measured by `eval_pct_broad_canary` (Phase 4).
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let pkg = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String("meta::pure::functions::boolean::tests::conjunctions::not".into()),
                Value::String("::".into()),
            ],
        )
        .expect("pathToElement must resolve the not-tests package");

    let (adapter, exclusions) = pct_canary_args(&model);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg, Value::String("".into()), adapter, exclusions],
        )
        .expect("runPCTTests must return a TestReport");

    let Value::Object(report_id) = report else {
        panic!("runPCTTests returned non-object: {report:?}");
    };

    let pass = read_report_counter(&evaluator, report_id, "passCount");
    let fail = read_report_counter(&evaluator, report_id, "failCount");
    let error = read_report_counter(&evaluator, report_id, "errorCount");
    let skip = read_report_counter(&evaluator, report_id, "skipCount");
    let total = pass + fail + error + skip;

    assert!(
        total > 0,
        "expected at least one PCT test discovered, got 0"
    );
    assert!(
        pass + fail + skip > 0,
        "expected at least one non-ERROR PCT outcome (pass={pass}, fail={fail}, error={error}, skip={skip}); \
         all-ERROR usually means generic adapter dispatch is broken — start there"
    );
}

/// Packages we know contain `<<PCT.test>>` functions in the platform tree.
/// Mirrors the surveyor's `eval_surveyor_broad_canary` package list, but
/// PCT discovery walks broader sub-trees (PCT tests live alongside their
/// `<<PCT.function>>` declarations rather than in a `tests/` subdir, so
/// rooting at the function package picks them up too).
const PCT_BROAD_CANARY_PACKAGES: &[&str] = &[
    "meta::pure::functions::boolean",
    "meta::pure::functions::collection",
    "meta::pure::functions::lang",
    "meta::pure::functions::math",
    "meta::pure::functions::string",
    "meta::pure::functions::date",
    "meta::pure::functions::meta",
    "meta::pure::functions::multiplicity",
    "meta::pure::functions::asserts",
    "meta::pure::functions::relation",
];

#[test]
#[ignore = "broad PCT canary across meta::pure::functions::* — prints full bucket breakdown"]
fn eval_pct_broad_canary() {
    // The Phase 4 deliverable: how strong is the Rust runtime against the
    // 501-function PCT contract Java enforces on every build? Per-package
    // bucket counts are the signal.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();

    let mut grand_pass = 0i64;
    let mut grand_fail = 0i64;
    let mut grand_error = 0i64;
    let mut grand_skip = 0i64;

    for pkg in PCT_BROAD_CANARY_PACKAGES {
        let mut evaluator = Evaluator::new(&model, &registry);
        let pkg_val = match evaluator.call(
            "meta::pure::functions::meta::pathToElement",
            &[Value::String(SmolStr::new(pkg)), Value::String("::".into())],
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[{pkg}] pathToElement failed: {e}");
                continue;
            }
        };
        let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
        let report_result = evaluator.call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg_val, Value::String("".into()), adapter, exclusions],
        );
        let Ok(Value::Object(report_id)) = report_result else {
            eprintln!("[{pkg}] runPCTTests failed: {report_result:?}");
            continue;
        };
        let pass = read_report_counter(&evaluator, report_id, "passCount");
        let fail = read_report_counter(&evaluator, report_id, "failCount");
        let error = read_report_counter(&evaluator, report_id, "errorCount");
        let skip = read_report_counter(&evaluator, report_id, "skipCount");
        grand_pass += pass;
        grand_fail += fail;
        grand_error += error;
        grand_skip += skip;
        eprintln!(
            "[{pkg}] pass={pass} fail={fail} error={error} skip={skip} total={}",
            pass + fail + error + skip
        );
    }
    eprintln!(
        "\n=== PCT GRAND TOTAL ===\n  pass={grand_pass} fail={grand_fail} error={grand_error} skip={grand_skip} total={}",
        grand_pass + grand_fail + grand_error + grand_skip
    );
}

/// Harvest every distinct `Function not found: FQN` signalled by an
/// ERROR-bucket result across all PCT packages. Mirrors
/// `eval_surveyor_missing_natives_harvest` but rooted on the broader PCT
/// tree — surfaces gaps the `<<test.Test>>` surveyor can't reveal because
/// PCT tests exercise function *implementations* directly.
#[test]
#[ignore = "diagnostic: list missing-native FQNs across all PCT packages"]
fn eval_pct_missing_natives_harvest() {
    use std::collections::{BTreeMap, BTreeSet};

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut by_simple: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for pkg in PCT_BROAD_CANARY_PACKAGES {
        let mut evaluator = Evaluator::new(&model, &registry);
        let pkg_val = match evaluator.call(
            "meta::pure::functions::meta::pathToElement",
            &[Value::String(SmolStr::new(pkg)), Value::String("::".into())],
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
        let Ok(Value::Object(report_id)) = evaluator.call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg_val, Value::String("".into()), adapter, exclusions],
        ) else {
            continue;
        };
        let results = evaluator
            .heap()
            .get_property_values(report_id, "results")
            .unwrap_or_else(|_| im_rc::Vector::new());
        for val in results.iter() {
            let Value::Object(res_id) = val else {
                continue;
            };
            let status = evaluator
                .heap()
                .get_property_values(*res_id, "status")
                .ok()
                .and_then(|v| v.iter().next().cloned());
            if !matches!(&status, Some(Value::EnumValue { member, .. }) if member.as_str() == "ERROR")
            {
                continue;
            }
            let msg = evaluator
                .heap()
                .get_property_values(*res_id, "message")
                .ok()
                .and_then(|v| v.iter().next().cloned())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            for line in msg.lines() {
                let trimmed = line.trim_matches('"').trim();
                if let Some(rest) = trimmed.strip_prefix("Function not found: ") {
                    let fqn = rest.trim().trim_end_matches('"').to_string();
                    let simple = fqn.split('_').next().unwrap_or(&fqn).to_string();
                    by_simple.entry(simple).or_default().insert(fqn);
                }
            }
        }
    }

    eprintln!(
        "\n=== PCT missing-native harvest: {} distinct simple names, {} distinct mangled FQNs ===",
        by_simple.len(),
        by_simple.values().map(BTreeSet::len).sum::<usize>()
    );
    for (simple, fqns) in &by_simple {
        eprintln!("\n[{}] {} ({} variants)", fqns.len(), simple, fqns.len());
        for fqn in fqns {
            eprintln!("    {fqn}");
        }
    }
}

#[test]
fn find_pct_adapter_resolves_in_memory() {
    // Phase 6 contract: `find_pct_adapter(model, "In-Memory")` discovers the
    // shipped `meta::pure::test::pct::testAdapterForInMemoryExecution`
    // function by walking the model for `<<PCT.adapter>>`-stereotyped
    // functions whose `PCT.adapterName` tag matches.
    use legend_pure_runtime::native::testing::find_pct_adapter;

    let model = compile_with_platform("");
    let id = find_pct_adapter(&model, "In-Memory").expect("In-Memory adapter must resolve");
    let node = model.get_node(id);
    assert_eq!(
        node.name.as_str(),
        "testAdapterForInMemoryExecution_Function_1__X_o_",
    );
}

#[test]
fn find_pct_adapter_unknown_name_returns_none() {
    use legend_pure_runtime::native::testing::find_pct_adapter;
    let model = compile_with_platform("");
    assert!(find_pct_adapter(&model, "DefinitelyNotARealAdapterName").is_none());
}

#[test]
fn eval_pct_load_manifest_essential_resolves() {
    // Phase 2 contract: `loadPCTManifest('pct_essential_native.json')` must
    // resolve the embedded platform manifest, parse it, and build a
    // PCTManifest heap object carrying the resolved adapter Function and an
    // empty exclusions Map. Invoke via a thin Pure wrapper because
    // `Evaluator::call` routes through `call_user_function`, which evaluates
    // the (empty) body of `native function` declarations and silently
    // returns `Unit`. A Pure-level call site lets Pass 2.1's name mangling
    // reach the registered native via the eval_function_call dispatch chain.
    let model = compile_with_platform(
        r"
        import meta::pure::test::pct::*;

        function test::loadEssential(): PCTManifest[1] {
            loadPCTManifest('pct_essential_native.json');
        }
        ",
    );
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), "loadEssential__PCTManifest_1_".into()])
        .expect("test::loadEssential resolves");
    let report = evaluator
        .call_user_function_by_id(fn_id)
        .expect("loadEssential must succeed");
    let Value::Object(manifest_id) = report else {
        panic!("loadPCTManifest wrapper returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    assert_eq!(
        heap.classifier(manifest_id).unwrap(),
        "meta::pure::test::pct::PCTManifest"
    );

    let adapter = heap
        .get_property_values(manifest_id, "adapter")
        .expect("adapter slot")
        .iter()
        .next()
        .cloned()
        .expect("adapter populated");
    let Value::Element(adapter_id) = adapter else {
        panic!("adapter slot not Element: {adapter:?}");
    };
    let adapter_node = model.get_node(adapter_id);
    assert_eq!(
        adapter_node.name.as_str(),
        "testAdapterForInMemoryExecution_Function_1__X_o_",
        "adapter resolves to in-memory adapter"
    );

    let exclusions = heap
        .get_property_values(manifest_id, "exclusions")
        .expect("exclusions slot")
        .iter()
        .next()
        .cloned()
        .expect("exclusions populated");
    let Value::Map(state) = exclusions else {
        panic!("exclusions slot not Map: {exclusions:?}");
    };
    assert!(
        state.borrow().entries.is_empty(),
        "shipped pct_essential_native.json has empty exclusions"
    );
}

#[test]
fn eval_pct_run_from_path_essential_manifest() {
    // End-to-end: the developer-facing entry point. Mirrors what
    // `legend test --pct` will invoke.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTestsFromPath",
            &[
                Value::String("meta::pure::functions::boolean::tests::conjunctions::not".into()),
                Value::String("".into()),
                Value::String("pct_essential_native.json".into()),
            ],
        )
        .expect("runPCTTestsFromPath must succeed");

    let Value::Object(report_id) = report else {
        panic!("runPCTTestsFromPath returned non-object: {report:?}");
    };
    let pass = read_report_counter(&evaluator, report_id, "passCount");
    let total = pass
        + read_report_counter(&evaluator, report_id, "failCount")
        + read_report_counter(&evaluator, report_id, "errorCount")
        + read_report_counter(&evaluator, report_id, "skipCount");
    assert!(total > 0, "expected at least one PCT test discovered");
    assert!(pass > 0, "expected at least one PASS via the manifest path");
}

/// Per-package PCT ERROR histogram — clusters error messages by their
/// first useful line so a long tail of similar failures collapses to one
/// bucket. Mirrors the `eval_surveyor_*_error_histogram` shape.
fn pct_error_histogram(package: &str) {
    use std::collections::BTreeMap;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let pkg_val = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String(SmolStr::new(package)),
                Value::String("::".into()),
            ],
        )
        .expect("pathToElement");
    let (adapter, exclusions) = pct_canary_args(&model);
    let Ok(Value::Object(report_id)) = evaluator.call(
        "meta::pure::test::surveyor::runPCTTests",
        &[pkg_val, Value::String("".into()), adapter, exclusions],
    ) else {
        eprintln!("[{package}] runPCTTests failed");
        return;
    };

    let results = evaluator
        .heap()
        .get_property_values(report_id, "results")
        .unwrap_or_else(|_| im_rc::Vector::new());
    let mut histogram: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut outcome_count = 0usize;
    for val in results.iter() {
        let Value::Object(res_id) = val else { continue };
        let status = evaluator
            .heap()
            .get_property_values(*res_id, "status")
            .ok()
            .and_then(|v| v.iter().next().cloned());
        if !matches!(&status, Some(Value::EnumValue { member, .. }) if member.as_str() == "ERROR") {
            continue;
        }
        outcome_count += 1;
        let fqn = evaluator
            .heap()
            .get_property_values(*res_id, "fqn")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let msg = evaluator
            .heap()
            .get_property_values(*res_id, "message")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let bucket = msg
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("<empty error message>")
            .trim()
            .to_string();
        histogram
            .entry(bucket)
            .or_default()
            .push(format!("{fqn} || {msg}"));
    }

    eprintln!("\n=== {package} PCT ERROR histogram ({outcome_count} results) ===");
    let mut rows: Vec<_> = histogram.iter().collect();
    rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (bucket, tests) in rows {
        eprintln!("\n[{}] {}", tests.len(), bucket);
        for (i, t) in tests.iter().enumerate().take(20) {
            eprintln!("    {}. {}", i + 1, t);
        }
        if tests.len() > 20 {
            eprintln!("    ... and {} more", tests.len() - 20);
        }
    }
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for boolean package"]
fn eval_pct_boolean_error_histogram() {
    pct_error_histogram("meta::pure::functions::boolean");
}

#[test]
fn eval_year_native_directly() {
    // year(%2015) directly — no arrow, no eval/lambda — should return 2015.
    let r = eval_pure(
        "function test::f(): Integer[1] { %2015->year(); }",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_minute_precision_date_isolated() {
    // The line that fails inside testYear: minute-precision datetime
    // followed by ->year(). Standalone test isolates the issue from
    // the multi-statement body to make sure it's not state pollution.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17:09->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_three_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_four_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
#[ignore = "diagnostic: print exact error messages for the BigNumber adjust tests"]
fn eval_pct_bignumber_messages() {
    // Run each BigNumber adjust test and print its error message so we
    // can pin the exclusion-list strings.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let testnames = [
        "testAdjustByMonthsBigNumber",
        "testAdjustByWeeksBigNumber",
        "testAdjustByDaysBigNumber",
        "testAdjustByHoursBigNumber",
        "testAdjustByMinutesBigNumber",
    ];
    for tn in &testnames {
        let mut evaluator = Evaluator::new(&model, &registry);
        let pkg = match evaluator.call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String("meta::pure::functions::date::tests".into()),
                Value::String("::".into()),
            ],
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (adapter, exclusions) = pct_canary_args(&model);
        let report = match evaluator.call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg, Value::String("".into()), adapter, exclusions],
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Value::Object(report_id) = report else { continue };
        let results = evaluator
            .heap()
            .get_property_values(report_id, "results")
            .unwrap_or_else(|_| im_rc::Vector::new());
        for v in results.iter() {
            let Value::Object(rid) = v else { continue };
            let fqn = evaluator
                .heap()
                .get_property_values(*rid, "fqn")
                .ok()
                .and_then(|v| v.iter().next().cloned())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            if !fqn.contains(tn) {
                continue;
            }
            let msg = evaluator
                .heap()
                .get_property_values(*rid, "message")
                .ok()
                .and_then(|v| v.iter().next().cloned())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            // Strip the source-location prefix "Execution error\n" and
            // the trailing "\nFull Stack: …" — leaves just the inner
            // PureRuntimeError::Display payload that exclusion-message
            // matching compares against.
            let core = msg
                .lines()
                .nth(1)
                .map(|l| l.trim_matches('"').to_string())
                .unwrap_or_default();
            eprintln!("\n=== {fqn} ===\nFULL: {msg}\nCORE: {core}");
        }
    }
}

#[test]
fn eval_year_hour_precision_no_let() {
    // T17 hour-precision but no `let adapter` binding — call the adapter
    // by FQN inline.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            testAdapterForInMemoryExecution_Function_1__X_o_->eval(|%2015-04-15T17->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_hour_precision_direct() {
    // Just `year(T17 date)` — no eval/lambda/adapter at all.
    let r = eval_pure(
        "function test::f(): Integer[1] { %2015-04-15T17->year(); }",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_hour_precision_no_assert() {
    // Hour-only T17 precision, but no assertEquals wrapper. If this
    // passes, the issue is the assertEquals + T17 combo. If it fails,
    // the issue is the T17 literal itself in a let-bound context.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_minute_precision_with_assert() {
    // Same minute-precision date but wrapped in assertEquals. Tests if
    // it's the assertEquals wrapper or the hour-only precision.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_hour_precision_with_let_only() {
    // Just the let + hour-precision year call. No subsequent statements.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_hour_precision_first() {
    // Reorder to put hour-precision FIRST. If the issue is sequence-
    // dependent compiler inference, this would isolate it.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_five_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_pair_two_precisions() {
    // Try just two consecutive assertEquals — see if state pollution
    // appears already at this size.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_datediff_weeks_sat_to_sun_eq_1() {
    let r = eval_pure(
        r"
        function test::f(): Integer[1] {
            %2015-07-04->dateDiff(%2015-07-05, DurationUnit.WEEKS);
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(1));
}

#[test]
#[ignore = "diagnostic: testYear's full body fails on a multi-precision date — Phase 6 follow-up"]
fn eval_year_full_test_body() {
    // Inline the full testYear body (sans the PCT.test annotation
    // round-trip) to trace whether the multi-precision date inputs
    // matter.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09:21->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09:21.398->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_via_pct_adapter_lambda() {
    // Exact PCT shape: lambda-of-lambda where inner uses ->year() on a date.
    // testYear pattern: assertEquals(2015, $f->eval(|%2015->year()));
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

/// Dump every non-PASS test in `package` with its FQN + first line of
/// its message. Reads through `pct_canary_args_with_rust_exclusions`
/// so excluded tests show as PASS (i.e., excluded from the dump). Used
/// by Phase 7+ to triage FAIL/ERROR clusters package-by-package.
fn pct_status_dump(package: &str) {
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);
    let pkg = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String(SmolStr::new(package)),
                Value::String("::".into()),
            ],
        )
        .expect("pathToElement");
    let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
    let Ok(Value::Object(report_id)) = evaluator.call(
        "meta::pure::test::surveyor::runPCTTests",
        &[pkg, Value::String("".into()), adapter, exclusions],
    ) else {
        eprintln!("[{package}] runPCTTests failed");
        return;
    };
    let results = evaluator
        .heap()
        .get_property_values(report_id, "results")
        .unwrap_or_else(|_| im_rc::Vector::new());
    eprintln!("\n=== {package} non-PASS dump ===");
    for v in results.iter() {
        let Value::Object(rid) = v else { continue };
        let status = evaluator
            .heap()
            .get_property_values(*rid, "status")
            .ok()
            .and_then(|v| v.iter().next().cloned());
        let bucket = match &status {
            Some(Value::EnumValue { member, .. }) if member.as_str() == "PASS" => continue,
            Some(Value::EnumValue { member, .. }) => member.to_string(),
            _ => "?".into(),
        };
        let fqn = evaluator
            .heap()
            .get_property_values(*rid, "fqn")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let msg = evaluator
            .heap()
            .get_property_values(*rid, "message")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let summary = msg
            .lines()
            .nth(1)
            .map(|l| l.trim_matches('"').to_string())
            .unwrap_or_else(|| msg.lines().next().unwrap_or("").to_string());
        eprintln!("[{bucket}] {fqn}\n  {summary}");
    }
}

#[test]
#[ignore = "diagnostic: PCT non-PASS dump for string package"]
fn eval_pct_string_status_dump() {
    pct_status_dump("meta::pure::functions::string");
}

#[test]
#[ignore = "diagnostic: PCT non-PASS dump for math package"]
fn eval_pct_math_status_dump() {
    pct_status_dump("meta::pure::functions::math");
}

#[test]
#[ignore = "diagnostic: PCT non-PASS dump for boolean package"]
fn eval_pct_boolean_status_dump() {
    pct_status_dump("meta::pure::functions::boolean");
}

#[test]
#[ignore = "diagnostic: PCT non-PASS dump for collection package"]
fn eval_pct_collection_status_dump() {
    pct_status_dump("meta::pure::functions::collection");
}

#[test]
#[ignore = "diagnostic: PCT non-PASS dump for lang package"]
fn eval_pct_lang_status_dump() {
    pct_status_dump("meta::pure::functions::lang");
}

#[test]
#[ignore = "diagnostic: list all date tests' status + message"]
fn eval_pct_date_status_dump() {
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);
    let pkg = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String("meta::pure::functions::date::tests".into()),
                Value::String("::".into()),
            ],
        )
        .expect("pkg");
    let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
    let Ok(Value::Object(report_id)) = evaluator.call(
        "meta::pure::test::surveyor::runPCTTests",
        &[pkg, Value::String("".into()), adapter, exclusions],
    ) else { return };
    let results = evaluator.heap().get_property_values(report_id, "results").unwrap_or_else(|_| im_rc::Vector::new());
    for v in results.iter() {
        let Value::Object(rid) = v else { continue };
        let status = evaluator.heap().get_property_values(*rid, "status").ok().and_then(|v| v.iter().next().cloned());
        let bucket = match &status {
            Some(Value::EnumValue { member, .. }) if member.as_str() == "PASS" => continue,
            Some(Value::EnumValue { member, .. }) => member.to_string(),
            _ => "?".into(),
        };
        let fqn = evaluator.heap().get_property_values(*rid, "fqn").ok().and_then(|v| v.iter().next().cloned()).and_then(|v| match v { Value::String(s) => Some(s.to_string()), _ => None }).unwrap_or_default();
        let msg = evaluator.heap().get_property_values(*rid, "message").ok().and_then(|v| v.iter().next().cloned()).and_then(|v| match v { Value::String(s) => Some(s.to_string()), _ => None }).unwrap_or_default();
        eprintln!("\n[{bucket}] {fqn}\n  {msg}");
    }
}

#[test]
#[ignore = "diagnostic: probe a single date test"]
fn eval_pct_date_probe() {
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    for testname in &[
        "meta::pure::functions::date::tests::testYear",
        "meta::pure::functions::date::tests::testHour",
        "meta::pure::functions::date::tests::testHasMinute",
        "meta::pure::functions::date::tests::testDateFromHour",
        "meta::pure::functions::date::tests::testDatePartYearOnly",
    ] {
        let mut evaluator = Evaluator::new(&model, &registry);
        let pkg = evaluator
            .call(
                "meta::pure::functions::meta::pathToElement",
                &[
                    Value::String(SmolStr::new(*testname)),
                    Value::String("::".into()),
                ],
            );
        let pkg = match pkg { Ok(v) => v, Err(_) => { eprintln!("not found: {testname}"); continue; } };
        let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
        // testname is the test fn itself, not a package; use it as adapter input
        let _ = pkg;
        let _ = adapter;
        let _ = exclusions;
        // Use the parent path
        let parent = testname.rsplit_once("::").map(|(p, _)| p).unwrap_or("");
        let pkg2 = match evaluator.call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String(SmolStr::new(parent)),
                Value::String("::".into()),
            ],
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
        let report = match evaluator.call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg2, Value::String("".into()), adapter, exclusions],
        ) {
            Ok(v) => v,
            Err(e) => { eprintln!("{testname}: surveyor failed: {e}"); continue; }
        };
        let Value::Object(report_id) = report else { continue };
        let results = evaluator.heap().get_property_values(report_id, "results").unwrap_or_else(|_| im_rc::Vector::new());
        for v in results.iter() {
            let Value::Object(rid) = v else { continue };
            let fqn = evaluator.heap().get_property_values(*rid, "fqn").ok().and_then(|v| v.iter().next().cloned()).and_then(|v| match v { Value::String(s) => Some(s.to_string()), _ => None }).unwrap_or_default();
            if !fqn.contains(testname.split("::").last().unwrap()) { continue; }
            let msg = evaluator.heap().get_property_values(*rid, "message").ok().and_then(|v| v.iter().next().cloned()).and_then(|v| match v { Value::String(s) => Some(s.to_string()), _ => None }).unwrap_or_default();
            eprintln!("\n=== {testname} ===\n{msg}");
        }
    }
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for math package"]
fn eval_pct_math_error_histogram() {
    pct_error_histogram("meta::pure::functions::math");
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for string package"]
fn eval_pct_string_error_histogram() {
    pct_error_histogram("meta::pure::functions::string");
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for collection package"]
fn eval_pct_collection_error_histogram() {
    pct_error_histogram("meta::pure::functions::collection");
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for lang package"]
fn eval_pct_lang_error_histogram() {
    pct_error_histogram("meta::pure::functions::lang");
}

#[test]
#[ignore = "diagnostic: surveyor failure histogram for multiplicity package"]
fn eval_surveyor_multiplicity_error_histogram() {
    surveyor_error_histogram("meta::pure::functions::multiplicity");
}

#[test]
#[ignore = "diagnostic: PCT ERROR histogram for date package"]
fn eval_pct_date_error_histogram() {
    pct_error_histogram("meta::pure::functions::date");
}

// ===========================================================================
// PCT monotonic baseline lock
// ===========================================================================

/// Minimum PCT pass count across `PCT_BROAD_CANARY_PACKAGES`. Bump this
/// after each phase commit confirms a higher pass count via
/// `eval_pct_broad_canary`. A tactical fix that flips one new test PASS
/// while breaking three previously-passing tests fails this lock —
/// cheapest possible defense against silent regressions in a 1000+ line
/// diff. The "no tactical test-pass hacks" policy in CLAUDE.md is the
/// principle; this is the enforcement.
///
/// Update protocol:
/// 1. Land the fix.
/// 2. Run `cargo test --test eval_tests eval_pct_broad_canary -- --ignored
///    --nocapture` and read the new GRAND TOTAL pass count.
/// 3. Bump this constant to the new value (or floor of it if the fix is
///    expected to gain over time).
///
/// History (oldest first):
/// - 333 — initial baseline at PCT harness shipping (commit f8ebca6263c)
/// - 348 — Phase 1: generic `compare` native cleared 15 tests across
///   boolean (+4), collection (+4), lang (+7)
/// - 368 — Phase 2: tail / zip / values / split / parseDecimal natives
///   cleared 20 tests across collection (+15) and string (+5)
/// - 379 — Phase 3: round/2 (+ banker's rounding) / substring/2 /
///   indexOf/3 / add/3 / range/2 overloads cleared 11 tests across
///   collection (+4), math (+4 inc 2 FAIL→PASS via half-even), string (+3)
/// - 388 — Phase 4: numeric coercion via promote_pair (Decimal+Float
///   promotion) cleared 9 math tests (rem with mixed types)
/// - 395 — Phase 5: multiplicity-aware Match + 3-arg overload cleared
///   7 lang tests (match-pattern with empty / multi-element subjects
///   against [0..1] / [*] / [1..*] params)
/// - 398 — Phase 5b: Sort routes through compare_values (cross-type
///   sort works), at error message matches Java exactly. Cleared 3
///   collection tests; surveyor `asserts` package went 20/4/1 → 25/0/0.
/// - 417 — Phase 6 (part 1): date subsystem. `Adjust` now accepts
///   `Value::EnumValue` for the DurationUnit arg (was string-only),
///   adds Milliseconds/Microseconds/Nanoseconds support via new
///   `PureDate::add_*` methods, and routes through jiff's `try_*`
///   span builders to error gracefully on out-of-range adjustments
///   instead of panicking. Cleared 19 date PCT tests.
/// - 434 — Phase 6 (part 2): hour-only datetime literals (`%2015-04-15T17`)
///   now parse + carry `TimePrecision::Hour` (was rejected by
///   `parse_datetime`'s `time_parts.len() < 2` gate), Rust-port
///   exclusion list seeded for the 5 BigNumber adjust tests that
///   expect years outside i16 range, and `apply_exclusion` matches
///   exclusion messages by substring (so entries can pin just the
///   PureRuntimeError text without the full Display wrapper).
///   Cleared 17 date PCT tests; date package now 47/1/5.
/// - 438 — Phase 6 (part 3): `datePart` passes through year- and
///   month-only dates unchanged (Java parity per the platform
///   comment); `dateDiff(YEARS|MONTHS)` uses calendar-component math
///   (`b.year - a.year`, `(b.year - a.year)*12 + (b.month - a.month)`)
///   instead of jiff's `until` (which measures elapsed time, not
///   calendar deltas); `dateDiff(WEEKS)` counts Sunday-boundary
///   crossings with direction-asymmetric half-open intervals
///   (forward `(a, b]` vs backward `[b, a)`); `to_civil_date` no
///   longer requires day precision (year/month-only dates default to
///   month=1/day=1, well-formed for week/year arithmetic). date
///   package now 51/0/2 — only TZ + sub-second datetime literal
///   parsing edge cases remain.
/// - 460 — Phase 7 (part 1): string subsystem.
///   parseBoolean is now case-insensitive ("True", "TRUE", … all
///   accepted, Java parity). toString routes through a dedicated
///   `pure_to_string` renderer instead of `Value::Display`: strings
///   unquoted, dates without leading `%`, Pair → `<a, b>`, List →
///   `[values…]` (recursive), Class/Element → simple-leaf name, enum
///   value → bare member. format gains zero-pad width (`%05d`),
///   precision (`%.4f` rounds half-to-even), date specifier (`%t`),
///   date-with-pattern (`%t{yyyy-MM-dd HH:mm:ss}` covering
///   yyyy/MM/dd/HH/hh/h/mm/ss/SSS/a/Z/X plus quoted literals and
///   [TZ] prefix), and `%r` repr now escapes backslash + single
///   quote per Pure source rules. Cleared 23 string PCT tests.
/// - 2026-04-26 → 461: `pure_to_string` now dispatches heap objects
///   through their class's `toString()` qualified property (with
///   generalization walk), mirroring Java
///   `ToString.findBestToStringFunction`. Removed the hardcoded
///   `Pair`/`List` classifier-string match arms — those types format
///   identically because their platform `.pure` `toString()` QPs run
///   under the new dispatch path. Drops the combined fail+error
///   bucket from 78 to 36 (most of which were assertEq mismatches
///   cascading off the broken object formatter).
/// - 2026-04-26 → 466: Float equality now mirrors Java
///   `CompiledSupport.eq` (normalizes `-0.0`→`0.0`, NaN==NaN);
///   `parseDate` rewritten to accept the same lenient ISO shapes the
///   `%`-literal lowering recognises (single-digit month/day, `Z`
///   suffix, `±HHMM` offset). +1 testParseZero (parseFloat) and the
///   3 parseDate tests; net +5 PASS, -2 FAIL, -3 ERROR.
/// - 2026-04-26 → 467: `parseDecimal(string, precision, scale)`
///   3-arg form was reading `values[1]` (precision) as the rounding
///   scale — off-by-one in argument indexing. Fixed to use
///   `values[2]` (scale). Resolves
///   `testParseDecimalWithPrecisionScale`.
/// - 2026-04-26 → 475: `binary_op` (the comparison/logical operator
///   lowering) now routes through `resolve_function_call` so the
///   compiler's type-based overload narrowing picks the right
///   `lessThan(Date,Date)` / `lessThan(Boolean,Boolean)` /
///   `lessThan(String,String)` etc. platform overload instead of
///   falling through the runtime's prefix-name fallback (which
///   always picked the Number native and errored on non-Number
///   operands). Mirrors `variadic_op`. Resolves the entire
///   boolean-inequality cluster: testGreaterThan_Date,
///   testGreaterThanEqual_Date, testLessThan_Date,
///   testLessThanEqual_Date plus the same four for Boolean
///   operands. Net +8 PASS, error count 13 → 5.
/// - 2026-04-26 → 481: error-message-pinning fixes. `sqrt`/`asin`/
///   `acos` now throw "Unable to compute X of N" on out-of-domain
///   inputs (Java's `Sqrt.java:53`/`ArcSine.java:53`/
///   `ArcCosine.java:53` semantics — Java throws when the IEEE-754
///   result is NaN). `range` step error message changed from
///   "range: step cannot be zero" to "range step must not be 0".
///   `rem` divide-by-zero now formats as "Cannot divide N by zero"
///   (parametric on dividend, mirroring `Rem.java:?`). `slice`
///   throws "The low bound (X) can't be higher than the high bound
///   (Y) in a slice operation" instead of returning empty. New
///   `java_number_string` helper in `native::math` renders integer-
///   valued doubles as "2.0" (matching Java's `Double.toString`).
///   Resolves testSquareRootError, testArcSineError,
///   testArcCosineError, testRangeStepError, testRemError,
///   testSliceError. Net +6 PASS.
/// - 2026-04-26 → **448 (reset)**: branch was rebased onto upstream
///   `legend-pure-rust` which refactored the platform `.pure`
///   surveyor / PCT-test definitions. Total discoverable tests in
///   the canary's six packages went 497 → 465; the absolute baseline
///   numbers above describe deltas against a no-longer-current
///   discovery set. Re-anchored at the current pass count of 445
///   (pre-manifest) + 3 (testLarge{Times,Minus,Plus} exclusions
///   loaded from `crates/runtime/tests/pct_rust_port.json`) = 448.
///   Future phases ratchet this baseline up against this new total.
const PCT_PASS_BASELINE: i64 = 448;

/// Minimum `<<test.Test>>` surveyor pass count across the same packages
/// as [`PCT_BROAD_CANARY_PACKAGES`]. The PCT lock catches regressions in
/// PCT-specific dispatch; this companion lock catches regressions in the
/// surveyor path that runs against the metamodel reflection / lang /
/// collection / etc. tests Java exercises via `<<test.Test>>`.
///
/// Update protocol: same as `PCT_PASS_BASELINE` — bump after a phase
/// commit confirms the new pass count via `eval_surveyor_broad_canary`.
///
/// History:
/// - 211 — initial 100% pass rate at PCT harness shipping (commit
///   f8ebca6263c). Held through Phase 1–5 across the 7-package canary.
/// - 233 — Phase 5b initial: expanded lock to include `asserts`
///   (20 PASS), `multiplicity` (1 PASS), `relation` (1 PASS). The CLI
///   default `legend test --package Root` walks all of these, so locking
///   only the 7-package subset was hiding 22 additional surveyor passes
///   from regression detection.
/// - 238 — Phase 5b complete: asserts cleared via Java-parity `at`
///   error message + Sort routing through compare_values. asserts
///   package went 20/4/1 → 25/0/0; no regressions elsewhere.
/// - 243 — Phase 5c: Multiplicity-constant property shim (PureZero /
///   PureOne / ZeroOne / ZeroMany / OneMany expose `upperBound` /
///   `lowerBound` slots as `MultiplicityValue` heap objects with
///   `.value` populated) + `toOneMany` native + 2-arg toOne overload.
///   multiplicity package went 1/2/6 → 6/0/3.
/// - 244 — Phase 5d: classifierGenericType shim on Function elements
///   (synthesises GenericType→FunctionType chain so `someFn->functionType()`
///   reflection works) + multiplicity slot populated on deactivated
///   FunctionCall AST nodes from declared return_multiplicity. Cleared
///   testHasUpperBoundNonConcrete.
/// - 246 — Phase 5d-3: `evaluateAndDeactivate` populates `.multiplicity`
///   on the deactivated LambdaFunction wrapper from the last body
///   expression's declared return_multiplicity (Pure semantics: a
///   lambda's return mult is the last expression's mult). Cleared
///   testToOneMultiplicity + testToOneManyMultiplicity. Surveyor is
///   now 246/0/0 — 100% pass on `<<test.Test>>` across every
///   `meta::pure::functions::*` package the CLI walks.
const SURVEYOR_PASS_BASELINE: i64 = 246;

#[test]
fn eval_pct_baseline_lock() {
    // Active regression lock — runs as part of the default `cargo test`
    // sweep, no `#[ignore]`. Walks the same package list as
    // `eval_pct_broad_canary` and asserts the cumulative pass count is
    // at least PCT_PASS_BASELINE.
    //
    // Cost: ~5–10 seconds because it executes every PCT test in every
    // package on every CI run. That's the price of preventing silent
    // pass-count regressions; the alternative is a tactical hack that
    // games one test and goes unnoticed for weeks.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();

    let mut grand_pass = 0i64;
    let mut per_pkg = Vec::new();
    for pkg in PCT_BROAD_CANARY_PACKAGES {
        let mut evaluator = Evaluator::new(&model, &registry);
        let pkg_val = match evaluator.call(
            "meta::pure::functions::meta::pathToElement",
            &[Value::String(SmolStr::new(pkg)), Value::String("::".into())],
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
        let Ok(Value::Object(report_id)) = evaluator.call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg_val, Value::String("".into()), adapter, exclusions],
        ) else {
            continue;
        };
        let pass = read_report_counter(&evaluator, report_id, "passCount");
        grand_pass += pass;
        per_pkg.push((*pkg, pass));
    }

    assert!(
        grand_pass >= PCT_PASS_BASELINE,
        "PCT pass count regressed: got {grand_pass}, baseline is {PCT_PASS_BASELINE}.\n\
         Per-package: {per_pkg:#?}\n\
         Either you regressed a previously-passing test (fix it) or you legitimately \
         dropped support (lower the baseline with a comment explaining why)."
    );
}

/// Surveyor packages — covers every `<<test.Test>>`-bearing package
/// under `meta::pure::functions::*`. Mirrors what `legend test` (the
/// CLI default) walks via `--package Root`, so the lock catches any
/// regression a user would see at the CLI.
///
/// The seven core packages are rooted at `::tests` (where their
/// `<<test.Test>>` functions live). `asserts` is the exception: some
/// of its tests are declared directly under `asserts::*` (alongside
/// the assertion natives they test) rather than `asserts::tests::*`,
/// so the lock targets the parent package to catch all of them. The
/// CLI's `--package Root` walk picks them all up uniformly.
const SURVEYOR_BROAD_CANARY_PACKAGES: &[&str] = &[
    "meta::pure::functions::meta::tests",
    "meta::pure::functions::collection::tests",
    "meta::pure::functions::string::tests",
    "meta::pure::functions::math::tests",
    "meta::pure::functions::date::tests",
    "meta::pure::functions::boolean::tests",
    "meta::pure::functions::lang::tests",
    "meta::pure::functions::asserts",
    "meta::pure::functions::multiplicity",
    "meta::pure::functions::relation",
];

#[test]
fn eval_surveyor_baseline_lock() {
    // Companion to eval_pct_baseline_lock — guards <<test.Test>> coverage
    // (the metamodel reflection / lang / collection tests Java exercises
    // via the surveyor's runTestsFromPath path). Phase 1–5 fixes touched
    // dispatch, comparison, arithmetic, and match-pattern code that the
    // surveyor also walks; this lock ensures none of those touches
    // regressed a previously-passing surveyor test.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();

    let mut grand_pass = 0i64;
    let mut per_pkg = Vec::new();
    for pkg in SURVEYOR_BROAD_CANARY_PACKAGES {
        let mut evaluator = Evaluator::new(&model, &registry);
        let Ok(Value::Object(report_id)) = evaluator.call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[Value::String(SmolStr::new(pkg)), Value::String("".into())],
        ) else {
            continue;
        };
        let pass = read_report_counter(&evaluator, report_id, "passCount");
        grand_pass += pass;
        per_pkg.push((*pkg, pass));
    }

    assert!(
        grand_pass >= SURVEYOR_PASS_BASELINE,
        "Surveyor (<<test.Test>>) pass count regressed: got {grand_pass}, \
         baseline is {SURVEYOR_PASS_BASELINE}.\n\
         Per-package: {per_pkg:#?}\n\
         Either you regressed a previously-passing test (fix it) or you legitimately \
         dropped support (lower the baseline with a comment explaining why)."
    );
}
