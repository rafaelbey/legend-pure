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

//! Asserts the built `libmydsl_pure_jni.{dylib,so,dll}` exports
//! every `Java_org_finos_legend_pure_rust_*` symbol the stock
//! `legend-pure-parser-jni` cdylib ships. This is the JNI cdylib
//! equivalent of the `examples/mydsl-extension/tests/smoke.rs`
//! discovery proofs.
//!
//! The test depends on the cdylib having been built (`cargo build`
//! before `cargo test`, or `cargo test` itself which builds the
//! cdylib as part of its dep resolution).

use std::path::PathBuf;
use std::process::Command;

/// All `Java_*` symbols downstream cdylibs must export to be a
/// drop-in replacement for `libpure_rust_jni`. Sourced from
/// `crates/jni/src/lib.rs` — keep in sync when adding upstream
/// entries.
const EXPECTED_SYMBOLS: &[&str] = &[
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeEvaluate",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetProperty",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetClassifier",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeNew",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeContext",
    "Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeInstance",
    "Java_org_finos_legend_pure_rust_bindings_PureBindingsGenerator_nativeGenerateBindings",
];

fn cdylib_path() -> PathBuf {
    // tests run with CWD = the crate root, so `target/` is sibling.
    // `cfg!` chooses the right extension per platform.
    let ext = if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let prefix = if cfg!(target_os = "windows") {
        ""
    } else {
        "lib"
    };
    PathBuf::from("target/debug").join(format!("{prefix}mydsl_pure_jni.{ext}"))
}

#[test]
fn cdylib_exports_every_upstream_java_symbol() {
    let cdylib = cdylib_path();
    if !cdylib.exists() {
        // Cargo test sometimes runs before the cdylib's first build
        // — typical fresh-clone case. Fall back to `cargo build`.
        let status = Command::new("cargo")
            .arg("build")
            .status()
            .expect("cargo build must succeed");
        assert!(status.success(), "cargo build failed");
        assert!(
            cdylib.exists(),
            "cdylib still missing after cargo build: {}",
            cdylib.display(),
        );
    }

    // `nm -gU <path>` on macOS/Linux dumps externally-visible
    // (`-g`) defined (`U`) symbols. Windows uses `dumpbin /EXPORTS`
    // but the macOS/Linux path is the common case; Windows users
    // verify by hand per the README.
    if cfg!(target_os = "windows") {
        eprintln!("symbols.rs: skipping symbol scan on Windows — verify via `dumpbin /EXPORTS`");
        return;
    }
    let nm_args = if cfg!(target_os = "macos") {
        vec!["-gU"]
    } else {
        // GNU nm: -D for dynamic symbols (.so equivalent of macOS -gU).
        vec!["-D", "--defined-only"]
    };
    let output = Command::new("nm")
        .args(&nm_args)
        .arg(&cdylib)
        .output()
        .expect("nm must be available");
    assert!(
        output.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // macOS prefixes user symbols with an extra leading underscore;
    // strip it for cross-platform matching.
    let normalize = |name: &str| {
        if cfg!(target_os = "macos") {
            format!(" _{name}")
        } else {
            format!(" {name}")
        }
    };

    let mut missing: Vec<&str> = Vec::new();
    for expected in EXPECTED_SYMBOLS {
        let needle = normalize(expected);
        if !stdout.contains(&needle) {
            missing.push(expected);
        }
    }
    assert!(
        missing.is_empty(),
        "cdylib {} is missing JNI symbols: {missing:#?}\n\n\
         Forwarder regression — every `Java_*` in EXPECTED_SYMBOLS must \
         have a matching `#[no_mangle] pub extern \"system\" fn` in \
         src/lib.rs.",
        cdylib.display(),
    );
}
