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

//! Validation tests for the TDS `CompilerExtension` using inline
//! source (no platform load): per-row arity, type-reference
//! resolution. The end-to-end "metamodel resolves cleanly" test
//! lives in `tests/end_to_end.rs` and loads the embedded platform.

use indoc::indoc;
use legend_pure_dsl_tds::compiler::TDSExtension;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_islands(
        source,
        "compiler_smoke.pure",
        default_island_parsers(),
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

#[allow(clippy::needless_pass_by_value)]
fn compile_with_tds(sources: Vec<SourceFile>, ext: &TDSExtension) -> Vec<String> {
    let exts: [&dyn CompilerExtension; 1] = [ext];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors.iter().map(|e| e.message.clone()).collect(),
    }
}

#[test]
fn untyped_columns_pass_validation() {
    let source = indoc! {r"
        function my::test::run(): Any[*]
        {
            #TDS
              a, b, c
              1, 2, 3
              4, 5, 6
            #
        }
    "};
    let file = parse(source);
    let ext = TDSExtension::new();
    let errors = compile_with_tds(vec![file], &ext);

    let tds_errors: Vec<&String> = errors.iter().filter(|e| e.contains("TDS")).collect();
    assert!(
        tds_errors.is_empty(),
        "expected no TDS errors on untyped columns; got {tds_errors:?}"
    );
}

#[test]
fn validate_reports_row_arity_mismatch() {
    let source = indoc! {r"
        function my::test::run(): Any[*]
        {
            #TDS
              a, b, c
              1, 2
              4, 5, 6
            #
        }
    "};
    let file = parse(source);
    let ext = TDSExtension::new();
    let errors = compile_with_tds(vec![file], &ext);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("TDS row 1") && e.contains("expected 3")),
        "expected row-arity error mentioning row 1; got {errors:?}",
    );
}

#[test]
fn validate_reports_unresolved_column_type() {
    let source = indoc! {r"
        function my::test::run(): Any[*]
        {
            #TDS
              a:DoesNotExist, b
              1, 2
            #
        }
    "};
    let file = parse(source);
    let ext = TDSExtension::new();
    let errors = compile_with_tds(vec![file], &ext);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("TDS column 'a'") && e.contains("DoesNotExist")),
        "expected unresolved-type error for column 'a'; got {errors:?}",
    );
}

#[test]
fn primitive_type_references_resolve() {
    // `String` and `Integer` are bootstrap M3 primitive types — they
    // resolve even without loading the full platform.
    let source = indoc! {r"
        function my::test::run(): Any[*]
        {
            #TDS
              a:String, b:Integer
              foo, 1
            #
        }
    "};
    let file = parse(source);
    let ext = TDSExtension::new();
    let errors = compile_with_tds(vec![file], &ext);

    let tds_errors: Vec<&String> = errors.iter().filter(|e| e.contains("TDS column")).collect();
    assert!(
        tds_errors.is_empty(),
        "expected primitive types to resolve; got {tds_errors:?}"
    );
}
