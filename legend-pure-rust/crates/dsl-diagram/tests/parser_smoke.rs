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

//! End-to-end parse of a `###Diagram` section through
//! `parse_with_sections` + `DiagramSectionParser`.

use indoc::indoc;
use legend_pure_dsl_diagram::ast::{DiagramDef, DiagramView};
use legend_pure_dsl_diagram::parser::DiagramSectionParser;
use legend_pure_parser_ast::element::Element;

fn parse(source: &str) -> legend_pure_parser_ast::SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "diagram_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(DiagramSectionParser)],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

#[test]
fn parses_minimal_diagram_with_typeview() {
    let source = indoc! {r"
        ###Diagram
        Diagram model::test::TestDiagram(width=5000.3, height=2700.6)
        {
            TypeView A(type=model::test::A,
                       stereotypesVisible=true,
                       attributesVisible=true,
                       attributeStereotypesVisible=true,
                       attributeTypesVisible=true,
                       lineWidth=1.0,
                       position=(874.0, 199.46875),
                       width=353.0,
                       height=57.1875)
        }
    "};

    let file = parse(source);
    let section = &file.sections[0];
    assert_eq!(section.kind.as_str(), "Diagram");
    assert_eq!(section.elements.len(), 1);

    let dsl = match &section.elements[0] {
        Element::DSLElement(d) => d,
        other => panic!("expected DSLElement, got {other:?}"),
    };
    let diagram = dsl
        .as_any()
        .downcast_ref::<DiagramDef>()
        .expect("downcast to DiagramDef");

    assert_eq!(diagram.name.value.as_str(), "TestDiagram");
    let pkg = diagram.package.as_ref().expect("package").to_string();
    assert_eq!(pkg, "model::test");
    let geom = diagram.geometry.as_ref().expect("geometry");
    assert!((geom.width - 5000.3).abs() < 1e-6);
    assert!((geom.height - 2700.6).abs() < 1e-6);

    assert_eq!(diagram.views.len(), 1);
    let DiagramView::Type(tv) = &diagram.views[0] else {
        panic!("expected TypeView");
    };
    assert_eq!(tv.id.as_str(), "A");
    assert_eq!(tv.type_ref.full_path(), "model::test::A");
    assert_eq!(tv.stereotypes_visible, Some(true));
    assert_eq!(tv.attributes_visible, Some(true));
    assert_eq!(tv.attribute_stereotypes_visible, Some(true));
    assert_eq!(tv.attribute_types_visible, Some(true));
    assert_eq!(tv.line_width, Some(1.0));
    let pos = tv.position.as_ref().expect("position");
    assert!((pos.x - 874.0).abs() < 1e-6);
    assert!((pos.y - 199.46875).abs() < 1e-6);
    assert_eq!(tv.width, Some(353.0));
    assert_eq!(tv.height, Some(57.1875));
}

#[test]
fn parses_full_fixture_with_all_view_kinds() {
    // Mirror of the upstream Java test fixture
    // (`legend-pure-dsl-diagram/.../TestDiagramCompilation.java`),
    // adapted to use quoted color strings instead of bare `#FFFFCC`
    // since the core lexer doesn't yet handle hash-color literals.
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
                       stereotypesVisible=true, attributesVisible=true,
                       attributeStereotypesVisible=true, attributeTypesVisible=true,
                       color='#FFFFCC', lineWidth=1.0,
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
                                stereotypesVisible=true, nameVisible=false,
                                color='#000000', lineWidth=1.0,
                                lineStyle=SIMPLE,
                                points=[(132.5, 77.0), (155.2, 77.0)],
                                label='A.prop',
                                source=A, target=B,
                                propertyPosition=(132.5, 76.2),
                                multiplicityPosition=(132.5, 80.0))
            GeneralizationView B_A(color='#000000', lineWidth=1.0,
                                   lineStyle=SIMPLE,
                                   points=[(132.5, 77.0), (155.2, 77.0)],
                                   label='',
                                   source=B, target=A)
        }
    "};

    let file = parse(source);
    let section = &file.sections[0];
    let dsl = match &section.elements[0] {
        Element::DSLElement(d) => d,
        other => panic!("expected DSLElement, got {other:?}"),
    };
    let diagram = dsl
        .as_any()
        .downcast_ref::<DiagramDef>()
        .expect("downcast to DiagramDef");

    assert_eq!(
        diagram.views.len(),
        5,
        "expected 5 views, got {}",
        diagram.views.len()
    );

    let mut type_count = 0;
    let mut assoc_count = 0;
    let mut prop_count = 0;
    let mut gen_count = 0;
    for v in &diagram.views {
        match v {
            DiagramView::Type(_) => type_count += 1,
            DiagramView::Association(_) => assoc_count += 1,
            DiagramView::Property(_) => prop_count += 1,
            DiagramView::Generalization(_) => gen_count += 1,
        }
    }
    assert_eq!(
        (type_count, assoc_count, prop_count, gen_count),
        (2, 1, 1, 1)
    );

    // Check association edge wiring
    let DiagramView::Association(av) = diagram
        .views
        .iter()
        .find(|v| matches!(v, DiagramView::Association(_)))
        .expect("AssociationView")
    else {
        unreachable!()
    };
    assert_eq!(av.source.as_ref().map(smol_str::SmolStr::as_str), Some("A"));
    assert_eq!(av.target.as_ref().map(smol_str::SmolStr::as_str), Some("B"));
    assert_eq!(av.points.len(), 2);
    assert_eq!(av.label.as_ref().map(|s| s.value.as_str()), Some("A to B"));

    // Check PropertyView's Class.prop reference resolves
    let DiagramView::Property(pv) = diagram
        .views
        .iter()
        .find(|v| matches!(v, DiagramView::Property(_)))
        .expect("PropertyView")
    else {
        unreachable!()
    };
    assert_eq!(pv.property.class.full_path(), "model::test::A");
    assert_eq!(pv.property.property.as_str(), "prop");
}
