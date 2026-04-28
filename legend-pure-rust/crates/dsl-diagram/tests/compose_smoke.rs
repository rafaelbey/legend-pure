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

//! Round-trip: parse a Diagram source, compose it back, parse again,
//! assert the second AST equals the first (modulo `source_info`).

use indoc::indoc;
use legend_pure_dsl_diagram::ast::DiagramDef;
use legend_pure_dsl_diagram::compose::{compose_diagram, compose_diagram_section};
use legend_pure_dsl_diagram::parser::DiagramSectionParser;
use legend_pure_parser_ast::element::Element;

fn parse(source: &str) -> DiagramDef {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "compose_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(DiagramSectionParser)],
    );
    let file = match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    };
    let dsl = match &file.sections[0].elements[0] {
        Element::DSLElement(d) => d,
        other => panic!("expected DSLElement, got {other:?}"),
    };
    dsl.as_any()
        .downcast_ref::<DiagramDef>()
        .expect("DiagramDef")
        .clone()
}

/// Compares two DiagramDefs ignoring source_info — fields are equal
/// when their structural payload matches even if spans differ. We
/// achieve this cheaply by composing each side and comparing the
/// emitted text.
fn structurally_equal(a: &DiagramDef, b: &DiagramDef) -> bool {
    compose_diagram(a) == compose_diagram(b)
}

#[test]
fn diagram_round_trip_preserves_structure() {
    let source = indoc! {r"
        ###Diagram
        Diagram model::test::TestDiagram(width=5000.3, height=2700.6)
        {
            TypeView A(type=model::test::A,
                       stereotypesVisible=true, attributesVisible=true,
                       attributeStereotypesVisible=true, attributeTypesVisible=true,
                       color='#FFFFCC', lineWidth=1.0,
                       position=(874.0, 199.46875), width=353.0, height=57.1875)
            TypeView B(type=model::test::B,
                       color='#FFFFCC',
                       position=(75.0, 97.1875), width=113.0, height=57.1875)
            AssociationView A2B(association=model::test::A2B,
                                stereotypesVisible=true, nameVisible=false,
                                color='#000000', lineWidth=1.0,
                                lineStyle=SIMPLE,
                                points=[(132.5, 77.0), (155.2, 77.0)],
                                label='A to B',
                                source=A, target=B,
                                sourcePropertyPosition=(132.5, 76.2),
                                sourceMultiplicityPosition=(132.5, 80.0),
                                targetPropertyPosition=(155.2, 76.2),
                                targetMultiplicityPosition=(155.2, 80.0))
            PropertyView A_prop(property=model::test::A.prop,
                                source=A, target=B,
                                propertyPosition=(132.5, 76.2),
                                multiplicityPosition=(132.5, 80.0))
            GeneralizationView B_A(source=B, target=A)
        }
    "};

    let parsed_first = parse(source);
    let composed = compose_diagram_section(&[&parsed_first]);
    let parsed_second = parse(&composed);

    assert!(
        structurally_equal(&parsed_first, &parsed_second),
        "round-trip failed.\n--- composed ---\n{composed}\n--- second parse views ---\n{:?}",
        parsed_second.views
    );
}

#[test]
fn compose_diagram_section_emits_section_header() {
    let source = indoc! {r"
        ###Diagram
        Diagram pkg::Tiny
        {
            TypeView X(type=pkg::X)
        }
    "};
    let parsed = parse(source);
    let composed = compose_diagram_section(&[&parsed]);
    assert!(
        composed.starts_with("###Diagram\n"),
        "section composer must emit the section header; got:\n{composed}"
    );
}
