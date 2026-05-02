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

//! Reproduction tests for parser gaps surfaced when compiling engine-side
//! Pure repos through the Rust pipeline. Each test is a minimal
//! reproduction of one cataloged failure in
//! `legend-engine-rust/crates/integration-smoke/tests/relational_metamodel_compiles.rs`.

mod helpers;

use helpers::parse_ok;

/// `core_functions_unclassified/meta/create/newAssociation.pure:28`
/// `assertInstanceOf($x, Association)` — `Association` used as a value
/// in expression position. The parser needs to treat `Association` (and
/// other element keywords like `Measure`, `Primitive`) as identifier-
/// like in expression-primary context, mirroring `Class`/`Enum`/etc.
#[test]
fn association_as_value_in_expression() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): Boolean[1]
{
    assertInstanceOf($x, Association)
}",
    );
}

#[test]
fn measure_as_value_in_expression() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): Boolean[1]
{
    assertInstanceOf($x, Measure)
}",
    );
}

#[test]
fn primitive_as_value_in_expression() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): Boolean[1]
{
    assertInstanceOf($x, Primitive)
}",
    );
}

/// `core_functions_unclassified/meta/get.pure:23`
/// `^CO_Location a(place='Jersey City, NJ')` — "new with name" pattern:
/// the optional identifier between the type and the property block names
/// the new instance.
#[test]
fn new_instance_with_optional_instance_name() {
    let _ = parse_ok(
        r"###Class
Class my::CO_Location
{
    place: String[1];
}

###Pure
function my::test(): my::CO_Location[1]
{
    ^my::CO_Location a(place='Jersey City, NJ')
}",
    );
}

/// `core_functions_unclassified/milestoning/reverseMilestoningTransforms.pure:26`
/// Match with a typed-parameter lambda whose type carries a generic and
/// a multiplicity: `[c:Class<Any>[1]| body, a:Association[1]| body]`.
#[test]
fn match_with_generic_typed_lambda_param() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): String[1]
{
    $x->match([c: Class<Any>[1]| 'class', a: Association[1]| 'association'])
}",
    );
}

/// Probe: bare-lambda with a non-generic typed param works fine.
#[test]
fn bare_lambda_with_simple_type() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): String[1]
{
    [c: String[1]| 'class']
}",
    );
}

/// Probe: bare-lambda with a generic typed param — does the generic on
/// its own work, or is it the multiplicity pairing that fails?
#[test]
fn bare_lambda_with_generic_typed_param() {
    let _ = parse_ok(
        r"###Pure
function my::test(x: Any[1]): String[1]
{
    [c: Class<Any>[1]| 'class']
}",
    );
}
