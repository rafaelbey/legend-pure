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

//! Pilot pin for the `MappingDSLPopulator` runtime hydration.
//!
//! Locks the architectural contract: after building an evaluator
//! with `MappingDSLPopulator`, each Mapping element's heap row
//! exposes `classMappings` as a populated collection of child rows,
//! each with the correct M3 classifier and basic `id` / `class` /
//! `root` / `name` slots filled.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::value::Value;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "heap_population_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

/// Compile a single source, register the Mapping extension, return
/// the resulting PureModel.
fn compile(file: SourceFile) -> legend_pure_parser_pure::model::PureModel {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts)
        .unwrap_or_else(|p| p.model)
}

/// Find the heap handle for the (single, expected) Mapping
/// `Element::DSLInstance` in the model. The runtime classifies the
/// row by `metatype_of`, which falls back to `Any` when the M3
/// metamodel for `meta::pure::mapping::Mapping` isn't loaded into
/// the test model — so we look up via the element's `ElementId`,
/// not by classifier match.
fn find_mapping_handle<H: legend_pure_runtime::hooks::EvalHooks>(
    model: &legend_pure_parser_pure::model::PureModel,
    evaluator: &Evaluator<'_, H>,
) -> Option<legend_pure_runtime::heap::ObjectHandle> {
    use legend_pure_parser_pure::ids::ElementId;
    use legend_pure_parser_pure::model::Element;
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::DSLInstance(d) = element else {
                continue;
            };
            if d.dsl_name.as_str() != "Mapping" {
                continue;
            }
            let eid = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if let Some(h) = evaluator.heap().object_for_element(eid) {
                return Some(h);
            }
        }
    }
    None
}

fn classifier_of_object(value: &Value) -> Option<String> {
    let Value::Object(handle) = value else {
        return None;
    };
    Some(handle.borrow().classifier().to_string())
}

fn id_of_object(value: &Value) -> Option<String> {
    let Value::Object(handle) = value else {
        return None;
    };
    let entry = handle.borrow();
    let values = entry.get_property_values("id");
    let first = values.iter().next()?.clone();
    if let Value::String(s) = first {
        Some(s.to_string())
    } else {
        None
    }
}

#[test]
fn mapping_class_mappings_property_populates_with_classifiers() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          name : String[1];
        }

        Class my::test::FirmSrc
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure
          {
            ~src my::test::FirmSrc
            name : $src.legalName
          }
        )
    "};
    let file = parse(source);
    let model = compile(file);

    // Evaluator with the new populator wired in.
    let populator = MappingDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);

    // Walk the heap looking for the Mapping handle by classifier.
    let mapping_handle =
        find_mapping_handle(&model, &evaluator).expect("must find a Mapping heap row");

    // Read `classMappings` — must be one entry, classified as
    // PureInstanceSetImplementation, with id="firm_set".
    let class_mappings = mapping_handle.borrow().get_property_values("classMappings");
    let cms: Vec<Value> = class_mappings.iter().cloned().collect();
    assert_eq!(
        cms.len(),
        1,
        "expected exactly one populated class-mapping; got {}",
        cms.len()
    );
    assert_eq!(
        classifier_of_object(&cms[0]).as_deref(),
        Some("meta::pure::mapping::PureInstanceSetImplementation"),
        "Pure body should map to PureInstanceSetImplementation classifier"
    );
    assert_eq!(
        id_of_object(&cms[0]).as_deref(),
        Some("firm_set"),
        "explicit [firm_set] id must thread to heap row's `id` property"
    );

    // Mapping's own simple `name` slot.
    let name_values = mapping_handle.borrow().get_property_values("name");
    let name = name_values.iter().next().cloned();
    assert!(
        matches!(name, Some(Value::String(ref s)) if s.as_str() == "M"),
        "Mapping `name` slot must hold the simple-name segment; got {name:?}"
    );
}

#[test]
fn populator_handles_multiple_body_kinds() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Status
        {
          code : String[1];
        }

        Enum my::test::StatusEnum { ACTIVE, INACTIVE }

        Class my::test::FirmSrc
        {
          status : String[1];
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Status[status_set] : Pure
          {
            ~src my::test::FirmSrc
            code : $src.status
          }

          my::test::StatusEnum : EnumerationMapping StatusMapping
          {
            ACTIVE   : ['A'],
            INACTIVE : ['I']
          }
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = MappingDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);

    let mapping_handle =
        find_mapping_handle(&model, &evaluator).expect("must find a Mapping heap row");

    let cms_pvec = mapping_handle.borrow().get_property_values("classMappings");
    let cms: Vec<Value> = cms_pvec.iter().cloned().collect();
    assert_eq!(cms.len(), 2, "expected 2 class-mappings");

    let mut classifiers: Vec<String> = cms.iter().filter_map(classifier_of_object).collect();
    classifiers.sort();
    assert_eq!(
        classifiers,
        vec![
            "meta::pure::mapping::EnumerationMapping".to_string(),
            "meta::pure::mapping::PureInstanceSetImplementation".to_string(),
        ],
        "Pure body and Enumeration body must each map to the right M3 classifier"
    );
}

#[test]
fn populator_no_op_when_no_dsl_instances() {
    // Source without any `###Mapping` section: populator must be a
    // no-op, evaluator construction stays clean, no panics.
    let source = indoc! {r"
        ###Pure
        Class my::test::Standalone
        {
          field : String[1];
        }
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = MappingDSLPopulator;
    let _evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);
    // If we got here without panic, the no-op contract holds.
}
