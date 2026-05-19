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

//! Phase B-2 — milestoning QP runtime dispatch.
//!
//! Phase A's synthesis pass emits qualified-property *signatures* on
//! milestoned classes; Phase A leaves the bodies empty. This file
//! exercises the runtime helper in `crate::milestoning` that detects
//! the generated stereotype on dispatch and executes filter-by-date
//! semantics against the edge-point property.
//!
//! Three call shapes covered:
//!   - `$customer.address($date)` — single-date filter, businesstemporal target
//!   - `$customer.address($pd, $bd)` — bitemporal target
//!   - `$customer.addressAllVersionsInRange($start, $end)` — inclusive range

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
// Single-date QP — `address(td)` on a non-milestoned owner with a
// businesstemporal target.
// -------------------------------------------------------------------------

#[test]
fn property_to_businesstemporal_target_filters_by_single_date() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class test::Customer
{
    addresses: test::Address[*];
}

function test::pickByDate(): Integer[1]
{
    let a1 = ^test::Address(line='one', businessDate=%2024-01-01);
    let a2 = ^test::Address(line='two', businessDate=%2024-06-15);
    let a3 = ^test::Address(line='three', businessDate=%2024-06-15);
    // Phase A moved `addresses` to original_milestoned_properties and
    // replaced it with `addressesAllVersions: Address[*]`. Construct
    // using the edge-point name.
    let c = ^test::Customer(addressesAllVersions=[$a1, $a2, $a3]);
    $c.addresses(%2024-06-15)->size()
}
";
    let result = eval_pure(source, "pickByDate__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 addresses matching %2024-06-15, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Range QP — `addressesAllVersionsInRange(start, end)`.
// -------------------------------------------------------------------------

#[test]
fn property_to_businesstemporal_target_range_query() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class test::Customer
{
    addresses: test::Address[*];
}

function test::rangeQuery(): Integer[1]
{
    let a1 = ^test::Address(line='one', businessDate=%2024-01-01);
    let a2 = ^test::Address(line='two', businessDate=%2024-03-15);
    let a3 = ^test::Address(line='three', businessDate=%2024-06-30);
    let a4 = ^test::Address(line='four', businessDate=%2024-12-31);
    let c = ^test::Customer(addressesAllVersions=[$a1, $a2, $a3, $a4]);
    $c.addressesAllVersionsInRange(%2024-02-01, %2024-07-01)->size()
}
";
    let result = eval_pure(source, "rangeQuery__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 addresses in range, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Bitemporal QP — `position(pd, bd)`.
// -------------------------------------------------------------------------

#[test]
fn property_to_bitemporal_target_filters_by_both_dates() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> test::Position
{
    qty: Integer[1];
}

Class test::Book
{
    positions: test::Position[*];
}

function test::pickByBoth(): Integer[1]
{
    let p1 = ^test::Position(qty=1, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p2 = ^test::Position(qty=2, processingDate=%2024-01-01, businessDate=%2024-06-01);
    let p3 = ^test::Position(qty=3, processingDate=%2024-01-01, businessDate=%2024-07-01);
    let p4 = ^test::Position(qty=4, processingDate=%2024-02-01, businessDate=%2024-06-01);
    let b = ^test::Book(positionsAllVersions=[$p1, $p2, $p3, $p4]);
    $b.positions(%2024-01-01, %2024-06-01)->size()
}
";
    let result = eval_pure(source, "pickByBoth__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 positions matching both dates, got {result:?}",
    );
}

// -------------------------------------------------------------------------
// Single-target QP — multiplicity narrowed from [1] preserves shape.
// -------------------------------------------------------------------------

#[test]
fn single_multiplicity_qp_returns_singleton() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class test::Customer
{
    address: test::Address[1];
}

function test::singleton(): String[1]
{
    let a = ^test::Address(line='hello', businessDate=%2024-01-01);
    // Customer has property address: Address[1] which Phase A moved to
    // original_milestoned_properties and replaced with addressAllVersions:
    // Address[*]. To exercise the QP, build a Customer with the
    // edge-point property directly populated.
    let c = ^test::Customer(addressAllVersions=$a);
    $c.address(%2024-01-01).line
}
";
    let result = eval_pure(source, "singleton__String_1_");
    assert!(
        matches!(&result, Value::String(s) if s == "hello"),
        "expected 'hello', got {result:?}",
    );
}
