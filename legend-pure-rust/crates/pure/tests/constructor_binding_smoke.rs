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

//! `^Class(...)` constructor binding tests
//! (T-20260511-02 / T-20260511-03 / T-20260511-04).
//!
//! All three live in the same cross-chunk validator, so a single test
//! file exercises them together against tiny `.pure` fixtures. Each
//! test wraps the offending `^Class(...)` in a thin function so it
//! survives the parser + lowering all the way to `finalize_model`.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn run(source: &str) -> Vec<CompilationError> {
    let sf = parse(source, "/test/constructor.pure");
    let mut model = init_bootstrap_model();
    let (_r, mut errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    errs.extend(finalize_model(&mut model, &[], &[]));
    errs
}

fn count<F: Fn(&CompilationErrorKind) -> bool>(errs: &[CompilationError], pred: F) -> usize {
    errs.iter().filter(|e| pred(&e.kind)).count()
}

// --- T-20260511-04 (UnknownProperty) -----------------------------------

#[test]
fn unknown_property_in_constructor_errors() {
    let errs = run(r"
        Class abc::Class1 {}
        function abc::mk(): abc::Class1[1] { ^abc::Class1(propd = '') }
    ");
    let hits = count(
        &errs,
        |k| matches!(k, CompilationErrorKind::UnknownProperty { property_name, .. } if property_name.as_str() == "propd"),
    );
    assert!(
        hits >= 1,
        "expected UnknownProperty for 'propd'; got: {errs:?}"
    );
}

// --- T-20260511-02 (ConstructorMissingRequiredProperty) ----------------

#[test]
fn missing_required_one_property_errors() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1() }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorMissingRequiredProperty { property_name, .. }
                if property_name.as_str() == "propA"
        )
    });
    assert!(
        hits >= 1,
        "expected ConstructorMissingRequiredProperty for 'propA'; got: {errs:?}"
    );
}

#[test]
fn optional_property_omitted_passes() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[0..1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1() }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorMissingRequiredProperty { .. }
        )
    });
    assert_eq!(
        hits, 0,
        "[0..1] is optional; no missing-required error expected; got: {errs:?}"
    );
}

#[test]
fn default_satisfies_required_check() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1] = 0; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1() }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorMissingRequiredProperty { .. }
        )
    });
    assert_eq!(
        hits, 0,
        "default value satisfies required check; got: {errs:?}"
    );
}

#[test]
fn inherited_required_property_errors() {
    let errs = run(r"
        Class abc::Base { propBase : Integer[1]; }
        Class abc::Sub extends abc::Base {}
        function abc::mk(): abc::Sub[1] { ^abc::Sub() }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorMissingRequiredProperty { property_name, .. }
                if property_name.as_str() == "propBase"
        )
    });
    assert!(
        hits >= 1,
        "inherited required property should also fire; got: {errs:?}"
    );
}

// --- T-20260511-03 (ConstructorPropertyTypeMismatch) -------------------

#[test]
fn wrong_value_type_errors() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1(propA = '') }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorPropertyTypeMismatch { property_name, .. }
                if property_name.as_str() == "propA"
        )
    });
    assert!(
        hits >= 1,
        "expected ConstructorPropertyTypeMismatch for 'propA'; got: {errs:?}"
    );
}

#[test]
fn empty_value_for_one_prop_errors() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1(propA = []) }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorPropertyTypeMismatch { property_name, .. }
                if property_name.as_str() == "propA"
        )
    });
    assert!(
        hits >= 1,
        "[] doesn't satisfy [1]; expected mult-mismatch; got: {errs:?}"
    );
}

#[test]
fn multi_value_for_one_prop_errors() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1(propA = [1, 2]) }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorPropertyTypeMismatch { property_name, .. }
                if property_name.as_str() == "propA"
        )
    });
    assert!(
        hits >= 1,
        "[1,2] doesn't satisfy [1]; expected mult-mismatch; got: {errs:?}"
    );
}

#[test]
fn compatible_value_passes() {
    let errs = run(r"
        Class abc::Class1 { propA : Integer[1]; }
        function abc::mk(): abc::Class1[1] { ^abc::Class1(propA = 1) }
    ");
    let hits = count(&errs, |k| {
        matches!(
            k,
            CompilationErrorKind::ConstructorPropertyTypeMismatch { .. }
                | CompilationErrorKind::ConstructorMissingRequiredProperty { .. }
                | CompilationErrorKind::UnknownProperty { .. }
        )
    });
    assert_eq!(
        hits, 0,
        "compatible value with required prop satisfied should produce no constructor errors; got: {errs:?}"
    );
}
