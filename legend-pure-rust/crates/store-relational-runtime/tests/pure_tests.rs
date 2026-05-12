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

//! Runs the Pure `<<test.Test>>` functions that ship with the
//! `platform_store_relational` repo under
//! `legend-pure-m2-store-relational-pure/src/main/resources/
//!  platform_store_relational/tests/`.
//!
//! These tests are the canonical end-to-end coverage for the relational
//! native bodies — they ship with the platform itself (mirroring the
//! convention used by `meta::pure::functions::asserts::testSimpleAssertError`
//! and its peers under `legend-pure-core/.../assertError.pure`). Both
//! the Java surveyor and the Rust integration runner discover them
//! through the standard repo cascade.
//
// `PureException` is intentionally rich (call stack), so the
// `Result<T, PureException>` types lighting up `result_large_err` are
// allowed crate-wide here too.
#![allow(clippy::result_large_err)]

use legend_pure_core_platform::repo;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::value::Value;
use legend_pure_store_relational_runtime::RelationalStoreExtension;
use smol_str::SmolStr;

fn auto_imports() -> Vec<SmolStr> {
    legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect()
}

fn build_model() -> PureModel {
    let repos = repo::Repo::default_with_build_snapshots();
    assert!(
        repos.iter().any(|r| r
            .meta()
            .is_some_and(|m| m.name == "platform_store_relational")),
        "platform_store_relational repo missing — relational tests need the build snapshots dir set",
    );
    match repo::load(&repos, &auto_imports()) {
        Ok(m) => m,
        Err(partial) => {
            for e in partial.errors.iter().take(20) {
                eprintln!("compile error: {e}");
            }
            panic!(
                "platform compile failed: {} error(s) — see stderr above",
                partial.errors.len()
            );
        }
    }
}

fn run_test_fn(model: &PureModel, mangled_fn: &str) -> Result<Value, String> {
    let ext = RelationalStoreExtension;
    let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
    let db_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
    let cm_pop = legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
    let mut eval = Evaluator::new_default_with_extensions_and_populators(
        model,
        &[&ext],
        &[&mapping_pop, &db_pop, &cm_pop],
    );
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

// The DuckDB-only routing assertion lives in `duckdb_smoke.rs` (it's a
// Rust-port-specific contract, not a behaviour both stacks share).

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

#[test]
fn load_values_to_db_table_inserts_rows_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testLoadValuesToDbTableInsertsRows__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

// H2-targeted Pure tests. These require `LEGEND_PURE_H2_JAR` (or
// `[extension.relational.h2]` in legend-pure-classpath.toml) at run
// time. There is intentionally no skip-on-missing-jar guard here —
// the platform root surveyor also runs these and demands strict
// pass, so the configuration is mandatory for this repo.

#[test]
fn execute_in_h2_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInH2__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn execute_in_h2_varchar_roundtrip_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testExecuteInH2VarcharRoundtrip__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}

#[test]
fn create_temp_table_in_h2_passes() {
    let model = build_model();
    let v = run_test_fn(&model, "testCreateTempTableInH2__Boolean_1_").expect("ran");
    assert_eq!(v, Value::Boolean(true));
}
