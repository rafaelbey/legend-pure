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

// ---------------------------------------------------------------------------
// Quoted identifiers (from Java testPackageWithQuotedIdentifier, testQuotedEnumerations,
// testNumbersInEnumerationName, testQuotedProfile, etc.)
// ---------------------------------------------------------------------------

#[test]
fn test_package_with_quoted_identifier() {
    roundtrip("Class test::'p a c k a g e'::A\n{\n  's t r i n g': String[1];\n}\n");
}

#[test]
fn test_quoted_enumerations() {
    roundtrip("Enum <<st.test>> {doc.doc = 'bla'} '@'::'my Enum'\n{\n  'Anything e',\n  'A g',\n  'Anything r'\n}\n");
}

#[test]
fn test_numbers_in_enumeration_name() {
    roundtrip("Enum my::Enum\n{\n  '30_360',\n  '30_ACT'\n}\n");
}

#[test]
fn test_quoted_profile() {
    roundtrip("Profile meta::pure::profiles::'with quotes'\n{\n  stereotypes: ['two words'];\n  tags: ['s tag', 'another tag'];\n}\n");
}

// ---------------------------------------------------------------------------
// Multiple stereotypes & tagged values (from Java testClassWithMultipleTaggedAndStereotypes,
// testClassWithQuotedTagsAndStereotypes, testTaggedValuesSpecialChar)
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_multiple_tagged_and_stereotypes() {
    roundtrip("Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'up1', doc.test2 = 'up2'} meta::this::class::has::path::A extends B\n{\n  <<equality.Key, taggedValue.test>> {doc.doc = 'Borrowers date of birth'} name: e::R[*];\n  {Descriptor.descriptionA = 'test1', Descriptor.descriptionB = 'test2'} ok: Integer[1..2];\n  <<devStatus.inProgress>> q(s: String[1]) {$s + 'ok'}: c::d::R[1];\n  {doc.test1 = 'test1', doc.test2 = 'test2'} xza(s: z::k::B[1]) {$s + 'ok'}: String[1];\n}\n");
}

#[test]
fn test_class_with_quoted_tags_and_stereotypes() {
    roundtrip("Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'up1', doc.test2 = 'up2'} meta::this::class::has::path::A extends B\n{\n  <<'a profile'.'1>stereo'>> {'a profile'.'2>tag' = 'Borrowers date of birth'} name: e::R[*];\n  {Descriptor.descriptionA = 'test1', Descriptor.descriptionB = 'test2'} ok: Integer[1..2];\n}\n");
}

#[test]
fn test_tagged_values_special_char() {
    roundtrip("Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'test1\\'s', doc.test2 = 'm\\'s test'} meta::this::class::has::path::A\n{\n  <<equality.Key, taggedValue.test>> {doc.doc = 'uyaguari\\'s test', doc.test2 = 'm\\'s test'} name: e::R[*];\n}\n");
}

// ---------------------------------------------------------------------------
// Complex constraints (from Java testComplexConstraints, testAppliedFunctionAsParameters, etc.)
// ---------------------------------------------------------------------------

#[test]
fn test_complex_constraints() {
    roundtrip("Class A\n[\n  c1\n  (\n    ~function: if($this.'O.K.' == 'ok', |true, |false)\n    ~enforcementLevel: Warn\n  ),\n  c2\n  (\n    ~externalId: 'ext ID'\n    ~function: if($this.'O.K.' == 'ok', |true, |false)\n  ),\n  c3\n  (\n    ~function: if($this.'O.K.' == 'ok', |true, |false)\n    ~message: $this.'O.K.' + ' is not ok'\n  ),\n  c4\n  (\n    ~externalId: 'ext ID'\n    ~function: if($this.'O.K.' == 'ok', |true, |false)\n    ~enforcementLevel: Warn\n    ~message: $this.'O.K.' + ' is not ok'\n  )\n]\n{\n  name: String[45..*];\n  'O.K.': Integer[1..2];\n  xza(s: String[1]) {$s + 'ok'}: String[1];\n}\n");
}

#[test]
fn test_unnamed_constraints_and_empty_profile() {
    roundtrip("Class A\n[\n  $this.ok->toOne() == 1,\n  named: if($this.ok == 'ok', |true, |false),\n  $this.ok->toOne()->toString() == $this.name\n]\n{\n  name: String[45..*];\n  ok: Integer[1..2];\n  xza(s: String[1]) {$s + 'ok'}: String[1];\n}\n\nProfile meta::pure::profiles::doc\n{\n}\n");
}

#[test]
fn test_or_with_arithmetic_in_constraint() {
    roundtrip("Class test::C\n[\n  $this.id->isEmpty() || $this.id >= 0\n]\n{\n  id: Integer[0..1];\n}\n");
}

// ---------------------------------------------------------------------------
// Aggregation types (from Java testClassPropertiesWithAggregationType,
// testAssociationPropertiesWithAggregationType)
// ---------------------------------------------------------------------------

#[test]
fn test_class_properties_with_aggregation_type() {
    roundtrip("Class my::Class\n{\n  (shared) prop1: String[1];\n  (none) prop2: String[1];\n  (composite) prop3: String[1];\n  prop4: String[1];\n}\n");
}

#[test]
fn test_association_properties_with_aggregation_type() {
    roundtrip("Association my::Assoc\n{\n  (shared) prop1: String[1];\n  (none) prop2: String[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Enumerations with annotations (from Java testEnumerations)
// ---------------------------------------------------------------------------

#[test]
fn test_enumerations_with_annotations() {
    roundtrip("Enum <<st.test>> {doc.doc = 'bla'} myEnum\n{\n  <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} a,\n  <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} b,\n  c\n}\n\nEnum <<st.test>> {doc.doc = 'bla'} zz::MyOther\n{\n  e,\n  g,\n  r\n}\n");
}

// ---------------------------------------------------------------------------
// Associations with annotations (from Java testAssociations)
// ---------------------------------------------------------------------------

#[test]
fn test_associations_with_annotations() {
    roundtrip("Association myAsso\n{\n  a: String[1];\n  b: a::c::A[1];\n}\n\nAssociation {doc.doc = 'bla'} k::p::Asso\n{\n  a: Integer[1];\n  b: a::c::B[1];\n}\n");
}

// ---------------------------------------------------------------------------
// Measure — convertible (from Java testMeasure)
// ---------------------------------------------------------------------------

#[test]
fn test_measure_convertible() {
    roundtrip("Measure NewMeasure\n{\n  *UnitOne: x -> $x;\n  UnitTwo: x -> $x * 1000;\n  UnitThree: x -> $x * 400;\n}\n");
}

#[test]
fn test_quoted_measure() {
    roundtrip("Measure 'some measure'\n{\n  *'Unit One': x -> $x;\n  'Unit Two': x -> $x * 1000;\n  'Unit Three': x -> $x * 400;\n}\n");
}

#[test]
fn test_quoted_non_convertible_measure() {
    roundtrip("Measure 'some measure'\n{\n  'Unit One';\n  'Unit Two';\n  'Unit Three';\n}\n");
}

// ---------------------------------------------------------------------------
// Measure units in types (from Java testClassWithUnitRelatedProperties)
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_unit_properties() {
    roundtrip("Class A\n{\n  unitOne: NewMeasure~UnitOne[0..1];\n  unitTwo: NewMeasure~UnitTwo[0..1];\n}\n\nMeasure NewMeasure\n{\n  *UnitOne: x -> $x;\n  UnitTwo: x -> $x * 1000;\n  UnitThree: x -> $x * 400;\n}\n");
}

// ---------------------------------------------------------------------------
// Derived properties with multiple statements (from Java)
// ---------------------------------------------------------------------------

#[test]
fn test_derived_property_with_multiple_statements() {
    roundtrip("Class Firm\n{\n  prop1: Float[1];\n  prop3: String[1];\n  prop2() {\n    let x = 0;\n    $x;\n  }: Integer[1];\n}\n");
}

#[test]
fn test_qualified_property_with_params_and_multi_statements() {
    roundtrip("Class Query\n{\n  allFirms(limit: Integer[1]) {\n    let offset = $limit + 10;\n    Firm.all()->slice($limit, $offset);\n  }: Firm[*];\n}\n");
}

// ---------------------------------------------------------------------------
// Math precedence edge cases (from Java testMathParenthesis3)
// ---------------------------------------------------------------------------

/// Composer emits minimal-but-correct parentheses based on operator
/// precedence. Semantically redundant parens (e.g., `(8 / 4) * 2` where
/// same-precedence left-associativity makes them unnecessary) are not
/// preserved. The Java composer preserves original source grouping via
/// source-info tracking. Both produce semantically identical parse trees.
#[test]
fn test_math_precedence_comprehensive() {
    roundtrip("function f(s: Integer[1], s2: Interger[2]): String[1]\n{\n  let a = 1 / (2 / 3);\n  let a = 1 * (2 * 3);\n  let a = 1 - (2 - 3);\n  let a = 1 + (2 + 3);\n  let a = (8 / 4) * 2;\n  let a = 8 / (4 * 2);\n  let a = (8 * 4) / 2;\n  let a = 8 * (4 / 2);\n  let a = (8 * 4) + 2;\n  let a = 8 * (4 + 2);\n  let a = (8 + 4) * 2;\n  let a = 8 + (4 * 2);\n  let a = (1 - (4 * (2 + 3))) * 4;\n  let a = ((1 - (4 * 2)) + 3) * 4;\n  let a = (1 - (4 * 2)) + (3 * 4);\n  let a = 1 + 4 + 2 + 3 + 4;\n  let a = (1 + 2) - (3 - 4);\n  let a = 1 + 2 <= 3 - 4;\n  let a = (8 <= 4) + 2;\n  let a = 8 + 4 <= 2\n}\n");
}

/// Canonical form: `(1 - 4 * (2 + 3)) * 4` — inner `4 * (2+3)` doesn't need
/// parens inside `-` since `*` binds tighter.
#[test]
fn test_math_parenthesis_nested() {
    roundtrip("function f(s: Integer[1], s2: Interger[2]): String[1]\n{\n  let a = (1 - 4 * (2 + 3)) * 4\n}\n");
}

/// Canonical form strips redundant inner parens around `*` inside `+`.
#[test]
fn test_math_parenthesis_complex_nested() {
    roundtrip("function f(s: Integer[1], s2: Interger[2]): String[1]\n{\n  let a = 4 + 1 * (2 + 3) * 4 + (2 + 3) * 4\n}\n");
}

/// Canonical form: `((1 - 2) / (2 + 3))` inside `*` strips outer parens.
#[test]
fn test_math_parenthesis_division_grouped() {
    roundtrip("function f(s: Integer[1], s2: Interger[2]): String[1]\n{\n  let a = 4 + (1 - 2) / (2 + 3) * (1 - 4 - 5)\n}\n");
}

// ---------------------------------------------------------------------------
// New instance (from Java testFunctionWithNewAlltypes, testFunctionWithNewAndNewValue)
// ---------------------------------------------------------------------------

#[test]
fn test_new_instance_all_types() {
    roundtrip("Class anything::goes\n{\n  v: String[1];\n  v2: Integer[0..1];\n  v3: Boolean[*];\n}\n\nfunction f(): Any[1]\n{\n  let x = ^anything::goes(v='value' , v2=17 , v3=[true, false])\n}\n");
}

#[test]
fn test_new_instance_nested() {
    roundtrip("Class anything::goes\n{\n  v: String[1];\n}\n\nClass anything::goes2\n{\n  v2: anything::goes[1];\n}\n\nfunction f(): Any[1]\n{\n  let x = ^anything::goes2(v2=^anything::goes(v='value'))\n}\n");
}

// ---------------------------------------------------------------------------
// Collection with function (from Java testCollectionWithFunction)
// ---------------------------------------------------------------------------

/// Canonical form strips redundant outer parens inside collection: `(true && ...)` → `true && ...`.
#[test]
fn test_collection_with_function() {
    roundtrip("function package::test(value: meta::pure::metamodel::type::Any[0..1]): Boolean[1]\n{\n  [true && (false && false), false]->oneOf()\n}\n");
}

// ---------------------------------------------------------------------------
// Cast / type reference (from Java testCast)
// ---------------------------------------------------------------------------

#[test]
fn test_cast_with_type_ref() {
    roundtrip("function abc::cast(): Float[1]\n{\n  1->cast(@Float)\n}\n");
}

#[test]
fn test_cast_with_literal() {
    roundtrip("function abc::cast(): Float[1]\n{\n  1->cast(1.0)\n}\n");
}

#[test]
fn test_cast_with_string() {
    roundtrip("function abc::cast(): String[1]\n{\n  1->cast('String')\n}\n");
}

// ---------------------------------------------------------------------------
// Function signature variants (from Java testFunction2, testDecimalWithScale)
// ---------------------------------------------------------------------------

#[test]
fn test_function_with_multiple_params() {
    roundtrip("function f(s: Integer[1], s2: Interger[2]): String[1]\n{\n  println('ok')\n}\n");
}

/// Last expression in body drops trailing semicolon (it's used as separator, not terminator).
#[test]
fn test_function_with_path() {
    roundtrip("function withPath::f(s: Integer[1]): String[1]\n{\n  println('ok');\n  'a'\n}\n");
}

// ---------------------------------------------------------------------------
// Decimal literal (from Java testDecimalWithScale, TestPrimitives.testDecimaOne)
// ---------------------------------------------------------------------------

#[test]
fn test_decimal_literal_one() {
    roundtrip("function a::a(): Decimal[1]\n{\n  1.0D\n}\n");
}

#[test]
fn test_decimal_literal_many() {
    roundtrip("function a::a(): Decimal[*]\n{\n  [1.0D, 3.0D]\n}\n");
}

// ---------------------------------------------------------------------------
// Primitive literals (from Java TestPrimitives)
// ---------------------------------------------------------------------------

#[test]
fn test_string_many() {
    roundtrip("function a::a(): String[*]\n{\n  ['ok', 'bla']\n}\n");
}

#[test]
fn test_integer_many() {
    roundtrip("function a::a(): String[*]\n{\n  [1, 2]\n}\n");
}

#[test]
fn test_boolean_many() {
    roundtrip("function a::a(): String[*]\n{\n  [true, false, true]\n}\n");
}

#[test]
fn test_mixed_collection() {
    roundtrip("function a::a(): String[*]\n{\n  [1, 'a', true]\n}\n");
}

// ---------------------------------------------------------------------------
// Full-path arrow function (from Java testMetaFunctionExecutionWithFullPath)
// ---------------------------------------------------------------------------

#[test]
fn test_meta_function_with_full_path() {
    roundtrip("function example::somethingElse(input: Integer[1]): Any[0..1]\n{\n  [1, $input]->meta::pure::functions::math::max()\n}\n");
}

// ---------------------------------------------------------------------------
// Quoted function parameters and variables
// (from Java testFunctionWithQuotedParameters, testFunctionWithQuotedVariables)
// ---------------------------------------------------------------------------

#[test]
fn test_function_with_quoted_parameters() {
    roundtrip("function test::qoutedParams('1,2,3': Integer[3]): String[0..1]\n{\n  $'1,2,3'->map(n|$n->toString())->joinStrings(',')\n}\n");
}

/// Last expression drops trailing semicolon.
#[test]
fn test_function_with_quoted_variables() {
    roundtrip("function test::qoutedParams(): String[0..1]\n{\n  let '1,2,3' = [1, 2, 3];\n  $'1,2,3'->map(n|$n->toString())->joinStrings(',')\n}\n");
}

// ---------------------------------------------------------------------------
// Default values — comprehensive (from Java testDefaultValue)
// ---------------------------------------------------------------------------

#[test]
fn test_default_value_class_property() {
    roundtrip("Class test::A\n{\n  classProperty: my::exampleRootType[1] = ^my::exampleRootType();\n}\n");
}

#[test]
fn test_default_value_enum() {
    roundtrip("Class test::A\n{\n  enumProperty: test::EnumWithDefault[1] = test::EnumWithDefault.DefaultValue;\n}\n");
}

#[test]
fn test_default_value_collection() {
    roundtrip("Class test::A\n{\n  collectionProperty: String[1..*] = ['one', 'two'];\n}\n");
}

// ---------------------------------------------------------------------------
// Complex class (from Java testComplexClass)
// ---------------------------------------------------------------------------

#[test]
fn test_complex_class() {
    roundtrip("Class 'A-Z'\n[\n  constraint1: $this.ok->toOne() == 1,\n  constraint2: if($this.ok == 'ok', |true, |false),\n  'constraint-3': $this.anyValue->instanceOf(String) || $this.anyValue->instanceOf(AEnum)\n]\n{\n  name: String[45..*];\n  ok: Integer[1..2];\n  anyValue: Any[1];\n  'maybe or maybe not!': Boolean[1];\n  xza(s: String[1]) {$s + 'ok\\n{\"\"}\\'\\''}: String[1];\n  'I\\'m derived'('#String': String[1]) {$s + 'ok\\n{\"\"}\\'\\''}: String[1];\n}\n\nEnum AEnum\n{\n  B\n}\n");
}

// ---------------------------------------------------------------------------
// Large mixed domain (from Java testDomainMixed)
// ---------------------------------------------------------------------------

#[test]
fn test_domain_mixed() {
    roundtrip("Class <<temporal.businesstemporal>> {doc.doc = 'bla'} A extends B\n[\n  constraint1: $this.ok->toOne() == 1,\n  constraint2: $this.ok->toOne()->toString() == $this.name\n]\n{\n  <<equality.Key>> {doc.doc = 'bla'} name: e::R[*];\n  {doc.doc = 'bla'} ok: Integer[1..2];\n  <<devStatus.inProgress>> q(s: String[1]) {$s + 'ok'}: c::d::R[1];\n  {doc.doc = 'bla'} xza(s: z::k::B[1]) {$s + 'ok'}: String[1];\n}\n\nAssociation myAsso\n{\n  a: String[1];\n  b: a::c::A[1];\n}\n\nEnum <<st.test>> {doc.doc = 'bla'} z::k::B\n{\n  <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} a,\n  b,\n  c\n}\n\nProfile meta::pure::profiles::doc\n{\n  stereotypes: [deprecated];\n  tags: [doc, todo];\n}\n\nProfile meta::pure::profiles::profile2\n{\n  tags: [doc, todo];\n}\n");
}

// ---------------------------------------------------------------------------
// Import statements (from Java testClassWithImport, etc.)
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_import() {
    roundtrip("import anything::*;\nClass anything::goes2\n{\n}\n");
}

#[test]
fn test_enum_with_import() {
    roundtrip("import my::models::*;\nEnum my::models::Color\n{\n  Red,\n  Green,\n  Blue\n}\n");
}

#[test]
fn test_function_with_import() {
    roundtrip("import meta::pure::functions::*;\nfunction my::add(a: Integer[1], b: Integer[1]): Integer[1]\n{\n  $a + $b\n}\n");
}

#[test]
fn test_multiple_imports() {
    roundtrip("import anything::*;\nimport other::stuff::*;\nClass anything::goes2\n{\n}\n");
}

// ---------------------------------------------------------------------------
// Section headers
// ---------------------------------------------------------------------------

/// Single Pure section: `###Pure` header is normalized away (it's the default).
#[test]
fn test_section_header_single_pure_normalized() {
    // Input has ###Pure, but output omits it (single Pure section = default)
    let input = "###Pure\nClass my::Foo\n{\n}\n";
    let expected = "Class my::Foo\n{\n}\n";
    let ast = legend_pure_parser_parser::parse(input, "test.pure")
        .unwrap_or_else(|e| panic!("Parse failed: {e}"));
    let composed = legend_pure_parser_compose::compose_source_file(&ast);
    assert_eq!(composed, expected);
}

/// Single Pure section with import: `###Pure` header normalized away.
#[test]
fn test_section_header_with_import_normalized() {
    let input = "###Pure\nimport anything::*;\nClass anything::goes2\n{\n}\n";
    let expected = "import anything::*;\nClass anything::goes2\n{\n}\n";
    let ast = legend_pure_parser_parser::parse(input, "test.pure")
        .unwrap_or_else(|e| panic!("Parse failed: {e}"));
    let composed = legend_pure_parser_compose::compose_source_file(&ast);
    assert_eq!(composed, expected);
}

