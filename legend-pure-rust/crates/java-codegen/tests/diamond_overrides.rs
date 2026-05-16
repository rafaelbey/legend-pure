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

//! Codegen emission for diamond-inheritance property overrides.
//!
//! The validator in `crates/pure/src/validate.rs::validate_property_overrides`
//! rejects mismatched-multiplicity / mismatched-type diamonds as
//! `PropertyConflict` errors — but the codegen still has to emit
//! something compilable when run on a partial model. These tests
//! pin the multiplicity-intersection and type-narrowing policy so
//! the emitted Java is consistent under both validated and partial
//! models.

mod common;

use legend_pure_java_codegen::{FqnInput, Options, generate};

fn run(source: &str, fqn: &str) -> Vec<legend_pure_java_codegen::JavaFile> {
    let model = common::compile_with_platform(Some(source));
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new(fqn)];
    generate(&model, &fns, &[], &[], &opts).expect("codegen succeeds")
}

fn find_class<'a>(
    files: &'a [legend_pure_java_codegen::JavaFile],
    simple_name: &str,
) -> &'a legend_pure_java_codegen::JavaFile {
    files
        .iter()
        .find(|f| f.relative_path.ends_with(format!("{simple_name}.java")))
        .unwrap_or_else(|| panic!("expected {simple_name}.java in output; got {files:#?}"))
}

#[test]
fn matching_multiplicities_pass_through_unchanged() {
    // Both ancestors declare `name : String[1]`. This is the M3 platform's
    // canonical diamond (Type.name shadowing ModelElement.name). The join
    // class should emit an `@Override default String name() { ... }`
    // using the [1] body — which is the abstract-requireNonNull fallback,
    // since `[1]` has no sensible empty default.
    let src = r"
    Class user_test::LeftBranch { name : String[1]; }
    Class user_test::RightBranch { name : String[1]; }
    Class user_test::Join extends user_test::LeftBranch, user_test::RightBranch {}
    function user_test::seed(): user_test::Join[1] { ^user_test::Join() }
    ";
    let files = run(src, "user_test::seed__Join_1_");
    let join = find_class(&files, "Join");
    assert!(
        join.contents.contains("@Override default String name()"),
        "Join must emit @Override default for diamond `name`: {}",
        join.contents
    );
    assert!(
        join.contents
            .contains("java.util.Objects.requireNonNull(null, \"abstract diamond property\")"),
        "Matching [1] mults must emit the abstract-fallback body: {}",
        join.contents
    );
}

#[test]
fn intersection_narrows_to_pureone_when_one_branch_requires_one() {
    // LeftBranch requires `[1]`, RightBranch allows `[0..1]`. The only
    // multiplicity satisfying both contracts is `[1]` (the intersection).
    // The emitted Java return type must therefore be `String` (not
    // `Optional<String>`) and the default body must be the abstract
    // fallback — never `Optional.empty()`, which would violate the
    // `[1]` contract on LeftBranch.
    let src = r"
    Class user_test::LeftBranch { value : String[1]; }
    Class user_test::RightBranch { value : String[0..1]; }
    Class user_test::Join extends user_test::LeftBranch, user_test::RightBranch {}
    function user_test::seed(): user_test::Join[1] { ^user_test::Join() }
    ";
    let files = run(src, "user_test::seed__Join_1_");
    let join = find_class(&files, "Join");
    assert!(
        join.contents.contains("@Override default String value()"),
        "Intersection of [1] and [0..1] must be [1] (bare String): {}",
        join.contents
    );
    assert!(
        !join.contents.contains("Optional<String> value()"),
        "Intersection of [1] and [0..1] must NOT emit Optional<String>: {}",
        join.contents
    );
    assert!(
        join.contents
            .contains("java.util.Objects.requireNonNull(null, \"abstract diamond property\")"),
        "[1] intersection must use the abstract fallback body: {}",
        join.contents
    );
}

#[test]
fn intersection_narrows_to_zero_or_one_when_one_branch_caps_upper() {
    // LeftBranch is `[0..1]`, RightBranch is `[*]`. The intersection is
    // `[0..1]` — the narrower of the two — and the default body should
    // be `Optional.empty()` rather than `Collections.emptyList()`.
    let src = r"
    Class user_test::LeftBranch { items : String[0..1]; }
    Class user_test::RightBranch { items : String[*]; }
    Class user_test::Join extends user_test::LeftBranch, user_test::RightBranch {}
    function user_test::seed(): user_test::Join[1] { ^user_test::Join() }
    ";
    let files = run(src, "user_test::seed__Join_1_");
    let join = find_class(&files, "Join");
    assert!(
        join.contents.contains("Optional<String> items()"),
        "Intersection of [0..1] and [*] must be [0..1] (Optional<String>): {}",
        join.contents
    );
    assert!(
        join.contents.contains("return java.util.Optional.empty();"),
        "[0..1] intersection must default to Optional.empty(): {}",
        join.contents
    );
    assert!(
        !join.contents.contains("Collections.emptyList()"),
        "[0..1] intersection must NOT emit a list default body: {}",
        join.contents
    );
}

#[test]
fn most_specific_type_wins_when_one_is_subtype_of_other() {
    // LeftBranch declares `value : Number[1]`, RightBranch declares
    // `value : Integer[1]`. Integer is a subtype of Number in the
    // platform, so the join class should emit `Integer` (the most
    // specific common type), not Number.
    let src = r"
    Class user_test::LeftBranch { value : Number[1]; }
    Class user_test::RightBranch { value : Integer[1]; }
    Class user_test::Join extends user_test::LeftBranch, user_test::RightBranch {}
    function user_test::seed(): user_test::Join[1] { ^user_test::Join() }
    ";
    let files = run(src, "user_test::seed__Join_1_");
    let join = find_class(&files, "Join");
    assert!(
        join.contents.contains("@Override default Long value()"),
        "Most-specific type Integer (Long in Java) must win over Number: {}",
        join.contents
    );
    // Number renders as `Number` in Java — make sure we did NOT pick it.
    assert!(
        !join.contents.contains("@Override default Number value()"),
        "Less-specific type Number must NOT be chosen: {}",
        join.contents
    );
}
