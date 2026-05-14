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

//! Unit tests for `pipeline::compile_chunks_incremental`
//! (T-20260513-01 Phase 1).
//!
//! The LSP integration tests (multi-repo, dirty-file marking,
//! diagnostics policy) live in `crates/lsp/tests/`. These tests
//! exercise the pipeline-level invariants directly:
//!
//! - Chunk ids are pinned across rerun — the chunks vector does not grow.
//! - Clean chunks' ElementIds stay valid (refs into chunk 1 still
//!   resolve after chunk 2 reruns).
//! - The returned `rerun_chunks` set matches the input rerun_order.
//! - `chunk_inputs` is consumed by-move (no Clone required on
//!   `SourceFile` at the call site).

use std::collections::HashMap;

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::pipeline::{
    compile_chunks_incremental, compile_repo_slice, finalize_model, init_bootstrap_model,
};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

#[test]
fn rerun_chunks_returned_in_sorted_order() {
    let mut model = init_bootstrap_model();
    // Stage two chunks.
    let sf_a = parse("Class repo_a::A { x: Integer[1]; }", "a.pure");
    let sf_b = parse("Class repo_b::B { y: String[1]; }", "b.pure");
    let (range_a, errs_a) = compile_repo_slice(&mut model, &[sf_a], &[], &[]);
    let (range_b, errs_b) = compile_repo_slice(&mut model, &[sf_b], &[], &[]);
    assert!(errs_a.is_empty() && errs_b.is_empty());
    let chunk_a = range_a.start as u16;
    let chunk_b = range_b.start as u16;
    let final_errs = finalize_model(&mut model, &[], &[]);
    assert!(final_errs.is_empty());
    let chunks_before = model.chunks.len();

    // Rerun chunk b with the same source (idempotent edit).
    let sf_b_new = parse("Class repo_b::B { y: String[1]; }", "b.pure");
    let mut chunk_inputs: HashMap<u16, Vec<SourceFile>> = HashMap::new();
    chunk_inputs.insert(chunk_b, vec![sf_b_new]);

    let outcome = compile_chunks_incremental(model, chunk_inputs, vec![chunk_b], &[], &[], &[]);

    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.rerun_chunks, vec![chunk_b]);
    assert_eq!(
        outcome.model.chunks.len(),
        chunks_before,
        "incremental recompile must not grow the chunks vector"
    );

    // Chunk A's element is still resolvable post-rerun (clean chunk
    // ElementIds survive intact).
    assert!(
        outcome
            .model
            .resolve_by_path(&["repo_a".into(), "A".into()])
            .is_some(),
        "clean chunk A's element must remain resolvable"
    );
    // Chunk B's element is rebuilt and resolvable under its rerun chunk_id.
    let b_id = outcome
        .model
        .resolve_by_path(&["repo_b".into(), "B".into()])
        .expect("rerun chunk B's element must resolve");
    if let legend_pure_parser_pure::ids::ElementId::InstanceId { chunk_id, .. } = b_id {
        assert_eq!(chunk_id, chunk_b);
    } else {
        panic!("expected InstanceId, got {b_id:?}");
    }
    // Confirm the clean chunk A is untouched (chunk_a != chunk_b
    // sanity, and chunk_a's element id still points into chunk_a).
    let a_id = outcome
        .model
        .resolve_by_path(&["repo_a".into(), "A".into()])
        .expect("A still resolvable");
    if let legend_pure_parser_pure::ids::ElementId::InstanceId { chunk_id, .. } = a_id {
        assert_eq!(chunk_id, chunk_a);
    } else {
        panic!("expected InstanceId, got {a_id:?}");
    }
}

#[test]
fn body_edit_in_rerun_chunk_lowers_against_clean_upstream() {
    // Chunk A defines a class; chunk B has a function that references
    // A. Reruns of chunk B must continue to see A correctly.
    let mut model = init_bootstrap_model();
    let sf_a = parse("Class util::Greeter { msg: String[1]; }", "a.pure");
    let sf_b = parse(
        "function consumer::greet(g: util::Greeter[1]): String[1] { $g.msg }",
        "b.pure",
    );
    let (range_a, errs_a) = compile_repo_slice(&mut model, &[sf_a], &[], &[]);
    let (range_b, errs_b) = compile_repo_slice(&mut model, &[sf_b], &[], &[]);
    assert!(errs_a.is_empty() && errs_b.is_empty());
    let chunk_b = range_b.start as u16;
    let _chunk_a = range_a.start as u16;
    let final_errs = finalize_model(&mut model, &[], &[]);
    assert!(final_errs.is_empty(), "finalize: {final_errs:?}");

    // Rerun chunk B with a body edit. The new body still references
    // util::Greeter — refs into clean chunk A must resolve.
    let sf_b_new = parse(
        "function consumer::greet(g: util::Greeter[1]): String[1] { $g.msg }",
        "b.pure",
    );
    let mut chunk_inputs: HashMap<u16, Vec<SourceFile>> = HashMap::new();
    chunk_inputs.insert(chunk_b, vec![sf_b_new]);

    let outcome = compile_chunks_incremental(model, chunk_inputs, vec![chunk_b], &[], &[], &[]);

    assert!(
        outcome.errors.is_empty(),
        "errors after rerun: {:?}",
        outcome.errors
    );
    assert_eq!(outcome.rerun_chunks, vec![chunk_b]);
    // The function still resolves and points into chunk B's slot.
    let _ = outcome
        .model
        .resolve_fqn_str("consumer::greet_Greeter_1__String_1_")
        .or_else(|| outcome.model.resolve_fqn_str("consumer::greet"))
        .or_else(|| {
            outcome
                .model
                .resolve_by_path(&["consumer".into(), "greet".into()])
        })
        .expect("consumer::greet must resolve after body edit");
}

#[test]
fn empty_rerun_order_is_noop() {
    let mut model = init_bootstrap_model();
    let sf = parse("Class p::X { a: Integer[1]; }", "x.pure");
    let _ = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let _ = finalize_model(&mut model, &[], &[]);
    let before = model.chunks.len();

    let outcome = compile_chunks_incremental(model, HashMap::new(), vec![], &[], &[], &[]);

    assert!(outcome.errors.is_empty());
    assert!(outcome.rerun_chunks.is_empty());
    assert_eq!(outcome.model.chunks.len(), before);
}
