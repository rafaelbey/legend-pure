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

//! Phase B-3 — milestoning date-context propagation smoke tests.
//!
//! Each fixture writes the *implicit* milestoned-target property-access
//! form (`$c.address` rather than `$c.address($d)`). The propagation pass
//! must rewrite these to `QualifiedPropertyCall`s with the in-scope date
//! injected. Two angles:
//!
//! - **Structural** — inspect the lowered IR of the compiled function and
//!   verify the rewrite happened. Catches regressions in the rewriter
//!   even if dispatch happens to muddle through at runtime.
//! - **End-to-end** — actually evaluate the function and assert the
//!   value matches what the explicit form would have produced.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ExprKind, ValueSpec};
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

/// Resolve a test function's compiled `Function.body` (as a `Vec<ValueSpec>`).
fn function_body(model: &PureModel, fqn: &str) -> Vec<ValueSpec> {
    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("function test::{fqn} not found"));
    match model.get_element(fn_id) {
        Element::Function(f) => f.body.iter().cloned().collect(),
        other => panic!("expected Function, got {other:?}"),
    }
}

/// Walk a body looking for the first `QualifiedPropertyCall` matching `name`.
/// Returns the matched call's argument count (1 = receiver only,
/// 2 = receiver + one date, etc.).
fn find_qp_call(body: &[ValueSpec], name: &str) -> Option<usize> {
    for vs in body {
        if let Some(n) = find_qp_call_in_value_spec(vs, name) {
            return Some(n);
        }
    }
    None
}

fn find_qp_call_in_value_spec(vs: &ValueSpec, name: &str) -> Option<usize> {
    match vs.kind.as_ref() {
        ExprKind::QualifiedPropertyCall(d) if d.function_name == name => Some(d.arguments.len()),
        ExprKind::FunctionCall(d) | ExprKind::PropertyCall(d) => {
            for a in &d.arguments {
                if let Some(n) = find_qp_call_in_value_spec(a, name) {
                    return Some(n);
                }
            }
            None
        }
        ExprKind::QualifiedPropertyCall(d) => {
            for a in &d.arguments {
                if let Some(n) = find_qp_call_in_value_spec(a, name) {
                    return Some(n);
                }
            }
            None
        }
        ExprKind::Lambda { body, .. } => {
            for b in body {
                if let Some(n) = find_qp_call_in_value_spec(b, name) {
                    return Some(n);
                }
            }
            None
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                if let Some(n) = find_qp_call_in_value_spec(e, name) {
                    return Some(n);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_property_call(body: &[ValueSpec], name: &str) -> bool {
    body.iter().any(|vs| find_property_call_in(vs, name))
}

fn find_property_call_in(vs: &ValueSpec, name: &str) -> bool {
    match vs.kind.as_ref() {
        ExprKind::PropertyCall(d) if d.function_name == name => true,
        ExprKind::FunctionCall(d)
        | ExprKind::PropertyCall(d)
        | ExprKind::QualifiedPropertyCall(d) => {
            d.arguments.iter().any(|a| find_property_call_in(a, name))
        }
        ExprKind::Lambda { body, .. } => body.iter().any(|b| find_property_call_in(b, name)),
        ExprKind::Collection { elements } => {
            elements.iter().any(|e| find_property_call_in(e, name))
        }
        _ => false,
    }
}

// -------------------------------------------------------------------------
// Structural: variants 3 + 5 — getAll source + map lambda binding
// -------------------------------------------------------------------------

#[test]
fn implicit_address_in_map_lambda_is_rewritten_to_qp() {
    // Customer is businesstemporal; Address is businesstemporal; Customer
    // has property address: Address[1]. Phase A moved address to
    // original_milestoned_properties and synthesised address(td) +
    // addressAllVersions. The user code writes the IMPLICIT form
    // `$c.address` inside `Customer.all($d)->map(c | $c.address)` —
    // propagation must detect:
    //   - `Customer.all($d)` returns Customer[*] with date $d in context
    //   - map's lambda binds c with that context
    //   - `$c.address` rewrites to `$c.address($d)`
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::probe(d: Date[1]): test::Address[*]
{
    test::Customer->getAll($d)->map(c | $c.address)
}
";
    let model = compile_with_platform(source);
    let body = function_body(&model, "probe_Date_1__Address_MANY_");
    // Confirm the QP call exists with receiver + 1 date arg.
    let qp_arity = find_qp_call(&body, "address").unwrap_or_else(|| {
        panic!("expected propagation to produce a QualifiedPropertyCall 'address'; body: {body:?}")
    });
    assert_eq!(
        qp_arity, 2,
        "expected `address` QP to be called with receiver + 1 date arg, got arity {qp_arity}"
    );
    // And no plain PropertyCall for `address` remains.
    assert!(
        !find_property_call(&body, "address"),
        "expected no plain PropertyCall('address') after propagation"
    );
}

// -------------------------------------------------------------------------
// Negative: no source available — propagation leaves the call alone.
// -------------------------------------------------------------------------

#[test]
fn implicit_property_with_no_source_emits_b4_missing_date_error() {
    // `$c` is a function-parameter Customer — no milestoning context.
    // Propagation can't supply a date, so B-4.2 fires a hard error.
    use legend_pure_parser_pure::error::CompilationErrorKind;
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::probe(c: test::Customer[1]): test::Address[*]
{
    $c.address
}
";
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(source));
    let partial = legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports)
        .expect_err("compile should surface the missing-date diagnostic");
    let found = partial.errors.iter().find(|e| {
        matches!(
            e.kind,
            CompilationErrorKind::MilestoningMissingDateContext { .. }
        )
    });
    assert!(
        found.is_some(),
        "expected MilestoningMissingDateContext, errors: {:?}",
        partial
            .errors
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>()
    );
}

// -------------------------------------------------------------------------
// End-to-end: explicit + implicit return the same value
// -------------------------------------------------------------------------

// -------------------------------------------------------------------------
// Variant 4 — AutoMap: collection property access threads milestoning
// -------------------------------------------------------------------------

#[test]
fn automap_collection_runtime_returns_filtered_addresses() {
    // End-to-end variant 4: seed multiple Customer / Address versions,
    // run the AutoMap form, assert the runtime returns the date-filtered
    // result.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::countAddresses(): Integer[1]
{
    let a1 = ^test::Address(line='jan-A', businessDate=%2024-01-01);
    let a2 = ^test::Address(line='jan-B', businessDate=%2024-01-01);
    let c1 = ^test::Customer(addressAllVersions=$a1, businessDate=%2024-01-01);
    let c2 = ^test::Customer(addressAllVersions=$a2, businessDate=%2024-01-01);
    test::Customer->getAll(%2024-01-01).address->size()
}
";
    let result = eval_pure(source, "countAddresses__Integer_1_");
    assert!(
        matches!(result, Value::Integer(2)),
        "expected 2 addresses via AutoMap propagation, got {result:?}",
    );
}

#[test]
fn automap_on_collection_receiver_propagates_dates() {
    // `Customer->getAll($d)` returns Customer[*]. Without an explicit
    // map(c | …), writing `.address` on the [*] result is the AutoMap
    // shape. Java's parser desugars this to `xs->map(c | $c.address)`;
    // the Rust lowerer keeps it as a PropertyCall on the collection.
    // Either way, propagation must recognise that the receiver carries
    // dates and inject them.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::pickAddress(d: Date[1]): test::Address[*]
{
    test::Customer->getAll($d).address
}
";
    let model = compile_with_platform(source);
    let body = function_body(&model, "pickAddress_Date_1__Address_MANY_");
    let address_arity = find_qp_call(&body, "address").unwrap_or_else(|| {
        panic!("expected AutoMap propagation to produce QP 'address'; body: {body:?}")
    });
    assert_eq!(
        address_arity, 2,
        "expected `.address` QP to be called with receiver + 1 date arg, got arity {address_arity}"
    );
}

// -------------------------------------------------------------------------
// Variant 2 — explicit QP propagates its dates downstream
// -------------------------------------------------------------------------

#[test]
fn explicit_qp_dates_propagate_to_nested_milestoned_access() {
    // Customer.address(d) returns Address (milestoned). Address has a
    // milestoned-target property `street: Street[1]`. Writing
    // `$c.address(%2024-01-01).street` should auto-thread %2024-01-01
    // to the nested `.street` call via variant 2 propagation.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Street
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    street: test::Street[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::nestedAccess(): test::Street[*]
{
    test::Customer->getAll(%2024-01-01)->map(c | $c.address.street)
}
";
    let model = compile_with_platform(source);
    let body = function_body(&model, "nestedAccess__Street_MANY_");
    // Look for the inner QP `street` — it should have receiver + 1 date arg.
    let street_arity = find_qp_call(&body, "street").unwrap_or_else(|| {
        panic!("expected propagation to rewrite nested `.street` access; body: {body:?}")
    });
    assert_eq!(
        street_arity, 2,
        "expected `.street` QP to be called with receiver + 1 date arg, got arity {street_arity}"
    );
}

#[test]
fn implicit_form_returns_same_value_as_explicit_form() {
    // Build a small heap, ask for one specific date via both forms,
    // assert results match.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> test::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> test::Customer
{
    address: test::Address[1];
}

function test::explicitForm(): String[*]
{
    let a1 = ^test::Address(line='jan', businessDate=%2024-01-01);
    let c = ^test::Customer(addressAllVersions=$a1, businessDate=%2024-01-01);
    test::Customer->getAll(%2024-01-01)->map(c | $c.address(%2024-01-01).line)
}

function test::implicitForm(): String[*]
{
    let a1 = ^test::Address(line='jan', businessDate=%2024-01-01);
    let c = ^test::Customer(addressAllVersions=$a1, businessDate=%2024-01-01);
    test::Customer->getAll(%2024-01-01)->map(c | $c.address.line)
}
";
    let explicit = eval_pure(source, "explicitForm__String_MANY_");
    let implicit = eval_pure(source, "implicitForm__String_MANY_");
    // Both forms should return the same value — explicit uses the QP
    // directly; implicit relies on B-3 propagation to inject the date.
    // For this fixture (one seeded Customer + one seeded Address) we
    // expect ['jan'] from both.
    assert_eq!(format!("{explicit:?}"), format!("{implicit:?}"));
}
