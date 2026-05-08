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

//! Phase E: store-substitution validator smoke tests.
//!
//! Mirrors Java's `StoreSubstitutionValidator` and the substitution-cycle
//! detector inside `DatabaseSubstitutionHandler.collectStoreSubstitutionsAlongPath`.
//!
//! Each fixture compiles a `###Pure` + `###Mapping` source through the
//! compile pipeline and asserts the right validator diagnostic fires
//! (or doesn't, for the positive cases). Mapping-include cycle
//! detection lives in `validator_smoke.rs::cyclic_mapping_include_errors` —
//! this module only covers store-substitution-specific rules.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "store_substitution_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn compile_errors(source: &str) -> Vec<CompilationError> {
    let file = parse(source);
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

// ---------------------------------------------------------------------------
// E1: Substitution endpoint FQN resolution
// ---------------------------------------------------------------------------

#[test]
fn substitution_source_unresolved_errors() {
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        // Only my::test::DbB exists; my::test::DbA is the substitution
        // target but the source `my::test::Missing` does not exist.
        Class my::test::DbA {}
        Class my::test::DbB {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::Missing -> my::test::DbA]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Store substitution source")
                && e.message.contains("my::test::Missing")),
        "expected unresolved-source error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn substitution_target_unresolved_errors() {
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::DbA -> my::test::MissingTarget]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Store substitution target")
                && e.message.contains("my::test::MissingTarget")),
        "expected unresolved-target error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn substitution_with_known_endpoints_passes() {
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        // Source + target both exist as classes (we don't yet
        // discriminate by store kind — element-existence is the bar).
        Class my::test::DbA {}
        Class my::test::DbB {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::DbA -> my::test::DbB]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Store substitution")),
        "expected no Store-substitution errors; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// E2: Substitution-chain cycle detection
// ---------------------------------------------------------------------------

#[test]
fn substitution_chain_acyclic_passes() {
    // A→C and B→C are independent edges; no cycle.
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}
        Class my::test::DbB {}
        Class my::test::DbC {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Middle
        (
          include my::test::Inner [my::test::DbA -> my::test::DbC]
        )

        Mapping my::test::Outer
        (
          include my::test::Middle [my::test::DbB -> my::test::DbC]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Cyclic Store Substitution")),
        "expected acyclic graph; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn substitution_chain_with_a_to_b_to_a_errors() {
    // BMapping has A→C; OuterMapping has C→A through include of
    // BMapping. Combined edges: {A→C, C→A} — cycle.
    // (Mirror of Java's `TestCyclicStoreSubstitutionInMappingHierarchy`
    // direct case.)
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}
        Class my::test::DbC {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Middle
        (
          include my::test::Inner [my::test::DbA -> my::test::DbC]
        )

        Mapping my::test::Outer
        (
          include my::test::Middle [my::test::DbC -> my::test::DbA]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Cyclic Store Substitution")
                && e.message.contains("my::test::Db")),
        "expected substitution-cycle error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn substitution_self_loop_errors() {
    // A→A — self-loop.
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::DbA -> my::test::DbA]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Cyclic Store Substitution")),
        "expected self-loop cycle error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// E3: Substitution source must be a store used by the included mapping
// ---------------------------------------------------------------------------

// E3 fixtures use Pure-DSL bodies whose `referenced_stores()` is the
// empty default. Java parity: a Pure-DSL mapping carries no store, so
// any substitution against it has no valid source store and the
// validator fires. Relational-body fixtures live in the
// dsl-relational test crate (where `RelationalClassMappingBody`'s
// override of `referenced_stores()` lives).

#[test]
fn pure_dsl_included_mapping_has_no_stores_so_substitution_source_errors() {
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}
        Class my::test::DbB {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::DbA -> my::test::DbB]

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Store Substitution Error")
                && e.message.contains("my::test::DbA")
                && e.message.contains("my::test::Inner")),
        "expected source-not-in-included-stores error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn substitution_chain_through_pure_dsl_inner_errors() {
    // Java parity: a Pure-DSL inner mapping has no
    // `referenced_stores()`, so the substitution rewrite map can
    // never produce an accessible store at any level. Two-level
    // substitution chain through Pure-DSL bodies should error at
    // both levels — the `[DbA -> DbX]` first because Inner doesn't
    // use DbA, and `[DbX -> DbY]` second because Middle doesn't
    // expose DbX (Inner had nothing to rewrite).
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }
        Class my::test::DbA {}
        Class my::test::DbX {}
        Class my::test::DbY {}

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Middle
        (
          include my::test::Inner [my::test::DbA -> my::test::DbX]
        )

        Mapping my::test::Outer
        (
          include my::test::Middle [my::test::DbX -> my::test::DbY]

          my::test::B : Pure { y : 'b' }
        )
    "});
    // Both Middle's [DbA -> DbX] and Outer's [DbX -> DbY] should fail
    // because the Pure-DSL inner exposes no stores.
    let dbx_errors: Vec<&String> = errors
        .iter()
        .filter_map(|e| {
            if e.message.contains("Store Substitution Error")
                && (e.message.contains("DbA") || e.message.contains("DbX"))
            {
                Some(&e.message)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        dbx_errors.len(),
        2,
        "expected two errors (Middle's [DbA->DbX] + Outer's [DbX->DbY]); got: {dbx_errors:#?}"
    );
}

#[test]
fn substitution_without_brackets_passes() {
    // No store substitutions on the include — validator should be
    // entirely silent for this shape.
    let errors = compile_errors(indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }

        ###Mapping
        Mapping my::test::Inner
        (
          my::test::A : Pure { x : 'a' }
        )

        Mapping my::test::Outer
        (
          include my::test::Inner

          my::test::B : Pure { y : 'b' }
        )
    "});
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Store substitution")),
        "expected no errors for include without substitutions; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
