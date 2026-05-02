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

//! Locks the `TDSIslandParser` end-to-end: a `#TDS\n cols\n rows\n#`
//! expression embedded in a function body parses through
//! `parse_with_islands` + `default_island_parsers()` (provided by
//! `dsl-tds`), produces an `IslandExpression` whose content
//! downcasts to `TDSExpr`, and preserves the column + row shape.

use indoc::indoc;
use legend_pure_dsl_tds::ast::TDSExpr;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_ast::expression::Expression;

fn parse_first_expression(src: &str) -> Expression {
    // Wrap the body in a function so it parses as a top-level
    // expression. `\n` literals inside `src` already align with the
    // raw-content body shape the parser expects.
    let wrapped = format!("function test::f(): Any[*]\n{{\n{src}\n}}");
    let file = legend_pure_parser_parser::parse_with_islands(
        &wrapped,
        "tds_smoke.pure",
        default_island_parsers(),
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    });
    let elem = &file.sections[0].elements[0];
    let Element::Function(f) = elem else {
        panic!("expected Function, got {elem:?}");
    };
    f.body.first().cloned().expect("function has body")
}

fn assert_tds(expr: &Expression) -> TDSExpr {
    let Expression::Island(isl) = expr else {
        panic!("expected Island, got {expr:?}");
    };
    isl.content
        .as_any()
        .downcast_ref::<TDSExpr>()
        .expect("downcast to TDSExpr")
        .clone()
}

#[test]
fn untyped_columns_with_two_rows() {
    let expr = parse_first_expression(indoc! {"
        #TDS
          value, other, name
          1, 3, A
          2, 4, B
        #
    "});
    let tds = assert_tds(&expr);
    let col_names: Vec<&str> = tds.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(col_names, vec!["value", "other", "name"]);
    assert!(tds.columns.iter().all(|c| c.type_ref.is_none()));

    assert_eq!(tds.rows.len(), 2);
    assert_eq!(tds.rows[0].len(), 3);
    assert_eq!(tds.rows[0][0].raw.as_str(), "1");
    assert_eq!(tds.rows[0][2].raw.as_str(), "A");
    assert_eq!(tds.rows[1][1].raw.as_str(), "4");
}

#[test]
fn columns_with_simple_types() {
    let expr = parse_first_expression(indoc! {"
        #TDS
          value:Float, other:Number, name
          1, 3, A
          2, 4, B
        #
    "});
    let tds = assert_tds(&expr);

    let value = &tds.columns[0];
    assert_eq!(value.name.as_str(), "value");
    let value_type = value.type_ref.as_ref().expect("value has type_ref");
    assert_eq!(value_type.name.as_str(), "Float");
    assert!(value_type.multiplicity.is_none());

    let name = &tds.columns[2];
    assert_eq!(name.name.as_str(), "name");
    assert!(name.type_ref.is_none());
}

#[test]
fn columns_with_typed_multiplicity_and_quoted_name() {
    let expr = parse_first_expression(indoc! {"
        #TDS
          value : Float[1], other: Numeric(10,4)[1], 'name ok' :String
          1, 3, A
          2, 4, B
        #
    "});
    let tds = assert_tds(&expr);

    assert_eq!(tds.columns[0].name.as_str(), "value");
    let value_type = tds.columns[0].type_ref.as_ref().unwrap();
    assert_eq!(value_type.name.as_str(), "Float");
    assert_eq!(value_type.multiplicity.as_ref().unwrap().as_str(), "1");

    let other_type = tds.columns[1].type_ref.as_ref().unwrap();
    assert_eq!(other_type.name.as_str(), "Numeric(10,4)");
    assert_eq!(other_type.multiplicity.as_ref().unwrap().as_str(), "1");

    assert_eq!(tds.columns[2].name.as_str(), "name ok");
    let name_type = tds.columns[2].type_ref.as_ref().unwrap();
    assert_eq!(name_type.name.as_str(), "String");
    assert!(name_type.multiplicity.is_none());
}

#[test]
fn empty_rows_section_parses_with_only_header() {
    let expr = parse_first_expression(indoc! {"
        #TDS
          a, b, c
        #
    "});
    let tds = assert_tds(&expr);
    let col_names: Vec<&str> = tds.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(col_names, vec!["a", "b", "c"]);
    assert_eq!(tds.rows.len(), 0);
}

#[test]
fn cells_accept_unquoted_datetime_literals() {
    // Real shape from `core_functions_relation/relation/functions/olap/reduce.pure`.
    // The lexer breaks `2024-01-29T00:32:34.000000000+0000` into many tokens
    // (identifier `T00`, `:`, integers, `.`, `+`, …); CSV-style cell parsing
    // collects them all into one `TDSCell.raw` until the next `,` or row end.
    let expr = parse_first_expression(indoc! {"
        #TDS
          p, o, i
          0, 2024-01-29T00:32:34.000000000+0000, 10
          100, 2024-01-31T00:32:34.000000000+0000, 30
        #
    "});
    let tds = assert_tds(&expr);

    assert_eq!(tds.rows.len(), 2);
    assert_eq!(tds.rows[0][0].raw.as_str(), "0");
    assert_eq!(
        tds.rows[0][1].raw.as_str(),
        "2024-01-29T00:32:34.000000000+0000"
    );
    assert_eq!(tds.rows[0][2].raw.as_str(), "10");
    assert_eq!(
        tds.rows[1][1].raw.as_str(),
        "2024-01-31T00:32:34.000000000+0000"
    );
}

#[test]
fn cells_accept_negative_integers_and_decimals() {
    // Real shape from `core_dataquality_test/dataquality_relation_helper_test.pure`
    // (negatives) and several `core_functions_standard/math/aggregator/*.pure`
    // (decimals/floats). The lexer emits `-3` as `[Minus, IntegerLiteral]` —
    // `parse_cell` must consume both and concatenate into a single
    // `TDSCell.raw`.
    let expr = parse_first_expression(indoc! {"
        #TDS
          v: Integer, f: Float, d: Decimal
          -3, -3.14, 99.99D
          42, 0.5, -10.0D
        #
    "});
    let tds = assert_tds(&expr);

    assert_eq!(tds.rows.len(), 2);
    assert_eq!(tds.rows[0][0].raw.as_str(), "-3");
    assert_eq!(tds.rows[0][1].raw.as_str(), "-3.14");
    assert_eq!(tds.rows[0][2].raw.as_str(), "99.99D");
    assert_eq!(tds.rows[1][0].raw.as_str(), "42");
    assert_eq!(tds.rows[1][1].raw.as_str(), "0.5");
    assert_eq!(tds.rows[1][2].raw.as_str(), "-10.0D");
}
