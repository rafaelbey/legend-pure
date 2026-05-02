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

//! Inline `EnumerationMapping <name>` transformer reference inside
//! a Pure property mapping. Mirrors Java's
//! `TestModelMapping.testM2MMappingWithEnumerationMapping` (positive)
//! and `testM2MMappingWithInvalidEnumerationMapping` (negative).
//!
//! Syntax: `propName : EnumerationMapping NameOfMapping : transformExpr`.
//! The local name (e.g. `StateMapping`) refers to the optional
//! `mapping_name` slot of an `EnumerationMapping` declared elsewhere
//! in this Mapping or any transitively included mapping.
//!
//! Validator rules:
//! 1. The transformer name must resolve to a known sibling
//!    EnumerationMapping (via own class_mappings + transitive
//!    include walk).
//! 2. The property's declared type must be that EnumerationMapping's
//!    target enumeration. Java's diagnostic shape:
//!    "Property : [state] is of type : [my::State] but enumeration
//!    mapping : [OptionMapping] is defined on enumeration :
//!    [my::Option]."

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef, PureClassMappingBody};
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

fn pure_body(m: &MappingDef, idx: usize) -> &PureClassMappingBody {
    let cm = m
        .class_mappings
        .get(idx)
        .expect("expected at least one class mapping");
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("class mapping {idx} is not a Pure body");
    };
    body
}

fn compile(sources: Vec<SourceFile>) -> Vec<CompilationError> {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

// ----- Parser coverage --------------------------------------------------

#[test]
fn parser_records_inline_transformer_name() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::TargetProduct : Pure
          {
            ~src pkg::SourceProduct
            id : $src.id,
            state : EnumerationMapping StateMapping : $src.state
          }
        )
    "};
    let file = parse("inline_xfm.pure", source);
    let body = pure_body(first_mapping(&file), 0);
    // First entry: `id` — no transformer.
    assert_eq!(body.property_mappings[0].property_name.as_str(), "id");
    assert!(body.property_mappings[0].transformer.is_none());
    // Second entry: `state` — transformer `StateMapping`.
    assert_eq!(body.property_mappings[1].property_name.as_str(), "state");
    assert_eq!(
        body.property_mappings[1].transformer.as_deref(),
        Some("StateMapping"),
    );
}

#[test]
fn parser_does_not_consume_bare_enumeration_mapping_identifier() {
    // Two-token-lookahead resolves the ambiguity vs. a transform
    // expression that happens to start with the bare identifier
    // `EnumerationMapping`. Without this check, the parser would
    // misread `prop : EnumerationMapping->size()` as a transformer
    // prefix and choke. Using `EnumerationMapping` followed by `.`
    // (not Identifier+Colon) exercises the negative side of the
    // lookahead.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::TargetProduct : Pure
          {
            ~src pkg::SourceProduct
            id : $src.id
          }
        )
    "};
    let file = parse("no_xfm.pure", source);
    let body = pure_body(first_mapping(&file), 0);
    assert!(body.property_mappings[0].transformer.is_none());
}

#[test]
fn round_trip_preserves_inline_transformer() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::TargetProduct : Pure
          {
            ~src pkg::SourceProduct
            id : $src.id,
            state : EnumerationMapping StateMapping : $src.state
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);
    // Token-level survival: composer must emit the prefix verbatim.
    assert!(
        composed.contains("EnumerationMapping StateMapping"),
        "composer dropped the inline transformer prefix; composed:\n{composed}"
    );
    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);
    let b1 = pure_body(m1, 0);
    let b2 = pure_body(m2, 0);
    assert_eq!(
        b1.property_mappings[1].transformer,
        b2.property_mappings[1].transformer
    );
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn matching_enumeration_validates_clean() {
    // Mirrors Java testM2MMappingWithEnumerationMapping. The
    // property `state : my::State[1]` is annotated with
    // `EnumerationMapping StateMapping`, and the StateMapping
    // declares its target as `my::State` — so types align.
    let source = indoc! {r"
        ###Pure
        Class my::SourceProduct { id : Integer[1]; state : String[1]; }

        Class my::TargetProduct { id : Integer[1]; state : my::State[1]; }

        Enum my::State { ACTIVE, INACTIVE }

        ###Mapping
        Mapping my::modelMapping
        (
          my::TargetProduct : Pure
          {
            ~src my::SourceProduct
            id : $src.id,
            state : EnumerationMapping StateMapping : $src.state
          }

          my::State : EnumerationMapping StateMapping
          {
            ACTIVE   : 'ACTIVE',
            INACTIVE : 'INACTIVE'
          }
        )
    "};
    let file = parse("inline_clean.pure", source);
    let errors = compile(vec![file]);
    let xfm_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("EnumerationMapping") || e.message.contains("enumeration mapping")
        })
        .collect();
    assert!(
        xfm_errors.is_empty(),
        "expected no inline-transformer errors; got: {:#?}",
        xfm_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_transformer_name_errors() {
    // Reference an EnumerationMapping name that doesn't exist in
    // this mapping or any included mapping.
    let source = indoc! {r"
        ###Pure
        Class my::SourceProduct { state : String[1]; }
        Class my::TargetProduct { state : my::State[1]; }
        Enum my::State { ACTIVE, INACTIVE }

        ###Mapping
        Mapping my::M
        (
          my::TargetProduct : Pure
          {
            ~src my::SourceProduct
            state : EnumerationMapping NonExistentMapping : $src.state
          }
        )
    "};
    let file = parse("inline_unknown.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message
                .contains("EnumerationMapping 'NonExistentMapping'")
                && e.message.contains("not declared")
        }),
        "expected unresolved-transformer error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn enumeration_target_mismatch_errors() {
    // Mirrors Java testM2MMappingWithInvalidEnumerationMapping:
    // property is `state : my::State[1]` but the named transformer
    // (`OptionMapping`) is defined on `my::Option`. The diagnostic
    // shape mirrors the Java message verbatim.
    let source = indoc! {r"
        ###Pure
        Class my::SourceProduct { state : String[1]; }
        Class my::TargetProduct { state : my::State[1]; }
        Enum my::State { ACTIVE, INACTIVE }
        Enum my::Option { CALL, PUT }

        ###Mapping
        Mapping my::modelMapping
        (
          my::TargetProduct : Pure
          {
            ~src my::SourceProduct
            state : EnumerationMapping OptionMapping : $src.state
          }

          my::Option : EnumerationMapping OptionMapping
          {
            CALL : 'ACTIVE',
            PUT  : 'INACTIVE'
          }
        )
    "};
    let file = parse("inline_mismatch.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            // Match the Java diagnostic shape: property name,
            // property's enum type, transformer name, transformer's
            // target enum type — all in one message.
            e.message.contains("Property : [state]")
                && e.message.contains("[my::State]")
                && e.message.contains("[OptionMapping]")
                && e.message.contains("[my::Option]")
        }),
        "expected enum-mismatch error in Java-parity shape; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn transformer_resolves_via_included_mapping() {
    // The visibility walk for transformer-name lookup uses the
    // same transitive-include logic as visible_class_mapping_ids.
    // Declare the EnumerationMapping in `enumMap`, reference it
    // from a property mapping in `modelMap` which `include`s
    // `enumMap`. Should validate clean.
    let source = indoc! {r"
        ###Pure
        Class my::SourceProduct { state : String[1]; }
        Class my::TargetProduct { state : my::State[1]; }
        Enum my::State { ACTIVE, INACTIVE }

        ###Mapping
        Mapping my::enumMap
        (
          my::State : EnumerationMapping StateMapping
          {
            ACTIVE   : 'ACTIVE',
            INACTIVE : 'INACTIVE'
          }
        )

        Mapping my::modelMap
        (
          include my::enumMap

          my::TargetProduct : Pure
          {
            ~src my::SourceProduct
            state : EnumerationMapping StateMapping : $src.state
          }
        )
    "};
    let file = parse("inline_via_include.pure", source);
    let errors = compile(vec![file]);
    let xfm_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("EnumerationMapping") || e.message.contains("enumeration mapping")
        })
        .collect();
    assert!(
        xfm_errors.is_empty(),
        "transformer resolved via include should validate clean; got: {:#?}",
        xfm_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
