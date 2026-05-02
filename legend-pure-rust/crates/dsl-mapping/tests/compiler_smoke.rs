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

//! Compiler-extension smoke: `MappingExtension::declare()` registers
//! parsed `MappingDef`s under their FQN; duplicates raise
//! `DuplicateElement` diagnostics.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "compiler_smoke.pure",
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

#[allow(clippy::needless_pass_by_value)]
fn compile_with_mapping(sources: Vec<SourceFile>, extension: &MappingExtension) -> Vec<String> {
    let exts: [&dyn CompilerExtension; 1] = [extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors.iter().map(|e| e.message.clone()).collect(),
    }
}

#[test]
fn extension_registers_mapping_under_fqn() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            legalName : 'literal'
          }
        )
    "};
    let file = parse(source);
    let extension = MappingExtension::new();
    let _errors = compile_with_mapping(vec![file], &extension);

    let registered = extension.mappings();
    assert!(
        registered.contains_key("pkg::M"),
        "expected mapping FQN registered; got keys: {:?}",
        registered.keys().collect::<Vec<_>>()
    );
}

#[test]
fn duplicate_mapping_fqn_reports_error_and_keeps_first() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure { legalName : 'a' }
        )
        Mapping pkg::M
        (
          pkg::Firm : Pure { legalName : 'b' }
        )
    "};
    let file = parse(source);
    let extension = MappingExtension::new();
    let errors = compile_with_mapping(vec![file], &extension);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("Duplicate mapping") && e.contains("pkg::M")),
        "expected duplicate-mapping error; got {errors:?}"
    );
    let map = extension.mappings();
    assert_eq!(
        map.keys().filter(|k| k.contains("pkg::M")).count(),
        1,
        "expected exactly one pkg::M entry preserved"
    );
}
