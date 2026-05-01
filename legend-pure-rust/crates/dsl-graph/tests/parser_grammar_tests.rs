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

//! Graph-fetch grammar parsing — snapshot-based AST validation.
//!
//! Moved from `crates/parser/tests/test_graph_fetch.rs` so the core parser
//! crate carries zero graph-fetch identifiers. The plug-in is registered
//! via `default_island_parsers()` for every fixture below.

use legend_pure_parser_ast::SourceFile;

fn parse_ok(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_islands(
        source,
        "test.pure",
        legend_pure_dsl_graph::parser::default_island_parsers(),
    )
    .unwrap_or_else(|e| {
        panic!(
            "Expected parse to succeed, but got error(s): {:?}",
            e.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

#[test]
fn basic_with_qualifier() {
    let file = parse_ok(
        r"###Pure
function my::test(): Any[*]
{
    my::Person.all()->graphFetch(
        #{
            my::Person {
                firstName,
                lastName,
                employeesByFirstName(['Peter']) {
                    firstName,
                    lastName
                }
            }
        }#
    )
}",
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
fn subtype_at_root() {
    let file = parse_ok(
        r"###Pure
function my::test(): Any[*]
{
    my::Firm.all()->graphFetch(
        #{
            my::Firm {
                name,
                subType(@my::FirmSubType) {
                    SubTypeName
                }
            }
        }#
    )
}",
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
fn subtype_with_alias() {
    let file = parse_ok(
        r"###Pure
function my::test(): Any[*]
{
    my::Firm.all()->graphFetch(
        #{
            my::Firm {
                name,
                subType(@my::FirmSubType) {
                    'alias1' : SubTypeName
                }
            }
        }#
    )
}",
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
fn subtype_not_at_root_error() {
    // SubType depth validation is a semantic concern, not a parser concern.
    // The parser accepts syntactically valid subType at any depth;
    // the semantic layer validates graph fetch tree structural rules.
    let file = parse_ok(
        r"###Pure
function my::test(): Any[*]
{
    my::Firm.all()->graphFetch(
        #{
            my::Firm {
                name {
                    subType(@my::SubType) {
                        SubTypeName
                    }
                }
            }
        }#
    )
}",
    );
    insta::assert_debug_snapshot!(file);
}
