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
//! parse `#{ Type { … } }#` via dsl-graph parser
//! → compose via dsl-graph composer
//! → re-parse → assert structural equality.

use legend_pure_dsl_graph::compose::default_island_composers;
use legend_pure_dsl_graph::parser::default_island_parsers;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_compose::island::compose_island_with;
use legend_pure_parser_compose::writer::IndentWriter;

fn parse(src: &str) -> legend_pure_parser_ast::SourceFile {
    let wrapped = format!("function test::f(): Any[*] {{ {src} }}");
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

#[test]
fn graph_fetch_round_trips_through_dsl_graph_composer() {
    let original = "#{
            my::Person {
                firstName,
                lastName,
                address {
                    city
                }
            }
        }#";
    let parsed_first = parse(original);
    let composed = first_island_text(&parsed_first);

    // The composed text should still parse, and the second compose
    // should match the first (idempotency).
    let parsed_second = parse(&composed);
    let composed_again = first_island_text(&parsed_second);

    assert_eq!(
        composed, composed_again,
        "round-trip lost structural equality.\n--- first ---\n{composed}\n--- second ---\n{composed_again}"
    );
}
