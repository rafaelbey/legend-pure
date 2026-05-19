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

//! Phase C — milestoning grammar shortcuts.
//!
//! Java's milestoning grammar block lets a class reference desugar to a
//! `getAll*` native call:
//!
//! - `Class.all` / `Class.all()` → `getAll(Class)`
//! - `Class.all($d)` → `getAll(Class, $d)` (single-temporal)
//! - `Class.all($pd, $bd)` → `getAll(Class, $pd, $bd)` (bitemporal)
//! - `Class.allVersions()` → `getAllVersions(Class)`
//! - `Class.allVersionsInRange($s, $e)` → `getAllVersionsInRange(Class, $s, $e)`
//!
//! The Rust port lowers all five at parse-to-IR time in
//! `crates/pure/src/lower/member_access.rs`. These tests exercise each form
//! end-to-end: seed instances, call via the shortcut, assert the count.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

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
    match legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports) {
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

// -------------------------------------------------------------------------
// Class.all(date) — single-date shortcut
// -------------------------------------------------------------------------

#[test]
fn dot_all_single_date_filters_businesstemporal_class() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::countOnDate(): Integer[1]
{
    let c1 = ^test::Customer(name='alice', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='bob', businessDate=%2024-06-15);
    let c3 = ^test::Customer(name='carol', businessDate=%2024-01-01);
    test::Customer.all(%2024-01-01)->size()
}
";
    let result = eval_pure(source, "countOnDate__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 customers on date, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Class.all(pd, bd) — bitemporal shortcut
// -------------------------------------------------------------------------

#[test]
fn dot_all_two_dates_filters_bitemporal_class() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> test::Position
{
    qty: Integer[1];
}

function test::countByBoth(): Integer[1]
{
    let p1 = ^test::Position(qty=1, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p2 = ^test::Position(qty=2, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p3 = ^test::Position(qty=3, processingDate=%2024-02-01, businessDate=%2024-06-01);
    test::Position.all(%2024-01-01, %2024-06-01)->size()
}
";
    let result = eval_pure(source, "countByBoth__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 positions matching both dates, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Class.allVersions() — no filter
// -------------------------------------------------------------------------

#[test]
fn dot_all_versions_returns_every_instance() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::countAll(): Integer[1]
{
    let c1 = ^test::Customer(name='a', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='b', businessDate=%2024-06-15);
    let c3 = ^test::Customer(name='c', businessDate=%2024-12-31);
    test::Customer.allVersions()->size()
}
";
    let result = eval_pure(source, "countAll__Integer_1_");
    assert!(
        matches!(result, Value::Integer(3)),
        "expected 3 versions total, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Class.allVersionsInRange(start, end) — inclusive range
// -------------------------------------------------------------------------

#[test]
fn dot_all_versions_in_range_filters_inclusive() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::countInRange(): Integer[1]
{
    let c1 = ^test::Customer(name='a', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='b', businessDate=%2024-03-15);
    let c3 = ^test::Customer(name='c', businessDate=%2024-06-30);
    let c4 = ^test::Customer(name='d', businessDate=%2024-12-31);
    test::Customer.allVersionsInRange(%2024-02-01, %2024-07-01)->size()
}
";
    let result = eval_pure(source, "countInRange__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 customers in range, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Class.all (no parens) — back-compat with the existing zero-arg path
// -------------------------------------------------------------------------

#[test]
fn dot_all_no_parens_still_works_on_non_milestoned_class() {
    let source = "\
Class test::Plain
{
    n: Integer[1];
}

function test::countAll(): Integer[1]
{
    let p1 = ^test::Plain(n=1);
    let p2 = ^test::Plain(n=2);
    test::Plain.all->size()
}
";
    let result = eval_pure(source, "countAll__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 plain instances, got {result:?}",
    );
}
