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

//! End-to-end test of the relation-store DSL `CompilerExtension`.
//!
//! Each fixture defines `meta::pure::store::Store` inline (extends
//! `PackageableElement`, an M3 bootstrap class) so the test does not
//! load the full platform — keeping the test fast and self-contained.
//! The extension's `define_bodies` pass walks function bodies, finds
//! `#>{ … }#` islands, and asserts `path[0]` resolves to a Store
//! subclass.

use indoc::indoc;
use legend_pure_dsl_store::compiler::RelationStoreExtension;
use legend_pure_dsl_store::parser::default_island_parsers;
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
fn compile_with_store(sources: Vec<SourceFile>, ext: &RelationStoreExtension) -> Vec<String> {
    let exts: [&dyn CompilerExtension; 1] = [ext];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors.iter().map(|e| e.message.clone()).collect(),
    }
}

#[test]
fn happy_path_store_subclass_validates() {
    let source = indoc! {r"
        Class meta::pure::store::Store extends PackageableElement
        {
        }

        Class my::mainDb extends meta::pure::store::Store
        {
        }

        function my::test::run(): Any[*]
        {
            #>{my::mainDb.PersonTable}#
        }
    "};
    let file = parse(source);
    let ext = RelationStoreExtension::new();
    let errors = compile_with_store(vec![file], &ext);

    // Core lowering still rejects all islands with
    // `UnsupportedExpression` until dsl-store implements lowering;
    // dsl-store's role here is *validation*, not lowering. So expect
    // the unsupported-expression error to remain, but no Store-shape
    // errors.
    let store_errors: Vec<&String> = errors
        .iter()
        .filter(|e| e.contains("RelationStoreAccessor"))
        .collect();
    assert!(
        store_errors.is_empty(),
        "expected no RelationStoreAccessor errors on a valid Store subclass; got {store_errors:?}"
    );
}

#[test]
fn validate_reports_unresolved_store_path() {
    let source = indoc! {r"
        Class meta::pure::store::Store extends PackageableElement
        {
        }

        function my::test::run(): Any[*]
        {
            #>{nonexistent::DB.PersonTable}#
        }
    "};
    let file = parse(source);
    let ext = RelationStoreExtension::new();
    let errors = compile_with_store(vec![file], &ext);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("RelationStoreAccessor") && e.contains("nonexistent::DB")),
        "expected unresolved-store error mentioning the FQN; got {errors:?}",
    );
}

#[test]
fn validate_reports_non_store_subclass() {
    let source = indoc! {r"
        Class meta::pure::store::Store extends PackageableElement
        {
        }

        Class my::Person
        {
            name: String[1];
        }

        function my::test::run(): Any[*]
        {
            #>{my::Person.PersonTable}#
        }
    "};
    let file = parse(source);
    let ext = RelationStoreExtension::new();
    let errors = compile_with_store(vec![file], &ext);

    assert!(
        errors.iter().any(|e| e.contains("RelationStoreAccessor")
            && e.contains("my::Person")
            && e.contains("not a subclass")),
        "expected not-a-subclass error mentioning the bad FQN; got {errors:?}",
    );
}

#[test]
fn store_island_inside_arrow_call_is_validated() {
    // The store accessor appears as the receiver of `->filter(...)`.
    // The walker must recurse into ArrowFunction.target.
    let source = indoc! {r"
        Class meta::pure::store::Store extends PackageableElement
        {
        }

        function my::test::run(): Any[*]
        {
            #>{nonexistent::DB.PersonTable}#->filter(t|true)
        }
    "};
    let file = parse(source);
    let ext = RelationStoreExtension::new();
    let errors = compile_with_store(vec![file], &ext);

    assert!(
        errors
            .iter()
            .any(|e| e.contains("RelationStoreAccessor") && e.contains("nonexistent::DB")),
        "expected unresolved-store error inside arrow receiver; got {errors:?}",
    );
}
