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

//! Stage-5 OperationSetImplementation coverage:
//!
//! Parsing:
//!   - parse_with_sections accepts `parserName == "Operation"` and
//!     constructs a `ClassMappingBody::Operation(...)` tree carrying
//!     the function path + parenthesized comma-separated parameter
//!     IDs (including the empty form).
//!   - Round-trip composer reproduces the parsed tree.
//!   - The unsupported merge form (`[ids], { lambda }`) is rejected
//!     with a pointed error rather than silently mis-parsed.
//!
//! Validation (positives + negatives):
//!   - Positive: a union-style operation referencing two sibling
//!     pure-instance set implementations validates clean.
//!   - Negative: operation function FQN does not resolve.
//!   - Negative: parameter ID does not match any sibling
//!     class-mapping ID.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef, OperationClassMappingBody};
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
        vec![Box::new(MappingSectionParser)],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn try_parse(name: &str, source: &str) -> Result<SourceFile, Vec<String>> {
    legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser)],
    )
    .map_err(|p| p.errors.iter().map(ToString::to_string).collect())
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

fn operation_body(m: &MappingDef, idx: usize) -> &OperationClassMappingBody {
    let cm = m
        .class_mappings
        .get(idx)
        .expect("expected at least one class mapping");
    let ClassMappingBody::Operation(body) = &cm.body else {
        panic!("class mapping {idx} is not an Operation body");
    };
    body
}

fn compile(sources: Vec<SourceFile>) -> (Vec<CompilationError>, MappingExtension) {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    let errors = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };
    (errors, extension)
}

// ----- Parser coverage --------------------------------------------------

#[test]
fn parses_operation_with_two_parameters() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Person[op] : Operation
          {
            pkg::ops::union_OperationSetImplementation_1__SetImplementation_MANY_(rel1, rel2)
          }
        )
    "};
    let file = parse("op_two.pure", source);
    let m = first_mapping(&file);
    let body = operation_body(m, 0);
    assert_eq!(body.parameters.len(), 2);
    assert_eq!(body.parameters[0].id.as_str(), "rel1");
    assert_eq!(body.parameters[1].id.as_str(), "rel2");
    assert_eq!(
        body.operation.name.as_str(),
        "union_OperationSetImplementation_1__SetImplementation_MANY_"
    );
}

#[test]
fn parses_operation_with_no_parameters() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Person[rel1] : Operation
          {
            pkg::ops::a__SetImplementation_MANY_()
          }
        )
    "};
    let file = parse("op_zero.pure", source);
    let body = operation_body(first_mapping(&file), 0);
    assert!(body.parameters.is_empty());
}

#[test]
fn parses_operation_with_trailing_semicolon() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Person[rel1] : Operation
          {
            pkg::ops::a__SetImplementation_MANY_(p1);
          }
        )
    "};
    let file = parse("op_semi.pure", source);
    let body = operation_body(first_mapping(&file), 0);
    assert_eq!(body.parameters.len(), 1);
}

#[test]
fn merge_form_is_rejected_with_pointed_error() {
    // The `mergeParameters` form (`[ids], { lambda }`) is reserved
    // for a follow-up sub-stage. Make sure we surface a deliberate
    // diagnostic rather than mis-parsing it as an empty parameter
    // list (or worse, silently consuming the bracket as an id).
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Person[op] : Operation
          {
            pkg::ops::merge([p1, p2], { p1: pkg::A[1], p2: pkg::B[1] | $p1.id == $p2.id })
          }
        )
    "};
    let result = try_parse("merge.pure", source);
    let errors = result.expect_err("expected the merge form to be rejected");
    assert!(
        errors.iter().any(|e| e.contains("mergeParameters")),
        "expected error message to mention mergeParameters; got: {errors:?}"
    );
}

#[test]
fn round_trip_through_composer() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Person[op] : Operation
          {
            pkg::ops::union_OperationSetImplementation_1__SetImplementation_MANY_(rel1, rel2)
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);
    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);
    // The leading `*` (root) marker survives. Without this assertion
    // the composer could silently drop the marker without any
    // existing test catching it, breaking unique-root-per-class
    // routing downstream.
    assert!(m1.class_mappings[0].is_root && m2.class_mappings[0].is_root);
    // Class-mapping ID also survives.
    assert_eq!(m1.class_mappings[0].id, m2.class_mappings[0].id);
    let b1 = operation_body(m1, 0);
    let b2 = operation_body(m2, 0);
    // Operation function FQN survives.
    assert_eq!(
        b1.operation.name, b2.operation.name,
        "operation name diverged after round-trip"
    );
    assert_eq!(
        b1.operation.package.as_ref().map(ToString::to_string),
        b2.operation.package.as_ref().map(ToString::to_string),
        "operation package diverged after round-trip"
    );
    // Parameter IDs survive in order.
    assert_eq!(b1.parameters.len(), b2.parameters.len());
    for (p1, p2) in b1.parameters.iter().zip(b2.parameters.iter()) {
        assert_eq!(p1.id, p2.id);
    }
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn union_of_two_pure_instances_validates_clean() {
    // The Operation body's function path (`my::test::a`) refers to a
    // user-defined function returning `SetImplementation[*]`. The
    // mangled name `a__SetImplementation_MANY_` is what the model
    // registers after Pass 2.1 (name mangling). Each parameter ID
    // must resolve against the class-mapping ID set: `rel1` and
    // `rel2` are explicit `[id]`s on the two Pure class mappings.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person
        {
          firstName : String[1];
          lastName : String[1];
        }

        Class my::test::PersonSrcA { firstName : String[1]; }
        Class my::test::PersonSrcB { lastName : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::PersonMapping
        (
          *my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_(rel1, rel2)
          }

          my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrcA
            firstName : $src.firstName
          }

          my::test::Person[rel2] : Pure
          {
            ~src my::test::PersonSrcB
            lastName : $src.lastName
          }
        )
    "};
    let file = parse("op_clean.pure", source);
    let (errors, ext) = compile(vec![file]);
    let op_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("Operation function") || e.message.contains("Operation parameter")
        })
        .collect();
    assert!(
        op_errors.is_empty(),
        "expected no Operation-validator errors; got: {:#?}",
        op_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(ext.mappings().contains_key("my::test::PersonMapping"));
}

#[test]
fn empty_parameters_validates_clean() {
    // The Java grammar permits `()` and `OperationGraphBuilder` emits
    // an empty `parameters` list — the validator must not invent
    // parameter-resolution errors out of thin air.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_()
          }
        )
    "};
    let file = parse("op_empty_clean.pure", source);
    let (errors, _ext) = compile(vec![file]);
    let op_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("Operation function") || e.message.contains("Operation parameter")
        })
        .collect();
    assert!(
        op_errors.is_empty(),
        "expected no Operation-validator errors for empty parameters; got: {:#?}",
        op_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn simple_name_does_not_silently_match_overload() {
    // Stay faithful to Java's exact-FQN behavior: writing a bare
    // simple name (`my::test::a`) must not silently match the mangled
    // overload (`my::test::a__SetImplementation_MANY_`). A
    // simple-name fallback would let users accidentally pick a wrong
    // overload that happens to live in the same package.
    let source = indoc! {r"
        ###Pure
        Class my::test::Person { name : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::a()
          }
        )
    "};
    let file = parse("op_simple_name.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Operation function") && e.message.contains("my::test::a")),
        "expected unresolved-operation-function error for simple-name FQN; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_operation_function_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Person
        {
          firstName : String[1];
        }

        Class my::test::PersonSrc { firstName : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::doesNotExist__SetImplementation_MANY_(rel1)
          }

          my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrc
            firstName : $src.firstName
          }
        )
    "};
    let file = parse("op_unresolved_fn.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("Operation function") && e.message.contains("doesNotExist")
        }),
        "expected unresolved-operation-function error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_parameter_id_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Person
        {
          firstName : String[1];
        }

        Class my::test::PersonSrc { firstName : String[1]; }

        function my::test::a() : meta::pure::mapping::SetImplementation[*]
        {
          []
        }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Person[op] : Operation
          {
            my::test::a__SetImplementation_MANY_(notARealId)
          }

          my::test::Person[rel1] : Pure
          {
            ~src my::test::PersonSrc
            firstName : $src.firstName
          }
        )
    "};
    let file = parse("op_unknown_param.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("Operation parameter") && e.message.contains("notARealId")
        }),
        "expected Operation-parameter unresolved error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
