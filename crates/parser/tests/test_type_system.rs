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

//! Type system parsing tests — snapshot-based AST validation.

mod helpers;

use helpers::parse_ok;

#[test]
#[ignore = "parser not yet implemented"]
fn type_arguments() {
    let file = parse_ok(
        r#"###Pure
function my::test(r: Result<String>[1]): Result<String>[1]
{
    $r
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn cast_with_relation() {
    let file = parse_ok(
        r#"###Pure
function my::test(x: Any[1]): Any[1]
{
    $x->cast(@Relation<(a:Integer)>)
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn type_variable_values() {
    let file = parse_ok(
        r#"###Pure
function my::test(r: Res(1)[1], v: VARCHAR(200)[1]): Res(1)[1]
{
    $r
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn generics_and_variables() {
    let file = parse_ok(
        r#"###Pure
function my::test(r: Res<String>(1, 'a')[1]): Res<String>(1, 'a')[1]
{
    $r
}"#,
    );
    insta::assert_debug_snapshot!(file);
}

#[test]
#[ignore = "parser not yet implemented"]
fn relation_column_types() {
    let file = parse_ok(
        r#"###Pure
function my::test(r: X<(a:Integer(200), z:V('ok'))>[1]): X<(a:Integer(200), z:V('ok'))>[1]
{
    $r
}"#,
    );
    insta::assert_debug_snapshot!(file);
}
