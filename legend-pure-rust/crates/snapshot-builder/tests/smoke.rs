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

//! Smoke tests for the snapshot-builder helper.
//!
//! These run against the live source trees under `legend-pure-core/` and
//! `legend-pure-dsl/`. They're slow (full platform compile each), so the
//! suite is small: one per scenario.

use std::path::{Path, PathBuf};

use legend_pure_parser_pure::purem::read_repo;
use legend_pure_snapshot_builder::{
    BuildError, CompileRequest, DEFAULT_PLATFORM_AUTO_IMPORTS, compile_to_purem,
};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../legend-pure-rust/crates/snapshot-builder
    // Walk up two levels to .../legend-pure-rust then one more to repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("expected ancestor 3 of CARGO_MANIFEST_DIR")
        .to_path_buf()
}

fn platform_descriptor() -> PathBuf {
    workspace_root().join("legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json")
}

fn platform_dsl_store_descriptor() -> PathBuf {
    workspace_root().join(
        "legend-pure-dsl/legend-pure-dsl-store/legend-pure-m2-dsl-store-pure/src/main/resources/platform_dsl_store.definition.json",
    )
}

#[test]
fn builds_platform_purem_and_round_trips() {
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

    let bytes = std::fs::read(&out).expect("read output");
    assert!(
        bytes.len() > 100_000,
        "platform.purem should be non-trivial; got {} bytes",
        bytes.len()
    );

    let slice = read_repo(&bytes).expect("read_repo should accept the freshly-written blob");
    let element_count: u32 = slice.chunks.iter().map(|c| c.elements.len()).sum();
    assert!(
        element_count > 100,
        "platform slice should contain many elements; got {element_count}"
    );
}

#[test]
fn builds_platform_dsl_store_with_dependency() {
    // platform_dsl_store depends on platform — supplying both descriptors
    // exercises the topo-sort and cross-repo resolution paths.
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("platform_dsl_store.purem");

    let descs = vec![platform_descriptor(), platform_dsl_store_descriptor()];
    compile_to_purem(CompileRequest {
        descriptors: &descs,
        target: "platform_dsl_store",
        output: &out,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    })
    .expect("compile_to_purem should succeed for dsl_store");

    let bytes = std::fs::read(&out).expect("read output");
    let slice = read_repo(&bytes).expect("dsl_store .purem must round-trip");
    assert!(
        !slice.chunks.is_empty(),
        "dsl_store slice must contain at least one chunk"
    );
}

#[test]
fn build_is_byte_deterministic() {
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

    let a = std::fs::read(&out_a).expect("read a");
    let b = std::fs::read(&out_b).expect("read b");
    assert_eq!(
        a, b,
        "two builds of the same platform must be byte-identical"
    );
}

#[test]
fn unknown_target_errors_cleanly() {
    let descs = vec![platform_descriptor()];
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("nope.purem");
    let err = compile_to_purem(CompileRequest {
        descriptors: &descs,
        target: "nonexistent_repo",
        output: &out,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    })
    .expect_err("unknown target must error");
    assert!(matches!(err, BuildError::UnknownTarget(_)), "got: {err:?}");
}
