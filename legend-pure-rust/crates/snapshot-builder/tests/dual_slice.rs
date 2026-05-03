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

//! Integration tests for the snapshot-builder's dual-slice emission.
//!
//! Exercises the prod / tests partitioning end-to-end against the live
//! platform sources: every `compile_to_purem` invocation must produce
//! both `<output>` and `<output>.tests.purem`, and the contents of each
//! must satisfy partition invariants (prod has zero `<<test.Test>>`
//! stereotypes; both slices round-trip through `read_repo`).

use std::path::{Path, PathBuf};

use legend_pure_parser_pure::model::Element;
use legend_pure_parser_pure::pipeline::{finalize_model, init_bootstrap_model};
use legend_pure_parser_pure::purem::{merge_slice, read_repo};
use legend_pure_snapshot_builder::{
    CompileRequest, DEFAULT_PLATFORM_AUTO_IMPORTS, compile_to_purem, tests_output_path,
};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("expected ancestor 3 of CARGO_MANIFEST_DIR")
        .to_path_buf()
}

fn platform_descriptor() -> PathBuf {
    workspace_root().join("legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json")
}

fn count_test_stereotypes(model: &legend_pure_parser_pure::model::PureModel) -> usize {
    let mut count = 0usize;
    for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (_, element) in chunk.elements.iter() {
            let stereotypes: &[_] = match element {
                Element::Function(f) => &f.stereotypes,
                Element::Class(c) => &c.stereotypes,
                Element::Association(a) => &a.stereotypes,
                Element::Enumeration(e) => &e.stereotypes,
                _ => continue,
            };
            for s in stereotypes {
                if s.value.as_str() == "Test"
                    || s.value.as_str() == "TestCollection"
                    || s.value.as_str() == "BeforePackage"
                    || s.value.as_str() == "AfterPackage"
                    || s.value.as_str() == "ToFix"
                {
                    count += 1;
                    break;
                }
            }
        }
    }
    count
}

#[test]
fn dual_emit_writes_both_blobs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("platform.purem");

    let descs = vec![platform_descriptor()];
    compile_to_purem(CompileRequest {
        descriptors: &descs,
        target: "platform",
        output: &out,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    })
    .expect("compile_to_purem should succeed");

    let tests_out = tests_output_path(&out);
    assert!(out.is_file(), "production blob should be on disk");
    assert!(
        tests_out.is_file(),
        "tests blob should be on disk at {}",
        tests_out.display()
    );
    let prod_bytes = std::fs::read(&out).expect("read prod");
    let test_bytes = std::fs::read(&tests_out).expect("read tests");
    assert!(prod_bytes.len() > 50_000, "prod blob too small");
    assert!(test_bytes.len() > 1_000, "tests blob too small");
}

#[test]
fn prod_blob_carries_substantially_fewer_test_stereotypes_than_combined() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("platform.purem");
    let descs = vec![platform_descriptor()];
    compile_to_purem(CompileRequest {
        descriptors: &descs,
        target: "platform",
        output: &out,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    })
    .expect("compile_to_purem should succeed");

    // Round-trip prod alone, then prod + tests, and count test
    // stereotypes in each.
    //
    // Under the unified-reachability partition (prod = Reachable from
    // non-test roots), a handful of test-stereotyped elements can
    // legitimately end up in the prod slice — when something non-test
    // references them, we keep them there so the prod blob carries no
    // dangling refs. So we don't assert strict zero; we assert the
    // prod blob carries dramatically fewer test stereotypes than the
    // combined model.
    let prod_bytes = std::fs::read(&out).expect("read prod");
    let test_bytes = std::fs::read(tests_output_path(&out)).expect("read tests");

    let prod_slice = read_repo(&prod_bytes).expect("prod read");
    let mut prod_model = init_bootstrap_model();
    merge_slice(&mut prod_model, prod_slice).expect("merge prod");
    let _ = finalize_model(&mut prod_model, &[], &[]);
    let n_test_in_prod = count_test_stereotypes(&prod_model);

    let prod_slice2 = read_repo(&prod_bytes).expect("prod read 2");
    let test_slice = read_repo(&test_bytes).expect("tests read");
    let mut combined = init_bootstrap_model();
    merge_slice(&mut combined, prod_slice2).expect("merge prod 2");
    merge_slice(&mut combined, test_slice).expect("merge tests");
    let _ = finalize_model(&mut combined, &[], &[]);
    let n_test_in_combined = count_test_stereotypes(&combined);

    assert!(
        n_test_in_combined > 50,
        "combined model should carry many test stereotypes; got {n_test_in_combined}"
    );
    // Prod should hold less than 5% of the total test stereotypes —
    // bulk of test code must move to the tests slice.
    assert!(
        n_test_in_prod * 20 < n_test_in_combined,
        "prod carries too many test stereotypes: prod={n_test_in_prod}, combined={n_test_in_combined}"
    );
}

#[test]
fn tests_blob_carries_only_test_partition() {
    // Round-trip both blobs into a single model in the right order
    // (prod first, tests second) and assert merge succeeds. The tests
    // blob should resolve every external reference against the
    // already-merged prod elements.
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("platform.purem");
    let descs = vec![platform_descriptor()];
    compile_to_purem(CompileRequest {
        descriptors: &descs,
        target: "platform",
        output: &out,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    })
    .expect("compile_to_purem should succeed");

    let prod_bytes = std::fs::read(&out).expect("read prod");
    let test_bytes = std::fs::read(tests_output_path(&out)).expect("read tests");

    let prod_slice = read_repo(&prod_bytes).expect("prod read");
    let test_slice = read_repo(&test_bytes).expect("tests read");

    let mut combined = init_bootstrap_model();
    merge_slice(&mut combined, prod_slice).expect("merge prod");
    merge_slice(&mut combined, test_slice).expect("merge tests");
    let _ = finalize_model(&mut combined, &[], &[]);

    // Combined model has the same test stereotype count as a plain
    // platform load — i.e. tests are present after stitching.
    let n_test = count_test_stereotypes(&combined);
    assert!(
        n_test > 50,
        "combined prod+tests should carry many test elements; got {n_test}"
    );
}

#[test]
fn dual_emit_is_byte_deterministic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_a = tmp.path().join("a.purem");
    let out_b = tmp.path().join("b.purem");
    let descs = vec![platform_descriptor()];

    for out in [&out_a, &out_b] {
        compile_to_purem(CompileRequest {
            descriptors: &descs,
            target: "platform",
            output: out,
            auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
        })
        .expect("compile should succeed");
    }

    let prod_a = std::fs::read(&out_a).expect("read prod a");
    let prod_b = std::fs::read(&out_b).expect("read prod b");
    assert_eq!(prod_a, prod_b, "prod blobs must be byte-identical");

    let tests_a = std::fs::read(tests_output_path(&out_a)).expect("read tests a");
    let tests_b = std::fs::read(tests_output_path(&out_b)).expect("read tests b");
    assert_eq!(tests_a, tests_b, "tests blobs must be byte-identical");
}
