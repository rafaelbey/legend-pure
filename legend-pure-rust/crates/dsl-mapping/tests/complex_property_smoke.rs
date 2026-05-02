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

//! Complex (class-typed) property transforms in a Pure body.
//! Mirrors Java's `TestModelMapping`:
//!
//! - `testComplexTypePropertyMapping` — happy path with `firms : Firm[*]`
//!   target property and matching source-class shape.
//! - `testComplexTypePropertyMappingError` — transform's class type
//!   doesn't match property's class type. Java diagnostic shape:
//!   "Type Error: '_Firm' not a subtype of 'Firm'".
//!
//! Where the existing `type_check_smoke.rs` covers primitive-typed
//! properties (`String`, `Integer`), this file fills the gap for
//! class-typed properties — i.e. that the existing Stage-3.5
//! subtype-check rule fires on Named-vs-Named mismatches as well as
//! primitive ones.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
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

fn compile(sources: Vec<SourceFile>) -> Vec<CompilationError> {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

#[test]
fn class_typed_property_with_matching_class_validates_clean() {
    // Mirrors Java testComplexTypePropertyMapping: target property
    // `firms : Firm[*]` and source has `firms : Firm[*]` of the
    // same Class type. Both class mappings are present.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::Person { firms : my::test::Firm[*]; }

        Class my::test::FirmSrc { legalName : String[1]; }
        Class my::test::PersonSrc { firms : my::test::Firm[*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSrc
            legalName : $src.legalName
          }

          my::test::Person : Pure
          {
            ~src my::test::PersonSrc
            firms : $src.firms
          }
        )
    "};
    let file = parse("complex_clean.pure", source);
    let errors = compile(vec![file]);
    let typed_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("Transform")
                || e.message.contains("not a subtype")
                || e.message.contains("Multiplicity")
        })
        .collect();
    assert!(
        typed_errors.is_empty(),
        "expected no type-mismatch errors for matching class-typed property; got: {:#?}",
        typed_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn class_typed_property_with_wrong_class_errors() {
    // Mirrors Java testComplexTypePropertyMappingError: target
    // property `firms : Firm[*]` but the source-class-shape produces
    // `_Firm[*]` (a different Class). The existing Stage-3.5
    // subtype check should fire on Named-vs-Named mismatches just
    // as it does on primitives — this test pins that.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::OtherFirm { legalName : String[1]; }
        Class my::test::Person { firms : my::test::Firm[*]; }

        Class my::test::PersonSrc { otherFirms : my::test::OtherFirm[*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Person : Pure
          {
            ~src my::test::PersonSrc
            firms : $src.otherFirms
          }
        )
    "};
    let file = parse("complex_wrong.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            // Match the Stage-3.5 diagnostic shape for type
            // mismatches: "Transform for property 'X.Y' has type ...
            // not a subtype of the property's declared type ..."
            e.message.contains("Transform")
                && e.message.contains("firms")
                && e.message.contains("OtherFirm")
                && e.message.contains("not a subtype")
        }),
        "expected Class-typed type-mismatch error mentioning OtherFirm + 'not a subtype'; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn class_typed_property_with_subclass_validates_clean() {
    // Subtype-aware: if the transform's class is a subtype of the
    // property's declared class, it should validate clean (i.e. the
    // is_subtype check considers inheritance, not just identity).
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }
        Class my::test::SpecialFirm extends my::test::Firm { extraField : String[1]; }
        Class my::test::Person { firms : my::test::Firm[*]; }

        Class my::test::PersonSrc { specialFirms : my::test::SpecialFirm[*]; }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Person : Pure
          {
            ~src my::test::PersonSrc
            firms : $src.specialFirms
          }
        )
    "};
    let file = parse("complex_subclass.pure", source);
    let errors = compile(vec![file]);
    let subtype_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("not a subtype"))
        .collect();
    assert!(
        subtype_errors.is_empty(),
        "subclass-of-Firm should be a valid Firm transform; got: {:#?}",
        subtype_errors
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>()
    );
}
