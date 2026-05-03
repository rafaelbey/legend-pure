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

//! Stage-3 navigation-path lowering smoke tests.
//!
//! Validates that `Expression::NavigationPath` lowers to
//! `ExprKind::PathLiteral` with a resolved start type and lowered
//! step parameters. First-step property is validated against the
//! start type — subsequent steps are runtime-resolved (Stage 4).
//!
//! Mirrors a subset of Java's `m2.dsl.path.test.TestDSLCompilation`.
//! Lives in the runtime crate because it needs the full platform
//! compile (the path metamodel + `plus`/`+` etc. resolved).

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ExprKind, Expression, PathStepLowered};

// ---------------------------------------------------------------------------
// Helpers (mirror the eval_tests.rs platform helpers in miniature so this
// file can stand alone without cross-test-module imports).
// ---------------------------------------------------------------------------

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
    }
}

/// Compiles `user_source` against the full embedded platform.
/// Returns the model plus any errors. Soft-fails (returns the
/// partial model) so error-asserting tests can introspect it.
fn compile(user_source: &str) -> (PureModel, Vec<CompilationError>) {
    let mut repos = Repo::default_with_build_snapshots();
    repos.push(synthetic_user_repo(user_source));
    let auto_imports: Vec<smol_str::SmolStr> =
        legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| smol_str::SmolStr::new(s))
            .collect();
    match legend_pure_core_platform::repo::load(&repos, &auto_imports) {
        Ok(model) => (model, vec![]),
        Err(partial) => (partial.model, partial.errors),
    }
}

fn function_body(model: &PureModel, fqn: &[&str]) -> Vec<Expression> {
    let segments: Vec<smol_str::SmolStr> = fqn.iter().map(|s| smol_str::SmolStr::new(*s)).collect();
    model
        .resolve_function_by_path(&segments)
        .map(|id| match model.get_element(id) {
            Element::Function(f) => f.body.iter().cloned().collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

fn extract_path_literal(expr: &Expression) -> Option<(&[PathStepLowered], Option<&str>)> {
    if let ExprKind::PathLiteral { steps, name, .. } = &*expr.kind {
        return Some((steps.as_slice(), name.as_deref()));
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn simple_single_step_path_lowers_clean() {
    let (model, errors) = compile(
        r"###Pure
Class test::Person
{
    name: String[1];
}

function test::path_test(): Any[*]
{
    #/test::Person/name#
}",
    );
    assert!(
        errors.is_empty(),
        "expected clean compile, got errors: {errors:?}"
    );

    let body = function_body(&model, &["test", "path_test"]);
    let body0 = body.first().expect("function body present");
    let (steps, alias) = extract_path_literal(body0).expect("body should be PathLiteral");
    assert_eq!(steps.len(), 1, "exactly one path step");
    assert_eq!(steps[0].property_name.as_str(), "name");
    assert!(steps[0].parameters.is_empty(), "no params on /name");
    assert!(alias.is_none(), "no alias suffix expected");
}

#[test]
fn unknown_first_step_property_emits_unknown_property_error() {
    // Java parity (TestDSLCompilation.testPathWithNonExistentProperty):
    // `nonExistent` is not declared on Person. First-step validation
    // in `lower_navigation_path` fires UnknownProperty.
    let (_model, errors) = compile(
        r"###Pure
Class test::Person
{
    name: String[1];
}

function test::bad_path(): Any[*]
{
    #/test::Person/nonExistent#
}",
    );
    let unknowns: Vec<&CompilationError> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::UnknownProperty { .. }))
        .collect();
    assert!(
        !unknowns.is_empty(),
        "expected at least one UnknownProperty error, got: {errors:?}"
    );
    let CompilationErrorKind::UnknownProperty {
        type_name,
        property_name,
    } = &unknowns[0].kind
    else {
        panic!("expected UnknownProperty kind")
    };
    assert_eq!(property_name.as_str(), "nonExistent");
    assert!(
        type_name.contains("Person"),
        "type_name should mention Person, got {type_name}",
    );
}

#[test]
fn alias_suffix_round_trips_into_path_literal() {
    let (model, errors) = compile(
        r"###Pure
Class test::Person
{
    name: String[1];
}

function test::aliased(): Any[*]
{
    #/test::Person/name!myAlias#
}",
    );
    assert!(
        errors.is_empty(),
        "expected clean compile, got errors: {errors:?}"
    );
    let body = function_body(&model, &["test", "aliased"]);
    let body0 = body.first().expect("function body present");
    let (_, alias) = extract_path_literal(body0).expect("PathLiteral");
    assert_eq!(alias, Some("myAlias"));
}

#[test]
fn multi_step_path_records_all_steps_in_order() {
    // Stage 3 only validates the first step's property against the
    // start type. Subsequent step names are recorded as-is —
    // runtime walks the chain in Stage 4 with type-arg substitution.
    // This test asserts the lowering preserves step order/names so
    // Stage 4 has the right input.
    let (model, errors) = compile(
        r"###Pure
Class test::Address
{
    street: String[1];
}

Class test::Person
{
    address: test::Address[1];
}

function test::nested(): Any[*]
{
    #/test::Person/address/street#
}",
    );
    assert!(
        errors.is_empty(),
        "expected clean compile, got errors: {errors:?}"
    );
    let body = function_body(&model, &["test", "nested"]);
    let body0 = body.first().expect("function body present");
    let (steps, _) = extract_path_literal(body0).expect("PathLiteral");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].property_name.as_str(), "address");
    assert_eq!(steps[1].property_name.as_str(), "street");
}

#[test]
fn parameter_lowers_as_value_spec_expression() {
    // QualifiedProperty body uses `+` (resolves to platform `plus`) so
    // we need the platform compiled. Confirms parameters are lowered
    // through the standard expression machinery, not a special path.
    let (model, errors) = compile(
        r"###Pure
Class test::Person2
{
    nameWithTitle(title: String[1]) { $title + ' Smith' }: String[1];
}

function test::param_path(): Any[*]
{
    #/test::Person2/nameWithTitle('Mr')#
}",
    );
    assert!(
        errors.is_empty(),
        "expected clean compile, got errors: {errors:?}"
    );
    let body = function_body(&model, &["test", "param_path"]);
    let body0 = body.first().expect("function body present");
    let (steps, _) = extract_path_literal(body0).expect("PathLiteral");
    assert_eq!(steps[0].property_name.as_str(), "nameWithTitle");
    assert_eq!(steps[0].parameters.len(), 1);
    assert!(
        matches!(*steps[0].parameters[0].kind, ExprKind::StringLiteral(_)),
        "expected StringLiteral parameter, got: {:?}",
        steps[0].parameters[0].kind
    );
}
