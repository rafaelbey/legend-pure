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

//! Cross-hierarchy property override compat tests.
//!
//! Java parity: `ClassValidator.validatePropertyOverrides`. Simple
//! properties are invariant on type+mult; qualified properties follow
//! LSP rules (covariant return, contravariant params). Validation
//! lives in `crate::validate::validate_property_overrides`, called
//! from `finalize_model` once the merged model is in hand.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn run(source: &str) -> Vec<CompilationError> {
    let sf = parse(source, "/test/property_overrides.pure");
    let mut model = init_bootstrap_model();
    let (_r, mut errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    errs.extend(finalize_model(&mut model, &[], &[]));
    errs
}

fn collect_conflicts(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::PropertyConflict { .. }))
        .collect()
}

// ---------------------------------------------------------------------------
// Simple property overrides
// ---------------------------------------------------------------------------

#[test]
fn simple_override_same_type_and_mult_is_legal() {
    // Mirrors the M3 platform pattern where `Type.name` shadows
    // `ModelElement.name` — both `String[1]`, so no conflict.
    let errs = run(
        r"
        Class abc::Base { prop : String[1]; }
        Class abc::Sub extends abc::Base { prop : String[1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "identical type+mult override should be legal; got: {errs:?}"
    );
}

#[test]
fn simple_override_different_type_is_rejected() {
    let errs = run(
        r"
        Class abc::Base { prop : String[1]; }
        Class abc::Sub extends abc::Base { prop : Integer[1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "type-mismatch override should conflict; got: {errs:?}"
    );
    let CompilationErrorKind::PropertyConflict {
        class_name,
        super_class_name,
        property_name,
    } = &hits[0].kind
    else {
        unreachable!();
    };
    assert_eq!(class_name.as_str(), "Sub");
    assert_eq!(super_class_name.as_str(), "Base");
    assert_eq!(property_name.as_str(), "prop");
}

#[test]
fn simple_override_different_mult_is_rejected() {
    let errs = run(
        r"
        Class abc::Base { prop : String[1]; }
        Class abc::Sub extends abc::Base { prop : String[0..1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "mult-mismatch override should conflict; got: {errs:?}"
    );
}

#[test]
fn override_with_subtype_is_rejected_for_simple_property() {
    // Simple properties are invariant — even a more-specific subtype
    // (Integer <: Number) is rejected. Java parity: only qualified
    // properties get the LSP relaxation.
    let errs = run(
        r"
        Class abc::Base { prop : Number[1]; }
        Class abc::Sub extends abc::Base { prop : Integer[1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "covariant override on a simple property should conflict; got: {errs:?}"
    );
}

#[test]
fn transitive_inheritance_chain_is_validated() {
    // Conflict must surface across a B → A → Root chain when B
    // redeclares a property declared two levels up.
    let errs = run(
        r"
        Class abc::Root { prop : String[1]; }
        Class abc::Middle extends abc::Root {}
        Class abc::Leaf extends abc::Middle { prop : Integer[1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "transitive override conflict should be detected; got: {errs:?}"
    );
    let CompilationErrorKind::PropertyConflict {
        class_name,
        super_class_name,
        ..
    } = &hits[0].kind
    else {
        unreachable!();
    };
    assert_eq!(class_name.as_str(), "Leaf");
    assert_eq!(super_class_name.as_str(), "Root");
}

// ---------------------------------------------------------------------------
// Qualified property overrides
// ---------------------------------------------------------------------------

#[test]
fn qualified_override_identical_signature_is_legal() {
    let errs = run(
        r"
        Class abc::Base {
            describe(){'hi'}: String[1];
        }
        Class abc::Sub extends abc::Base {
            describe(){'hello'}: String[1];
        }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "identical-signature QP override should be legal; got: {errs:?}"
    );
}

#[test]
fn qualified_override_covariant_return_is_legal() {
    // Return Integer <: Number on subclass — LSP-legal.
    let errs = run(
        r"
        Class abc::Base {
            value(){0}: Number[1];
        }
        Class abc::Sub extends abc::Base {
            value(){1}: Integer[1];
        }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "covariant QP return type should be legal; got: {errs:?}"
    );
}

#[test]
fn qualified_override_incompatible_return_is_rejected() {
    // Number is NOT a subtype of Integer — illegal narrowing on the
    // wrong direction.
    let errs = run(
        r"
        Class abc::Base {
            value(){1}: Integer[1];
        }
        Class abc::Sub extends abc::Base {
            value(){1.0}: Number[1];
        }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "incompatible QP return type should conflict; got: {errs:?}"
    );
}

// ---------------------------------------------------------------------------
// QP same-name different-arity coexist (Java treats them as distinct)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Diamond inheritance — two parallel ancestors carrying the same name
// ---------------------------------------------------------------------------

#[test]
fn diamond_with_incompatible_parallel_declarations_conflicts_at_join() {
    // Neither `LeftBranch` nor `RightBranch` extends the other, so the
    // conflict only manifests at `Join`, which sees both. Java's nested
    // MRO loop catches this; we mirror it.
    let errs = run(
        r"
        Class abc::LeftBranch { name : String[1]; }
        Class abc::RightBranch { name : Integer[1]; }
        Class abc::Join extends abc::LeftBranch, abc::RightBranch {}
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        !hits.is_empty(),
        "diamond conflict should surface at the join class; got: {errs:?}"
    );
    let CompilationErrorKind::PropertyConflict {
        class_name,
        property_name,
        ..
    } = &hits[0].kind
    else {
        unreachable!();
    };
    assert_eq!(
        class_name.as_str(),
        "Join",
        "conflict should be attributed to the join class"
    );
    assert_eq!(property_name.as_str(), "name");
}

#[test]
fn diamond_with_identical_parallel_declarations_is_legal() {
    // `Type.name` shadowing `ModelElement.name` is the M3 platform's
    // canonical diamond — both `String[1]`, so the redeclaration is
    // benign. Verify that pattern in miniature.
    let errs = run(
        r"
        Class abc::LeftBranch { name : String[1]; }
        Class abc::RightBranch { name : String[1]; }
        Class abc::Join extends abc::LeftBranch, abc::RightBranch {}
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "identical parallel declarations should be legal; got: {errs:?}"
    );
}

#[test]
fn diamond_conflict_reported_once_not_per_descendant() {
    // Without dedup, every descendant of the join class would re-report
    // the same `(LeftBranch.name, RightBranch.name)` conflict. The
    // dedup HashSet collapses those.
    let errs = run(
        r"
        Class abc::LeftBranch { name : String[1]; }
        Class abc::RightBranch { name : Integer[1]; }
        Class abc::Join extends abc::LeftBranch, abc::RightBranch {}
        Class abc::LeafA extends abc::Join {}
        Class abc::LeafB extends abc::Join {}
        ",
    );
    let hits = collect_conflicts(&errs);
    assert_eq!(
        hits.len(),
        1,
        "diamond conflict should be reported exactly once; got {} hits: {errs:?}",
        hits.len()
    );
}

// ---------------------------------------------------------------------------
// Generic specialization (parametric class with a concrete subclass)
// ---------------------------------------------------------------------------

#[test]
fn generic_specialization_with_matching_property_is_legal() {
    // The parent declares `value : T[1]`. The child fixes `T = String` at
    // the supertype reference and redeclares `value : String[1]`. The
    // validator's structural-compat helper is permissive on the
    // `TypeExpr::Generic` placeholder, so this should be accepted.
    let errs = run(
        r"
        Class abc::Container<T> { value : T[1]; }
        Class abc::StringContainer extends abc::Container<String> { value : String[1]; }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "concrete specialization of a generic property should be legal; got: {errs:?}"
    );
}

#[test]
fn qualified_with_different_param_count_coexists_with_inherited() {
    // Java matches inherited QPs on `(name, paramCount)`. A subclass QP
    // with the same name but different arity doesn't trigger override
    // validation against an inherited QP with a different arity.
    let errs = run(
        r"
        Class abc::Base {
            describe(){'base'}: String[1];
        }
        Class abc::Sub extends abc::Base {
            describe(suffix : String[1]){$suffix}: String[1];
        }
        ",
    );
    let hits = collect_conflicts(&errs);
    assert!(
        hits.is_empty(),
        "differing-arity QPs should coexist; got: {errs:?}"
    );
}
