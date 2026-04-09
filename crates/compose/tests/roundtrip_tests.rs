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

use indoc::indoc;
use legend_pure_parser_compose::compose_source_file;

/// Parse source text, compose it back, and assert roundtrip equality.
///
/// This mirrors Java's `test(code)` method from `TestGrammarRoundtrip`:
/// 1. Parse code → AST
/// 2. Compose AST → grammar text
/// 3. Assert composed == original
/// 4. Also verify idempotency: compose(parse(composed)) == composed
fn round_trip(source: &str) {
    let ast = legend_pure_parser_parser::parse(source, "test.pure")
        .unwrap_or_else(|e| panic!("Parse failed:\n{e}\n\nSource:\n{source}"));

    let composed = compose_source_file(&ast);
    assert_eq!(
        source, composed,
        "\n=== ROUNDTRIP MISMATCH ===\n\n--- Expected ---\n{source}\n\n--- Got ---\n{composed}\n"
    );

    // Idempotency: compose(parse(composed)) == composed
    let ast2 = legend_pure_parser_parser::parse(&composed, "test.pure").unwrap_or_else(|e| {
        panic!("Re-parse of composed output failed:\n{e}\n\nComposed:\n{composed}")
    });
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
    round_trip(indoc! {"
        Enum myEnum
        {
          a,
          b,
          c
        }
    "});
}

#[test]
fn test_enumeration_with_one_value() {
    round_trip(indoc! {"
        Enum myEnum
        {
          a
        }
    "});
}

// ---------------------------------------------------------------------------
// Profile tests
// ---------------------------------------------------------------------------

#[test]
fn test_profile() {
    round_trip(indoc! {"
        Profile meta::pure::profiles::doc
        {
          stereotypes: [deprecated];
          tags: [description, todo];
        }
    "});
}

// ---------------------------------------------------------------------------
// Class tests
// ---------------------------------------------------------------------------

#[test]
fn test_simple_class() {
    round_trip(indoc! {"
        Class A
        {
        }
    "});
}

#[test]
fn test_class_with_property() {
    round_trip(indoc! {"
        Class A
        {
          name: String[1];
        }
    "});
}

#[test]
fn test_class_with_multiple_properties() {
    round_trip(indoc! {"
        Class model::Person
        {
          name: String[1];
          age: Integer[0..1];
          active: Boolean[1];
        }
    "});
}

#[test]
fn test_class_with_super_type() {
    round_trip(indoc! {"
        Class my::A extends my::B
        {
        }
    "});
}

#[test]
fn test_class_with_multiple_super_types() {
    round_trip(indoc! {"
        Class my::A extends B, C
        {
        }
    "});
}

// ---------------------------------------------------------------------------
// Association tests
// ---------------------------------------------------------------------------

#[test]
fn test_association() {
    round_trip(indoc! {"
        Association myAsso
        {
          a: String[1];
          b: String[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Function tests
// ---------------------------------------------------------------------------

#[test]
fn test_simple_function() {
    round_trip(indoc! {"
        function f(): String[1]
        {
          'hello'
        }
    "});
}

#[test]
fn test_function_with_params() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Integer[2]): String[1]
        {
          println('ok')
        }
    "});
}

// ---------------------------------------------------------------------------
// Multiple elements
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_elements() {
    round_trip(indoc! {"
        Class A
        {
          name: String[1];
        }

        Class B
        {
          value: Integer[1];
        }
    "});
}

#[test]
fn test_class_and_enum() {
    round_trip(indoc! {"
        Class my::A
        {
          status: my::Status[1];
        }

        Enum my::Status
        {
          Active,
          Inactive
        }
    "});
}

// ---------------------------------------------------------------------------
// Class with annotations
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_stereotype() {
    round_trip(indoc! {"
        Class <<temporal.businesstemporal>> model::Person
        {
          name: String[1];
        }
    "});
}

#[test]
fn test_class_with_tagged_value() {
    round_trip(indoc! {"
        Class {doc.description = 'A person'} model::Person
        {
          name: String[1];
        }
    "});
}

#[test]
fn test_class_with_stereotype_and_tagged_value() {
    round_trip(indoc! {"
        Class <<temporal.businesstemporal>> {doc.description = 'A person'} model::Person
        {
          name: String[1];
        }
    "});
}

#[test]
fn test_property_with_stereotype_and_tagged_value() {
    round_trip(indoc! {"
        Class A
        {
          <<doc.deprecated>> {doc.description = 'old name'} name: String[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

#[test]
fn test_simple_unnamed_constraint() {
    round_trip(indoc! {"
        Class A
        [
          $this.name->isNotEmpty()
        ]
        {
          name: String[1];
        }
    "});
}

#[test]
fn test_named_constraint() {
    round_trip(indoc! {"
        Class A
        [
          nameNotEmpty: $this.name->isNotEmpty()
        ]
        {
          name: String[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Qualified properties
// ---------------------------------------------------------------------------

#[test]
fn test_qualified_property() {
    round_trip(indoc! {"
        Class A
        {
          name: String[1];
          fullName(sep: String[1]) {$this.name + $sep}: String[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Expression roundtrips
// ---------------------------------------------------------------------------

#[test]
fn test_variable() {
    round_trip(indoc! {"
        function f(x: Integer[1]): Integer[1]
        {
          $x
        }
    "});
}

#[test]
fn test_string_literal() {
    round_trip(indoc! {"
        function f(): String[1]
        {
          'Hello World!'
        }
    "});
}

#[test]
fn test_integer_literal() {
    round_trip(indoc! {"
        function f(): Integer[1]
        {
          42
        }
    "});
}

#[test]
fn test_float_literal() {
    round_trip(indoc! {"
        function f(): Float[1]
        {
          3.14
        }
    "});
}

#[test]
fn test_boolean_literal() {
    round_trip(indoc! {"
        function f(): Boolean[1]
        {
          true
        }
    "});
}

#[test]
fn test_arithmetic_simple() {
    round_trip(indoc! {"
        function f(): Integer[1]
        {
          1 + 2
        }
    "});
}

#[test]
fn test_arithmetic_precedence_mul_add() {
    round_trip(indoc! {"
        function f(): Integer[1]
        {
          1 + 2 * 3
        }
    "});
}

#[test]
fn test_comparison() {
    round_trip(indoc! {"
        function f(): Boolean[1]
        {
          1 == 2
        }
    "});
}

#[test]
fn test_logical() {
    round_trip(indoc! {"
        function f(): Boolean[1]
        {
          true && false
        }
    "});
}

#[test]
fn test_not() {
    round_trip(indoc! {"
        function f(): Boolean[1]
        {
          !true
        }
    "});
}

#[test]
fn test_let_binding() {
    round_trip(indoc! {"
        function f(): String[1]
        {
          let x = 'hello';
          $x
        }
    "});
}

#[test]
fn test_collection() {
    round_trip(indoc! {"
        function f(): Integer[*]
        {
          [1, 2, 3]
        }
    "});
}

#[test]
fn test_arrow_function() {
    round_trip(indoc! {"
        function f(): Integer[*]
        {
          [1, 2, 3]->filter(x|$x > 1)
        }
    "});
}

#[test]
fn test_member_access() {
    round_trip(indoc! {"
        function f(p: Person[1]): String[1]
        {
          $p.name
        }
    "});
}

#[test]
fn test_lambda_in_arrow() {
    round_trip(indoc! {"
        function f(): Integer[*]
        {
          [1, 2, 3]->map(x|$x + 1)
        }
    "});
}

#[test]
fn test_function_overloading() {
    round_trip(indoc! {"
        function model::test(a: String[1]): String[1]
        {
          'a'
        }

        function model::test(a: String[1], b: Integer[1]): String[1]
        {
          'a'
        }
    "});
}

// ---------------------------------------------------------------------------
// Default values (from Java testDefaultValue)
// ---------------------------------------------------------------------------

#[test]
fn test_default_value_string() {
    round_trip(indoc! {"
        Class test::A
        {
          stringProperty: String[1] = 'default';
        }
    "});
}

#[test]
fn test_default_value_integer() {
    round_trip(indoc! {"
        Class test::A
        {
          integerProperty: Integer[1] = 0;
        }
    "});
}

#[test]
fn test_default_value_boolean() {
    round_trip(indoc! {"
        Class test::A
        {
          booleanProperty: Boolean[1] = false;
        }
    "});
}

#[test]
fn test_default_value_float() {
    round_trip(indoc! {"
        Class test::A
        {
          floatProperty: Float[1] = 0.12;
        }
    "});
}

// ---------------------------------------------------------------------------
// New instance (from Java testFunctionWithNew)
// ---------------------------------------------------------------------------

#[test]
fn test_new_instance() {
    round_trip(indoc! {"
        Class anything::goes
        {
          v: String[1];
        }

        function f(): Any[1]
        {
          let x = ^anything::goes(v='value')
        }
    "});
}

// ---------------------------------------------------------------------------
// Measure
// ---------------------------------------------------------------------------

#[test]
fn test_measure_non_convertible() {
    round_trip(indoc! {"
        Measure pkg::Currency
        {
          USD;
          GBP;
          EUR;
        }
    "});
}

// ---------------------------------------------------------------------------
// Underscores (from Java testUnderscores)
// ---------------------------------------------------------------------------

#[test]
fn test_underscores() {
    round_trip(indoc! {"
        function my::under_score::function_example(): Any[1]
        {
          my::under_score::function_example2()
        }

        function my::under_score::function_example2(): Any[1]
        {
          'a'
        }
    "});
}

// ---------------------------------------------------------------------------
// Lambda forms — all variations
// ---------------------------------------------------------------------------

/// Bare single untyped lambda: `x|$x + 1`
#[test]
fn test_lambda_bare_single_untyped() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [1, 2, 3]->filter(x|$x > 1)
        }
    "});
}

/// Bare single typed lambda: `{x: Integer[1]|$x + 1}`
#[test]
fn test_lambda_bare_single_typed() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [1, 2, 3]->filter({x: Integer[1]|$x > 1})
        }
    "});
}

/// Single untyped in braces roundtrips as bare form
#[test]
fn test_lambda_braced_single_untyped() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [1, 2, 3]->map(x|$x + 10)
        }
    "});
}

/// Braced multi untyped lambda: `{x, y|$x + $y}`
#[test]
fn test_lambda_braced_multi_untyped() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [1, 2]->fold({x, y|$x + $y}, 0)
        }
    "});
}

/// Braced multi typed lambda: `{x: Integer[1], y: Integer[1]|$x + $y}`
#[test]
fn test_lambda_braced_multi_typed() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [1, 2]->fold({x: Integer[1], y: Integer[1]|$x + $y}, 0)
        }
    "});
}

/// No-param lambda: `|'hello'`
#[test]
fn test_lambda_no_param() {
    round_trip(indoc! {"
        function f(): Any[1]
        {
          |'hello'
        }
    "});
}

/// Lambda in let binding
#[test]
fn test_lambda_in_let() {
    round_trip(indoc! {"
        function f(): Any[1]
        {
          let fn = {x: Integer[1]|$x + 1};
          $fn
        }
    "});
}

/// Nested lambdas
#[test]
fn test_lambda_nested() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          [4, 5]->filter(x|[1, 2]->exists(y|$y == $x))
        }
    "});
}

/// Multiple lambda args in arrow function
#[test]
fn test_lambda_multiple_args() {
    round_trip(indoc! {"
        function f(): Any[*]
        {
          something()->func(x|$x + 1, 'hello', y|$y * 2)
        }
    "});
}

// ---------------------------------------------------------------------------
// Quoted identifiers
// ---------------------------------------------------------------------------

#[test]
fn test_package_with_quoted_identifier() {
    round_trip(indoc! {"
        Class test::'p a c k a g e'::A
        {
          's t r i n g': String[1];
        }
    "});
}

#[test]
fn test_quoted_enumerations() {
    round_trip(indoc! {"
        Enum <<st.test>> {doc.doc = 'bla'} '@'::'my Enum'
        {
          'Anything e',
          'A g',
          'Anything r'
        }
    "});
}

#[test]
fn test_numbers_in_enumeration_name() {
    round_trip(indoc! {"
        Enum my::Enum
        {
          '30_360',
          '30_ACT'
        }
    "});
}

#[test]
fn test_quoted_profile() {
    round_trip(indoc! {"
        Profile meta::pure::profiles::'with quotes'
        {
          stereotypes: ['two words'];
          tags: ['s tag', 'another tag'];
        }
    "});
}

// ---------------------------------------------------------------------------
// Multiple stereotypes & tagged values
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_multiple_tagged_and_stereotypes() {
    round_trip(indoc! {"
        Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'up1', doc.test2 = 'up2'} meta::this::class::has::path::A extends B
        {
          <<equality.Key, taggedValue.test>> {doc.doc = 'Borrowers date of birth'} name: e::R[*];
          {Descriptor.descriptionA = 'test1', Descriptor.descriptionB = 'test2'} ok: Integer[1..2];
          <<devStatus.inProgress>> q(s: String[1]) {$s + 'ok'}: c::d::R[1];
          {doc.test1 = 'test1', doc.test2 = 'test2'} xza(s: z::k::B[1]) {$s + 'ok'}: String[1];
        }
    "});
}

#[test]
fn test_class_with_quoted_tags_and_stereotypes() {
    round_trip(indoc! {"
        Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'up1', doc.test2 = 'up2'} meta::this::class::has::path::A extends B
        {
          <<'a profile'.'1>stereo'>> {'a profile'.'2>tag' = 'Borrowers date of birth'} name: e::R[*];
          {Descriptor.descriptionA = 'test1', Descriptor.descriptionB = 'test2'} ok: Integer[1..2];
        }
    "});
}

#[test]
fn test_tagged_values_special_char() {
    // Note: Pure strings use \' for escaped single quotes inside single-quoted strings
    round_trip(
        "Class <<temporal.businesstemporal, taggedValue.Number2>> {doc.test1 = 'test1\\'s', doc.test2 = 'm\\'s test'} meta::this::class::has::path::A\n{\n  <<equality.Key, taggedValue.test>> {doc.doc = 'uyaguari\\'s test', doc.test2 = 'm\\'s test'} name: e::R[*];\n}\n",
    );
}

// ---------------------------------------------------------------------------
// Complex constraints
// ---------------------------------------------------------------------------

#[test]
fn test_complex_constraints() {
    round_trip(indoc! {"
        Class A
        [
          c1
          (
            ~function: if($this.'O.K.' == 'ok', |true, |false)
            ~enforcementLevel: Warn
          ),
          c2
          (
            ~externalId: 'ext ID'
            ~function: if($this.'O.K.' == 'ok', |true, |false)
          ),
          c3
          (
            ~function: if($this.'O.K.' == 'ok', |true, |false)
            ~message: $this.'O.K.' + ' is not ok'
          ),
          c4
          (
            ~externalId: 'ext ID'
            ~function: if($this.'O.K.' == 'ok', |true, |false)
            ~enforcementLevel: Warn
            ~message: $this.'O.K.' + ' is not ok'
          )
        ]
        {
          name: String[45..*];
          'O.K.': Integer[1..2];
          xza(s: String[1]) {$s + 'ok'}: String[1];
        }
    "});
}

#[test]
fn test_unnamed_constraints_and_empty_profile() {
    round_trip(indoc! {"
        Class A
        [
          $this.ok->toOne() == 1,
          named: if($this.ok == 'ok', |true, |false),
          $this.ok->toOne()->toString() == $this.name
        ]
        {
          name: String[45..*];
          ok: Integer[1..2];
          xza(s: String[1]) {$s + 'ok'}: String[1];
        }

        Profile meta::pure::profiles::doc
        {
        }
    "});
}

#[test]
fn test_or_with_arithmetic_in_constraint() {
    round_trip(indoc! {"
        Class test::C
        [
          $this.id->isEmpty() || $this.id >= 0
        ]
        {
          id: Integer[0..1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Aggregation types
// ---------------------------------------------------------------------------

#[test]
fn test_class_properties_with_aggregation_type() {
    round_trip(indoc! {"
        Class my::Class
        {
          (shared) prop1: String[1];
          (none) prop2: String[1];
          (composite) prop3: String[1];
          prop4: String[1];
        }
    "});
}

#[test]
fn test_association_properties_with_aggregation_type() {
    round_trip(indoc! {"
        Association my::Assoc
        {
          (shared) prop1: String[1];
          (none) prop2: String[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Enumerations with annotations
// ---------------------------------------------------------------------------

#[test]
fn test_enumerations_with_annotations() {
    round_trip(indoc! {"
        Enum <<st.test>> {doc.doc = 'bla'} myEnum
        {
          <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} a,
          <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} b,
          c
        }

        Enum <<st.test>> {doc.doc = 'bla'} zz::MyOther
        {
          e,
          g,
          r
        }
    "});
}

// ---------------------------------------------------------------------------
// Associations with annotations
// ---------------------------------------------------------------------------

#[test]
fn test_associations_with_annotations() {
    round_trip(indoc! {"
        Association myAsso
        {
          a: String[1];
          b: a::c::A[1];
        }

        Association {doc.doc = 'bla'} k::p::Asso
        {
          a: Integer[1];
          b: a::c::B[1];
        }
    "});
}

// ---------------------------------------------------------------------------
// Measure — convertible
// ---------------------------------------------------------------------------

#[test]
fn test_measure_convertible() {
    round_trip(indoc! {"
        Measure NewMeasure
        {
          *UnitOne: x -> $x;
          UnitTwo: x -> $x * 1000;
          UnitThree: x -> $x * 400;
        }
    "});
}

#[test]
fn test_quoted_measure() {
    round_trip(indoc! {"
        Measure 'some measure'
        {
          *'Unit One': x -> $x;
          'Unit Two': x -> $x * 1000;
          'Unit Three': x -> $x * 400;
        }
    "});
}

#[test]
fn test_quoted_non_convertible_measure() {
    round_trip(indoc! {"
        Measure 'some measure'
        {
          'Unit One';
          'Unit Two';
          'Unit Three';
        }
    "});
}

// ---------------------------------------------------------------------------
// Measure units in types
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_unit_properties() {
    round_trip(indoc! {"
        Class A
        {
          unitOne: NewMeasure~UnitOne[0..1];
          unitTwo: NewMeasure~UnitTwo[0..1];
        }

        Measure NewMeasure
        {
          *UnitOne: x -> $x;
          UnitTwo: x -> $x * 1000;
          UnitThree: x -> $x * 400;
        }
    "});
}

// ---------------------------------------------------------------------------
// Derived properties with multiple statements
// ---------------------------------------------------------------------------

#[test]
fn test_derived_property_with_multiple_statements() {
    round_trip(indoc! {"
        Class Firm
        {
          prop1: Float[1];
          prop3: String[1];
          prop2() {
            let x = 0;
            $x;
          }: Integer[1];
        }
    "});
}

#[test]
fn test_qualified_property_with_params_and_multi_statements() {
    round_trip(indoc! {"
        Class Query
        {
          allFirms(limit: Integer[1]) {
            let offset = $limit + 10;
            Firm.all()->slice($limit, $offset);
          }: Firm[*];
        }
    "});
}

// ---------------------------------------------------------------------------
// Math precedence edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_math_precedence_comprehensive() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          let a = 1 / (2 / 3);
          let a = 1 * (2 * 3);
          let a = 1 - (2 - 3);
          let a = 1 + (2 + 3);
          let a = (8 / 4) * 2;
          let a = 8 / (4 * 2);
          let a = (8 * 4) / 2;
          let a = 8 * (4 / 2);
          let a = (8 * 4) + 2;
          let a = 8 * (4 + 2);
          let a = (8 + 4) * 2;
          let a = 8 + (4 * 2);
          let a = (1 - (4 * (2 + 3))) * 4;
          let a = ((1 - (4 * 2)) + 3) * 4;
          let a = (1 - (4 * 2)) + (3 * 4);
          let a = 1 + 4 + 2 + 3 + 4;
          let a = (1 + 2) - (3 - 4);
          let a = 1 + 2 <= 3 - 4;
          let a = (8 <= 4) + 2;
          let a = 8 + 4 <= 2
        }
    "});
}

#[test]
fn test_math_parenthesis_nested() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          let a = (1 - 4 * (2 + 3)) * 4
        }
    "});
}

#[test]
fn test_math_parenthesis_complex_nested() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          let a = 4 + 1 * (2 + 3) * 4 + (2 + 3) * 4
        }
    "});
}

#[test]
fn test_math_parenthesis_division_grouped() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          let a = 4 + (1 - 2) / (2 + 3) * (1 - 4 - 5)
        }
    "});
}

// ---------------------------------------------------------------------------
// New instance variants
// ---------------------------------------------------------------------------

#[test]
fn test_new_instance_all_types() {
    round_trip(indoc! {"
        Class anything::goes
        {
          v: String[1];
          v2: Integer[0..1];
          v3: Boolean[*];
        }

        function f(): Any[1]
        {
          let x = ^anything::goes(v='value', v2=17, v3=[true, false])
        }
    "});
}

#[test]
fn test_new_instance_nested() {
    round_trip(indoc! {"
        Class anything::goes
        {
          v: String[1];
        }

        Class anything::goes2
        {
          v2: anything::goes[1];
        }

        function f(): Any[1]
        {
          let x = ^anything::goes2(v2=^anything::goes(v='value'))
        }
    "});
}

// ---------------------------------------------------------------------------
// Collection with function
// ---------------------------------------------------------------------------

#[test]
fn test_collection_with_function() {
    round_trip(indoc! {"
        function package::test(value: meta::pure::metamodel::type::Any[0..1]): Boolean[1]
        {
          [true && (false && false), false]->oneOf()
        }
    "});
}

// ---------------------------------------------------------------------------
// Cast / type reference
// ---------------------------------------------------------------------------

#[test]
fn test_cast_with_type_ref() {
    round_trip(indoc! {"
        function abc::cast(): Float[1]
        {
          1->cast(@Float)
        }
    "});
}

#[test]
fn test_cast_with_literal() {
    round_trip(indoc! {"
        function abc::cast(): Float[1]
        {
          1->cast(1.0)
        }
    "});
}

#[test]
fn test_cast_with_string() {
    round_trip(indoc! {"
        function abc::cast(): String[1]
        {
          1->cast('String')
        }
    "});
}

// ---------------------------------------------------------------------------
// Function signature variants
// ---------------------------------------------------------------------------

#[test]
fn test_function_with_multiple_params() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          println('ok')
        }
    "});
}

#[test]
fn test_function_with_path() {
    round_trip(indoc! {"
        function withPath::f(s: Integer[1]): String[1]
        {
          println('ok');
          'a'
        }
    "});
}

// ---------------------------------------------------------------------------
// Decimal literals
// ---------------------------------------------------------------------------

#[test]
fn test_decimal_literal_one() {
    round_trip(indoc! {"
        function a::a(): Decimal[1]
        {
          1.0D
        }
    "});
}

#[test]
fn test_decimal_literal_many() {
    round_trip(indoc! {"
        function a::a(): Decimal[*]
        {
          [1.0D, 3.0D]
        }
    "});
}

// ---------------------------------------------------------------------------
// Primitive literals
// ---------------------------------------------------------------------------

#[test]
fn test_string_many() {
    round_trip(indoc! {"
        function a::a(): String[*]
        {
          ['ok', 'bla']
        }
    "});
}

#[test]
fn test_integer_many() {
    round_trip(indoc! {"
        function a::a(): String[*]
        {
          [1, 2]
        }
    "});
}

#[test]
fn test_boolean_many() {
    round_trip(indoc! {"
        function a::a(): String[*]
        {
          [true, false, true]
        }
    "});
}

#[test]
fn test_mixed_collection() {
    round_trip(indoc! {"
        function a::a(): String[*]
        {
          [1, 'a', true]
        }
    "});
}

// ---------------------------------------------------------------------------
// Full-path arrow function
// ---------------------------------------------------------------------------

#[test]
fn test_meta_function_with_full_path() {
    round_trip(indoc! {"
        function example::somethingElse(input: Integer[1]): Any[0..1]
        {
          [1, $input]->meta::pure::functions::math::max()
        }
    "});
}

// ---------------------------------------------------------------------------
// Quoted function parameters and variables
// ---------------------------------------------------------------------------

#[test]
fn test_function_with_quoted_parameters() {
    round_trip(indoc! {"
        function test::qoutedParams('1,2,3': Integer[3]): String[0..1]
        {
          $'1,2,3'->map(n|$n->toString())->joinStrings(',')
        }
    "});
}

#[test]
fn test_function_with_quoted_variables() {
    round_trip(indoc! {"
        function test::qoutedParams(): String[0..1]
        {
          let '1,2,3' = [1, 2, 3];
          $'1,2,3'->map(n|$n->toString())->joinStrings(',')
        }
    "});
}

// ---------------------------------------------------------------------------
// Default values — comprehensive
// ---------------------------------------------------------------------------

#[test]
fn test_default_value_class_property() {
    round_trip(indoc! {"
        Class test::A
        {
          classProperty: my::exampleRootType[1] = ^my::exampleRootType();
        }
    "});
}

#[test]
fn test_default_value_enum() {
    round_trip(indoc! {"
        Class test::A
        {
          enumProperty: test::EnumWithDefault[1] = test::EnumWithDefault.DefaultValue;
        }
    "});
}

#[test]
fn test_default_value_collection() {
    round_trip(indoc! {"
        Class test::A
        {
          collectionProperty: String[1..*] = ['one', 'two'];
        }
    "});
}

// ---------------------------------------------------------------------------
// Complex class (from Java testComplexClass)
// ---------------------------------------------------------------------------

#[test]
fn test_complex_class() {
    // This test uses inline \n because it contains complex escape sequences
    // that interact with indoc's dedent logic
    round_trip(
        "Class 'A-Z'\n[\n  constraint1: $this.ok->toOne() == 1,\n  constraint2: if($this.ok == 'ok', |true, |false),\n  'constraint-3': $this.anyValue->instanceOf(String) || $this.anyValue->instanceOf(AEnum)\n]\n{\n  name: String[45..*];\n  ok: Integer[1..2];\n  anyValue: Any[1];\n  'maybe or maybe not!': Boolean[1];\n  xza(s: String[1]) {$s + 'ok\\n{\"\"}\\'\\''}: String[1];\n  'I\\'m derived'('#String': String[1]) {$s + 'ok\\n{\"\"}\\'\\''}: String[1];\n}\n\nEnum AEnum\n{\n  B\n}\n",
    );
}

// ---------------------------------------------------------------------------
// Large mixed domain
// ---------------------------------------------------------------------------

#[test]
fn test_domain_mixed() {
    round_trip(indoc! {"
        Class <<temporal.businesstemporal>> {doc.doc = 'bla'} A extends B
        [
          constraint1: $this.ok->toOne() == 1,
          constraint2: $this.ok->toOne()->toString() == $this.name
        ]
        {
          <<equality.Key>> {doc.doc = 'bla'} name: e::R[*];
          {doc.doc = 'bla'} ok: Integer[1..2];
          <<devStatus.inProgress>> q(s: String[1]) {$s + 'ok'}: c::d::R[1];
          {doc.doc = 'bla'} xza(s: z::k::B[1]) {$s + 'ok'}: String[1];
        }

        Association myAsso
        {
          a: String[1];
          b: a::c::A[1];
        }

        Enum <<st.test>> {doc.doc = 'bla'} z::k::B
        {
          <<equality.Key, taggedValue.test>> {doc.doc = 'Tag Value for enum Value'} a,
          b,
          c
        }

        Profile meta::pure::profiles::doc
        {
          stereotypes: [deprecated];
          tags: [doc, todo];
        }

        Profile meta::pure::profiles::profile2
        {
          tags: [doc, todo];
        }
    "});
}

// ---------------------------------------------------------------------------
// Import statements
// ---------------------------------------------------------------------------

#[test]
fn test_class_with_import() {
    round_trip(indoc! {"
        import anything::*;
        Class anything::goes2
        {
        }
    "});
}

#[test]
fn test_enum_with_import() {
    round_trip(indoc! {"
        import my::models::*;
        Enum my::models::Color
        {
          Red,
          Green,
          Blue
        }
    "});
}

#[test]
fn test_function_with_import() {
    round_trip(indoc! {"
        import meta::pure::functions::*;
        function my::add(a: Integer[1], b: Integer[1]): Integer[1]
        {
          $a + $b
        }
    "});
}

#[test]
fn test_multiple_imports() {
    round_trip(indoc! {"
        import anything::*;
        import other::stuff::*;
        Class anything::goes2
        {
        }
    "});
}

// ---------------------------------------------------------------------------
// Section headers
// ---------------------------------------------------------------------------

/// Single Pure section: `###Pure` header is normalized away (it's the default).
#[test]
fn test_section_header_single_pure_normalized() {
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

// ---------------------------------------------------------------------------
// More expression coverage (from Java TestDomainGrammarRoundtrip)
// ---------------------------------------------------------------------------

#[test]
fn test_function_println() {
    round_trip(indoc! {"
        function f(s: Integer[1], s2: Interger[2]): String[1]
        {
          println('ok')
        }
    "});
}

#[test]
fn test_to_with_type_ref() {
    round_trip(indoc! {"
        function abc::to(): Float[1]
        {
          toVariant(1)->to(@Float)
        }
    "});
}

#[test]
fn test_to_with_literal() {
    round_trip(indoc! {"
        function abc::to(): Float[1]
        {
          toVariant(1)->to(1.0)
        }
    "});
}

#[test]
fn test_to_with_string() {
    round_trip(indoc! {"
        function abc::to(): String[1]
        {
          toVariant(1)->to('String')
        }
    "});
}

#[test]
fn test_to_many_with_type_ref() {
    round_trip(indoc! {"
        function abc::to(): Float[*]
        {
          toVariant(1)->toMany(@Float)
        }
    "});
}

#[test]
fn test_function_with_new() {
    round_trip(indoc! {"
        Class anything::goes
        {
          v: String[1];
        }

        function f(): Any[1]
        {
          let x = ^anything::goes(v='value')
        }
    "});
}

#[test]
fn test_function_with_new_all_types() {
    round_trip(indoc! {"
        Class anything::goes
        {
          v: String[1];
          v2: Integer[0..1];
          v3: Boolean[*];
        }

        function f(): Any[1]
        {
          let x = ^anything::goes(v='value', v2=1, v3=[true, false])
        }
    "});
}

#[test]
fn test_function_test() {
    round_trip(indoc! {"
        function my::testFunc(s: String[1]): String[1]
        {
          $s + ' world'
        }
        {
          test1 | testFunc('hello') => 'hello world';
        }
    "});
}

#[test]
fn test_boolean_precedence_or_and() {
    round_trip(indoc! {"
        function withPath::f(s: Integer[1]): String[1]
        {
          false || true && false;
          'a'
        }
    "});
}

#[test]
fn test_boolean_precedence_and_or() {
    round_trip(indoc! {"
        function withPath::f(s: Integer[1]): String[1]
        {
          (true || false) && true;
          'a'
        }
    "});
}

#[test]
fn test_multi_if_expressions() {
    round_trip(indoc! {"
        function model::firms(): String[1]
        {
          if($this.name == 'A', |'FirmA', |if($this.name == 'B', |'FirmB', |'Other'))
        }
    "});
}

#[test]
fn test_instance_with_default_value() {
    round_trip(indoc! {"
        Class my::Address
        {
          street: String[1] = 'Main St';
          city: String[1] = 'NY';
        }
    "});
}

// ---------------------------------------------------------------------------
// Graph Fetch Trees — ported from Java TestDomainGrammarRoundtrip
// ---------------------------------------------------------------------------

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
              ->subType(@test::FirmSubType){
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
              ->subType(@test::FirmSubType1){
                SubTypeName1
              },
              ->subType(@test::FirmSubType2){
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
              ->subType(@test::FirmSubType){
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
              ->subType(@test::FirmSubType1){
                'alias1':SubTypeName
              },
              ->subType(@test::FirmSubType2){
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
