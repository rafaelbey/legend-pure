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

//! End-to-end round-trip:
//! parse `#TDS\n cols\n rows\n#` via dsl-tds parser
//! → compose via dsl-tds composer
//! → re-parse → assert structural equality of the AST.

use indoc::indoc;
use legend_pure_dsl_tds::ast::TDSExpr;
use legend_pure_dsl_tds::compose::default_island_composers;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_compose::island::compose_island_with;
use legend_pure_parser_compose::writer::IndentWriter;

fn parse(src: &str) -> legend_pure_parser_ast::SourceFile {
    let wrapped = format!("function test::f(): Any[*]\n{{\n{src}\n}}");
    legend_pure_parser_parser::parse_with_islands(
        &wrapped,
        "compose_smoke.pure",
        default_island_parsers(),
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

fn first_island_text(file: &legend_pure_parser_ast::SourceFile) -> String {
    let elem = &file.sections[0].elements[0];
    let Element::Function(f) = elem else {
        panic!("expected Function");
    };
    let expr = f.body.first().expect("non-empty body");
    let Expression::Island(isl) = expr else {
        panic!("expected Island");
    };
    let mut w = IndentWriter::new();
    compose_island_with(&mut w, isl, &default_island_composers());
    w.finish()
}

fn first_tds(file: &legend_pure_parser_ast::SourceFile) -> TDSExpr {
    let elem = &file.sections[0].elements[0];
    let Element::Function(f) = elem else {
        panic!("expected Function");
    };
    let Expression::Island(isl) = f.body.first().expect("body") else {
        panic!("expected Island");
    };
    isl.content
        .as_any()
        .downcast_ref::<TDSExpr>()
        .expect("downcast")
        .clone()
}

#[test]
fn untyped_tds_round_trips_structurally() {
    let parsed = parse(indoc! {"
        #TDS
          value, other, name
          1, 3, A
          2, 4, B
        #
    "});
    let original_tds = first_tds(&parsed);
    let composed = first_island_text(&parsed);

    let parsed_again = parse(&composed);
    let composed_again = first_island_text(&parsed_again);
    let composed_tds = first_tds(&parsed_again);

    // Two text composes should be byte-identical (idempotency).
    assert_eq!(
        composed, composed_again,
        "compose idempotency failed.\n--- first ---\n{composed}\n--- second ---\n{composed_again}"
    );

    // The AST should also round-trip structurally — column names,
    // type refs, and row cells all preserved.
    let cols_a: Vec<&str> = original_tds
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let cols_b: Vec<&str> = composed_tds
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(cols_a, cols_b);
    assert_eq!(original_tds.rows.len(), composed_tds.rows.len());
    for (r1, r2) in original_tds.rows.iter().zip(&composed_tds.rows) {
        let v1: Vec<&str> = r1.iter().map(|c| c.raw.as_str()).collect();
        let v2: Vec<&str> = r2.iter().map(|c| c.raw.as_str()).collect();
        assert_eq!(v1, v2);
    }
}

#[test]
fn typed_columns_round_trip_with_types_and_multiplicity() {
    let parsed = parse(indoc! {"
        #TDS
          value : Float[1], other: Numeric(10,4)[1], 'name ok' :String
          1, 3, A
          2, 4, B
        #
    "});
    let original_tds = first_tds(&parsed);
    let composed = first_island_text(&parsed);
    let composed_tds = first_tds(&parse(&composed));

    let original_types: Vec<Option<String>> = original_tds
        .columns
        .iter()
        .map(|c| {
            c.type_ref.as_ref().map(|t| {
                format!(
                    "{}{}",
                    t.name,
                    t.multiplicity
                        .as_ref()
                        .map_or(String::new(), |m| format!("[{m}]"))
                )
            })
        })
        .collect();
    let composed_types: Vec<Option<String>> = composed_tds
        .columns
        .iter()
        .map(|c| {
            c.type_ref.as_ref().map(|t| {
                format!(
                    "{}{}",
                    t.name,
                    t.multiplicity
                        .as_ref()
                        .map_or(String::new(), |m| format!("[{m}]"))
                )
            })
        })
        .collect();
    assert_eq!(original_types, composed_types);
}

#[test]
fn header_only_tds_round_trips() {
    let parsed = parse(indoc! {"
        #TDS
          a, b, c
        #
    "});
    let composed = first_island_text(&parsed);
    let parsed_again = parse(&composed);
    let composed_again = first_island_text(&parsed_again);
    assert_eq!(composed, composed_again);
}
