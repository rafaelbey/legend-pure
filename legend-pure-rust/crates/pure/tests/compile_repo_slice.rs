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

//! Phase 0 contract: `init_bootstrap_model` + `compile_repo_slice` +
//! `finalize_model` is structurally equivalent to a single
//! `compile_with_extensions` call when fed the same sources.
//!
//! Also verifies cross-repo composition: a class in one slice can extend a
//! class in an earlier slice, and the resolver finds it without help.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::pipeline::{
    compile_repo_slice, compile_with_extensions, finalize_model, init_bootstrap_model,
};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

#[test]
fn staged_api_matches_single_call_two_classes() {
    // Single-call baseline
    let sf_a = parse("Class repo_a::A { x: Integer[1]; }", "a.pure");
    let sf_b = parse("Class repo_b::B { y: String[1]; }", "b.pure");
    let baseline = compile_with_extensions(&[sf_a.clone(), sf_b.clone()], &[], &[])
        .expect("baseline compile should succeed");

    // Staged: bootstrap, then two compile_repo_slice calls in order
    let mut model = init_bootstrap_model();
    let (range_a, errs_a) = compile_repo_slice(&mut model, &[sf_a], &[], &[]);
    assert!(errs_a.is_empty(), "slice A errors: {errs_a:?}");
    let (range_b, errs_b) = compile_repo_slice(&mut model, &[sf_b], &[], &[]);
    assert!(errs_b.is_empty(), "slice B errors: {errs_b:?}");
    let final_errs = finalize_model(&mut model, &[], &[]);
    assert!(final_errs.is_empty(), "finalize errors: {final_errs:?}");

    // Each slice should land in its own chunk after the bootstrap chunk (0).
    assert_eq!(range_a, 1..2, "slice A should be a single new chunk");
    assert_eq!(range_b, 2..3, "slice B should be a single new chunk");

    // Both models resolve A and B identically.
    let baseline_a = baseline
        .resolve_by_path(&["repo_a".into(), "A".into()])
        .expect("baseline has A");
    let staged_a = model
        .resolve_by_path(&["repo_a".into(), "A".into()])
        .expect("staged has A");
    assert_eq!(
        baseline.get_node(baseline_a).name,
        model.get_node(staged_a).name,
        "A should have the same name"
    );

    let baseline_b = baseline
        .resolve_by_path(&["repo_b".into(), "B".into()])
        .expect("baseline has B");
    let staged_b = model
        .resolve_by_path(&["repo_b".into(), "B".into()])
        .expect("staged has B");
    assert_eq!(
        baseline.get_node(baseline_b).name,
        model.get_node(staged_b).name
    );

    // Chunk counts intentionally differ: baseline puts both source files in
    // one chunk (bootstrap + 1 = 2). Staged puts each slice in its own chunk
    // (bootstrap + slice A + slice B = 3). This is the partitioning that
    // makes per-repo serialization possible.
    assert_eq!(baseline.chunks.len(), 2);
    assert_eq!(model.chunks.len(), 3);
}

#[test]
fn cross_slice_supertype_resolves() {
    // Slice 1 declares Animal; slice 2 declares Dog extends Animal.
    // The resolver in slice 2 must find Animal in the running model.
    let sf_animal = parse("Class zoo::Animal { name: String[1]; }", "animal.pure");
    let sf_dog = parse(
        "Class kennel::Dog extends zoo::Animal { breed: String[1]; }",
        "dog.pure",
    );

    let mut model = init_bootstrap_model();

    let (_r1, e1) = compile_repo_slice(&mut model, &[sf_animal], &[], &[]);
    assert!(e1.is_empty(), "Animal slice errors: {e1:?}");

    let (_r2, e2) = compile_repo_slice(&mut model, &[sf_dog], &[], &[]);
    assert!(e2.is_empty(), "Dog slice errors: {e2:?}");

    let e3 = finalize_model(&mut model, &[], &[]);
    assert!(e3.is_empty(), "finalize errors: {e3:?}");

    // Verify the supertype resolved to Animal's ElementId, not a placeholder.
    let dog_id = model
        .resolve_by_path(&["kennel".into(), "Dog".into()])
        .expect("Dog should resolve");
    let animal_id = model
        .resolve_by_path(&["zoo".into(), "Animal".into()])
        .expect("Animal should resolve");
    let Element::Class(dog) = model.get_element(dog_id) else {
        panic!("Dog should be a Class");
    };

    assert_eq!(
        dog.super_types.len(),
        1,
        "Dog should have exactly one supertype"
    );
    let super_id = match &dog.super_types[0] {
        legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => *element,
        other => panic!("expected Named supertype, got {other:?}"),
    };
    assert_eq!(super_id, animal_id, "Dog's supertype should be Animal");
}

#[test]
fn empty_slices_are_noops() {
    // Calling compile_repo_slice with no sources should add no chunks and
    // no errors. Useful for empty repos in a classpath.
    let mut model = init_bootstrap_model();
    let chunks_before = model.chunks.len();

    let (range, errors) = compile_repo_slice(&mut model, &[], &[], &[]);
    assert!(errors.is_empty(), "empty slice produced errors: {errors:?}");
    assert_eq!(range, chunks_before..chunks_before);
    assert_eq!(model.chunks.len(), chunks_before);
}

#[test]
fn finalize_alone_on_bootstrap_model_produces_no_errors() {
    // Bootstrap-only model should pass validation.
    let mut model: PureModel = init_bootstrap_model();
    let errors = finalize_model(&mut model, &[], &[]);
    assert!(
        errors.is_empty(),
        "bootstrap-only model finalized with errors: {errors:?}"
    );
}
