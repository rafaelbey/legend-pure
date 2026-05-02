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

//! Inline local-property declaration in a Pure body:
//! `+name : Type[mult] : transformExpr`. Mirrors Java's
//! `TestModelMapping.testLocalPropertyWithInvalidType` and
//! `testLocalPropertyTypeError`. The user is *declaring* a new
//! property visible only from this mapping, alongside the
//! transform that fills it.
//!
//! Validator rules:
//! 1. The declared type must resolve. Java's diagnostic shape:
//!    "Strixng has not been defined!" (the misspelt type FQN
//!    appears verbatim).
//! 2. The transform's return type must match the declared
//!    type/multiplicity.

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
fn parser_records_local_property_declaration() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            legalName : $src.allNames->at(0),
            +otherNames : String[*] : $src.allNames->tail()
          }
        )
    "};
    let file = parse("local_decl.pure", source);
    let body = pure_body(first_mapping(&file), 0);
    // First entry: regular property — no local declaration.
    assert!(body.property_mappings[0].local_property.is_none());
    // Second entry: `+otherNames : String[*]` — local-property
    // declaration recorded with type `String` and multiplicity `[*]`.
    let local = body.property_mappings[1]
        .local_property
        .as_ref()
        .expect("expected local_property on +otherNames entry");
    assert_eq!(local.type_ref.name.as_str(), "String");
}

#[test]
fn round_trip_preserves_local_property() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            legalName : $src.allNames->at(0),
            +otherNames : String[*] : $src.allNames->tail()
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);
    // Token-level survival: composer must emit `+`, the type, and
    // the multiplicity bracket verbatim.
    assert!(
        composed.contains("+otherNames : String[*]"),
        "composer dropped the local-property prefix; composed:\n{composed}"
    );
    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);
    let b1 = pure_body(m1, 0);
    let b2 = pure_body(m2, 0);
    assert_eq!(
        b1.property_mappings[1].local_property.is_some(),
        b2.property_mappings[1].local_property.is_some(),
    );
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn local_property_with_undefined_type_errors() {
    // Mirrors Java testLocalPropertyWithInvalidType: misspelt
    // type `Strixng` should surface as a "has not been defined"
    // diagnostic rather than e.g. a property-not-found cascade.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::FirmSource { name : String[1]; allNames : String[1..*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.name,
            +otherNames : Strixng[*] : $src.allNames
          }
        )
    "};
    let file = parse("local_undef_type.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Strixng") && e.message.contains("not been defined")),
        "expected 'Strixng has not been defined!' diagnostic; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn local_property_with_type_mismatch_errors() {
    // Mirrors Java testLocalPropertyTypeError: declared type
    // doesn't match the transform's return type. The Java test
    // uses `->size()` to produce Integer-vs-String mismatch; this
    // Rust port uses a direct property reference of the wrong
    // type (Integer-typed property assigned into a String-declared
    // local) so the test doesn't depend on `->size()` being
    // resolvable in the test fixture's auto-imports. Same rule
    // exercised: declared-type vs transform-return-type subtype
    // check on a local property.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::FirmSource { headcount : Integer[1]; allNames : String[1..*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.allNames,
            +otherNames : String[1] : $src.headcount
          }
        )
    "};
    let file = parse("local_type_mismatch.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("otherNames")
                && e.message.contains("not a subtype")
                && e.message.contains("String")
        }),
        "expected 'not a subtype of String' error mentioning otherNames; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn local_property_with_matching_types_validates_clean() {
    // Positive: transform return type matches declared type/mult.
    // Use direct property references to keep the test independent
    // of `->size()`/`->tail()` resolvability in this fixture.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::FirmSource { name : String[1]; allNames : String[1..*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.name,
            +otherNames : String[*] : $src.allNames
          }
        )
    "};
    let file = parse("local_clean.pure", source);
    let errors = compile(vec![file]);
    let local_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("otherNames"))
        .collect();
    assert!(
        local_errors.is_empty(),
        "expected no errors for matching-types local property; got: {:#?}",
        local_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
