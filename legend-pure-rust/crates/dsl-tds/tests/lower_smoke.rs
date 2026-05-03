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

//! Locks the [`TDSIslandLowerer`] end-to-end against the platform.
//!
//! Compiles a small Pure source that contains a `#TDS#` literal,
//! with the TDS island lowerer registered alongside the platform.
//! Asserts no `Island expression lowering not yet implemented`
//! diagnostic fires — i.e. the island successfully lowered to a
//! `stringToTDS('<csv>')->cast(@TDS<…>)` synthetic AST that the
//! existing function-application + arrow-call lowering paths
//! processed cleanly.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::Repo;
use legend_pure_dsl_tds::compiler::TDSExtension;
use legend_pure_dsl_tds::lower::default_island_lowerers;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::pipeline::compile_with_extensions_and_islands;
use smol_str::SmolStr;

fn platform_files() -> &'static [SourceFile] {
    static CACHED: OnceLock<Vec<SourceFile>> = OnceLock::new();
    CACHED.get_or_init(|| {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let descriptors = [
            manifest.join(
                "../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json",
            ),
            manifest.join(
                "../../../legend-pure-dsl/legend-pure-dsl-tds/legend-pure-m2-dsl-tds-pure/src/main/resources/platform_dsl_tds.definition.json",
            ),
        ];
        let mut files = Vec::new();
        for desc in descriptors {
            let Ok(canonical) = desc.canonicalize() else {
                continue;
            };
            if let Ok(repo) = Repo::from_descriptor(&canonical) {
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

fn parse_user_source(source: &str, name: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_islands(source, name, default_island_parsers())
        .unwrap_or_else(|p| panic!("user source parse failed: {:?}", p.errors))
}

#[test]
fn tds_island_lowers_without_unsupported_expression_error() {
    let mut sources: Vec<SourceFile> = platform_files().to_vec();
    sources.push(parse_user_source(
        r"###Pure
function my::test::tdsLiteral(): meta::pure::metamodel::relation::TDS<meta::pure::metamodel::relation::Relation>[1]
{
    #TDS
       a: Integer, b: Float, name: String
       1, 1.5, foo
       2, 2.5, bar
    #
}",
        "tds_lower_smoke.pure",
    ));

    let auto_imports = auto_imports();
    let extensions: Vec<&dyn CompilerExtension> = vec![&TDSExtension];
    let lowerers = default_island_lowerers();

    let result =
        compile_with_extensions_and_islands(&sources, &auto_imports, &extensions, &lowerers);

    let errors = match result {
        Ok(_) => vec![],
        Err(p) => p.errors,
    };

    let unsupported_island = errors.iter().find(|e| {
        e.message
            .contains("Island expression lowering not yet implemented")
    });
    assert!(
        unsupported_island.is_none(),
        "TDS island lowerer was registered but `Expression::Island(_)` still hit the legacy panic path. Errors: {errors:#?}"
    );
}

#[test]
fn tds_island_falls_back_to_legacy_panic_when_no_lowerer_registered() {
    let mut sources: Vec<SourceFile> = platform_files().to_vec();
    sources.push(parse_user_source(
        r"###Pure
function my::test::tdsNoLowerer(): meta::pure::metamodel::relation::TDS<meta::pure::metamodel::relation::Relation>[1]
{
    #TDS
       a: Integer
       1
    #
}",
        "tds_no_lowerer.pure",
    ));

    let auto_imports = auto_imports();
    let extensions: Vec<&dyn CompilerExtension> = vec![&TDSExtension];
    // Empty lowerers slice — the dispatch should fall through to the
    // legacy "not yet implemented" diagnostic.
    let lowerers: Vec<Box<dyn legend_pure_parser_pure::island_lower::IslandLowerer>> = vec![];

    let result =
        compile_with_extensions_and_islands(&sources, &auto_imports, &extensions, &lowerers);

    let errors = match result {
        Ok(_) => vec![],
        Err(p) => p.errors,
    };

    let unsupported_island = errors.iter().find(|e| {
        e.message
            .contains("Island expression lowering not yet implemented")
    });
    assert!(
        unsupported_island.is_some(),
        "with no lowerer registered, `Expression::Island(_)` should still hit the legacy panic path. Errors: {errors:#?}"
    );
}
