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

//! Stage-3 validator coverage:
//!
//! - **Positive**: a model-to-model mapping with `~src`, multiple
//!   property mappings, and (where present) `~filter` validates with
//!   no Mapping-DSL diagnostics.
//! - **Negatives** (one per implemented rule):
//!   - unknown property name on the target class
//!   - unresolved `~src` class
//!   - unresolved class-mapping target class
//!   - unresolved `extends [superId]`
//!   - cyclic mapping include
//!   - mapping include FQN that doesn't reference any registered mapping
//!   - duplicate class-mapping ID within a single mapping
//!
//! The validator does *not* yet check filter return types or transform
//! return-type compatibility — those rules require pre-lowering of the
//! mapping body to `ValueSpec` nodes, deferred to a follow-up sub-stage
//! (Stage 3.5). The structural rules above still catch the most common
//! user errors.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
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

fn compile_with_mapping(sources: Vec<SourceFile>) -> (Vec<CompilationError>, MappingExtension) {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    let errors = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };
    (errors, extension)
}

/// Filter to errors whose message mentions Mapping-DSL concepts so we
/// can ignore unrelated M3 / extension noise that may exist in the
/// baseline.
fn mapping_dsl_errors(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| {
            e.message.contains("Mapping include")
                || e.message.contains("Class mapping")
                || e.message.contains("~src class")
                || e.message.contains("Super class-mapping")
                || e.message.contains("Cyclic mapping include")
                || matches!(&e.kind, CompilationErrorKind::UnknownProperty { .. })
        })
        .collect()
}

#[test]
fn valid_model_to_model_mapping_emits_no_validator_diagnostics() {
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
    let file = parse("valid_m2m.pure", source);
    let (errors, ext) = compile_with_mapping(vec![file]);
    let dsl_errors = mapping_dsl_errors(&errors);
    assert!(
        dsl_errors.is_empty(),
        "expected no Mapping-DSL errors; got: {:#?}",
        dsl_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    // Defence against silent skip: confirm the MappingExtension actually
    // ran and registered the user mapping. Without this, "zero Mapping
    // errors" would also pass if the extension never executed at all.
    assert!(
        ext.mappings().contains_key("my::test::FirmMapping"),
        "MappingExtension did not register the user mapping; registry keys: {:?}",
        ext.mappings().keys().collect::<Vec<_>>()
    );
}

#[test]
fn unknown_property_on_target_class_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            doesNotExist : 'literal'
          }
        )
    "};
    let file = parse("unknown_prop.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    let unknown: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                CompilationErrorKind::UnknownProperty { property_name, .. }
                    if property_name == "doesNotExist"
            )
        })
        .collect();
    assert!(
        !unknown.is_empty(),
        "expected UnknownProperty error for 'doesNotExist'; got errors: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_src_class_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::DoesNotExist
            legalName : $src.name
          }
        )
    "};
    let file = parse("unresolved_src.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("~src class") && e.message.contains("DoesNotExist")),
        "expected ~src unresolved error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_target_class_errors() {
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::DoesNotExist : Pure
          {
            anything : 'literal'
          }
        )
    "};
    let file = parse("unresolved_target.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Class mapping target")
                && e.message.contains("DoesNotExist")),
        "expected unresolved class-mapping-target error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_super_mapping_id_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm[child] extends [no_such_parent_id] : Pure
          {
            legalName : 'literal'
          }
        )
    "};
    let file = parse("unresolved_super.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Super class-mapping")
                && e.message.contains("no_such_parent_id")),
        "expected unresolved super-mapping-id error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn cyclic_mapping_include_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::A { x : String[1]; }
        Class my::test::B { y : String[1]; }

        ###Mapping
        Mapping my::test::M1
        (
          include my::test::M2

          my::test::A : Pure { x : 'a' }
        )
        Mapping my::test::M2
        (
          include my::test::M1

          my::test::B : Pure { y : 'b' }
        )
    "};
    let file = parse("cyclic.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    let cycle_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("Cyclic mapping include"))
        .collect();
    assert!(
        !cycle_errors.is_empty(),
        "expected at least one cyclic-include error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// Java parity:
// `TestCyclicMappingIncludeInMappingHierarchy.testAcyclicMappingIncludeAllowedInMappingHierarchy`.
// A 3-level acyclic chain (AMapping → BMapping → CMapping) should
// compile cleanly — no cycle, no mistaken cycle-detection trigger.
#[test]
fn acyclic_3_level_mapping_include_passes() {
    let source = indoc! {r"
        ###Pure
        Class test::A { x : String[1]; }
        Class test::B { y : String[1]; }
        Class test::C { z : String[1]; }

        ###Mapping
        Mapping test::CMapping
        (
          test::C : Pure { z : 'c' }
        )

        Mapping test::BMapping
        (
          include test::CMapping

          test::B : Pure { y : 'b' }
        )

        Mapping test::AMapping
        (
          include test::BMapping

          test::A : Pure { x : 'a' }
        )
    "};
    let file = parse("acyclic.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("Cyclic mapping include")),
        "expected acyclic 3-level chain to compile cleanly; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unresolved_mapping_include_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          include my::test::DoesNotExist

          my::test::Firm : Pure { legalName : 'literal' }
        )
    "};
    let file = parse("unresolved_include.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Mapping include") && e.message.contains("DoesNotExist")),
        "expected unresolved mapping-include error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn duplicate_class_mapping_id_within_mapping_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm[firm_id] : Pure { legalName : 'a' }
          my::test::Firm[firm_id] : Pure { legalName : 'b' }
        )
    "};
    let file = parse("dup_cm_id.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Duplicate class-mapping ID")
                && e.message.contains("firm_id")),
        "expected duplicate class-mapping ID error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn cross_mapping_super_id_resolution_succeeds() {
    // The super-mapping-ID lookup should walk includes — a child mapping
    // can reference a parent class-mapping ID declared in an included
    // mapping.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Mapping
        Mapping my::test::Base
        (
          my::test::Firm[firm_root] : Pure
          {
            legalName : 'base'
          }
        )
        Mapping my::test::Child
        (
          include my::test::Base

          my::test::Firm[firm_child] extends [firm_root] : Pure
          {
            legalName : 'child'
          }
        )
    "};
    let file = parse("cross_super.pure", source);
    let (errors, _ext) = compile_with_mapping(vec![file]);
    let super_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("Super class-mapping"))
        .collect();
    assert!(
        super_errors.is_empty(),
        "expected no super-mapping errors when extending an ID from an included mapping; \
         got: {super_errors:#?}"
    );
}
