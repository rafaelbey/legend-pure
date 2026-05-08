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

//! Phase E3 store-substitution validator parity tests using
//! `RelationalClassMappingBody` (where `referenced_stores()` is
//! non-empty so the validator has actual stores to compare against).
//!
//! `crates/dsl-mapping/tests/store_substitution_smoke.rs` covers
//! Pure-DSL bodies (where `referenced_stores()` returns empty by
//! default); this file covers the relational-bodied counterpart and
//! ports the structural shape of Java's
//! `TestStoreSubstitutionValidator.testInValidNestedStoreSubstitution`.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::{
    RelationalClassMappingBodyParser, RelationalSectionParser,
};
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "store_substitution_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![
            Box::new(RelationalSectionParser),
            Box::new(MappingSectionParser::with_body_parsers(vec![Box::new(
                RelationalClassMappingBodyParser,
            )])),
        ],
    )
    .expect("source must parse")
}

fn compile_errors(source: &str) -> Vec<CompilationError> {
    let file = parse(source);
    let mapping_ext = MappingExtension::new();
    let relational_ext = RelationalExtension::new();
    let exts: [&dyn CompilerExtension; 2] = [&mapping_ext, &relational_ext];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

// ---------------------------------------------------------------------------
// E3 with relational bodies — Java parity:
// `TestStoreSubstitutionValidator.testValidDirectStoreSubstitution`
// ---------------------------------------------------------------------------

#[test]
fn relational_substitution_source_used_in_inner_mapping_passes() {
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database a::PersonFirmDatabase1
        (
          Table PersonTable1 (id INT PRIMARY KEY, name VARCHAR(100))
        )

        ###Relational
        Database a::PersonFirmDatabase2
        (
          Table PersonTable2 (id INT PRIMARY KEY, name VARCHAR(100))
        )

        ###Pure
        Class a::Person { id : Integer[1]; name : String[1]; }

        ###Mapping
        Mapping a::PersonFirmMapping2
        (
          a::Person : Relational
          {
            ~mainTable [a::PersonFirmDatabase2]PersonTable2
            (
              id : PersonTable2.id,
              name : PersonTable2.name
            )
          }
        )

        Mapping a::PersonFirmMapping1
        (
          // Substitute PersonFirmDatabase2 (used by Inner via
          // ~mainTable) → PersonFirmDatabase1. Inner's reference is
          // valid; substitution is structurally legal.
          include a::PersonFirmMapping2 [a::PersonFirmDatabase2 -> a::PersonFirmDatabase1]
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Store Substitution Error")),
        "expected clean direct substitution; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// E3 with relational bodies — Java parity:
// `TestStoreSubstitutionValidator.testInValidDirectStoreSubstitution`.
// ---------------------------------------------------------------------------

#[test]
fn relational_substitution_source_not_in_inner_mapping_errors() {
    // Inner's class mapping references PersonFirmDatabase2 (via
    // ~mainTable). Outer's substitution claims to substitute
    // PersonFirmDatabase5 → PersonFirmDatabase1, but
    // PersonFirmDatabase5 is NOT used by Inner — error.
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database a::PersonFirmDatabase1
        (
          Table PersonTable1 (id INT PRIMARY KEY)
        )

        ###Relational
        Database a::PersonFirmDatabase2
        (
          Table PersonTable2 (id INT PRIMARY KEY)
        )

        ###Relational
        Database a::PersonFirmDatabase5
        (
          Table PersonTable5 (id INT PRIMARY KEY)
        )

        ###Pure
        Class a::Person { id : Integer[1]; }

        ###Mapping
        Mapping a::PersonFirmMapping2
        (
          a::Person : Relational
          {
            ~mainTable [a::PersonFirmDatabase2]PersonTable2
            (id : PersonTable2.id)
          }
        )

        Mapping a::PersonFirmMapping1
        (
          include a::PersonFirmMapping2 [a::PersonFirmDatabase5 -> a::PersonFirmDatabase1]
        )
    "});
    assert!(
        errors.iter().any(|e| e
            .message
            .contains("Store Substitution Error in mapping [a::PersonFirmMapping1]")
            && e.message.contains("a::PersonFirmDatabase5")
            && e.message.contains("a::PersonFirmMapping2")),
        "expected store-substitution-error matching Java's diagnostic; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// E3 with relational bodies — Java parity:
// `TestStoreSubstitutionValidator.testInValidNestedStoreSubstitution`.
// ---------------------------------------------------------------------------

#[test]
fn relational_substitution_via_inner_substitution_target_passes() {
    // Inner's PersonFirmDatabase4-typed mapping; Middle includes
    // Inner with `[PersonFirmDatabase5 -> PersonFirmDatabase3]`
    // (note: Inner doesn't actually USE PersonFirmDatabase5; this
    // test focuses on how Middle's substitution-targets become
    // accessible stores from Outer's perspective). Outer then
    // substitutes `PersonFirmDatabase3 -> PersonFirmDatabase1`,
    // which IS a Middle-accessible store.
    let errors = compile_errors(indoc! {r"
        ###Relational
        Database a::PersonFirmDatabase1
        (
          Table T1 (id INT PRIMARY KEY)
        )

        ###Relational
        Database a::PersonFirmDatabase2
        (
          Table T2 (id INT PRIMARY KEY)
        )

        ###Relational
        Database a::PersonFirmDatabase3
        (
          Table T3 (id INT PRIMARY KEY)
        )

        ###Pure
        Class a::PersonA { id : Integer[1]; }
        Class a::PersonB { id : Integer[1]; }

        ###Mapping
        Mapping a::Inner
        (
          a::PersonA : Relational
          {
            ~mainTable [a::PersonFirmDatabase2]T2
            (id : T2.id)
          }
        )

        Mapping a::Middle
        (
          // Middle's accessible stores = {DB2 (from inner), DB3
          // (substitution target rewriting DB2)}.
          include a::Inner [a::PersonFirmDatabase2 -> a::PersonFirmDatabase3]
        )

        Mapping a::Outer
        (
          include a::Middle [a::PersonFirmDatabase3 -> a::PersonFirmDatabase1]

          a::PersonB : Relational
          {
            ~mainTable [a::PersonFirmDatabase1]T1
            (id : T1.id)
          }
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Store Substitution Error")),
        "expected substitution via inner-target to pass; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
