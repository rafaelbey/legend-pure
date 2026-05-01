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

//! End-to-end test that loads the embedded platform (including
//! `platform_dsl_tds/tds.pure`) and verifies:
//!
//! 1. `meta::pure::metamodel::relation::TDS` resolves to a Class —
//!    confirms commit #14's `PureRepo` entry for `platform_dsl_tds`
//!    is wired correctly.
//! 2. A `#TDS{...}#` island whose columns reference platform types
//!    (`Float`, `String`) compiles without TDS-shape errors when
//!    the `TDSExtension` is registered alongside the platform.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::Repo;
use legend_pure_dsl_tds::compiler::TDSExtension;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::Element as ModelElement;
use smol_str::SmolStr;

// Cache the parsed platform sources across tests to avoid re-parsing
// the ~1300-element platform per test invocation.
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
fn tds_metaclass_resolves_in_loaded_platform() {
    let exts: [&dyn CompilerExtension; 1] = [&TDSExtension::new()];
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

    let id = model
        .resolve_by_path(&[
            SmolStr::new("meta"),
            SmolStr::new("pure"),
            SmolStr::new("metamodel"),
            SmolStr::new("relation"),
            SmolStr::new("TDS"),
        ])
        .expect("meta::pure::metamodel::relation::TDS should resolve in the embedded platform");
    assert!(
        matches!(model.get_element(id), ModelElement::Class(_)),
        "expected TDS to be a Class element"
    );
}

#[test]
fn tds_island_with_platform_types_validates_cleanly() {
    let user_src = "function my::test::run(): Any[*]\n\
                    {\n\
                        #TDS\n  \
                          value:Float, name:String\n  \
                          1, A\n  \
                          2, B\n\
                        #\n\
                    }\n";
    let user_ast = legend_pure_parser_parser::parse_with_islands(
        user_src,
        "<end_to_end>",
        default_island_parsers(),
    )
    .expect("user source should parse");

    let mut all_files: Vec<SourceFile> = platform_files().to_vec();
    all_files.push(user_ast);

    let exts: [&dyn CompilerExtension; 1] = [&TDSExtension::new()];
    let imports = auto_imports();
    let result =
        legend_pure_parser_pure::pipeline::compile_with_extensions(&all_files, &imports, &exts);

    let errors: Vec<String> = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors.iter().map(|e| e.message.clone()).collect(),
    };

    // The TDS validator should produce no TDS-shape errors:
    //   - "TDS row N has..." (arity)
    //   - "TDS column 'X' references unresolved type..."
    let tds_shape_errors: Vec<&String> = errors
        .iter()
        .filter(|e| {
            (e.contains("TDS row") || e.contains("TDS column"))
                && !e.contains("Island expression lowering")
        })
        .collect();
    assert!(
        tds_shape_errors.is_empty(),
        "expected no TDS shape errors with platform types; got {tds_shape_errors:?}"
    );
}
