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

//! End-to-end test of the Diagram `CompilerExtension`.
//!
//! Full chain: parse a `###Diagram` section via
//! `parse_with_sections` + `DiagramSectionParser` → compile via
//! `compile_with_extensions` registering `DiagramExtension` → assert
//! the diagram is registered and validation reports the expected
//! errors against bad references.

use indoc::indoc;
use legend_pure_dsl_diagram::compiler::DiagramExtension;
use legend_pure_dsl_diagram::parser::DiagramSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use smol_str::SmolStr;

fn parse(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "compiler_smoke.pure",
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

#[allow(clippy::needless_pass_by_value)] // tests pass Vec<SourceFile> directly for ergonomics
fn compile_with_diagram(
    sources: Vec<SourceFile>,
    extension: &DiagramExtension,
) -> (Vec<String>, legend_pure_parser_pure::model::PureModel) {
    let exts: [&dyn CompilerExtension; 1] = [extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(model) => (Vec::new(), model),
        Err(p) => (
            p.errors.iter().map(|e| e.message.clone()).collect(),
            p.model,
        ),
    }
}

#[test]
fn extension_registers_diagram_under_fqn() {
    let source = indoc! {r"
        ###Pure
        Class model::test::A { name: String[1]; }

        ###Diagram
        Diagram model::test::TinyDiagram
        {
            TypeView A(type=model::test::A)
        }
    "};
    let file = parse(source);
    let extension = DiagramExtension::new();
    let (errors, model) = compile_with_diagram(vec![file], &extension);

    let registered = DiagramExtension::diagrams_from_model(&model);
    assert!(
        registered
            .iter()
            .any(|(fqn, _)| fqn.as_str() == "model::test::TinyDiagram"),
        "expected diagram FQN registered; got keys: {:?}",
        registered.iter().map(|(fqn, _)| fqn).collect::<Vec<_>>()
    );

    // No validation errors: the TypeView's `model::test::A` was
    // declared in the same file's Pure section so it resolves.
    let unrelated_errors: Vec<&String> = errors
        .iter()
        .filter(|e| {
            // Drop pre-existing M3 errors unrelated to Diagram (none
            // expected here, but be defensive against bootstrap noise)
            !e.contains("Cannot resolve element")
                || e.contains("Diagram")
                || e.contains("TypeView")
                || e.contains("AssociationView")
                || e.contains("PropertyView")
                || e.contains("GeneralizationView")
        })
        .filter(|e| {
            e.contains("TypeView")
                || e.contains("AssociationView")
                || e.contains("PropertyView")
                || e.contains("GeneralizationView")
                || e.contains("Diagram")
        })
        .collect();
    assert!(
        unrelated_errors.is_empty(),
        "unexpected diagram-related errors: {unrelated_errors:?}"
    );
}

#[test]
fn validate_reports_unresolved_class_in_typeview() {
    let source = indoc! {r"
        ###Diagram
        Diagram model::Bad
        {
            TypeView Missing(type=model::test::DoesNotExist)
        }
    "};
    let file = parse(source);
    let extension = DiagramExtension::new();
    let (errors, _model) = compile_with_diagram(vec![file], &extension);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("Missing") && e.contains("DoesNotExist")),
        "expected unresolved-class error mentioning the TypeView id and the missing FQN; got {errors:?}",
    );
}

#[test]
fn validate_reports_dangling_edge_endpoints() {
    let source = indoc! {r"
        ###Pure
        Class model::test::A { name: String[1]; }

        ###Diagram
        Diagram model::test::EdgyDiagram
        {
            TypeView A(type=model::test::A)
            GeneralizationView G(source=A, target=NotDeclared)
        }
    "};
    let file = parse(source);
    let extension = DiagramExtension::new();
    let (errors, _model) = compile_with_diagram(vec![file], &extension);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("GeneralizationView") && e.contains("NotDeclared")),
        "expected dangling-endpoint error for `target=NotDeclared`; got {errors:?}",
    );
}

#[test]
fn duplicate_diagram_fqn_reports_error() {
    let source = indoc! {r"
        ###Diagram
        Diagram model::test::DupDiagram
        {
        }
        Diagram model::test::DupDiagram
        {
        }
    "};
    let file = parse(source);
    let extension = DiagramExtension::new();
    let (errors, model) = compile_with_diagram(vec![file], &extension);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("Duplicate diagram") && e.contains("DupDiagram")),
        "expected duplicate-diagram error; got {errors:?}",
    );
    // First registration is preserved.
    let map = DiagramExtension::diagrams_from_model(&model);
    assert_eq!(
        map.iter()
            .filter(|(fqn, _)| fqn.as_str().contains("DupDiagram"))
            .count(),
        1,
        "expected exactly one DupDiagram entry"
    );
    let _ = SmolStr::new("model::test::DupDiagram"); // silence warning if smol_str unused
}
