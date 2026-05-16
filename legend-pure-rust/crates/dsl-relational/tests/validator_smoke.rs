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

    // Drive the full extension lifecycle: `declare` registers the
    // databases, `define_bodies` builds the resolved snapshots that
    // post-B validators (A3' join-tree-node chain checks) read in
    // `validate`, and `validate` runs the structural checks.
    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let auto_imports: Vec<SmolStr> = Vec::new();
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.declare(&mut declare_ctx);
    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.define_bodies(&mut define_ctx);
    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
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
        scope: None,
    };
    extension.declare(&mut declare_ctx);
    // Phase B1+ validators (e.g. A3' join-tree-node chain checks)
    // depend on the resolved-database snapshot built in `define_bodies`.
    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.define_bodies(&mut define_ctx);
    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.validate(&mut validate_ctx);
    errors
}

// ===========================================================================
// Stage-9: Java-parity validator additions (B + C + D + F + G3)
// ===========================================================================

mod stage9 {
    use super::{assert_one_kind, run_validator, run_validator_with_mapping};
    use indoc::indoc;
    use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
    use smol_str::SmolStr;

    // ---------------------------------------------------------------
    // B: column-name uniqueness within a Table
    // (Java: TestNameSpaces::testColumnNameConflict)
    // ---------------------------------------------------------------

    #[test]
    fn column_name_conflict_within_table_errors() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, qty INT, qty FLOAT(8))
            )
        "});
        assert_one_kind(
            &errors,
            CompilationErrorKind::DuplicateProperty {
                class_name: SmolStr::new("t"),
                property_name: SmolStr::new("qty"),
            },
        );
    }

    // ---------------------------------------------------------------
    // C: milestoning column TYPE check
    // (Java: TestSimpleGrammar::testBusinessSnapshotDateColumnType)
    // ---------------------------------------------------------------

    #[test]
    fn milestoning_date_field_with_non_date_column_errors() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (
                milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
                id INT PRIMARY KEY, fromZ DATE, thruZ INT
              )
            )
        "});
        assert!(
            errors.iter().any(
                |e| matches!(e.kind, CompilationErrorKind::InvalidAnnotation { .. })
                    && e.message.contains("BUS_THRU")
                    && e.message.contains("Date / Timestamp")
            ),
            "expected milestoning-type-mismatch error on BUS_THRU; got {errors:#?}"
        );
    }

    #[test]
    fn milestoning_inclusive_field_with_non_boolean_column_errors() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (
                milestoning ( business (
                  BUS_FROM=fromZ,
                  BUS_THRU=thruZ,
                  THRU_IS_INCLUSIVE=fromZ
                ) )
                id INT PRIMARY KEY, fromZ DATE, thruZ DATE
              )
            )
        "});
        assert!(
            errors.iter().any(
                |e| matches!(e.kind, CompilationErrorKind::InvalidAnnotation { .. })
                    && e.message.contains("THRU_IS_INCLUSIVE")
                    && e.message.contains("Boolean / Bit")
            ),
            "expected milestoning Boolean type-mismatch error; got {errors:#?}"
        );
    }

    #[test]
    fn well_formed_milestoning_with_correct_types_passes() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (
                milestoning ( business (
                  BUS_FROM=fromZ,
                  BUS_THRU=thruZ,
                  THRU_IS_INCLUSIVE=incFlag
                ) )
                id INT PRIMARY KEY,
                fromZ DATE,
                thruZ TIMESTAMP,
                incFlag BOOLEAN
              )
            )
        "});
        let type_errors: Vec<&CompilationError> = errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::InvalidAnnotation { .. }))
            .collect();
        assert!(
            type_errors.is_empty(),
            "expected no type mismatches on correctly-typed milestoning; got {type_errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // D: ~filter [db]name resolves to a Filter
    // (Java: TestSimpleGrammar::wrongClassMappingFilterIdentifierCausesError)
    // ---------------------------------------------------------------

    #[test]
    fn relational_filter_ref_must_resolve_to_known_filter() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::Db
            (
              Table t (id INT PRIMARY KEY, qty INT)
              Filter realFilter (t.qty > 0)
            )

            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                ~filter [pkg::Db]missingFilter
                (id : [pkg::Db]t.id)
              }
            )
        "});
        assert_one_kind(
            &errors,
            CompilationErrorKind::UnresolvedElement {
                path: SmolStr::new("missingFilter"),
            },
        );
    }

    #[test]
    fn relational_filter_ref_resolves_via_include() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::Base
            (
              Table t (id INT PRIMARY KEY, qty INT)
              Filter activeOnly (t.qty > 0)
            )
            Database pkg::Wrapper
            (
              include pkg::Base
            )

            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                ~filter [pkg::Wrapper]activeOnly
                (id : [pkg::Base]t.id)
              }
            )
        "});
        let unresolved: Vec<&CompilationError> = errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::UnresolvedElement { .. }))
            .collect();
        assert!(
            unresolved.is_empty(),
            "expected ~filter to resolve via include; got {unresolved:#?}"
        );
    }

    // ---------------------------------------------------------------
    // F: Otherwise duplicate property
    // (Java: TestEmbeddedGrammar::redundantOtherwiseMappingsWithTargetId)
    // ---------------------------------------------------------------

    #[test]
    fn otherwise_with_duplicate_property_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                (
                  details (
                    foo : [pkg::Db]t.x
                  )
                    Otherwise(
                      [taxLocation] : [pkg::Db]@firmDetails,
                      [taxLocation] : [pkg::Db]@firmDetails
                    )
                )
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::DuplicateProperty { property_name, .. }
                    if property_name.as_str() == "taxLocation"
            ) && e.message.contains("Otherwise")),
            "expected Otherwise duplicate-property error; got {errors:#?}"
        );
    }

    // A5b: Java-parity — Otherwise embedded body must declare at
    // least one inner property mapping line. Java errors with
    // "Invalid Otherwise mapping found: ... has no embedded
    // mappings defined, please use a property mapping with Join
    // instead."
    #[test]
    fn otherwise_with_empty_embedded_body_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              X : Relational
              {
                (
                  details ()
                    Otherwise(
                      [taxLocation] : [pkg::Db]@firmDetails
                    )
                )
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str().contains("embedded body cannot be empty")
            )),
            "expected empty-Otherwise-body error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // G3: extends not allowed on AssociationMapping
    // (Java: TestMappingInheritanceValidOnlyForClassMappings::testMappingInheritanceInValidForAssociationMapping)
    // ---------------------------------------------------------------

    #[test]
    fn extends_on_association_mapping_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Mapping
            Mapping pkg::M
            (
              Other [base] : Relational
              {
                AssociationMapping
                (
                  endA[a, b] : [pkg::Db]@joinX,
                  endB[b, a] : [pkg::Db]@joinX
                )
              }

              Firm_Person extends [base] : Relational
              {
                AssociationMapping
                (
                  endA[a, b] : [pkg::Db]@joinX,
                  endB[b, a] : [pkg::Db]@joinX
                )
              }
            )
        "});
        assert!(
            errors.iter().any(
                |e| matches!(&e.kind, CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str().contains("extends"))
            ),
            "expected extends-on-AssociationMapping error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // A8: extends rules on class mappings
    // (Java: TestExtendGrammar.testExtendInvalidSetCannotBeSelf,
    //        testExtendInvalidIdWithInclude)
    // ---------------------------------------------------------------

    #[test]
    fn class_mapping_extending_itself_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Person { id : Integer[1]; }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Person[p_subclass] extends [p_subclass] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str() == "class mapping extends itself"
            )),
            "expected extends-self error; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_extending_unknown_id_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Person { id : Integer[1]; }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Person[childMap] extends [doesNotExist] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path }
                    if path.as_str() == "doesNotExist"
            )),
            "expected extends-unknown-id error; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_extending_known_id_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Parent { id : Integer[1]; }
            Class pkg::Child extends pkg::Parent {}

            ###Mapping
            Mapping pkg::M
            (
              *pkg::Parent[parentMap] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }

              pkg::Child[childMap] extends [parentMap] : Relational
              {
                (id : t.id)
              }
            )
        "});
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str().contains("extends itself")
            ) && !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path }
                    if path.as_str() == "parentMap"
            )),
            "expected clean extends; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // E4: extends id resolves through include closure
    // (Java parity: TestExtendGrammar.testExtendWithInclude)
    // ---------------------------------------------------------------

    #[test]
    fn class_mapping_extending_id_in_included_mapping_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Parent { id : Integer[1]; }
            Class pkg::Child extends pkg::Parent {}

            ###Mapping
            Mapping pkg::BaseMap
            (
              *pkg::Parent[parentMap] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }
            )

            Mapping pkg::DerivedMap
            (
              include pkg::BaseMap

              pkg::Child[childMap] extends [parentMap] : Relational
              {
                (id : t.id)
              }
            )
        "});
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path }
                    if path.as_str() == "parentMap"
            )),
            "expected extends id to resolve via include; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_extending_id_not_in_includes_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Parent { id : Integer[1]; }
            Class pkg::Child extends pkg::Parent {}

            ###Mapping
            Mapping pkg::BaseMap
            (
              *pkg::Parent[parentMap] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }
            )

            Mapping pkg::OtherMap
            (
              pkg::Parent[otherParent] : Relational
              {
                ~mainTable [pkg::db]t
                (id : t.id)
              }
            )

            Mapping pkg::DerivedMap
            (
              // No include of pkg::BaseMap — `parentMap` shouldn't
              // resolve from this mapping.
              include pkg::OtherMap

              pkg::Child[childMap] extends [parentMap] : Relational
              {
                (id : t.id)
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path }
                    if path.as_str() == "parentMap"
            )),
            "expected extends-id-unresolved error; got {errors:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase A2: validator (E) — `@joinName` resolution
// ---------------------------------------------------------------------------

mod join_resolution {
    use super::{assert_one_kind, run_validator, run_validator_with_mapping};
    use indoc::indoc;
    use legend_pure_parser_pure::error::CompilationErrorKind;
    use smol_str::SmolStr;

    // ---------------------------------------------------------------
    // E.1 — class-mapping FilterMappingBlock: `~filter [db]@joinSeq | [db2] filterName`
    // requires the joins to live in `db` (the first qualifier).
    // ---------------------------------------------------------------

    #[test]
    fn class_mapping_filter_with_unknown_join_in_chain_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
              Filter active (t.id > 0)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~filter [pkg::db](unknown)@missingJoin | [pkg::db] active
                (id : t.id)
              }
            )
        "});
        // Expect the unresolved `@missingJoin` error.
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "missingJoin"
            )),
            "expected unresolved @missingJoin error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // E.2 — class-mapping body line value with explicit `[db]@unknownJoin`
    // ---------------------------------------------------------------

    #[test]
    fn class_mapping_value_with_unknown_join_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                (id : [pkg::db]@noSuchJoin | dst.id)
              }
            )
        "});
        assert_one_kind(
            &errors,
            CompilationErrorKind::UnresolvedElement {
                path: SmolStr::new("noSuchJoin"),
            },
        );
    }

    #[test]
    fn class_mapping_value_with_unknown_database_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join sToD (src.fk = dst.id)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                (id : [pkg::nope]@sToD | dst.id)
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "pkg::nope"
            )),
            "expected unresolved-database error for [pkg::nope]; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_value_with_known_join_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join sToD (src.fk = dst.id)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                (id : [pkg::db]@sToD | dst.id)
              }
            )
        "});
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "sToD"
            )),
            "expected no unresolved-join error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // E.3 — view body referencing an unknown `@join` (contextual db).
    // ---------------------------------------------------------------

    #[test]
    fn view_with_unknown_join_in_value_errors() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Schema s
              (
                Table src (id INT PRIMARY KEY, fk INT)
                Table dst (id INT PRIMARY KEY)
                View v (id : @noSuchJoin | dst.id)
              )
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "noSuchJoin"
            )),
            "expected unresolved-join error in view body; got {errors:#?}"
        );
    }

    #[test]
    fn view_with_known_join_passes() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Schema s
              (
                Table src (id INT PRIMARY KEY, fk INT)
                Table dst (id INT PRIMARY KEY)
                View v (id : @sToD | dst.id)
              )
              Join sToD (src.fk = dst.id)
            )
        "});
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "sToD"
            )),
            "expected no unresolved-join error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // E.4 — view filter chain with unknown join; joins live in db1.
    // ---------------------------------------------------------------

    #[test]
    fn view_with_filter_chain_unknown_join_errors() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Schema s
              (
                Table t (id INT PRIMARY KEY, fid INT)
                View v
                (
                  ~filter [pkg::db]@nope | [pkg::filterDb] activeFilter
                  id : t.id
                )
              )
            )

            ###Relational
            Database pkg::filterDb
            (
              Table f (id INT PRIMARY KEY)
              Filter activeFilter (f.id > 0)
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "nope"
            )),
            "expected unresolved view-filter chain join error; got {errors:#?}"
        );
    }

    // ---------------------------------------------------------------
    // E.5 — view referencing a join that lives in an INCLUDED db.
    // ---------------------------------------------------------------

    #[test]
    fn view_join_resolves_via_include() {
        let errors = run_validator(indoc! {r"
            ###Relational
            Database pkg::base
            (
              Table t1 (id INT PRIMARY KEY, fk INT)
              Table t2 (id INT PRIMARY KEY)
              Join t1t2 (t1.fk = t2.id)
            )

            ###Relational
            Database pkg::derived
            (
              include pkg::base
              Schema s
              (
                Table local (x INT PRIMARY KEY)
                View v (id : @t1t2 | t2.id)
              )
            )
        "});
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "t1t2"
            )),
            "expected join to resolve via include; got {errors:#?}"
        );
    }

    // -----------------------------------------------------------------
    // Phase B7 — implicit-db `@join` resolution via `~mainTable [db]`
    // -----------------------------------------------------------------

    #[test]
    fn class_mapping_implicit_db_join_via_main_table_resolves() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join sToD (src.fk = dst.id)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]src
                (id : @sToD | dst.id)
              }
            )
        "});
        // `@sToD` lacks an explicit `[db]` — should still resolve
        // via the `~mainTable [pkg::db]` contextual db.
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "sToD"
            )),
            "expected implicit-db join to resolve via ~mainTable; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_implicit_db_unknown_join_via_main_table_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]t
                (id : @noSuchJoin | t.id)
              }
            )
        "});
        // The contextual db is `pkg::db`; `@noSuchJoin` doesn't exist
        // there.
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "noSuchJoin"
            )),
            "expected unresolved-implicit-db-join error; got {errors:#?}"
        );
    }

    // -----------------------------------------------------------------
    // Phase A3' — JoinTreeNodeValidation parity
    // -----------------------------------------------------------------

    #[test]
    fn join_chain_with_consistent_end_tables_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table mid (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join sToM (src.fk = mid.id)
              Join mToD (mid.fk = dst.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]src
                (id : @sToM > @mToD | dst.id)
              }
            )
        "});
        // src → @sToM → mid → @mToD → dst → trailing column on dst.
        let chain_errs: Vec<_> = errors
            .iter()
            .filter(|e| match &e.kind {
                CompilationErrorKind::InvalidAssociation { reason, .. } => {
                    reason.as_str().contains("ends at")
                }
                CompilationErrorKind::UnresolvedElement { path } => {
                    path.as_str().starts_with("join @")
                }
                _ => false,
            })
            .collect();
        assert!(
            chain_errs.is_empty(),
            "expected clean join chain; got chain errors: {chain_errs:#?}"
        );
    }

    #[test]
    fn join_chain_disconnected_from_source_table_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY)
              Table other (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join otherToDst (other.fk = dst.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]src
                (id : @otherToDst | dst.id)
              }
            )
        "});
        // ~mainTable says src; @otherToDst connects (other, dst) — does
        // not contain src.
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path }
                    if path.as_str().contains("otherToDst")
            )),
            "expected 'join does not contain source' error; got {errors:#?}"
        );
    }

    #[test]
    fn join_chain_target_mismatch_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table mid (id INT PRIMARY KEY, fk INT)
              Table dst (id INT PRIMARY KEY)
              Join sToM (src.fk = mid.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]src
                (id : @sToM | dst.id)
              }
            )
        "});
        // Chain ends at 'mid' but trailing column references 'dst'.
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str().contains("ends at 'mid'")
            )),
            "expected join-chain target-mismatch error; got {errors:#?}"
        );
    }

    #[test]
    fn join_chain_ending_back_at_source_passes() {
        // Self-join: src → @j → mid → @j_inv → src — chain ends at src,
        // trailing column is on src, no error.
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table src (id INT PRIMARY KEY, fk INT)
              Table mid (id INT PRIMARY KEY, src_fk INT)
              Join sToM (src.fk = mid.id)
              Join mToS (mid.src_fk = src.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::db]src
                (id : @sToM > @mToS | src.id)
              }
            )
        "});
        let chain_errs: Vec<_> = errors
            .iter()
            .filter(|e| match &e.kind {
                CompilationErrorKind::InvalidAssociation { reason, .. } => {
                    reason.as_str().contains("ends at")
                }
                CompilationErrorKind::UnresolvedElement { path } => {
                    path.as_str().starts_with("join @")
                }
                _ => false,
            })
            .collect();
        assert!(
            chain_errs.is_empty(),
            "expected self-loop chain to validate; got chain errors: {chain_errs:#?}"
        );
    }

    // -----------------------------------------------------------------
    // Phase A4 — RelationalAssociationImplementationValidator parity
    // -----------------------------------------------------------------

    #[test]
    fn association_with_well_formed_join_chain_passes() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table tradeT (id INT PRIMARY KEY, prodId INT)
              Table prodT (id INT PRIMARY KEY)
              Join tradeProd (tradeT.prodId = prodT.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }
            Class pkg::Product { id : Integer[1]; }
            Association pkg::TradeProd
            {
              trade : pkg::Trade[1];
              product : pkg::Product[1];
            }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Trade[tradeMap] : Relational
              {
                ~mainTable [pkg::db]tradeT
                (id : tradeT.id)
              }

              pkg::Product[prodMap] : Relational
              {
                ~mainTable [pkg::db]prodT
                (id : prodT.id)
              }

              pkg::TradeProd : Relational
              {
                AssociationMapping
                (
                  trade[prodMap, tradeMap] : [pkg::db]@tradeProd | tradeT.id,
                  product[tradeMap, prodMap] : [pkg::db]@tradeProd | prodT.id
                )
              }
            )
        "});
        let assoc_errs: Vec<_> = errors
            .iter()
            .filter(|e| match &e.kind {
                CompilationErrorKind::InvalidAssociation { reason, .. } => {
                    reason.as_str().contains("expected a join")
                        || reason.as_str().contains("ends at")
                        || reason.as_str().contains("no main table")
                }
                _ => false,
            })
            .collect();
        assert!(
            assoc_errs.is_empty(),
            "expected clean association mapping; got {assoc_errs:#?}"
        );
    }

    #[test]
    fn association_line_without_join_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table tradeT (id INT PRIMARY KEY, prodId INT)
              Table prodT (id INT PRIMARY KEY)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }
            Class pkg::Product { id : Integer[1]; }
            Association pkg::TradeProd
            {
              trade : pkg::Trade[1];
              product : pkg::Product[1];
            }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Trade[tradeMap] : Relational
              {
                ~mainTable [pkg::db]tradeT
                (id : tradeT.id)
              }

              pkg::Product[prodMap] : Relational
              {
                ~mainTable [pkg::db]prodT
                (id : prodT.id)
              }

              pkg::TradeProd : Relational
              {
                AssociationMapping
                (
                  trade[prodMap, tradeMap] : [pkg::db]tradeT.id,
                  product[tradeMap, prodMap] : [pkg::db]prodT.id
                )
              }
            )
        "});
        // Both lines lack a join sequence — Java emits "expected a join"
        // for each.
        let count = errors
            .iter()
            .filter(|e| {
                matches!(
                    &e.kind,
                    CompilationErrorKind::InvalidAssociation { reason, .. }
                        if reason.as_str() == "expected a join"
                )
            })
            .count();
        assert_eq!(
            count, 2,
            "expected two 'expected a join' errors; got {errors:#?}"
        );
    }

    #[test]
    fn association_unknown_source_id_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table tradeT (id INT PRIMARY KEY, prodId INT)
              Table prodT (id INT PRIMARY KEY)
              Join tradeProd (tradeT.prodId = prodT.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }
            Class pkg::Product { id : Integer[1]; }
            Association pkg::TradeProd
            {
              trade : pkg::Trade[1];
              product : pkg::Product[1];
            }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Trade[tradeMap] : Relational
              {
                ~mainTable [pkg::db]tradeT
                (id : tradeT.id)
              }

              pkg::Product[prodMap] : Relational
              {
                ~mainTable [pkg::db]prodT
                (id : prodT.id)
              }

              pkg::TradeProd : Relational
              {
                AssociationMapping
                (
                  trade[noSuchMap, tradeMap] : [pkg::db]@tradeProd | tradeT.id,
                  product[tradeMap, prodMap] : [pkg::db]@tradeProd | prodT.id
                )
              }
            )
        "});
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "noSuchMap"
            )),
            "expected unresolved-source-id error; got {errors:#?}"
        );
    }

    #[test]
    fn association_chain_disconnected_from_target_errors() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table tradeT (id INT PRIMARY KEY, fk INT)
              Table prodT (id INT PRIMARY KEY)
              Table other (id INT PRIMARY KEY, fk INT)
              Join tradeOther (tradeT.fk = other.id)
            )

            ###Pure
            Class pkg::Trade { id : Integer[1]; }
            Class pkg::Product { id : Integer[1]; }
            Association pkg::TradeProd
            {
              trade : pkg::Trade[1];
              product : pkg::Product[1];
            }

            ###Mapping
            Mapping pkg::M
            (
              pkg::Trade[tradeMap] : Relational
              {
                ~mainTable [pkg::db]tradeT
                (id : tradeT.id)
              }

              pkg::Product[prodMap] : Relational
              {
                ~mainTable [pkg::db]prodT
                (id : prodT.id)
              }

              pkg::TradeProd : Relational
              {
                AssociationMapping
                (
                  trade[prodMap, tradeMap] : [pkg::db]@tradeOther | other.id,
                  product[tradeMap, prodMap] : [pkg::db]@tradeOther | other.id
                )
              }
            )
        "});
        // Chain ends at 'other' but target main_table is 'prodT' /
        // 'tradeT'; one or both lines should error.
        assert!(
            errors.iter().any(|e| matches!(
                &e.kind,
                CompilationErrorKind::InvalidAssociation { reason, .. }
                    if reason.as_str().contains("ends at")
            )),
            "expected target-mismatch error; got {errors:#?}"
        );
    }

    #[test]
    fn class_mapping_scope_db_overrides_main_table_for_implicit_join() {
        let errors = run_validator_with_mapping(indoc! {r"
            ###Relational
            Database pkg::main
            (
              Table src (id INT PRIMARY KEY)
            )

            ###Relational
            Database pkg::other
            (
              Table dst (id INT PRIMARY KEY, fk INT)
              Join scopedJoin (dst.fk = dst.id)
            )

            ###Pure
            import pkg::*;
            Class pkg::Trade { id : Integer[1]; }

            ###Mapping
            Mapping pkg::TradeMap
            (
              pkg::Trade : Relational
              {
                ~mainTable [pkg::main]src
                (
                  id : src.id,
                  scope([pkg::other]) (
                    altId : @scopedJoin | dst.id
                  )
                )
              }
            )
        "});
        // `scope([pkg::other])` overrides `~mainTable [pkg::main]`.
        // `@scopedJoin` lives in pkg::other — should resolve.
        assert!(
            errors.iter().all(|e| !matches!(
                &e.kind,
                CompilationErrorKind::UnresolvedElement { path } if path.as_str() == "scopedJoin"
            )),
            "expected scope-db override to resolve join; got {errors:#?}"
        );
    }
}
