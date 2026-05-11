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

//! End-to-end test mirroring the upstream
//! `TestDiagramCompilation.java` fixture: a Pure section declaring
//! the underlying classes, then a `###Diagram` section with all
//! four view kinds (`TypeView` × 2, `AssociationView`, `PropertyView`,
//! `GeneralizationView`) referencing those classes.
//!
//! Validates the full Diagram-DSL stack:
//!
//! - Section parser routes `###Diagram` → `DiagramSectionParser`
//! - AST `Element::DSLElement(Box<DiagramDef>)` flows through
//! - `DiagramExtension::declare` registers the diagram by FQN
//! - `DiagramExtension::validate` resolves every view reference
//!   (type, association, property, source/target endpoints) without
//!   producing diagram-related errors

use indoc::indoc;
use legend_pure_dsl_diagram::ast::DiagramDef;
use legend_pure_dsl_diagram::compiler::DiagramExtension;
use legend_pure_dsl_diagram::compose::compose_diagram_section;
use legend_pure_dsl_diagram::parser::DiagramSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_pure::extension::CompilerExtension;

const FIXTURE: &str = indoc! {r"
    ###Pure
    Class model::test::A
    {
        prop : model::test::B[0..1];
    }

    Class model::test::B
    {
    }

    Association model::test::A2B
    {
        a : model::test::A[1];
        b : model::test::B[*];
    }

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

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "end_to_end.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(DiagramSectionParser)],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

fn compile_for_test(
    file: SourceFile,
    ext: &DiagramExtension,
) -> (Vec<String>, legend_pure_parser_pure::model::PureModel) {
    let exts: [&dyn CompilerExtension; 1] = [ext];
    match legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts) {
        Ok(model) => (Vec::new(), model),
        Err(p) => (
            p.errors.iter().map(|e| e.message.clone()).collect(),
            p.model,
        ),
    }
}

#[test]
fn full_fixture_compiles_without_diagram_errors_and_registers_diagram() {
    let file = parse(FIXTURE);
    let extension = DiagramExtension::new();
    let (errors, model) = compile_for_test(file, &extension);

    // Filter to diagram-layer errors. Anything from M3 (e.g. an
    // import the platform isn't loaded for) is not the contract
    // this test gates on.
    let diagram_errors: Vec<&String> = errors
        .iter()
        .filter(|e| {
            e.contains("Diagram")
                || e.contains("TypeView")
                || e.contains("AssociationView")
                || e.contains("PropertyView")
                || e.contains("GeneralizationView")
        })
        .collect();
    assert!(
        diagram_errors.is_empty(),
        "expected zero diagram errors; got {diagram_errors:?}",
    );

    let registered = DiagramExtension::diagrams_from_model(&model);
    let diagram = registered
        .iter()
        .find(|(fqn, _)| fqn.as_str() == "model::test::TestDiagram")
        .map_or_else(
            || {
                panic!(
                    "expected `model::test::TestDiagram` in registry; keys: {:?}",
                    registered.iter().map(|(fqn, _)| fqn).collect::<Vec<_>>()
                )
            },
            |(_, snap)| snap,
        );

    assert_eq!(diagram.views.len(), 5);
    let geom = diagram.geometry.as_ref().expect("geometry");
    assert!((geom.width - 5000.3).abs() < 1e-6);
    assert!((geom.height - 2700.6).abs() < 1e-6);

    // Tally view kinds: 2 TypeView, 1 AssociationView, 1 PropertyView,
    // 1 GeneralizationView. Snapshot uses the textual `view_kind` tag.
    let mut type_count = 0;
    let mut assoc_count = 0;
    let mut prop_count = 0;
    let mut gen_count = 0;
    for v in &diagram.views {
        match v.view_kind.as_str() {
            "TypeView" => type_count += 1,
            "AssociationView" => assoc_count += 1,
            "PropertyView" => prop_count += 1,
            "GeneralizationView" => gen_count += 1,
            _ => {}
        }
    }
    assert_eq!(
        (type_count, assoc_count, prop_count, gen_count),
        (2, 1, 1, 1)
    );
}

#[test]
fn full_fixture_round_trips_through_composer() {
    use legend_pure_dsl_diagram::compose::compose_diagram;

    // Parse → compose → parse a second time → assert structural
    // equality between the two parsed `DiagramDef`s. Anchors the
    // composer-parser bidirectional contract on the full fixture.
    let file_first = parse(FIXTURE);
    let diagram_first = extract_diagram(&file_first);

    let composed = compose_diagram_section(&[diagram_first]);
    let file_second = parse(&composed);
    let diagram_second = extract_diagram(&file_second);

    // Compare via composed-text equality (ignores source spans).
    assert_eq!(
        compose_diagram(diagram_first),
        compose_diagram(diagram_second),
        "round-trip through composer broke structural equality.\n--- first ---\n{}\n--- second ---\n{}",
        compose_diagram(diagram_first),
        compose_diagram(diagram_second),
    );
}

fn extract_diagram(file: &SourceFile) -> &DiagramDef {
    let section = file
        .sections
        .iter()
        .find(|s| s.kind.as_str() == "Diagram")
        .expect("Diagram section");
    let dsl = match &section.elements[0] {
        Element::DSLElement(d) => d,
        other => panic!("expected DSLElement, got {other:?}"),
    };
    dsl.as_any()
        .downcast_ref::<DiagramDef>()
        .expect("DiagramDef")
}
