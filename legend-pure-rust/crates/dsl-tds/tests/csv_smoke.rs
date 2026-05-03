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

//! Locks the shared CSV parser + per-column type inference used by
//! both the compile-time `#TDS#` lowerer and the runtime
//! `stringToTDS` native. Each test maps to a precedence rule in
//! [`legend_pure_dsl_tds::csv::infer_column`].

use indoc::indoc;
use legend_pure_dsl_tds::csv::{ColumnOverride, ColumnType, ParsedTDS, TypedCell, parse_and_infer};
use legend_pure_parser_pure::types::Multiplicity;

fn parse(csv: &str) -> ParsedTDS {
    parse_and_infer(csv, &[]).expect("parse_and_infer should succeed")
}

fn col_types(parsed: &ParsedTDS) -> Vec<ColumnType> {
    parsed.columns.iter().map(|c| c.type_tag).collect()
}

#[test]
fn integer_column_inferred_when_all_cells_are_integer_literals() {
    let parsed = parse(indoc! {"
        a, b
        1, 2
        3, 4
    "});
    assert_eq!(
        col_types(&parsed),
        vec![ColumnType::Integer, ColumnType::Integer]
    );
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::PureOne);
    assert_eq!(parsed.rows[0][0], Some(TypedCell::Integer(1)));
    assert_eq!(parsed.rows[1][1], Some(TypedCell::Integer(4)));
}

#[test]
fn negative_integers_are_integers_not_floats() {
    let parsed = parse(indoc! {"
        v
        -3
        42
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::Integer]);
    assert_eq!(parsed.rows[0][0], Some(TypedCell::Integer(-3)));
    assert_eq!(parsed.rows[1][0], Some(TypedCell::Integer(42)));
}

#[test]
fn float_column_inferred_when_any_cell_has_decimal_point() {
    let parsed = parse(indoc! {"
        v
        1.5
        2.0
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::Float]);
    assert!(matches!(parsed.rows[0][0], Some(TypedCell::Float(_))));
}

#[test]
fn decimal_column_inferred_with_d_suffix() {
    let parsed = parse(indoc! {"
        amount
        99.99D
        -10.5D
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::Decimal]);
    let TypedCell::Decimal(d0) = parsed.rows[0][0].as_ref().unwrap() else {
        panic!("expected Decimal cell");
    };
    assert_eq!(d0.as_str(), "99.99D");
}

#[test]
fn boolean_column_inferred_case_insensitive() {
    let parsed = parse(indoc! {"
        flag
        true
        FALSE
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::Boolean]);
    assert_eq!(parsed.rows[0][0], Some(TypedCell::Boolean(true)));
    assert_eq!(parsed.rows[1][0], Some(TypedCell::Boolean(false)));
}

#[test]
fn strict_date_column_inferred() {
    let parsed = parse(indoc! {"
        d
        2024-01-29
        1999-12-31
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::StrictDate]);
}

#[test]
fn datetime_column_inferred_with_offset() {
    let parsed = parse(indoc! {"
        ts
        2024-01-29T00:32:34.000+0000
        2024-01-30T15:00:00Z
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::DateTime]);
}

#[test]
fn string_column_inferred_when_quoted_cells_present() {
    let parsed = parse(indoc! {"
        name
        \"Alice\"
        \"Bob\"
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::String]);
    assert_eq!(parsed.rows[0][0], Some(TypedCell::String("Alice".into())));
}

#[test]
fn string_column_falls_back_when_cells_dont_match_any_typed_pattern() {
    let parsed = parse(indoc! {"
        word
        hello
        world
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::String]);
}

#[test]
fn quoted_string_takes_precedence_over_numeric_inference() {
    // Even though `'1'` looks integer, the quotes force String.
    let parsed = parse(indoc! {"
        v
        \"1\"
        \"2\"
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::String]);
    assert_eq!(parsed.rows[0][0], Some(TypedCell::String("1".into())));
}

#[test]
fn empty_cell_promotes_multiplicity_to_zero_or_one() {
    let parsed = parse_and_infer("v, name\n1, A\n, B\n3, C\n", &[]).expect("parse");
    assert_eq!(parsed.columns[0].name.as_str(), "v");
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::ZeroOrOne);
    assert_eq!(parsed.columns[1].multiplicity, Multiplicity::PureOne);
    assert_eq!(parsed.rows[1][0], None); // empty cell
}

#[test]
fn all_empty_column_defaults_to_string_zero_or_one() {
    let parsed = parse_and_infer("a, b\n1,\n2,\n", &[]).expect("parse");
    assert_eq!(parsed.columns[1].type_tag, ColumnType::String);
    assert_eq!(parsed.columns[1].multiplicity, Multiplicity::ZeroOrOne);
}

#[test]
fn header_override_replaces_inferred_type() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Float),
        multiplicity: None,
    }];
    let parsed = parse_and_infer("v\n1\n2\n", &overrides).expect("parse");
    assert_eq!(parsed.columns[0].type_tag, ColumnType::Float);
    // Cell was an integer literal; with Float override it should
    // materialise as Float.
    assert!(matches!(parsed.rows[0][0], Some(TypedCell::Float(_))));
}

#[test]
fn header_override_can_set_multiplicity_independently_of_type() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::ZeroOrOne),
    }];
    let parsed = parse_and_infer("v\n1\n2\n", &overrides).expect("parse");
    // No empty cells but the override forces ZeroOrOne.
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::ZeroOrOne);
    assert_eq!(parsed.columns[0].type_tag, ColumnType::Integer);
}

#[test]
fn empty_overrides_slice_means_full_inference() {
    let parsed = parse_and_infer("a\n1\n", &[]).expect("parse");
    assert_eq!(parsed.columns[0].type_tag, ColumnType::Integer);
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::PureOne);
}

#[test]
fn quoted_column_name_is_stripped_in_canonical_form() {
    let parsed = parse_and_infer("'name with space'\n'a'\n", &[]).expect("parse");
    assert_eq!(parsed.columns[0].name.as_str(), "name with space");
}

#[test]
fn embedded_comma_inside_quoted_cell_does_not_split() {
    let parsed =
        parse_and_infer("name\n\"Jersey City, NJ\"\n\"Austin, TX\"\n", &[]).expect("parse");
    assert_eq!(parsed.columns.len(), 1);
    assert_eq!(parsed.rows.len(), 2);
    assert_eq!(
        parsed.rows[0][0],
        Some(TypedCell::String("Jersey City, NJ".into()))
    );
}

#[test]
fn unterminated_quoted_string_is_an_error() {
    let err = parse_and_infer("name\n\"open\n", &[]).expect_err("should fail");
    assert!(
        err.message.to_lowercase().contains("unterminated"),
        "got: {err}"
    );
}

#[test]
fn row_arity_mismatch_is_an_error() {
    let err = parse_and_infer("a, b\n1\n", &[]).expect_err("should fail");
    assert!(err.message.contains("expected 2"), "got: {err}");
}

#[test]
fn empty_csv_is_an_error() {
    let err = parse_and_infer("", &[]).expect_err("should fail");
    assert!(err.message.to_lowercase().contains("empty"), "got: {err}");
}

#[test]
fn header_only_csv_parses_with_zero_rows() {
    let parsed = parse_and_infer("a, b, c\n", &[]).expect("parse");
    assert_eq!(parsed.columns.len(), 3);
    assert_eq!(parsed.rows.len(), 0);
}

#[test]
fn datetime_literal_with_subsecond_fraction_round_trips() {
    let parsed = parse(indoc! {"
        ts
        2024-01-29T00:32:34.000000000+0000
    "});
    assert_eq!(col_types(&parsed), vec![ColumnType::DateTime]);
    let TypedCell::DateTime(s) = parsed.rows[0][0].as_ref().unwrap() else {
        panic!("expected DateTime");
    };
    assert_eq!(s.as_str(), "2024-01-29T00:32:34.000000000+0000");
}

#[test]
fn pure_type_names_match_canonical_class_names() {
    assert_eq!(ColumnType::Integer.pure_type_name(), "Integer");
    assert_eq!(ColumnType::Float.pure_type_name(), "Float");
    assert_eq!(ColumnType::Decimal.pure_type_name(), "Decimal");
    assert_eq!(ColumnType::Boolean.pure_type_name(), "Boolean");
    assert_eq!(ColumnType::String.pure_type_name(), "String");
    assert_eq!(ColumnType::StrictDate.pure_type_name(), "StrictDate");
    assert_eq!(ColumnType::DateTime.pure_type_name(), "DateTime");
}
