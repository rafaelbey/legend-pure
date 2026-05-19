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

//! Phase B-1 — milestoning runtime natives smoke tests.
//!
//! Exercises `getAll(Class, Date)`, `getAll(Class, Date, Date)`, and
//! `getAllVersionsInRange(Class, Date, Date)` end-to-end against the real
//! platform: allocate milestoned class instances with explicit date
//! properties, invoke each native via a user-defined Pure function, and
//! assert the count of returned versions.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

// -------------------------------------------------------------------------
// Platform fixture (mirrors eval_tests.rs structure)
// -------------------------------------------------------------------------

struct PlatformFixture {
    repos: Vec<Repo>,
    auto_imports: Vec<SmolStr>,
}

fn platform_model() -> &'static PlatformFixture {
    static FIXTURE: OnceLock<PlatformFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let repos = Repo::default_with_build_snapshots();
        let auto_imports = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect();
        PlatformFixture {
            repos,
            auto_imports,
        }
    })
}

fn synthetic_user_repo(user_source: &str) -> Repo {
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
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    let result = legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports);
    match result {
        Ok(model) => model,
        Err(partial) => {
            for e in partial.errors.iter().take(10) {
                eprintln!("compile error: {e}");
            }
            partial.model
        }
    }
}

fn eval_pure(source: &str, fqn: &str) -> Value {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("function test::{fqn} not found"));
    eval.call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("eval error: {e}"))
}

fn eval_pure_err(source: &str, fqn: &str) -> String {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("function test::{fqn} not found"));
    eval.call_user_function_by_id(fn_id)
        .expect_err("expected eval to fail")
        .to_string()
}

// -------------------------------------------------------------------------
// getAll(Class, Date) — single-date variant
// -------------------------------------------------------------------------

#[test]
fn get_all_with_date_filters_businesstemporal_class() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::seedAndCount(): Integer[1]
{
    let c1 = ^test::Customer(name='alice', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='bob', businessDate=%2024-06-15);
    let c3 = ^test::Customer(name='carol', businessDate=%2024-01-01);
    test::Customer->getAll(%2024-01-01)->size()
}
";
    let result = eval_pure(source, "seedAndCount__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 customers matching %2024-01-01, got {result:?}",
    );
}

#[test]
fn get_all_with_date_filters_processingtemporal_class() {
    let source = "\
Class <<meta::pure::profiles::temporal.processingtemporal>> test::Snapshot
{
    label: String[1];
}

function test::seedAndCount(): Integer[1]
{
    let s1 = ^test::Snapshot(label='a', processingDate=%2024-03-01);
    let s2 = ^test::Snapshot(label='b', processingDate=%2024-03-01);
    let s3 = ^test::Snapshot(label='c', processingDate=%2024-04-01);
    test::Snapshot->getAll(%2024-03-01)->size()
}
";
    let result = eval_pure(source, "seedAndCount__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 snapshots matching %2024-03-01, got {result:?}",
    );
}

#[test]
fn get_all_with_date_on_bitemporal_class_errors() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> test::Position
{
    qty: Integer[1];
}

function test::callTwoArg(): Integer[1]
{
    let p = ^test::Position(qty=1, processingDate=%2024-01-01, businessDate=%2024-01-01);
    test::Position->getAll(%2024-01-01)->size()
}
";
    let err = eval_pure_err(source, "callTwoArg__Integer_1_");
    assert!(
        err.contains("bitemporal") || err.contains("Bitemporal"),
        "expected an error mentioning bitemporal, got: {err}"
    );
}

#[test]
fn get_all_with_date_on_non_milestoned_class_errors() {
    let source = "\
Class test::Plain
{
    n: Integer[1];
}

function test::callTwoArg(): Integer[1]
{
    let p = ^test::Plain(n=1);
    test::Plain->getAll(%2024-01-01)->size()
}
";
    let err = eval_pure_err(source, "callTwoArg__Integer_1_");
    assert!(
        err.contains("not milestoned") || err.contains("Plain"),
        "expected an error about non-milestoned class, got: {err}"
    );
}

// -------------------------------------------------------------------------
// getAll(Class, Date, Date) — bitemporal variant
// -------------------------------------------------------------------------

#[test]
fn get_all_bitemporal_filters_by_both_dates() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> test::Position
{
    qty: Integer[1];
}

function test::seedAndCount(): Integer[1]
{
    let p1 = ^test::Position(qty=1, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p2 = ^test::Position(qty=2, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p3 = ^test::Position(qty=3, processingDate=%2024-01-01, businessDate=%2024-07-01);
    let p4 = ^test::Position(qty=4, processingDate=%2024-02-01, businessDate=%2024-06-01);
    test::Position->getAll(%2024-01-01, %2024-06-01)->size()
}
";
    let result = eval_pure(source, "seedAndCount__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 positions matching both dates, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// getAllVersionsInRange(Class, Date, Date) — range query
// -------------------------------------------------------------------------

#[test]
fn get_all_versions_in_range_inclusive_for_businesstemporal() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::seedAndCount(): Integer[1]
{
    let c1 = ^test::Customer(name='a', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='b', businessDate=%2024-03-15);
    let c3 = ^test::Customer(name='c', businessDate=%2024-06-30);
    let c4 = ^test::Customer(name='d', businessDate=%2024-12-31);
    test::Customer->getAllVersionsInRange(%2024-02-01, %2024-07-01)->size()
}
";
    let result = eval_pure(source, "seedAndCount__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 customers in range, got {result:?}",
    );
}

#[test]
fn get_all_versions_in_range_on_non_milestoned_errors() {
    let source = "\
Class test::Plain
{
    n: Integer[1];
}

function test::callRange(): Integer[1]
{
    let p = ^test::Plain(n=1);
    test::Plain->getAllVersionsInRange(%2024-01-01, %2024-12-31)->size()
}
";
    let err = eval_pure_err(source, "callRange__Integer_1_");
    assert!(
        err.contains("not milestoned") || err.contains("Plain"),
        "expected error on non-milestoned class, got: {err}"
    );
}
