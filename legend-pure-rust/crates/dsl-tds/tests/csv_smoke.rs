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
    parsed.columns.iter().map(|c| c.type_tag.clone()).collect()
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

// ---------------------------------------------------------------------------
// Type-mismatch error reporting (override forces a type the data
// can't honour). Inference itself never produces a mismatch — these
// only fire when an explicit `name:Type` annotation overrides the
// data-inferred classification.
// ---------------------------------------------------------------------------

#[test]
fn integer_override_with_non_integer_cell_reports_error() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Integer),
        multiplicity: None,
    }];
    let err = parse_and_infer("v\nhello\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("Integer"), "got: {err}");
    assert!(err.message.contains("hello"), "got: {err}");
    // Row counter in the message is the 1-based source line (header is
    // line 1; the first data line is line 2). `err.line` matches.
    assert_eq!(err.line, 2);
    assert_eq!(err.column, 1);
}

#[test]
fn float_override_with_alphabetic_cell_reports_error() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Float),
        multiplicity: None,
    }];
    let err = parse_and_infer("v\nNaNish\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("Float"), "got: {err}");
    assert!(err.message.contains("NaNish"), "got: {err}");
}

#[test]
fn boolean_override_with_yes_no_cell_reports_error() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Boolean),
        multiplicity: None,
    }];
    let err = parse_and_infer("flag\nyes\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("Boolean"), "got: {err}");
    assert!(err.message.contains("yes"), "got: {err}");
    assert!(
        err.message.to_lowercase().contains("true") || err.message.contains("false"),
        "expected the diagnostic to suggest the valid forms; got: {err}"
    );
}

#[test]
fn type_mismatch_points_at_first_offending_row() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Integer),
        multiplicity: None,
    }];
    let err = parse_and_infer("v\n1\n2\nbroken\n4\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("broken"), "got: {err}");
    assert_eq!(err.line, 4, "expected the line of the bad cell, got: {err}");
}

#[test]
fn integer_override_accepts_integer_cells() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Integer),
        multiplicity: None,
    }];
    let parsed = parse_and_infer("v\n1\n42\n", &overrides).expect("should succeed");
    assert_eq!(parsed.columns[0].type_tag, ColumnType::Integer);
    assert_eq!(parsed.rows.len(), 2);
}

#[test]
fn boolean_override_accepts_case_insensitive_true_false() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Boolean),
        multiplicity: None,
    }];
    let parsed = parse_and_infer("flag\nTrue\nFALSE\n", &overrides).expect("should succeed");
    assert!(matches!(
        parsed.rows[0][0],
        Some(TypedCell::Boolean(true))
    ));
    assert!(matches!(
        parsed.rows[1][0],
        Some(TypedCell::Boolean(false))
    ));
}

// ---------------------------------------------------------------------------
// Multiplicity-mismatch error reporting (override forces a tighter
// bound than the data supports).
// ---------------------------------------------------------------------------

#[test]
fn pure_one_override_with_empty_cell_reports_error() {
    let overrides = vec![
        ColumnOverride::default(),
        ColumnOverride {
            type_tag: None,
            multiplicity: Some(Multiplicity::PureOne),
        },
    ];
    let err = parse_and_infer("a, b\n1, X\n2,\n3, Z\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("'b'"), "got: {err}");
    assert!(err.message.contains('1'), "got: {err}"); // multiplicity literal
    // The message uses 1-based data-row indexing — the empty cell is
    // on the second data row.
    assert!(err.message.contains("row 2"), "got: {err}");
    // `err.line` is the source line; header on line 1 + 2 data rows.
    assert_eq!(err.line, 3);
}

#[test]
fn one_or_many_override_with_empty_cell_reports_error() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::OneOrMany),
    }];
    let err = parse_and_infer("v\n1\n\n3\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("'v'"), "got: {err}");
    assert!(err.message.contains("1..*"), "got: {err}");
}

#[test]
fn zero_or_one_override_allows_empty_cells() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::ZeroOrOne),
    }];
    let parsed = parse_and_infer("v\n1\n\n3\n", &overrides).expect("should succeed");
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::ZeroOrOne);
    assert_eq!(parsed.rows[1][0], None);
}

#[test]
fn pure_one_override_with_all_cells_present_succeeds() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::PureOne),
    }];
    let parsed = parse_and_infer("v\n1\n2\n3\n", &overrides).expect("should succeed");
    assert_eq!(parsed.columns[0].multiplicity, Multiplicity::PureOne);
}

#[test]
fn range_override_with_min_one_validates_like_pure_one() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::Range {
            lower: 1,
            upper: Some(3),
        }),
    }];
    let err = parse_and_infer("v\n1\n\n3\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("1..3"), "got: {err}");
}

#[test]
fn range_override_with_min_zero_allows_empty_cells() {
    let overrides = vec![ColumnOverride {
        type_tag: None,
        multiplicity: Some(Multiplicity::Range {
            lower: 0,
            upper: Some(2),
        }),
    }];
    let parsed = parse_and_infer("v\n1\n\n3\n", &overrides).expect("should succeed");
    assert_eq!(parsed.rows.len(), 3);
    assert_eq!(parsed.rows[1][0], None);
}

// ---------------------------------------------------------------------------
// Qualified-class column types (`payload:meta::pure::metamodel::variant::Variant`
// and friends). Cells store as String; the type carries the package
// path so the lowerer can emit a fully-qualified TypeReference.
// ---------------------------------------------------------------------------

#[test]
fn qualified_class_override_carries_package_and_name() {
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Other {
            package: Some("meta::pure::metamodel::variant".into()),
            name: "Variant".into(),
        }),
        multiplicity: None,
    }];
    let parsed = parse_and_infer("v\n\"[1,2,3]\"\n", &overrides).expect("should succeed");
    let ColumnType::Other { package, name } = &parsed.columns[0].type_tag else {
        panic!("expected Other variant; got {:?}", parsed.columns[0].type_tag);
    };
    assert_eq!(package.as_ref().unwrap().as_str(), "meta::pure::metamodel::variant");
    assert_eq!(name.as_str(), "Variant");
    // Cells stored as String — the runtime reconstructs the typed
    // value from the raw text.
    assert!(matches!(parsed.rows[0][0], Some(TypedCell::String(_))));
}

#[test]
fn other_column_with_empty_cell_under_pure_one_still_reports_multiplicity_error() {
    // `Other` doesn't reject any non-empty text but still participates
    // in multiplicity validation.
    let overrides = vec![ColumnOverride {
        type_tag: Some(ColumnType::Other {
            package: None,
            name: "MyClass".into(),
        }),
        multiplicity: Some(Multiplicity::PureOne),
    }];
    let err = parse_and_infer("v\nfoo\n\nbaz\n", &overrides).expect_err("should fail");
    assert!(err.message.contains("MyClass"), "got: {err}");
    assert!(err.message.contains("row 2"), "got: {err}");
}

#[test]
fn pure_type_package_returns_path_for_other_with_qualified_name() {
    let t = ColumnType::Other {
        package: Some("meta::pure::metamodel::variant".into()),
        name: "Variant".into(),
    };
    assert_eq!(
        t.pure_type_package(),
        Some("meta::pure::metamodel::variant")
    );
    assert_eq!(t.pure_type_name(), "Variant");
}

#[test]
fn pure_type_package_returns_none_for_primitives() {
    assert_eq!(ColumnType::Integer.pure_type_package(), None);
    assert_eq!(ColumnType::String.pure_type_package(), None);
    assert_eq!(ColumnType::DateTime.pure_type_package(), None);
}

#[test]
fn pure_type_package_returns_none_for_other_with_bare_name() {
    let t = ColumnType::Other {
        package: None,
        name: "Variant".into(),
    };
    assert_eq!(t.pure_type_package(), None);
    assert_eq!(t.pure_type_name(), "Variant");
}
