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

//! Cross-package import-isolation smoke tests (T-20260510-01).
//!
//! Java parity: a short-name reference to an element in another package
//! must fail unless the using section has an explicit `import …::*;` —
//! or the using element is in the same package as the target.
//!
//! Pre-fix, the per-section `import_scopes` cache was mutated in place
//! with each element's implicit self-package, leaking package A's scope
//! into package B's resolution. These tests pin both the bug repro and
//! the regression-guard "what should still work" matrix.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::compile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

#[allow(clippy::result_large_err)]
fn try_compile(
    sources: &[(&str, &str)],
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sfs: Vec<SourceFile> = sources.iter().map(|(s, p)| parse(s, p)).collect();
    compile!(&sfs)
}

fn unresolved_hits<'a>(errors: &'a [CompilationError], name: &str) -> Vec<&'a CompilationError> {
    errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == name
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Bug repro: short cross-package ref, no import → error
// ---------------------------------------------------------------------------

#[test]
fn cross_package_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Class1 {}

Class other::Class2 extends Class1 {
  child : Class1[0..1];
}
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    let hits = unresolved_hits(&err.errors, "Class1");
    assert!(
        hits.len() >= 2,
        "expected ≥2 UnresolvedElement('Class1') (extends + property); got {:?}",
        err.errors
    );
}

// ---------------------------------------------------------------------------
// 2. With explicit `import abc::*;` → clean
// ---------------------------------------------------------------------------

#[test]
fn cross_package_short_name_with_import_compiles() {
    let src = r"
###Pure
Class abc::Class1 {}

###Pure
import abc::*;

Class other::Class2 extends Class1 {
  child : Class1[0..1];
}
";
    try_compile(&[(src, "test.pure")]).expect("must compile with explicit import");
}

// ---------------------------------------------------------------------------
// 3. FQN reference without import → clean (regression guard)
// ---------------------------------------------------------------------------

#[test]
fn cross_package_fqn_without_import_compiles() {
    let src = r"
###Pure
Class abc::Class1 {}

Class other::Class2 extends abc::Class1 {
  child : abc::Class1[0..1];
}
";
    try_compile(&[(src, "test.pure")]).expect("FQN reference must always compile");
}

// ---------------------------------------------------------------------------
// 4. Same-section, two packages, sibling short ref → error
//    (THE exact leak shape — A's package must not reach B)
// ---------------------------------------------------------------------------

#[test]
fn same_section_two_packages_no_cross_leak() {
    let src = r"
###Pure
Class abc::Class1 {}
Class other::Class2 {}

Class other::ChildC extends Class1 {}
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Class1").is_empty(),
        "Class1 (in `abc::`) must not be reachable from `other::ChildC`; got {:?}",
        err.errors,
    );
}

// ---------------------------------------------------------------------------
// 5. Cross-package short ref inside a function body → error (Pass 2b leak)
// ---------------------------------------------------------------------------

#[test]
fn cross_package_short_function_call_without_import_errors() {
    let src = r"
###Pure
function abc::myFunc(): Integer[1] { 42 }

function other::caller(): Integer[1] { myFunc() }
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    // Function dispatch may surface as UnresolvedElement or an
    // overload-resolution failure; just demand SOMETHING errored.
    assert!(
        !err.errors.is_empty(),
        "expected cross-package function call to error; got clean compile"
    );
}

// ---------------------------------------------------------------------------
// 6. Two elements in different sections same file → no leak between them
// ---------------------------------------------------------------------------

#[test]
fn different_sections_isolate_implicit_self_packages() {
    // Section 1 defines abc::A; Section 2 defines other::B and tries to
    // reference A by short name with no import. Must error — section 2
    // doesn't inherit section 1's implicit self-package.
    let src = r"
###Pure
Class abc::A {}

###Pure
Class other::B extends A {}
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "A").is_empty(),
        "section 2 must not see `abc::A` via section 1's implicit self-package; got {:?}",
        err.errors,
    );
}

// ---------------------------------------------------------------------------
// 7. Element-order independence
//    The pre-fix bug was symptomatic of element-order: A processed first
//    leaks `abc::` for B. Reversing the order should give the same
//    diagnostic. (If it doesn't, the fix is patching the symptom not the
//    cause.)
// ---------------------------------------------------------------------------

#[test]
fn cross_package_short_name_order_independent() {
    let forward = r"
###Pure
Class abc::Class1 {}
Class other::Class2 extends Class1 {}
";
    let reverse = r"
###Pure
Class other::Class2 extends Class1 {}
Class abc::Class1 {}
";
    let err_fwd = try_compile(&[(forward, "fwd.pure")]).expect_err("forward must not compile");
    let err_rev = try_compile(&[(reverse, "rev.pure")]).expect_err("reverse must not compile");
    assert!(
        !unresolved_hits(&err_fwd.errors, "Class1").is_empty(),
        "forward order: Class1 must be unresolved; got {:?}",
        err_fwd.errors,
    );
    assert!(
        !unresolved_hits(&err_rev.errors, "Class1").is_empty(),
        "reverse order: Class1 must be unresolved; got {:?}",
        err_rev.errors,
    );
}

// ---------------------------------------------------------------------------
// 8. Same-package reference (no import needed) → clean (regression guard)
// ---------------------------------------------------------------------------

#[test]
fn same_package_short_name_compiles() {
    let src = r"
###Pure
Class abc::Class1 {}

Class abc::Class2 extends Class1 {
  child : Class1[0..1];
}
";
    try_compile(&[(src, "test.pure")]).expect("same-package short refs must compile");
}

// ---------------------------------------------------------------------------
// Sweep coverage (T-20260510-01, closed 2026-05-11) — locks the sites
// beyond `extends` and property types that the original ticket's
// "verify, don't trust the list" note enumerated. Every site below
// reaches the chokepoint `resolve_unqualified`
// (crates/pure/src/resolve.rs) via `resolve_type_ref` or
// `resolve_element_ptr`; these tests prove the routing structurally
// rather than relying on the original ticket's hopeful claim.
//
// Not a forward TODO — the audit-end on T-20260510-01 confirmed no
// additional fix is needed (central chokepoint covers every site).
// ---------------------------------------------------------------------------

// 9. Function parameter type — short cross-package ref
//    (route: function-signature lowering → resolve_type_ref → resolve_unqualified).
#[test]
fn function_param_type_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Class1 {}

function other::usesIt(c : Class1[1]) : Integer[1] { 1 }
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Class1").is_empty(),
        "function param type must reject cross-package short ref; got {:?}",
        err.errors
    );
}

#[test]
fn function_param_type_short_name_with_import_compiles() {
    let src = r"
###Pure
Class abc::Class1 {}

###Pure
import abc::*;

function other::usesIt(c : Class1[1]) : Integer[1] { 1 }
";
    try_compile(&[(src, "test.pure")])
        .expect("function param type must accept short ref with explicit import");
}

// 10. Function return type — short cross-package ref
//     (same path as function param type but separately pinned).
#[test]
fn function_return_type_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Class1 {}

function other::makeOne() : Class1[1] { ^abc::Class1() }
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Class1").is_empty(),
        "function return type must reject cross-package short ref; got {:?}",
        err.errors
    );
}

// 11. `^Class(...)` constructor receiver — short cross-package class FQN
//     (route: constructor lowering → resolve_element_ptr → resolve_unqualified).
#[test]
fn constructor_receiver_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Class1 {}

function other::makeOne() : abc::Class1[1] { ^Class1() }
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Class1").is_empty(),
        "^Class(...) receiver must reject cross-package short class ref; got {:?}",
        err.errors
    );
}

// 12. `cast(@T)` — short cross-package type ref inside `@…`
//     (route: cast lowering → resolve_type_ref → resolve_unqualified).
#[test]
fn cast_type_arg_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Target {}

function other::useCast(a : Any[1]) : Target[1] { $a->cast(@Target) }
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Target").is_empty(),
        "cast(@T) must reject cross-package short type ref; got {:?}",
        err.errors
    );
}

// 13. Association end class — short cross-package ref on either side.
//     Java treats association ends as type refs; same path as property types.
#[test]
fn association_end_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Person {}

Association other::PersonRel
{
  person : Person[1];
  count : Integer[0..1];
}
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Person").is_empty(),
        "Association end type must reject cross-package short ref; got {:?}",
        err.errors
    );
}

// 14. Stereotype profile reference — short cross-package profile name.
//     (route: resolve_stereotypes → resolve_element_ptr → resolve_unqualified.)
#[test]
fn stereotype_profile_short_name_without_import_errors() {
    let src = r#"
###Pure
Profile abc::myProfile { stereotypes : [interesting]; }

Class <<myProfile.interesting>> other::Tagged {}
"#;
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "myProfile").is_empty(),
        "stereotype profile ref must reject cross-package short profile; got {:?}",
        err.errors
    );
}

// 15. Generic type argument — short cross-package type ref inside `<…>`.
//     (route: resolve_type_ref recurses through `type_arguments`.)
//     Declare a local generic class to keep the test self-contained
//     without relying on platform sources.
#[test]
fn generic_type_arg_short_name_without_import_errors() {
    let src = r"
###Pure
Class abc::Inner {}

Class lib::Box<T> {
  contents : T[1];
}

function other::wrap() : lib::Box<Inner>[1] {
  ^lib::Box<Inner>(contents = ^abc::Inner())
}
";
    let err = try_compile(&[(src, "test.pure")]).expect_err("must not compile");
    assert!(
        !unresolved_hits(&err.errors, "Inner").is_empty(),
        "generic type arg must reject cross-package short ref; got {:?}",
        err.errors
    );
}
