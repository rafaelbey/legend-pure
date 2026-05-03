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
//
//! Integration tests for `<<access.private>>` / `<<access.protected>>`
//! enforcement in the Pure compiler — Java-parity with
//! `legend-pure-core/.../tests/validation/TestAccess.java`.
//!
//! Each test stages a model by:
//! 1. Calling `init_bootstrap_model()` (M3 metaclasses + primitives).
//! 2. Compiling a synthetic `meta::pure::profiles::access` profile —
//!    the platform fixture isn't available at this layer, so we declare
//!    it inline (same approach as `purem::filter` unit tests).
//! 3. Compiling the user-source slice that exercises a private/protected
//!    reference.
//! 4. Calling `finalize_model()` — validation (Pass 3) is what
//!    `validate_access_levels` hooks into, and validation runs there.
//!
//! Assertions are message-substring + kind-tag based: the exact column
//! is not yet preserved through every reference shape (parser limitation
//! noted in the plan), so we check what Java would call the
//! "structural" outcome, not the precise pointer.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};

const ACCESS_PROFILE_SOURCE: &str = "Profile meta::pure::profiles::access { stereotypes: [public, protected, private, externalizable]; }";

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

/// Stage a fresh model that has the access profile registered. Returns
/// the bootstrapped model — the access profile lives in chunk 1.
fn fresh_model_with_access_profile() -> PureModel {
    let mut model = init_bootstrap_model();
    let access_sf = parse(ACCESS_PROFILE_SOURCE, "access.pure");
    let (_r, errs) = compile_repo_slice(&mut model, &[access_sf], &[], &[]);
    assert!(errs.is_empty(), "access profile compile errors: {errs:?}");
    model
}

/// Compile a single user source on top of a model with the access
/// profile, then run finalize. Returns every error finalize produced.
fn compile_with_access(source: &str) -> Vec<CompilationError> {
    let mut model = fresh_model_with_access_profile();
    let sf = parse(source, "test.pure");
    let (_r, slice_errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let mut all = slice_errs;
    all.extend(finalize_model(&mut model, &[], &[]));
    all
}

fn has_kind(errors: &[CompilationError], pred: impl Fn(&CompilationErrorKind) -> bool) -> bool {
    errors.iter().any(|e| pred(&e.kind))
}

fn has_not_accessible_with(
    errors: &[CompilationError],
    target_substring: &str,
    use_site: &str,
) -> bool {
    errors.iter().any(|e| {
        matches!(&e.kind,
            CompilationErrorKind::NotAccessible { target_fqn, use_site_package }
                if target_fqn.contains(target_substring) && use_site_package == use_site
        )
    })
}

// ---------------------------------------------------------------------------
// Private functions — same package (positive cases)
// ---------------------------------------------------------------------------

#[test]
fn private_call_in_same_package_succeeds() {
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg::publicFunc(s: String[1]): String[1] {\
              pkg::privateFunc($s, ' from public')\
         }",
    );
    assert!(
        !has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::NotAccessible { .. }
        )),
        "should not emit NotAccessible: {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Private functions — different package (negative cases)
// ---------------------------------------------------------------------------

#[test]
fn private_call_in_different_package_fails() {
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg1::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(s: String[1]): String[1] {\
              pkg1::privateFunc($s, ' from public')\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::privateFunc", "pkg2"),
        "expected NotAccessible(pkg1::privateFunc..., pkg2): {errs:#?}"
    );
}

#[test]
fn private_indirect_call_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateFunctionIndirectApplicationInDifferentPackage
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg1::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(s: String[1]): String[1] {\
              plus(['a', pkg1::privateFunc($s, ' from public'), 'b'])\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::privateFunc", "pkg2"),
        "expected NotAccessible(pkg1::privateFunc..., pkg2): {errs:#?}"
    );
}

#[test]
fn private_call_in_map_lambda_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateFunctionApplicationInMapInDifferentPackage
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg1::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(strings: String[*]): String[*] {\
              $strings->map(s | pkg1::privateFunc($s, ' from public'))\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::privateFunc", "pkg2"),
        "expected NotAccessible(pkg1::privateFunc..., pkg2): {errs:#?}"
    );
}

#[test]
fn private_function_reference_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateFunctionReferenceInDifferentPackage.
    // The mangled-FQN form treats the function as a PackageableFunction
    // value (not a call). Walker must visit `ExprKind::PackageableElementRef`.
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg1::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(): String[0..1] {\
              pkg1::privateFunc_String_1__String_1__String_1_.functionName\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::privateFunc", "pkg2"),
        "expected NotAccessible(pkg1::privateFunc..., pkg2): {errs:#?}"
    );
}

#[test]
fn private_call_in_let_lambda_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateFunctionApplicationInLetInDifferentPackage
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private>> pkg1::privateFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(strings: String[*]): String[*] {\
              let f = {s: String[1] | pkg1::privateFunc($s, ' from public')};\
              $strings->map($f);\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::privateFunc", "pkg2"),
        "expected NotAccessible(pkg1::privateFunc..., pkg2): {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Protected functions — same / sub-package (positive)
// ---------------------------------------------------------------------------

#[test]
fn protected_call_in_same_package_succeeds() {
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.protected>> pkg::protFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg::publicFunc(s: String[1]): String[1] {\
              pkg::protFunc($s, ' from public')\
         }",
    );
    assert!(
        !has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::NotAccessible { .. }
        )),
        "should not emit NotAccessible: {errs:#?}"
    );
}

#[test]
fn protected_call_in_subpackage_succeeds() {
    // Java parity: TestAccess.testProtectedFunctionApplicationInSubPackage
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.protected>> pkg::protFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg::sub::publicFunc(s: String[1]): String[1] {\
              pkg::protFunc($s, ' from public')\
         }",
    );
    assert!(
        !has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::NotAccessible { .. }
        )),
        "should not emit NotAccessible: {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Protected functions — different package (negative)
// ---------------------------------------------------------------------------

#[test]
fn protected_call_in_different_package_fails() {
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.protected>> pkg1::protFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg2::publicFunc(s: String[1]): String[1] {\
              pkg1::protFunc($s, ' from public')\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::protFunc", "pkg2"),
        "expected NotAccessible(pkg1::protFunc..., pkg2): {errs:#?}"
    );
}

#[test]
fn protected_call_in_sibling_package_fails() {
    // pkg::sibling is a sibling of `pkg::sub` — the protected fn lives in
    // `pkg::sub`, so `pkg::sibling` is not the same or below it.
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.protected>> pkg::sub::protFunc(\
              s1: String[1], s2: String[1]): String[1] { 'x' }\n\
         function pkg::sibling::publicFunc(s: String[1]): String[1] {\
              pkg::sub::protFunc($s, ' from public')\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg::sub::protFunc", "pkg::sibling"),
        "expected NotAccessible(pkg::sub::protFunc..., pkg::sibling): {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Private classes — same / different package
// ---------------------------------------------------------------------------

#[test]
fn private_class_reference_in_same_package_succeeds() {
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.private>> pkg::PrivateClass { name: String[1]; }\n\
         function pkg::publicFunc(s: String[1]): pkg::PrivateClass[1] {\
              ^pkg::PrivateClass(name=$s)\
         }",
    );
    assert!(
        !has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::NotAccessible { .. }
        )),
        "should not emit NotAccessible: {errs:#?}"
    );
}

#[test]
fn private_class_reference_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateClassReferenceInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.private>> pkg1::PrivateClass { name: String[1]; }\n\
         function pkg2::publicFunc(s: String[1]): pkg1::PrivateClass[1] {\
              ^pkg1::PrivateClass(name=$s)\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::PrivateClass", "pkg2"),
        "expected NotAccessible(pkg1::PrivateClass, pkg2): {errs:#?}"
    );
}

#[test]
fn private_class_extension_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateClassExtensionInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.private>> pkg1::PrivateClass { name: String[1]; }\n\
         Class pkg2::PublicSubclass extends pkg1::PrivateClass {}",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::PrivateClass", "pkg2"),
        "expected NotAccessible(pkg1::PrivateClass, pkg2): {errs:#?}"
    );
}

#[test]
fn private_class_property_type_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateClassPropertyTypeInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.private>> pkg1::PrivateClass { name: String[1]; }\n\
         Class pkg2::PublicClass { prop: pkg1::PrivateClass[1]; }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::PrivateClass", "pkg2"),
        "expected NotAccessible(pkg1::PrivateClass, pkg2): {errs:#?}"
    );
}

#[test]
fn private_class_assoc_property_type_in_different_package_fails() {
    // Java parity: TestAccess.testPrivateClassAssociationPropertyTypeInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.private>> pkg1::PrivateClass { name: String[1]; }\n\
         Class pkg1::PublicClass {}\n\
         Association pkg2::PublicAssociation {\
              left: pkg1::PrivateClass[1];\
              right: pkg1::PublicClass[1];\
         }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::PrivateClass", "pkg2"),
        "expected NotAccessible(pkg1::PrivateClass, pkg2): {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Protected classes
// ---------------------------------------------------------------------------

#[test]
fn protected_class_reference_in_subpackage_succeeds() {
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.protected>> pkg::ProtClass { name: String[1]; }\n\
         function pkg::sub::publicFunc(s: String[1]): pkg::ProtClass[1] {\
              ^pkg::ProtClass(name=$s)\
         }",
    );
    assert!(
        !has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::NotAccessible { .. }
        )),
        "should not emit NotAccessible: {errs:#?}"
    );
}

#[test]
fn protected_class_extension_in_different_package_fails() {
    // Java parity: TestAccess.testProtectedClassExtensionInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.protected>> pkg1::ProtClass { name: String[1]; }\n\
         Class pkg2::PublicSubclass extends pkg1::ProtClass {}",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::ProtClass", "pkg2"),
        "expected NotAccessible(pkg1::ProtClass, pkg2): {errs:#?}"
    );
}

#[test]
fn protected_class_property_type_in_different_package_fails() {
    // Java parity: TestAccess.testProtectedClassPropertyTypeInDifferentPackage
    let errs = compile_with_access(
        "Class <<meta::pure::profiles::access.protected>> pkg1::ProtClass { name: String[1]; }\n\
         Class pkg2::PublicClass { prop: pkg1::ProtClass[1]; }",
    );
    assert!(
        has_not_accessible_with(&errs, "pkg1::ProtClass", "pkg2"),
        "expected NotAccessible(pkg1::ProtClass, pkg2): {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Multiple access stereotypes
// ---------------------------------------------------------------------------

#[test]
fn multiple_access_stereotypes_on_function_fails() {
    // Java parity: TestAccess.testMultipleAccessLevels
    let errs = compile_with_access(
        "function <<meta::pure::profiles::access.private, meta::pure::profiles::access.protected>> \
              pkg::func(s1: String[1], s2: String[1]): String[1] { 'x' }",
    );
    assert!(
        has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::MultipleAccessLevels { .. }
        )),
        "expected MultipleAccessLevels: {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// Properties cannot carry access stereotypes
// ---------------------------------------------------------------------------

#[test]
fn private_stereotype_on_property_fails() {
    // Java parity: TestAccess.testPrivateProperty
    let errs = compile_with_access(
        "Class pkg::TestClass { <<meta::pure::profiles::access.private>> name: String[1]; }",
    );
    assert!(
        has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::AccessLevelNotAllowed { .. }
        )),
        "expected AccessLevelNotAllowed on property: {errs:#?}"
    );
}

#[test]
fn protected_stereotype_on_property_fails() {
    // Java parity: TestAccess.testProtectedProperty
    let errs = compile_with_access(
        "Class pkg::TestClass { <<meta::pure::profiles::access.protected>> name: String[1]; }",
    );
    assert!(
        has_kind(&errs, |k| matches!(
            k,
            CompilationErrorKind::AccessLevelNotAllowed { .. }
        )),
        "expected AccessLevelNotAllowed on property: {errs:#?}"
    );
}
