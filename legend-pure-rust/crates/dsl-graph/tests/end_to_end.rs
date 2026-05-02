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

//! End-to-end test for the graph-fetch DSL: loads the embedded
//! platform (including `platform_dsl_graph/graph.pure`), parses a
//! user `.pure` file containing `#{ … }#` islands with the dsl-graph
//! parser plug-in, runs the full compile pipeline with
//! `GraphFetchExtension` registered, and verifies:
//!
//! 1. `meta::pure::graphFetch::RootGraphFetchTree` resolves to a Class
//!    after the platform compile — confirms commit-3's descriptor entry
//!    embeds the metamodel correctly.
//! 2. A `#{ Class { knownProp1, knownProp2 } }#` island compiles
//!    without graph-fetch-shape errors when the extension is active.
//! 3. A `#{ Class { unknownProp } }#` island produces an
//!    `UnknownProperty` diagnostic from the extension (locks the
//!    validator path).

use std::sync::OnceLock;

use legend_pure_core_platform::repo::Repo;
use legend_pure_dsl_graph::compiler::GraphFetchExtension;
use legend_pure_dsl_graph::parser::default_island_parsers;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::CompilationErrorKind;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::Element as ModelElement;
use smol_str::SmolStr;

fn platform_files() -> &'static [SourceFile] {
    static CACHED: OnceLock<Vec<SourceFile>> = OnceLock::new();
    CACHED.get_or_init(|| {
        let repos = Repo::default_embedded();
        let mut files = Vec::new();
        for repo in &repos {
            for (content, path) in repo.sources() {
                match legend_pure_parser_parser::parse_with_islands(
                    content,
                    path,
                    default_island_parsers(),
                ) {
                    Ok(sf) => files.push(sf),
                    Err(partial) => files.push(partial.source_file),
                }
            }
        }
        files
    })
}

fn auto_imports() -> Vec<SmolStr> {
    legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect()
}

#[test]
fn graph_fetch_metaclass_resolves_in_loaded_platform() {
    let exts: [&dyn CompilerExtension; 1] = [&GraphFetchExtension::new()];
    let imports = auto_imports();
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(
        platform_files(),
        &imports,
        &exts,
    );
    let model = match result {
        Ok(m) => m,
        Err(p) => p.model,
    };

    for fqn in [
        ["meta", "pure", "graphFetch", "GraphFetchTree"].as_slice(),
        ["meta", "pure", "graphFetch", "RootGraphFetchTree"].as_slice(),
        ["meta", "pure", "graphFetch", "PropertyGraphFetchTree"].as_slice(),
        ["meta", "pure", "graphFetch", "SubTypeGraphFetchTree"].as_slice(),
    ] {
        let segments: Vec<SmolStr> = fqn.iter().map(|s| SmolStr::new(*s)).collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("graph-fetch metamodel class missing: {fqn:?}"));
        assert!(
            matches!(model.get_element(id), ModelElement::Class(_)),
            "expected {fqn:?} to be a Class element"
        );
    }
}

#[test]
fn graph_fetch_island_with_known_properties_validates_cleanly() {
    let user_src = "Class my::test::Person\n\
                    {\n  \
                      firstName : String[1];\n  \
                      lastName : String[1];\n\
                    }\n\
                    \n\
                    function my::test::run(): Any[1]\n\
                    {\n  \
                      #{ my::test::Person { firstName, lastName } }#\n\
                    }\n";
    let user_ast = legend_pure_parser_parser::parse_with_islands(
        user_src,
        "<end_to_end>",
        default_island_parsers(),
    )
    .expect("user source should parse");

    let mut all_files: Vec<SourceFile> = platform_files().to_vec();
    all_files.push(user_ast);

    let exts: [&dyn CompilerExtension; 1] = [&GraphFetchExtension::new()];
    let imports = auto_imports();
    let result =
        legend_pure_parser_pure::pipeline::compile_with_extensions(&all_files, &imports, &exts);

    let errors = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };

    // The graph-fetch validator should produce no graph-fetch-shape
    // errors — neither unresolved-class nor unknown-property — for an
    // island whose class and properties are all declared.
    let graph_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.starts_with("Graph-fetch ")
                && (e.message.contains("not found") || e.message.contains("does not resolve"))
        })
        .collect();
    assert!(
        graph_errors.is_empty(),
        "expected no graph-fetch shape errors; got {graph_errors:?}"
    );
}

#[test]
fn graph_fetch_island_with_unknown_property_errors_from_validator() {
    let user_src = "Class my::test::Person\n\
                    {\n  \
                      firstName : String[1];\n\
                    }\n\
                    \n\
                    function my::test::run(): Any[1]\n\
                    {\n  \
                      #{ my::test::Person { firstName, missingProperty } }#\n\
                    }\n";
    let user_ast = legend_pure_parser_parser::parse_with_islands(
        user_src,
        "<end_to_end>",
        default_island_parsers(),
    )
    .expect("user source should parse");

    let mut all_files: Vec<SourceFile> = platform_files().to_vec();
    all_files.push(user_ast);

    let exts: [&dyn CompilerExtension; 1] = [&GraphFetchExtension::new()];
    let imports = auto_imports();
    let result =
        legend_pure_parser_pure::pipeline::compile_with_extensions(&all_files, &imports, &exts);

    let errors = match result {
        Ok(_) => panic!("expected compilation errors for unknown property"),
        Err(p) => p.errors,
    };

    let unknown_property_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                CompilationErrorKind::UnknownProperty { property_name, .. }
                    if property_name == "missingProperty"
            )
        })
        .collect();
    assert!(
        !unknown_property_errors.is_empty(),
        "expected an UnknownProperty error from the graph-fetch validator for 'missingProperty'; \
         all errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn graph_fetch_island_with_unresolved_root_class_errors_from_validator() {
    let user_src = "function my::test::run(): Any[1]\n\
                    {\n  \
                      #{ does::not::exist::NoSuchClass { foo } }#\n\
                    }\n";
    let user_ast = legend_pure_parser_parser::parse_with_islands(
        user_src,
        "<end_to_end>",
        default_island_parsers(),
    )
    .expect("user source should parse");

    let mut all_files: Vec<SourceFile> = platform_files().to_vec();
    all_files.push(user_ast);

    let exts: [&dyn CompilerExtension; 1] = [&GraphFetchExtension::new()];
    let imports = auto_imports();
    let result =
        legend_pure_parser_pure::pipeline::compile_with_extensions(&all_files, &imports, &exts);

    let errors = match result {
        Ok(_) => panic!("expected compilation errors for unresolved class"),
        Err(p) => p.errors,
    };

    let unresolved: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("Graph-fetch root class") && e.message.contains("does not resolve")
        })
        .collect();
    assert!(
        !unresolved.is_empty(),
        "expected a graph-fetch root-class unresolved error; all errors: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
