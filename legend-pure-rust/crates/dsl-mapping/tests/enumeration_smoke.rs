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

//! Stage-4 EnumerationMapping coverage:
//!
//! Parsing:
//!   - parse_with_sections accepts `parserName == "EnumerationMapping"`
//!     and constructs a `ClassMappingBody::Enumeration(...)` tree
//!     covering the three source-value forms (string literal, integer
//!     literal, qualified enum reference) plus the multi-value
//!     bracketed form.
//!   - Round-trip composer reproduces the parsed tree.
//!
//! Validation (positives + negatives):
//!   - Positive: a string-sourced enum mapping over a real enumeration
//!     validates clean, and the FQN registers in `MappingExtension`.
//!   - Negative: target FQN resolves to a Class, not an Enumeration.
//!   - Negative: target enum value name doesn't exist on the
//!     enumeration.
//!   - Negative: source values mix string and integer kinds.
//!   - Negative: source enum-ref points at a non-existent enum value.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{
    ClassMappingBody, EnumSourceValue, EnumerationClassMappingBody, MappingDef,
};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::compose::compose_mapping_section;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn first_mapping(file: &SourceFile) -> &MappingDef {
    for section in &file.sections {
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem
                && let Some(m) = boxed.as_any().downcast_ref::<MappingDef>()
            {
                return m;
            }
        }
    }
    panic!("no MappingDef in parsed source");
}

fn enumeration_body(m: &MappingDef) -> &EnumerationClassMappingBody {
    let cm = m
        .class_mappings
        .first()
        .expect("expected at least one class mapping");
    let ClassMappingBody::Enumeration(body) = &cm.body else {
        panic!("expected Enumeration body");
    };
    body
}

fn compile(
    sources: Vec<SourceFile>,
) -> (
    Vec<CompilationError>,
    MappingExtension,
    legend_pure_parser_pure::model::PureModel,
) {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    let (errors, model) = match result {
        Ok(model) => (Vec::new(), model),
        Err(p) => (p.errors, p.model),
    };
    (errors, extension, model)
}

// ----- Parser coverage --------------------------------------------------

#[test]
fn parses_string_int_and_enum_ref_source_values() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Target : EnumerationMapping
          {
            VAL1 : 'a',
            VAL2 : 1,
            VAL3 : src::Other.X,
            VAL4 : ['b', 'c']
          }
        )
    "};
    let file = parse("enum_parse.pure", source);
    let m = first_mapping(&file);
    let body = enumeration_body(m);
    assert_eq!(body.value_mappings.len(), 4);
    assert!(matches!(
        body.value_mappings[0].source_values[0],
        EnumSourceValue::String { .. }
    ));
    assert!(matches!(
        body.value_mappings[1].source_values[0],
        EnumSourceValue::Integer { value: 1, .. }
    ));
    assert!(matches!(
        body.value_mappings[2].source_values[0],
        EnumSourceValue::EnumRef { .. }
    ));
    assert_eq!(body.value_mappings[3].source_values.len(), 2);
}

#[test]
fn round_trip_through_composer() {
    // Cover all three source-value kinds + the multi-value bracketed
    // form. Each value's content is asserted, not just the count, so
    // a composer that emits `'a'` for an integer or `pkg::A.X` for
    // `pkg::B.Y` would fail here rather than silently survive.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Target : EnumerationMapping
          {
            VAL1 : 'a',
            VAL2 : 42,
            VAL3 : src::Other.X,
            VAL4 : ['b', 'c']
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);
    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);
    let b1 = enumeration_body(m1);
    let b2 = enumeration_body(m2);
    assert_eq!(b1.value_mappings.len(), b2.value_mappings.len());
    for (vm1, vm2) in b1.value_mappings.iter().zip(b2.value_mappings.iter()) {
        assert_eq!(vm1.enum_value_name, vm2.enum_value_name);
        assert_eq!(vm1.source_values.len(), vm2.source_values.len());
        for (sv1, sv2) in vm1.source_values.iter().zip(vm2.source_values.iter()) {
            assert_eq!(
                source_value_signature(sv1),
                source_value_signature(sv2),
                "source value diverged after round-trip for '{}'\n  before: {sv1:?}\n  after:  {sv2:?}",
                vm1.enum_value_name
            );
        }
    }
}

/// A position-independent signature of a source value — kind + the
/// content that defines it, with no spans. Two source values with the
/// same signature are AST-identical modulo source positions.
fn source_value_signature(sv: &EnumSourceValue) -> String {
    match sv {
        EnumSourceValue::String { value, .. } => format!("String({value})"),
        EnumSourceValue::Integer { value, .. } => format!("Integer({value})"),
        EnumSourceValue::EnumRef {
            enumeration,
            value_name,
            ..
        } => {
            let pkg = enumeration
                .package
                .as_ref()
                .map(|p| format!("{p}::"))
                .unwrap_or_default();
            format!("EnumRef({pkg}{}.{value_name})", enumeration.name)
        }
    }
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn string_sourced_enum_mapping_validates_clean() {
    let source = indoc! {r"
        ###Pure
        Enum my::test::Color
        {
          RED, GREEN, BLUE
        }

        ###Mapping
        Mapping my::test::ColorMapping
        (
          my::test::Color : EnumerationMapping
          {
            RED : 'r',
            GREEN : 'g',
            BLUE : 'b'
          }
        )
    "};
    let file = parse("clean_enum.pure", source);
    let (errors, _ext, model) = compile(vec![file]);
    assert!(
        errors.is_empty(),
        "expected a clean compile; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        MappingExtension::mappings_from_model(&model)
            .iter()
            .any(|(fqn, _)| fqn.as_str() == "my::test::ColorMapping")
    );
}

#[test]
fn target_resolving_to_class_not_enum_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::NotAnEnum
        {
          n : String[1];
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::NotAnEnum : EnumerationMapping
          {
            FOO : 'x'
          }
        )
    "};
    let file = parse("target_class.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("EnumerationMapping target")
                && e.message.contains("NotAnEnum")
                && e.message.contains("Enumeration")),
        "expected target-not-an-Enumeration error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_target_enum_value_errors() {
    let source = indoc! {r"
        ###Pure
        Enum my::test::Color
        {
          RED, GREEN
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Color : EnumerationMapping
          {
            RED  : 'r',
            BLUE : 'b'
          }
        )
    "};
    let file = parse("unknown_enum_value.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Color") && e.message.contains("BLUE")),
        "expected unknown enum-value error mentioning BLUE; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn mixed_source_value_kinds_errors() {
    let source = indoc! {r"
        ###Pure
        Enum my::test::Color
        {
          RED, GREEN
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Color : EnumerationMapping
          {
            RED   : 'r',
            GREEN : 1
          }
        )
    "};
    let file = parse("mixed_kinds.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("mixes source value kinds")
                && e.message.contains("String")
                && e.message.contains("Integer")),
        "expected mixed-kinds error mentioning both String and Integer; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_source_enum_value_errors() {
    let source = indoc! {r"
        ###Pure
        Enum my::test::Color
        {
          RED, GREEN
        }

        Enum my::test::Source
        {
          A, B
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Color : EnumerationMapping
          {
            RED   : my::test::Source.A,
            GREEN : my::test::Source.NOPE
          }
        )
    "};
    let file = parse("bad_src_enum_val.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("my::test::Source") && e.message.contains("NOPE")),
        "expected unknown source-enum-value error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
