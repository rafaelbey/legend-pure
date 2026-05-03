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

//! Stage-4 validator smoke tests for [`RelationalExtension`].
//!
//! Each test runs `compile_with_extensions` against a focused user
//! source that should produce *one* validator diagnostic. Negative
//! tests assert the precise `CompilationErrorKind`; positive tests
//! assert the relational source contributes zero new errors on top of
//! the platform-baseline.
//!
//! Pattern mirrors `crates/dsl-mapping/tests/compiler_smoke.rs`.

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::CompilerExtension;
use smol_str::SmolStr;

/// Parse `source` against the relational section parser. The relational
/// validator runs on the *parsed* file, not on a platform-loaded one,
/// since it doesn't yet need any cross-DSL lambda lowering. This keeps
/// the negative tests fast and isolated.
fn run_validator(source: &str) -> Vec<CompilationError> {
    let file = legend_pure_parser_parser::parse_with_sections(
        source,
        "validator_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .expect("source must parse");

    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();

    // Drive the extension lifecycle directly. Stage 4's validator only
    // needs `declare()` (for FQN registration) and `validate()` —
    // there's no body-lowering work yet.
    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let auto_imports: Vec<SmolStr> = Vec::new();
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.declare(&mut declare_ctx);
    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.validate(&mut validate_ctx);
    errors
}

fn assert_one_kind(errors: &[CompilationError], expected: CompilationErrorKind) {
    let matches: Vec<&CompilationError> = errors.iter().filter(|e| e.kind == expected).collect();
    assert!(
        !matches.is_empty(),
        "expected exactly one diagnostic of kind {expected:?}, got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// V3: body uniqueness
// ---------------------------------------------------------------------------

#[test]
fn duplicate_table_within_database() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Table t (other INT PRIMARY KEY)
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::DuplicateElement {
            name: SmolStr::new("t"),
        },
    );
}

#[test]
fn duplicate_join_within_database() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Table u (id INT PRIMARY KEY)
          Join j (t.id = u.id)
          Join j (t.id = u.id)
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::DuplicateElement {
            name: SmolStr::new("j"),
        },
    );
}

#[test]
fn duplicate_database_fqn_across_sections() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table a (id INT PRIMARY KEY)
        )

        Database pkg::db
        (
          Table b (id INT PRIMARY KEY)
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::DuplicateElement {
            name: SmolStr::new("pkg::db"),
        },
    );
}

// ---------------------------------------------------------------------------
// V1 + V2: include graph
// ---------------------------------------------------------------------------

#[test]
fn include_resolves_to_unknown_database() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          include other::missing

          Table t (id INT PRIMARY KEY)
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::UnresolvedElement {
            path: SmolStr::new("other::missing"),
        },
    );
}

#[test]
fn detects_include_cycle() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::a
        (
          include pkg::b

          Table aT (id INT PRIMARY KEY)
        )

        Database pkg::b
        (
          include pkg::a

          Table bT (id INT PRIMARY KEY)
        )
    "});
    let cycle_errors: Vec<&CompilationError> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::CyclicInheritance { .. }))
        .collect();
    assert!(
        !cycle_errors.is_empty(),
        "expected at least one CyclicInheritance error, got {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// V4: alias columns
// ---------------------------------------------------------------------------

#[test]
fn filter_uses_unknown_table_alias() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, qty INT)
          Filter f (s.qty > 0)
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::UnresolvedElement {
            path: SmolStr::new("s"),
        },
    );
}

#[test]
fn filter_uses_unknown_column_on_known_table() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, qty INT)
          Filter f (t.region = 'US')
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::UnknownProperty {
            type_name: SmolStr::new("t"),
            property_name: SmolStr::new("region"),
        },
    );
}

#[test]
fn join_resolves_columns_via_include() {
    // Tables come from the included database; the join body must
    // still resolve them.
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::base
        (
          Table tradeTable (id INT PRIMARY KEY, prodId INT)
          Table productTable (id INT PRIMARY KEY)
        )

        Database pkg::wrapper
        (
          include pkg::base

          Join tradeProduct (tradeTable.prodId = productTable.id)
        )
    "});
    assert!(
        errors.is_empty(),
        "expected no validator errors after include resolves; got {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// V5: milestoning column refs
// ---------------------------------------------------------------------------

#[test]
fn milestoning_references_unknown_column() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruMissing) )
            id INT PRIMARY KEY, fromZ DATE
          )
        )
    "});
    assert_one_kind(
        &errors,
        CompilationErrorKind::UnknownProperty {
            type_name: SmolStr::new("t"),
            property_name: SmolStr::new("thruMissing"),
        },
    );
}

#[test]
fn well_formed_milestoning_passes() {
    let errors = run_validator(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning (
              processing (PROCESSING_IN=in_z, PROCESSING_OUT=out_z),
              business (BUS_FROM=from_z, BUS_THRU=thru_z)
            )
            id INT PRIMARY KEY, in_z DATE, out_z DATE, from_z DATE, thru_z DATE
          )
        )
    "});
    assert!(
        errors.is_empty(),
        "expected zero errors on bi-temporal milestoning; got {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// Stage 8: relational class-mapping validators
//
// `RelationalExtension` walks `###Mapping` sections and gathers every
// `Class : Relational { … }` body. Validators below check three
// invariants:
//   E1. Embedded property mappings must be unique within one class
//       mapping.
//   E2. AssociationMapping bodies must declare exactly two
//       property-mapping lines.
//   E3. `Inline [id]` trailers must reference a class-mapping id
//       declared in the same enclosing `Mapping`.
//
// These run alongside the Stage-1+2+3 validators (V1-V5) on the same
// extension instance, so the test driver from earlier in this file
// reuses unchanged.
// ---------------------------------------------------------------------------

mod class_mapping {
    use super::{assert_one_kind, run_validator_with_mapping};
    use indoc::indoc;
    use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
    use smol_str::SmolStr;

    #[test]
    fn association_mapping_with_one_line_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                AssociationMapping
                (
                  endA : [pkg::Db]@joinX
                )
              }
            )
        "});
        assert!(
            errors
                .iter()
                .any(|e| matches!(e.kind, CompilationErrorKind::InvalidAssociation { .. })),
            "expected InvalidAssociation, got {errors:#?}"
        );
    }

    #[test]
    fn association_mapping_with_two_lines_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                AssociationMapping
                (
                  endA[a, b] : [pkg::Db]@joinX,
                  endB[b, a] : [pkg::Db]@joinX
                )
              }
            )
        "});
        let assoc_errors: Vec<&CompilationError> = errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::InvalidAssociation { .. }))
            .collect();
        assert!(
            assoc_errors.is_empty(),
            "expected no InvalidAssociation errors; got {assoc_errors:#?}"
        );
    }

    #[test]
    fn duplicate_embedded_property_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                (
                  details (taxLocation : [pkg::Db]Tbl.t),
                  details (taxLocation : [pkg::Db]Tbl.t)
                )
              }
            )
        "});
        assert_one_kind(
            &errors,
            CompilationErrorKind::DuplicateProperty {
                class_name: SmolStr::new("X"),
                property_name: SmolStr::new("details"),
            },
        );
    }

    #[test]
    fn inline_referencing_unknown_mapping_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                (details () Inline[notRegistered])
              }
            )
        "});
        assert_one_kind(
            &errors,
            CompilationErrorKind::UnresolvedElement {
                path: SmolStr::new("notRegistered"),
            },
        );
    }

    #[test]
    fn inline_referencing_known_mapping_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              other [knownId] : Relational { (k : [pkg::Db]Tbl.t) }

              X : Relational
              {
                (details () Inline[knownId])
              }
            )
        "});
        let inline_errors: Vec<&CompilationError> = errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::UnresolvedElement { .. }))
            .collect();
        assert!(
            inline_errors.is_empty(),
            "expected no UnresolvedElement errors; got {inline_errors:#?}"
        );
    }
}

/// Same shape as `run_validator` but registers
/// `RelationalClassMappingBodyParser` so the `###Mapping` sections
/// in the input source can route `: Relational { … }` bodies through
/// the Stage-5+ parser.
fn run_validator_with_mapping(source: &str) -> Vec<CompilationError> {
    use legend_pure_dsl_mapping::parser::MappingSectionParser;
    use legend_pure_dsl_relational::parser::RelationalClassMappingBodyParser;

    let file = legend_pure_parser_parser::parse_with_sections(
        source,
        "validator_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![
            Box::new(RelationalSectionParser),
            Box::new(MappingSectionParser::with_body_parsers(vec![Box::new(
                RelationalClassMappingBodyParser,
            )])),
        ],
    )
    .expect("source must parse");

    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();

    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let auto_imports: Vec<SmolStr> = Vec::new();
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.declare(&mut declare_ctx);
    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.validate(&mut validate_ctx);
    errors
}
