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

//! Runs the Pure-native test functions under
//! `legend-pure-m2-store-relational-pure/src/test/resources/platform_store_relational_tests/`.
//!
//! These tests are the **canonical** end-to-end coverage for the
//! relational native bodies — the Pure source under
//! `platform_store_relational_tests/` is intended to be discovered by
//! both Java's and Rust's surveyors so they exercise the same Pure
//! `<<test.Test>>` functions. This integration test runs each function
//! by mangled FQN through the Rust evaluator.
//
// `PureException` is intentionally rich (call stack), so the
// `Result<T, PureException>` types lighting up `result_large_err` are
// allowed crate-wide here too.
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};

use legend_pure_core_platform::repo::{self, OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use legend_pure_store_relational_runtime::RelationalStoreExtension;
use smol_str::SmolStr;

/// Walk `src/test/resources/platform_store_relational_tests/` (relative
/// to this crate, which sits next to the rest of the Rust workspace) and
/// collect every `.pure` file as an `OwnedSourceFile`.
fn collect_test_source_files() -> Vec<OwnedSourceFile> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    // ../store-relational-runtime → workspace `crates/` → repo root
    let workspace = Path::new(manifest)
        .parent()
        .expect("crates dir")
        .parent()
        .expect("legend-pure-rust dir")
        .parent()
        .expect("repo root");
    let dir: PathBuf = workspace.join(
        "legend-pure-store/legend-pure-store-relational/legend-pure-m2-store-relational-pure/\
         src/test/resources/platform_store_relational_tests",
    );
    let mut files = Vec::new();
    for entry in walkdir(&dir) {
        let content = std::fs::read_to_string(&entry).expect("read test source");
        files.push(OwnedSourceFile {
            path: entry.to_string_lossy().into_owned(),
            content,
        });
    }
    assert!(
        !files.is_empty(),
        "expected at least one .pure file in {}",
        dir.display()
    );
    files
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("pure") {
            out.push(p);
        } else if p.is_dir() {
            out.extend(walkdir(&p));
        }
    }
    out.sort();
    out
}

fn auto_imports() -> Vec<SmolStr> {
    legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect()
}

fn test_repo() -> Repo {
    static USER_DEPS: &[&str] = &["platform", "platform_store_relational"];
    let files = collect_test_source_files();
    let meta = RepoMeta {
        name: "platform_store_relational_tests",
        pattern: "meta::relational::tests::.*",
        dependencies: USER_DEPS,
    };
    Repo::Filesystem {
        prefix: "/platform_store_relational_tests".into(),
        files,
        meta: Some(meta),
    }
}

fn build_model() -> PureModel {
    let mut repos = Repo::default_with_build_snapshots();
    assert!(
        repos.iter().any(|r| r
            .meta()
            .is_some_and(|m| m.name == "platform_store_relational")),
        "platform_store_relational repo missing — relational tests need the build snapshots dir set",
    );
    repos.push(test_repo());
    match repo::load(&repos, &auto_imports()) {
        Ok(m) => m,
        Err(partial) => {
            for e in partial.errors.iter().take(20) {
                eprintln!("compile error: {e}");
            }
            panic!(
                "Pure test sources failed to compile: {} error(s)",
                partial.errors.len()
            );
        }
    }
}

fn run_test_fn(model: &PureModel, mangled_fn: &str) -> Result<Value, String> {
    let ext = RelationalStoreExtension;
    let registry = NativeRegistry::with_extensions(&[&ext]);
    let mut eval = Evaluator::new(model, &registry);
    let fn_id = model
        .resolve_by_fqn(&[
            "meta".into(),
            "relational".into(),
            "tests".into(),
            mangled_fn.into(),
        ])
        .ok_or_else(|| format!("meta::relational::tests::{mangled_fn} not found"))?;
    eval.call_user_function_by_id(fn_id)
        .map_err(|e| e.to_string())
}

#[test]
fn pure_tests_compile() {
    // Compilation is non-trivial: it validates the .pure files
    // (syntax, references, type checks) against the live platform.
    let _ = build_model();
}

#[test]
fn execute_in_db_huge_int_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInDbHugeInt__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn execute_in_db_exposes_column_name_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInDbExposesColumnName__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn execute_in_db_varchar_roundtrip_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInDbVarcharRoundtrip__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn execute_in_db_rejects_non_duckdb_type_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInDbRejectsNonDuckDBType__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn create_temp_table_lifecycle_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testCreateTempTableLifecycle__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn create_temp_table_with_rely_on_finally_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testCreateTempTableWithRelyOnFinally__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}
