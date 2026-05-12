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

//! Stage-4 runtime evaluation smoke tests for navigation paths.
//!
//! Each test compiles a synthetic class fixture against the platform,
//! constructs a path expression, and asserts that calling
//! `evaluate(path, instance)` walks the steps and returns the right
//! value. Mirrors the structural shape of Java's
//! `m2.dsl.path.test.execution.TestDSLExecution`.

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

static USER_DEPS: &[&str] = &[
    "platform",
    "platform_precise_primitives",
    "platform_dsl_store",
    "platform_dsl_mapping",
    "platform_dsl_diagram",
    "platform_dsl_graph",
    "platform_dsl_tds",
    "platform_dsl_path",
    "platform_store_relational",
];

fn synthetic_user_repo(user_source: &str) -> Repo {
    let meta = RepoMeta {
        name: "user_test",
        pattern: ".*",
        dependencies: USER_DEPS,
    };
    Repo::Filesystem {
        prefix: "/user_test".into(),
        files: vec![OwnedSourceFile {
            path: "/user_test/test_source.pure".into(),
            content: user_source.into(),
        }],
        meta: Some(meta),
        source_root: None,
    }
}

fn compile_with_platform(user_source: &str) -> PureModel {
    let mut repos = Repo::default_with_build_snapshots();
    repos.push(synthetic_user_repo(user_source));
    let auto_imports: Vec<smol_str::SmolStr> =
        legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| smol_str::SmolStr::new(s))
            .collect();
    legend_pure_core_platform::repo::load(&repos, &auto_imports).unwrap_or_else(|partial| {
        eprintln!("compile_with_platform: errors:");
        for e in partial.errors.iter().take(5) {
            eprintln!("  - {e}");
        }
        partial.model
    })
}

fn eval_pure(source: &str, fqn: &str) -> Value {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&[smol_str::SmolStr::new("test"), smol_str::SmolStr::new(fqn)])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found"));
    eval.call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("Evaluation error: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn path_evaluates_single_property_access() {
    // Java parity (TestDSLExecution.testSingleProperty):
    // `#/Person/name#->eval(^Person(name='Bob'))` should return `'Bob'`.
    let result = eval_pure(
        r"###Pure
Class test::Person
{
    name: String[1];
}

function test::run(): String[1]
{
    let p = ^test::Person(name='Bob');
    let path = #/test::Person/name#;
    $path->eval($p)
}",
        "run__String_1_",
    );
    match result {
        Value::String(s) => assert_eq!(s.as_str(), "Bob"),
        other => panic!("Expected String('Bob'), got {other:?}"),
    }
}

#[test]
fn path_chains_through_object_graph() {
    // Multi-step path: address.street resolves dynamically against
    // each step's running receiver type.
    let result = eval_pure(
        r"###Pure
Class test::Addr
{
    street: String[1];
}

Class test::Person2
{
    address: test::Addr[1];
}

function test::run2(): String[1]
{
    let a = ^test::Addr(street='Main St');
    let p = ^test::Person2(address=$a);
    #/test::Person2/address/street#->eval($p)
}",
        "run2__String_1_",
    );
    match result {
        Value::String(s) => assert_eq!(s.as_str(), "Main St"),
        other => panic!("Expected String('Main St'), got {other:?}"),
    }
}

#[test]
fn path_value_is_a_first_class_function() {
    // A path stored in a let-binding survives like any other Function
    // value — it can be inspected with `->type()` etc. This mirrors
    // Java's `meta::pure::metamodel::path::Path` extending Function.
    // Here we just call it via `eval` indirectly through a let.
    let result = eval_pure(
        r"###Pure
Class test::Person3
{
    age: Integer[1];
}

function test::run3(): Integer[1]
{
    let p = ^test::Person3(age=42);
    let pathFn = #/test::Person3/age#;
    $pathFn->eval($p)
}",
        "run3__Integer_1_",
    );
    match result {
        Value::Integer(n) => assert_eq!(n, 42),
        other => panic!("Expected Integer(42), got {other:?}"),
    }
}
