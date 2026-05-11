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

//! Property default-value type+multiplicity compat tests (T-20260511-01).
//!
//! Stages a tiny `.pure` source, runs it through the standard pipeline,
//! collects `PropertyDefaultValueIncompatible` diagnostics from
//! `finalize_model`'s validation pass.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn run(source: &str) -> Vec<CompilationError> {
    let sf = parse(source, "/test/default_values.pure");
    let mut model = init_bootstrap_model();
    let (_r, mut errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    errs.extend(finalize_model(&mut model, &[], &[]));
    errs
}

fn collect_default_violations(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::PropertyDefaultValueIncompatible { .. }
            )
        })
        .collect()
}

#[test]
fn string_default_for_integer_prop_emits_violation() {
    let errs = run(r"Class abc::Foo { prop : Integer[1] = ''; }");
    let hits = collect_default_violations(&errs);
    assert!(
        !hits.is_empty(),
        "expected PropertyDefaultValueIncompatible; got: {errs:?}"
    );
    let CompilationErrorKind::PropertyDefaultValueIncompatible {
        property_name,
        expected,
        actual,
        ..
    } = &hits[0].kind
    else {
        unreachable!();
    };
    assert_eq!(property_name.as_str(), "prop");
    assert!(
        expected.as_str().contains("Integer"),
        "expected to mention Integer; got {expected}"
    );
    assert!(
        actual.as_str().contains("String"),
        "actual to mention String; got {actual}"
    );
}

#[test]
fn empty_list_default_for_one_prop_emits_violation() {
    // Multiplicity [0] (empty list) doesn't satisfy [1].
    let errs = run(r"Class abc::Foo { prop : Integer[1] = []; }");
    let hits = collect_default_violations(&errs);
    assert!(
        !hits.is_empty(),
        "expected mult-mismatch violation; got: {errs:?}"
    );
}

#[test]
fn multi_default_for_one_prop_emits_violation() {
    // Multiplicity [3] (list of 3) doesn't satisfy [1].
    let errs = run(r"Class abc::Foo { prop : Integer[1] = [1, 2, 3]; }");
    let hits = collect_default_violations(&errs);
    assert!(
        !hits.is_empty(),
        "expected mult-mismatch violation; got: {errs:?}"
    );
}

#[test]
fn compatible_default_passes() {
    let errs = run(r"Class abc::Foo { prop : Integer[1] = 1; }");
    let hits = collect_default_violations(&errs);
    assert!(hits.is_empty(), "expected no violations; got: {hits:?}");
}

#[test]
fn no_default_means_no_check() {
    let errs = run(r"Class abc::Foo { prop : Integer[1]; }");
    let hits = collect_default_violations(&errs);
    assert!(
        hits.is_empty(),
        "no default = no compat check fires; got: {hits:?}"
    );
}
