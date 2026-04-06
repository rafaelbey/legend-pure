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

//! Roundtrip tests — ported from Java's `TestDomainGrammarRoundtrip.java`.
//!
//! Each test parses Pure grammar text, composes it back to text, and asserts
//! the output matches the input exactly. This validates the full
//! parse → AST → compose pipeline.
//!
//! The test naming follows the Java test method names for easy cross-referencing.

use legend_pure_parser_compose::compose_source_file;

/// Parse source text, compose it back, and assert roundtrip equality.
///
/// This mirrors Java's `test(code)` method from `TestGrammarRoundtrip`:
/// 1. Parse code → AST
/// 2. Compose AST → grammar text
/// 3. Assert composed == original
/// 4. Also verify idempotency: compose(parse(composed)) == composed
fn roundtrip(source: &str) {
    let ast = legend_pure_parser_parser::parse(source, "test.pure")
        .unwrap_or_else(|e| panic!("Parse failed:\n{e}\n\nSource:\n{source}"));

    let composed = compose_source_file(&ast);
    assert_eq!(
        source, composed,
        "\n=== ROUNDTRIP MISMATCH ===\n\n--- Expected ---\n{source}\n\n--- Got ---\n{composed}\n"
    );

    // Idempotency: compose(parse(composed)) == composed
    let ast2 = legend_pure_parser_parser::parse(&composed, "test.pure")
        .unwrap_or_else(|e| panic!("Re-parse of composed output failed:\n{e}\n\nComposed:\n{composed}"));
    let composed2 = compose_source_file(&ast2);
    assert_eq!(
        composed, composed2,
        "\n=== IDEMPOTENCY FAILURE ===\n\n--- First compose ---\n{composed}\n\n--- Second compose ---\n{composed2}\n"
    );
}

// ---------------------------------------------------------------------------
// Enumeration tests
// ---------------------------------------------------------------------------

#[test]
fn test_enumeration() {
    roundtrip("Enum myEnum\n{\n  a,\n  b,\n  c\n}\n");
}

#[test]
fn test_enumeration_with_one_value() {
    roundtrip("Enum myEnum\n{\n  a\n}\n");
}

// ---------------------------------------------------------------------------
// Profile tests
// ---------------------------------------------------------------------------

#[test]
fn test_profile() {
    roundtrip("Profile meta::pure::profiles::doc\n{\n  stereotypes: [deprecated];\n  tags: [description, todo];\n}\n");
}

// ---------------------------------------------------------------------------
// Class tests
// ---------------------------------------------------------------------------

#[test]
fn test_simple_class() {
    roundtrip("Class A\n{\n}\n");
}

#[test]
fn test_class_with_property() {
    roundtrip("Class A\n{\n  name: String[1];\n}\n");
}

#[test]
fn test_class_with_multiple_properties() {
    roundtrip("Class model::Person\n{\n  name: String[1];\n  age: Integer[0..1];\n  active: Boolean[1];\n}\n");
}

#[test]
fn test_class_with_super_type() {
    roundtrip("Class my::A extends my::B\n{\n}\n");
}

#[test]
fn test_class_with_multiple_super_types() {
    roundtrip("Class my::A extends B, C\n{\n}\n");
}

// ---------------------------------------------------------------------------
// Association tests
// ---------------------------------------------------------------------------

#[test]
fn test_association() {
    roundtrip("Association myAsso\n{\n  a: String[1];\n  b: String[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Function tests
// ---------------------------------------------------------------------------

#[test]
fn test_simple_function() {
    roundtrip("function f(): String[1]\n{\n  'hello'\n}\n");
}

#[test]
fn test_function_with_params() {
    roundtrip("function f(s: Integer[1], s2: Integer[2]): String[1]\n{\n  println('ok')\n}\n");
}

// ---------------------------------------------------------------------------
// Multiple elements
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_elements() {
    roundtrip("Class A\n{\n  name: String[1];\n}\n\nClass B\n{\n  value: Integer[1];\n}\n");
}

#[test]
fn test_class_and_enum() {
    roundtrip(
        "Class my::A\n{\n  status: my::Status[1];\n}\n\nEnum my::Status\n{\n  Active,\n  Inactive\n}\n",
    );
}

// ---------------------------------------------------------------------------
// Class with annotations
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_stereotype() {
    roundtrip("Class <<temporal.businesstemporal>> model::Person\n{\n  name: String[1];\n}\n");
}

#[test]
fn test_class_with_tagged_value() {
    roundtrip("Class {doc.description = 'A person'} model::Person\n{\n  name: String[1];\n}\n");
}

#[test]
fn test_class_with_stereotype_and_tagged_value() {
    roundtrip("Class <<temporal.businesstemporal>> {doc.description = 'A person'} model::Person\n{\n  name: String[1];\n}\n");
}

#[test]
fn test_property_with_stereotype_and_tagged_value() {
    roundtrip("Class A\n{\n  <<doc.deprecated>> {doc.description = 'old name'} name: String[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

#[test]
fn test_simple_unnamed_constraint() {
    roundtrip("Class A\n[\n  $this.name->isNotEmpty()\n]\n{\n  name: String[1];\n}\n");
}

#[test]
fn test_named_constraint() {
    roundtrip("Class A\n[\n  nameNotEmpty: $this.name->isNotEmpty()\n]\n{\n  name: String[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Qualified properties
// ---------------------------------------------------------------------------

#[test]
fn test_qualified_property() {
    roundtrip("Class A\n{\n  name: String[1];\n  fullName(sep: String[1]) {$this.name + $sep}: String[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Expression roundtrips
// ---------------------------------------------------------------------------

#[test]
fn test_variable() {
    roundtrip("function f(x: Integer[1]): Integer[1]\n{\n  $x\n}\n");
}

#[test]
fn test_string_literal() {
    roundtrip("function f(): String[1]\n{\n  'Hello World!'\n}\n");
}

#[test]
fn test_integer_literal() {
    roundtrip("function f(): Integer[1]\n{\n  42\n}\n");
}

#[test]
fn test_float_literal() {
    roundtrip("function f(): Float[1]\n{\n  3.14\n}\n");
}

#[test]
fn test_boolean_literal() {
    roundtrip("function f(): Boolean[1]\n{\n  true\n}\n");
}

#[test]
fn test_arithmetic_simple() {
    roundtrip("function f(): Integer[1]\n{\n  1 + 2\n}\n");
}

#[test]
fn test_arithmetic_precedence_mul_add() {
    // `1 + 2 * 3` — no parens needed because * binds tighter
    roundtrip("function f(): Integer[1]\n{\n  1 + 2 * 3\n}\n");
}

#[test]
fn test_comparison() {
    roundtrip("function f(): Boolean[1]\n{\n  1 == 2\n}\n");
}

#[test]
fn test_logical() {
    roundtrip("function f(): Boolean[1]\n{\n  true && false\n}\n");
}

#[test]
fn test_not() {
    roundtrip("function f(): Boolean[1]\n{\n  !true\n}\n");
}

#[test]
fn test_let_binding() {
    roundtrip("function f(): String[1]\n{\n  let x = 'hello';\n  $x\n}\n");
}

#[test]
fn test_collection() {
    roundtrip("function f(): Integer[*]\n{\n  [1, 2, 3]\n}\n");
}

#[test]
fn test_arrow_function() {
    roundtrip("function f(): Integer[*]\n{\n  [1, 2, 3]->filter(x|$x > 1)\n}\n");
}

#[test]
fn test_member_access() {
    roundtrip("function f(p: Person[1]): String[1]\n{\n  $p.name\n}\n");
}

#[test]
fn test_lambda_in_arrow() {
    roundtrip("function f(): Integer[*]\n{\n  [1, 2, 3]->map(x|$x + 1)\n}\n");
}

#[test]
fn test_function_overloading() {
    roundtrip("function model::test(a: String[1]): String[1]\n{\n  'a'\n}\n\nfunction model::test(a: String[1], b: Integer[1]): String[1]\n{\n  'a'\n}\n");
}

// ---------------------------------------------------------------------------
// Default values (from Java testDefaultValue)
// ---------------------------------------------------------------------------

#[test]
fn test_default_value_string() {
    roundtrip("Class test::A\n{\n  stringProperty: String[1] = 'default';\n}\n");
}

#[test]
fn test_default_value_integer() {
    roundtrip("Class test::A\n{\n  integerProperty: Integer[1] = 0;\n}\n");
}

#[test]
fn test_default_value_boolean() {
    roundtrip("Class test::A\n{\n  booleanProperty: Boolean[1] = false;\n}\n");
}

#[test]
fn test_default_value_float() {
    roundtrip("Class test::A\n{\n  floatProperty: Float[1] = 0.12;\n}\n");
}

// ---------------------------------------------------------------------------
// New instance (from Java testFunctionWithNew)
// ---------------------------------------------------------------------------

#[test]
fn test_new_instance() {
    roundtrip("Class anything::goes\n{\n  v: String[1];\n}\n\nfunction f(): Any[1]\n{\n  let x = ^anything::goes(v='value')\n}\n");
}

// ---------------------------------------------------------------------------
// Measure
// ---------------------------------------------------------------------------

#[test]
fn test_measure_non_convertible() {
    roundtrip("Measure pkg::Currency\n{\n  USD;\n  GBP;\n  EUR;\n}\n");
}

// ---------------------------------------------------------------------------
// Underscores (from Java testUnderscores)
// ---------------------------------------------------------------------------

#[test]
fn test_underscores() {
    roundtrip("function my::under_score::function_example(): Any[1]\n{\n  my::under_score::function_example2()\n}\n\nfunction my::under_score::function_example2(): Any[1]\n{\n  'a'\n}\n");
}

// ---------------------------------------------------------------------------
// Lambda forms — all variations
// ---------------------------------------------------------------------------

/// Bare single untyped lambda: `x|$x + 1`
#[test]
fn test_lambda_bare_single_untyped() {
    roundtrip("function f(): Any[*]\n{\n  [1, 2, 3]->filter(x|$x > 1)\n}\n");
}

/// Bare single typed lambda: `x: Integer[1]|$x + 1`
#[test]
fn test_lambda_bare_single_typed() {
    roundtrip("function f(): Any[*]\n{\n  [1, 2, 3]->filter({x: Integer[1]|$x > 1})\n}\n");
}

/// Braced single untyped lambda: `{x|$x + 1}`
#[test]
fn test_lambda_braced_single_untyped() {
    // Single untyped in braces should roundtrip as bare form: x|$x
    roundtrip("function f(): Any[*]\n{\n  [1, 2, 3]->map(x|$x + 10)\n}\n");
}

/// Braced multi untyped lambda: `{x, y|$x + $y}`
#[test]
fn test_lambda_braced_multi_untyped() {
    roundtrip("function f(): Any[*]\n{\n  [1, 2]->fold({x, y|$x + $y}, 0)\n}\n");
}

/// Braced multi typed lambda: `{x: Integer[1], y: Integer[1]|$x + $y}`
#[test]
fn test_lambda_braced_multi_typed() {
    roundtrip("function f(): Any[*]\n{\n  [1, 2]->fold({x: Integer[1], y: Integer[1]|$x + $y}, 0)\n}\n");
}

/// No-param lambda: `|'hello'` (braced form `{|'hello'}` normalizes to bare)
#[test]
fn test_lambda_no_param() {
    roundtrip("function f(): Any[1]\n{\n  |'hello'\n}\n");
}

/// Lambda in let binding
#[test]
fn test_lambda_in_let() {
    roundtrip("function f(): Any[1]\n{\n  let fn = {x: Integer[1]|$x + 1};\n  $fn\n}\n");
}

/// Nested lambdas
#[test]
fn test_lambda_nested() {
    roundtrip("function f(): Any[*]\n{\n  [4, 5]->filter(x|[1, 2]->exists(y|$y == $x))\n}\n");
}

/// Multiple lambda args in arrow function
#[test]
fn test_lambda_multiple_args() {
    roundtrip("function f(): Any[*]\n{\n  something()->func(x|$x + 1, 'hello', y|$y * 2)\n}\n");
}
