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

//! Expression parsing tests — snapshot-based AST validation.

mod helpers;

use helpers::parse_ok;

#[test]
#[ignore = "parser not yet implemented"]
fn arithmetic_precedence() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Integer[1]
{
    (1 - 4 * (2 + 3)) * 4
}"#,
    );
    // Snapshot captures the full expression tree — verifies precedence via nesting
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn boolean_precedence() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Boolean[1]
{
    true || false && true
}"#,
    );
    // && should bind tighter than || in the AST tree
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn comparison_with_arithmetic() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Boolean[1]
{
    1 + 2 <= 3 - 4
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn or_with_arithmetic() {
    let file = parse_ok(
        r#"###Pure
function my::validate(this: my::Validated[1]): Boolean[1]
{
    $this.id->isEmpty() || $this.id >= 0
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn cast() {
    let file = parse_ok(
        r#"###Pure
function my::test(x: Any[1]): Float[1]
{
    $x->cast(@Float)
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn collection_with_function() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Boolean[1]
{
    [(true && false), false]->oneOf()
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn new_instance_nested() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Any[1]
{
    ^goes2(v2=^goes(v='value'))
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn let_binding() {
    let file = parse_ok(
        r#"###Pure
function my::test(): String[1]
{
    let x = 'hello';
    $x
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn lambda_variants() {
    let file = parse_ok(
        r#"###Pure
function my::test(): Any[*]
{
    [1, 2, 3]->filter(x | $x > 1)
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn enum_value_access() {
    let file = parse_ok(
        r#"###Pure
function my::test(): my::Color[1]
{
    my::Color.RED
}"#,
    );
    insta::assert_debug_snapshot!(file);
}
