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
