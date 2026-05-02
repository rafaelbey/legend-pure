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

//! Compiler-side behaviour for graph-fetch islands when no extension
//! is registered.
//!
//! When `compile()` is invoked without `GraphFetchExtension`, the core
//! lowering pass must reject any island expression with a clear
//! `UnsupportedExpression { kind: "Island" }`. End-to-end tests in
//! `tests/end_to_end.rs` exercise the opposite path — running
//! `compile_with_extensions` with `GraphFetchExtension` — and assert
//! that graph-fetch-specific diagnostics surface instead.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::compile;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_islands(
        source,
        "test.pure",
        legend_pure_dsl_graph::parser::default_island_parsers(),
    )
    .expect("parse failed")
}

#[allow(clippy::result_large_err)]
fn compile_one(
    source: &str,
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sf = parse(source);
    compile!(&[sf])
}

#[test]
fn island_expression_rejected_at_lowering() {
    // A structurally valid graph-fetch island that parses cleanly via
    // the dsl-graph plug-in but gets rejected during lowering because
    // dsl-graph's CompilerExtension isn't wired yet.
    let result = compile_one("function x(): Any[1] { #{some::Class{}}# }");
    assert!(result.is_err(), "island expression should fail lowering");
    let errors = &result.unwrap_err().errors;

    let island_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::UnsupportedExpression { .. }
            )
        })
        .collect();

    assert_eq!(
        island_errors.len(),
        1,
        "should have UnsupportedExpression error"
    );
    assert!(island_errors[0].message.contains("Island"));
}
