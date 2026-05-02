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

//! End-to-end tests for the `.purem` wire format.
//!
//! Covers Phase J's determinism gate (idempotence + double round-trip) on
//! a small in-test slice, plus structural parity after a round-trip
//! through `write_repo` → `read_repo` → `merge_slice`.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::pipeline::{
    compile_repo_slice, finalize_model, init_bootstrap_model,
};
use legend_pure_parser_pure::purem::{merge_slice, read_repo, slice_by_repo, write_repo};
use smol_str::SmolStr;

fn parse(s: &str, p: &str) -> SourceFile {
    legend_pure_parser_parser::parse(s, p).expect("parse should succeed")
}

#[test]
fn write_repo_is_idempotent() {
    // Same input → same bytes, twice in a row. Catches Postcard /
    // schema-hash drift on identical inputs.
    let mut model = init_bootstrap_model();
    let sf = parse("Class repo_a::Foo { x: Integer[1]; }", "foo.pure");
    let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    assert!(errs.is_empty(), "compile errors: {errs:?}");

    let slice = slice_by_repo(&model, 1..2);
    let a = write_repo(&slice).expect("write a");
    let b = write_repo(&slice).expect("write b");
    assert_eq!(a, b, "two writes of the same slice must produce equal bytes");
}

#[test]
fn write_then_read_round_trips() {
    let mut model = init_bootstrap_model();
    let sf = parse(
        "Class shop::Item { name: String[1]; price: Integer[0..1]; }",
        "item.pure",
    );
    let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    assert!(errs.is_empty());

    let slice = slice_by_repo(&model, 1..2);
    let bytes = write_repo(&slice).expect("write should succeed");
    let recovered =
        read_repo(&bytes).expect("read should succeed on freshly-written bytes");

    assert_eq!(slice.chunks.len(), recovered.chunks.len());
    assert_eq!(slice.external_refs, recovered.external_refs);
    assert_eq!(slice.element_packages, recovered.element_packages);
    assert_eq!(slice.source_chunk_range, recovered.source_chunk_range);
}

#[test]
fn double_round_trip_is_byte_identical() {
    // a = write(slice); b = write(read(a)); c = write(read(b));
    // assert a == b == c. Catches drift that only manifests on the
    // second pass (e.g. HashMap ordering hiding behind a Vec).
    let mut model = init_bootstrap_model();
    let sf = parse("Class p::A { f: String[1]; }", "a.pure");
    let (_r, _e) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let slice = slice_by_repo(&model, 1..2);

    let a = write_repo(&slice).expect("write 1");
    let s2 = read_repo(&a).expect("read 1");
    let b = write_repo(&s2).expect("write 2");
    let s3 = read_repo(&b).expect("read 2");
    let c = write_repo(&s3).expect("write 3");

    assert_eq!(a, b, "first round-trip must be byte-identical");
    assert_eq!(b, c, "second round-trip must be byte-identical");
}

#[test]
fn write_read_merge_resolves_in_fresh_model() {
    // Take a slice through bytes, merge into a fresh model, verify that
    // every element resolves and external refs are wired up correctly.
    let mut original = init_bootstrap_model();
    let sf_animal = parse("Class zoo::Animal { name: String[1]; }", "a.pure");
    let sf_dog = parse(
        "Class kennel::Dog extends zoo::Animal {}",
        "d.pure",
    );
    let (_r1, _e1) = compile_repo_slice(&mut original, &[sf_animal], &[], &[]);
    let (_r2, _e2) = compile_repo_slice(&mut original, &[sf_dog], &[], &[]);

    let animal_slice = slice_by_repo(&original, 1..2);
    let dog_slice = slice_by_repo(&original, 2..3);

    // Round-trip both through bytes.
    let animal_bytes = write_repo(&animal_slice).expect("write animal");
    let dog_bytes = write_repo(&dog_slice).expect("write dog");

    let animal_recovered = read_repo(&animal_bytes).expect("read animal");
    let dog_recovered = read_repo(&dog_bytes).expect("read dog");

    // Merge in dependency order into a fresh bootstrap model.
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, animal_recovered).expect("merge animal");
    merge_slice(&mut fresh, dog_recovered).expect("merge dog");
    let _ = finalize_model(&mut fresh, &[], &[]);

    let dog_id = fresh
        .resolve_by_path(&[SmolStr::new("kennel"), SmolStr::new("Dog")])
        .expect("Dog should resolve in fresh model");
    let animal_id = fresh
        .resolve_by_path(&[SmolStr::new("zoo"), SmolStr::new("Animal")])
        .expect("Animal should resolve in fresh model");

    let legend_pure_parser_pure::model::Element::Class(dog) = fresh.get_element(dog_id)
    else {
        panic!("Dog should be a Class");
    };
    assert_eq!(dog.super_types.len(), 1);
    let super_id = match &dog.super_types[0] {
        legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => *element,
        other => panic!("expected Named supertype, got {other:?}"),
    };
    assert_eq!(super_id, animal_id);
}

#[test]
fn read_rejects_bad_magic() {
    let bad = b"NOPE!".to_vec();
    let mut blob = bad;
    // Pad to header size; values past the magic check don't matter.
    blob.resize(64, 0);
    assert!(read_repo(&blob).is_err());
}

#[test]
fn read_rejects_corrupt_payload() {
    // Valid header + truncated/scrambled payload → ReadError::Postcard.
    let mut model = init_bootstrap_model();
    let sf = parse("Class p::C {}", "c.pure");
    let (_r, _e) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let slice = slice_by_repo(&model, 1..2);
    let mut bytes = write_repo(&slice).expect("write");

    // Scramble the last byte of the payload (header is the first 22).
    let n = bytes.len();
    bytes[n - 1] ^= 0xFF;
    assert!(read_repo(&bytes).is_err());
}
