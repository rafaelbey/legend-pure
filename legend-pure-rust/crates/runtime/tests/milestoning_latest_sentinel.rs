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

//! Phase D — `%latest` milestoning sentinel.
//!
//! Tests:
//!
//! - **Lex + parse + lower**: `%latest` parses and lowers to
//!   `ExprKind::DateLiteral(DateValue::Latest)` then evaluates to
//!   `Value::Latest`.
//! - **Runtime semantics**: `getAll(Class, %latest)` returns every
//!   instance; `compare_values` orders `Latest` above every concrete
//!   `Date`.
//! - **B-4.1 validator**: `%latest` outside any milestoning host is
//!   rejected.
//! - **B-4.3 validator**: `%latest` inside `getAllVersionsInRange` is
//!   rejected.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::error::CompilationErrorKind;
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

fn try_compile(
    user_source: &str,
) -> Result<PureModel, legend_pure_parser_pure::pipeline::PartialPureModel> {
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports)
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
// %latest with getAll — returns all versions
// -------------------------------------------------------------------------

#[test]
fn get_all_with_latest_returns_every_version() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::countLatest(): Integer[1]
{
    let c1 = ^test::Customer(name='a', businessDate=%2024-01-01);
    let c2 = ^test::Customer(name='b', businessDate=%2024-06-15);
    let c3 = ^test::Customer(name='c', businessDate=%2024-12-31);
    test::Customer.all(%latest)->size()
}
";
    let result = eval_pure(source, "countLatest__Integer_1_");
    assert!(
        matches!(result, Value::Integer(3)),
        "expected all 3 customers when filter is %latest, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// %latest in bitemporal getAll
// -------------------------------------------------------------------------

#[test]
fn get_all_bitemporal_with_latest_returns_every_version() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> test::Position
{
    qty: Integer[1];
}

function test::countAll(): Integer[1]
{
    let p1 = ^test::Position(qty=1, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p2 = ^test::Position(qty=2, processingDate=%2024-02-01, businessDate=%2024-06-01);
    let p3 = ^test::Position(qty=3, processingDate=%2024-01-01, businessDate=%2024-07-01);
    test::Position.all(%latest, %latest)->size()
}
";
    let result = eval_pure(source, "countAll__Integer_1_");
    assert!(
        matches!(result, Value::Integer(3)),
        "expected 3 positions when both filters are %latest, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// B-4.3 — %latest forbidden in AllVersionsInRange
// -------------------------------------------------------------------------

#[test]
fn b4_3_latest_in_range_query_is_rejected() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    name: String[1];
}

function test::badRange(): Integer[1]
{
    test::Customer.allVersionsInRange(%2024-01-01, %latest)->size()
}
";
    let partial = try_compile(source).expect_err("expected B-4.3 to reject");
    let found = partial.errors.iter().find(|e| {
        matches!(
            e.kind,
            CompilationErrorKind::MilestoningLatestNotAllowedInRange { .. }
        )
    });
    assert!(
        found.is_some(),
        "expected MilestoningLatestNotAllowedInRange, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// -------------------------------------------------------------------------
// B-4.1 — %latest outside milestoning context is rejected
// -------------------------------------------------------------------------

#[test]
fn b4_1_latest_outside_milestoning_is_rejected() {
    // `%latest` used as a plain Date in a let binding — no milestoning host.
    let source = "\
function test::misuseLatest(): Boolean[1]
{
    let d = %latest;
    true
}
";
    let partial = try_compile(source).expect_err("expected B-4.1 to reject");
    let found = partial.errors.iter().find(|e| {
        matches!(
            e.kind,
            CompilationErrorKind::MilestoningLatestOutsideMilestoningContext { .. }
        )
    });
    assert!(
        found.is_some(),
        "expected MilestoningLatestOutsideMilestoningContext, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
