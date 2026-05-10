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

//! Stage-3.5 type-check coverage:
//!
//! These rules require the lower-and-infer wrapper exposed in
//! `legend_pure_parser_pure::extension::lower_and_infer_expression`.
//! They were deferred from the original Stage-3 commit and now fire
//! cleanly under the Mapping validator.
//!
//! - **Negatives**:
//!   - `~filter` returning `Integer[1]` errors with a "Boolean[1]"
//!     diagnostic.
//!   - Transform whose return type is not a subtype of the property's
//!     declared type errors with a "not a subtype" diagnostic.
//!   - Transform with collection multiplicity targeting a `[1]`
//!     property errors with a multiplicity-incompatibility diagnostic.
//! - **Positive carry-over**: a Boolean filter and matching transforms
//!   continue to validate clean (overlaps with the Stage-3 positive
//!   test, but kept here so this file's negatives can be read against
//!   a baseline).

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

#[test]
fn boolean_filter_and_matching_transforms_validate_clean() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
          employees : Integer[1];
        }

        Class my::test::FirmSource
        {
          name : String[1];
          headcount : Integer[1];
          industry : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            ~filter $src.industry == 'Banking'
            legalName : $src.name,
            employees : $src.headcount
          }
        )
    "};
    let file = parse("clean.pure", source);
    let (errors, _ext, model) = compile(vec![file]);
    let mapping_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("Transform")
                || e.message.contains("~filter")
                || e.message.contains("Mapping")
        })
        .collect();
    assert!(
        mapping_errors.is_empty(),
        "expected no type-check errors; got: {:#?}",
        mapping_errors
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>()
    );
    assert!(
        MappingExtension::mappings_from_model(&model)
            .iter()
            .any(|(fqn, _)| fqn.as_str() == "my::test::FirmMapping"),
        "extension did not register the mapping"
    );
}

#[test]
fn filter_returning_integer_errors_with_boolean_message() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        Class my::test::FirmSource
        {
          name : String[1];
          headcount : Integer[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            ~filter $src.headcount
            legalName : $src.name
          }
        )
    "};
    let file = parse("filter_int.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("~filter")
            && e.message.contains("Boolean[1]")
            && e.message.contains("Integer")),
        "expected ~filter Boolean[1] error mentioning Integer; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn transform_with_wrong_type_errors_with_subtype_message() {
    // Property is String[1] but transform returns Integer[1].
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        Class my::test::FirmSource
        {
          headcount : Integer[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.headcount
          }
        )
    "};
    let file = parse("transform_wrong.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("Transform")
            && e.message.contains("legalName")
            && e.message.contains("not a subtype")
            && e.message.contains("String")),
        "expected transform-not-subtype error mentioning String; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn transform_with_wrong_multiplicity_errors() {
    // Property is String[1] but transform returns String[*] (a list).
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        Class my::test::FirmSource
        {
          aliases : String[*];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.aliases
          }
        )
    "};
    let file = parse("transform_mult.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("Transform")
            && e.message.contains("legalName")
            && e.message.contains("multiplicity")),
        "expected transform multiplicity-incompat error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
