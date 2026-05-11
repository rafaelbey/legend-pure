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
