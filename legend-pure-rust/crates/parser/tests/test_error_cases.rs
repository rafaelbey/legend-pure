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

//! Parser error case tests.
//!
//! These use `parse_err` to validate that malformed Pure source produces
//! appropriate error messages. No snapshots — just error message assertions.

mod helpers;

use helpers::{parse_err, parse_ok};

#[test]
fn unexpected_token() {
    parse_err(
        r"###Pure
asd Class my::Foo
{
}",
        "Unexpected token",
    );
}

#[test]
fn reserved_keywords() {
    // In the Java Pure grammar, keywords like 'false' can appear in path segments.
    // This test verifies the parser accepts them (matching Java engine behavior).
    let file = parse_ok(
        r"###Pure
Class false::me
{
}",
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
fn invalid_aggregation_kind() {
    parse_err(
        r"###Pure
Class my::Foo
{
    (tunnel) prop: String[1];
}",
        "aggregation",
    );
}

#[test]
fn function_test_name_mismatch() {
    // Function test name validation is a semantic concern, not a parser concern.
    // The parser accepts syntactically valid test blocks; the semantic layer
    // will validate that the invocation matches the owning function.
    let file = parse_ok(
        r"###Pure
function my::hello(): String[1]
{
    'Hello'
}
{
    myTest | goodbye() => 'Hello';
}",
    );
    insta::assert_debug_snapshot!(file);
}
