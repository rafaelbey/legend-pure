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

//! Graph-fetch round-trip tests — ported from Java's
//! `TestDomainGrammarRoundtrip.java` (graph-fetch section).
//!
//! Moved from `crates/compose/tests/roundtrip_tests.rs` so the core compose
//! crate carries zero graph-fetch identifiers. The plug-in is registered
//! via `default_island_parsers()` / `default_island_composers()` for every
//! fixture below.

use indoc::indoc;

/// Parse source text, compose it back, and assert roundtrip equality.
fn round_trip(source: &str) {
    let ast = parse_with_dsl_graph(source);

    let composed = compose_with_dsl_graph(&ast);
    assert_eq!(
        source, composed,
        "\n=== ROUNDTRIP MISMATCH ===\n\n--- Expected ---\n{source}\n\n--- Got ---\n{composed}\n"
    );

    // Idempotency: compose(parse(composed)) == composed
    let ast2 = parse_with_dsl_graph(&composed);
    let composed2 = compose_with_dsl_graph(&ast2);
    assert_eq!(
        composed, composed2,
        "\n=== IDEMPOTENCY FAILURE ===\n\n--- First compose ---\n{composed}\n\n--- Second compose ---\n{composed2}\n"
    );
}

fn compose_with_dsl_graph(sf: &legend_pure_parser_ast::SourceFile) -> String {
    legend_pure_parser_compose::section::compose_source_file_with(
        sf,
        legend_pure_dsl_graph::compose::default_island_composers(),
    )
}

fn parse_with_dsl_graph(source: &str) -> legend_pure_parser_ast::SourceFile {
    legend_pure_parser_parser::parse_with_islands(
        source,
        "test.pure",
        legend_pure_dsl_graph::parser::default_island_parsers(),
    )
    .unwrap_or_else(|e| panic!("Parse failed:\n{e}\n\nSource:\n{source}"))
}

#[test]
fn test_graph_fetch_simple() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            my::Person{
              firstName,
              lastName
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_nested() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            my::Person{
              firstName,
              address{
                city,
                street
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_with_qualifier() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              legalName,
              employeeCount,
              employeesByFirstName([]){
                firstName,
                lastName
              },
              employeesByFirstName('Peter'){
                firstName,
                lastName
              },
              employeesByFirstName(['Peter']){
                firstName,
                lastName
              },
              employeesByFirstName(['Peter', 'John']){
                firstName,
                lastName
              },
              employeesByFirstNameAndCity(['Peter', 'John'], ['New York']){
                firstName,
                lastName
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_subtype_at_root() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              legalName,
              subType(@test::FirmSubType){
                SubTypeName
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_multiple_subtypes() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              legalName,
              subType(@test::FirmSubType1){
                SubTypeName1
              },
              subType(@test::FirmSubType2){
                SubTypeName2
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_only_subtypes() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              subType(@test::FirmSubType){
                SubTypeName
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_subtype_with_alias() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              legalName,
              subType(@test::FirmSubType1){
                'alias1':SubTypeName
              },
              subType(@test::FirmSubType2){
                'alias2':SubTypeName
              }
            }
          }#
        }
    "});
}

#[test]
fn test_graph_fetch_property_subtype() {
    round_trip(indoc! {"
        function my::test(): Any[*]
        {
          #{
            test::Firm{
              legalName,
              employees->subType(@test::Manager){
                managerLevel
              }
            }
          }#
        }
    "});
}
