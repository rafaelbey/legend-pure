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

//! End-to-end tests for the Mapping DSL.
//!
//! 1. **Stage-1 sanity** (`mapping_metamodel_classes_resolve_in_loaded_platform`):
//!    embedded `platform_dsl_mapping` metamodel classes resolve as Class
//!    elements through `load_platform()`. Locks the descriptor wiring +
//!    embedding from Stage 1.
//!
//! 2. **Stage-2 user-source resolution**
//!    (`user_mapping_block_registers_against_loaded_platform`): a
//!    user-authored `###Mapping` block parses with
//!    `MappingSectionParser` + the embedded platform, the platform
//!    compile remains at zero new errors with `MappingExtension`
//!    registered, and the mapping FQN appears in
//!    `MappingExtension::mappings()`.

use indoc::indoc;
use legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS;
use legend_pure_core_platform::repo::Repo;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::Element as ModelElement;
use smol_str::SmolStr;

#[test]
fn mapping_metamodel_classes_resolve_in_loaded_platform() {
    let model = match legend_pure_core_platform::platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    for fqn in [
        ["meta", "pure", "mapping", "Mapping"].as_slice(),
        ["meta", "pure", "mapping", "SetImplementation"].as_slice(),
        ["meta", "pure", "mapping", "InstanceSetImplementation"].as_slice(),
        ["meta", "pure", "mapping", "PropertyMapping"].as_slice(),
        ["meta", "pure", "mapping", "EnumerationMapping"].as_slice(),
        ["meta", "pure", "mapping", "AssociationImplementation"].as_slice(),
    ] {
        let segments: Vec<SmolStr> = fqn.iter().map(|s| SmolStr::new(*s)).collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("mapping metamodel class missing: {fqn:?}"));
        assert!(
            matches!(model.get_element(id), ModelElement::Class(_)),
            "expected {fqn:?} to be a Class element"
        );
    }
}

#[test]
fn user_mapping_block_registers_against_loaded_platform() {
    // Baseline: count platform compile errors WITHOUT the user mapping
    // file so we can isolate any new errors introduced by Stage 2.
    let baseline_files = parse_platform_sources();
    let baseline_errors = compile_for_errors(&baseline_files);

    // The user source: a class plus a Pure-body mapping over it.
    // Lambdas inside the mapping are kept simple — Stage 2 doesn't
    // type-check them (Stage 3 will).
    let user_src = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        Class my::test::FirmSource
        {
          name : String[1];
        }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSource
            legalName : $src.name
          }
        )
    "};
    let user_ast = legend_pure_parser_parser::parse_with_sections(
        user_src,
        "<end_to_end>",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    )
    .expect("user source should parse");

    let mut all_files: Vec<SourceFile> = baseline_files.clone();
    all_files.push(user_ast);

    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    let result =
        legend_pure_parser_pure::pipeline::compile_with_extensions(&all_files, &imports, &exts);
    let errors = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };

    // Stage 2 must not introduce any new compile errors; lambdas
    // inside the mapping body live as raw expressions in the AST and
    // are intentionally not lowered.
    assert_eq!(
        errors.len(),
        baseline_errors,
        "expected no new compile errors after embedding a Stage-2 mapping; \
         baseline={baseline_errors}, with-mapping={}, errors:\n{:#?}",
        errors.len(),
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // The MappingExtension registry must contain the user FQN.
    let registered = extension.mappings();
    assert!(
        registered.contains_key("my::test::FirmMapping"),
        "expected my::test::FirmMapping in registry; got keys: {:?}",
        registered.keys().collect::<Vec<_>>()
    );
}

fn parse_platform_sources() -> Vec<SourceFile> {
    // Phase 3b: source files are no longer embedded. Re-parse from
    // the live source trees for `platform`, `platform_dsl_store` (a
    // mapping dep), and `platform_dsl_mapping` (the metamodel under
    // test).
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let descriptors = [
        manifest.join(
            "../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json",
        ),
        manifest.join(
            "../../../legend-pure-dsl/legend-pure-dsl-store/legend-pure-m2-dsl-store-pure/src/main/resources/platform_dsl_store.definition.json",
        ),
        manifest.join(
            "../../../legend-pure-dsl/legend-pure-dsl-mapping/legend-pure-m2-dsl-mapping-pure/src/main/resources/platform_dsl_mapping.definition.json",
        ),
    ];
    let mut files = Vec::new();
    for desc in descriptors {
        let Ok(canonical) = desc.canonicalize() else {
            continue;
        };
        if let Ok(repo) = Repo::from_descriptor(&canonical) {
            for (content, path) in repo.sources() {
                match legend_pure_parser_parser::parse(content, path) {
                    Ok(sf) => files.push(sf),
                    Err(partial) => files.push(partial.source_file),
                }
            }
        }
    }
    files
}

fn compile_for_errors(files: &[SourceFile]) -> usize {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    match legend_pure_parser_pure::pipeline::compile_with_extensions(files, &imports, &exts) {
        Ok(_) => 0,
        Err(p) => p.errors.len(),
    }
}
