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

//! End-to-end DuckDB smoke tests for the relational-store native bodies.
//!
//! Each test compiles a synthetic user Pure source alongside the
//! platform + `platform_store_relational` repos, builds an `Evaluator`
//! with [`RelationalStoreExtension`] wired in, and invokes the test
//! function by mangled FQN.
//
// `PureException` is intentionally rich (call stack) so the
// `Result<T, PureException>` types used by extension callbacks light up
// `clippy::result_large_err`. Mirror the lib-level allow here.
#![allow(clippy::result_large_err)]

use legend_pure_core_platform::repo;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use legend_pure_store_relational_runtime::RelationalStoreExtension;
use smol_str::SmolStr;

fn auto_imports() -> Vec<SmolStr> {
    legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect()
}

/// Build a synthetic user-source repo that depends on every standard
/// repo (including `platform_store_relational`).
fn user_repo(content: &str) -> repo::Repo {
    use repo::{OwnedSourceFile, RepoMeta};
    static USER_DEPS: &[&str] = &[
        "platform",
        "platform_precise_primitives",
        "platform_dsl_store",
        "platform_dsl_mapping",
        "platform_dsl_diagram",
        "platform_dsl_graph",
        "platform_dsl_tds",
        "platform_dsl_path",
        "platform_store_relational",
    ];
    let meta = RepoMeta {
        name: "user_test",
        pattern: ".*",
        dependencies: USER_DEPS,
    };
    repo::Repo::Filesystem {
        prefix: "/user_test".into(),
        files: vec![OwnedSourceFile {
            path: "/user_test/test_source.pure".into(),
            content: content.into(),
        }],
        meta: Some(meta),
    }
}

/// Compile a synthetic Pure source against the platform model and run
/// it through an `Evaluator` that has the relational extension wired in.
fn run_pure_function(source: &str, mangled_fn: &str) -> Value {
    let mut repos = repo::Repo::default_with_build_snapshots();
    assert!(
        repos.iter().any(|r| r
            .meta()
            .is_some_and(|m| m.name == "platform_store_relational")),
        "platform_store_relational repo missing — relational tests need LEGEND_PURE_BUILD_SNAPSHOTS_DIR \
         (or `cargo build` from the workspace root which sets it via the build script)",
    );
    repos.push(user_repo(source));
    let model: PureModel = match repo::load(&repos, &auto_imports()) {
        Ok(m) => m,
        Err(partial) => {
            for e in partial.errors.iter().take(10) {
                eprintln!("compile error: {e}");
            }
            panic!(
                "compile_with_platform: {} error(s) — see stderr above",
                partial.errors.len()
            );
        }
    };

    let ext = RelationalStoreExtension;
    let registry = NativeRegistry::with_extensions(&[&ext]);
    let mut eval = Evaluator::new(&model, &registry);

    let fn_id = model
        .resolve_by_fqn(&["user_test".into(), mangled_fn.into()])
        .unwrap_or_else(|| panic!("user_test::{mangled_fn} not found in model"));
    eval.call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("evaluation error: {e}"))
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

const SELECT_SUM: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::sum_2_plus_2(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    let rs = executeInDb('select 2+2 as v', $db, 0, 1000);
    $rs.rows->at(0).values->at(0)->cast(@Integer);
}
";

#[test]
fn execute_in_db_simple_select_returns_integer_result() {
    let v = run_pure_function(SELECT_SUM, "sum_2_plus_2__Integer_1_");
    assert_eq!(v, Value::Integer(4));
}

const COLUMN_NAMES: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::col_name(): String[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    let rs = executeInDb('select 1 as alpha', $db, 0, 1000);
    $rs.columnNames->at(0);
}
";

#[test]
fn execute_in_db_exposes_column_names() {
    let v = run_pure_function(COLUMN_NAMES, "col_name__String_1_");
    assert_eq!(v, Value::String(SmolStr::new("alpha")));
}

const VARCHAR_ROUNDTRIP: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::varchar_round_trip(): String[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    let rs = executeInDb('select \'hello\' as s', $db, 0, 1000);
    $rs.rows->at(0).values->at(0)->cast(@String);
}
";

#[test]
fn execute_in_db_returns_varchar_as_string() {
    let v = run_pure_function(VARCHAR_ROUNDTRIP, "varchar_round_trip__String_1_");
    assert_eq!(v, Value::String(SmolStr::new("hello")));
}

const TWO_EVALS_INDEPENDENT: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::row_count_in_isolated_db(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    // Two Evaluators get independent DuckDB connections, so a table
    // created in another evaluator's DB must not exist here.
    let rs = executeInDb(
        'select count(*) as n from duckdb_tables() where table_name = \'leak\'',
        $db, 0, 1000
    );
    $rs.rows->at(0).values->at(0)->cast(@Integer);
}
";

#[test]
fn per_evaluator_state_isolates_connections() {
    // Pre-arm: create a table in a separate evaluator, then verify a
    // fresh evaluator (different one) does not see it.
    let create_source = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::create_leak(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    executeInDb('CREATE TABLE leak(x INTEGER)', $db, 0, 1000);
    let rs = executeInDb(
        'select count(*) as n from duckdb_tables() where table_name = \'leak\'',
        $db, 0, 1000
    );
    $rs.rows->at(0).values->at(0)->cast(@Integer);
}
";
    let in_creator = run_pure_function(create_source, "create_leak__Integer_1_");
    assert_eq!(in_creator, Value::Integer(1), "creator sees its own table");

    // Fresh evaluator (different `run_pure_function` invocation = different `Evaluator`)
    let in_other = run_pure_function(
        TWO_EVALS_INDEPENDENT,
        "row_count_in_isolated_db__Integer_1_",
    );
    assert_eq!(
        in_other,
        Value::Integer(0),
        "fresh Evaluator must not see the previous Evaluator's table"
    );
}

const REJECTS_NON_DUCKDB: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::execute_in_h2(): Boolean[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.H2);
    assertError(
        | executeInDb('select 1', $db, 0, 1000),
        'executeInDb: DatabaseType.H2 is not supported yet (only DuckDB is implemented; H2/Postgres/etc. coming later)'
    );
}
";

// The Rust port routes only DatabaseType.DuckDB to a real backend; any
// other enum member must surface a deterministic not-implemented error
// (so failures explain themselves at the call site instead of producing
// engine-specific SQL errors). This contract is Rust-port-specific —
// the Java stack supports H2 et al. — so the assertion lives here as a
// Rust integration test rather than as a Pure `<<test.Test>>` function
// that both surveyors would discover.
#[test]
fn execute_in_db_rejects_non_duckdb_type() {
    let v = run_pure_function(REJECTS_NON_DUCKDB, "execute_in_h2__Boolean_1_");
    assert_eq!(v, Value::Boolean(true));
}

const CREATE_TEMP_TABLE: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::ddl_builder(name: String[1], cols: meta::relational::metamodel::Column[*], dbType: DatabaseType[1]): String[1]
{
    'CREATE TABLE ' + $name + ' (x INTEGER)';
}

function user_test::create_and_query_temp_table(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    createTempTable('tmp_t', [], user_test::ddl_builder_String_1__Column_MANY__DatabaseType_1__String_1_, $db);
    executeInDb('INSERT INTO tmp_t VALUES (7), (8), (9)', $db, 0, 1000);
    let rs = executeInDb('SELECT count(*) FROM tmp_t', $db, 0, 1000);
    let n = $rs.rows->at(0).values->at(0)->cast(@Integer);
    dropTempTable('tmp_t', $db);
    $n;
}
";

#[test]
fn create_temp_table_runs_lambda_ddl_and_dropdrops_it() {
    let v = run_pure_function(CREATE_TEMP_TABLE, "create_and_query_temp_table__Integer_1_");
    assert_eq!(v, Value::Integer(3));
}

const DROP_NON_EXISTENT_ERRORS: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::drop_non_existent(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    dropTempTable('never_existed', $db);
    0;
}
";

#[test]
fn drop_temp_table_on_unknown_name_errors() {
    let mut repos = repo::Repo::default_with_build_snapshots();
    repos.push(user_repo(DROP_NON_EXISTENT_ERRORS));
    let model: PureModel = match repo::load(&repos, &auto_imports()) {
        Ok(m) => m,
        Err(partial) => panic!("compile error: {} error(s)", partial.errors.len()),
    };
    let ext = RelationalStoreExtension;
    let registry = NativeRegistry::with_extensions(&[&ext]);
    let mut eval = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["user_test".into(), "drop_non_existent__Integer_1_".into()])
        .expect("resolve");
    let err = eval
        .call_user_function_by_id(fn_id)
        .expect_err("dropping a non-existent table must error");
    let msg = err.to_string();
    assert!(
        msg.contains("never_existed") || msg.to_lowercase().contains("does not exist"),
        "expected DuckDB to flag the missing table; got: {msg}",
    );
}

// ---------------------------------------------------------------------
// fetchDb*MetaData
// ---------------------------------------------------------------------

const FETCH_TABLES: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::tables_md_row_count(): Integer[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    executeInDb('CREATE TABLE t_meta_one (x INTEGER)', $db, 0, 1000);
    executeInDb('CREATE TABLE t_meta_two (y INTEGER)', $db, 0, 1000);
    let rs = fetchDbTablesMetaData($db, [], 't_meta_%');
    $rs.rows->size()->toOne();
}
";

#[test]
fn fetch_db_tables_metadata_returns_matching_rows() {
    let v = run_pure_function(FETCH_TABLES, "tables_md_row_count__Integer_1_");
    assert_eq!(v, Value::Integer(2));
}

const FETCH_COLUMNS: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::columns_md_first_col_name(): String[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    executeInDb('CREATE TABLE t_cols (alpha INTEGER, beta VARCHAR)', $db, 0, 1000);
    let rs = fetchDbColumnsMetaData($db, [], 't_cols', []);
    // Pull COLUMN_NAME via the named-cell accessor on Row.
    $rs.rows->at(0).value('COLUMN_NAME')->cast(@String);
}
";

#[test]
fn fetch_db_columns_metadata_returns_columns_by_name() {
    let v = run_pure_function(FETCH_COLUMNS, "columns_md_first_col_name__String_1_");
    assert_eq!(v, Value::String(SmolStr::new("alpha")));
}

const FETCH_SCHEMAS: &str = r"
import meta::external::store::relational::runtime::*;
import meta::relational::runtime::*;
import meta::relational::metamodel::execute::*;

function user_test::first_main_schema_name(): String[1]
{
    let db = ^TestDatabaseConnection(type = DatabaseType.DuckDB);
    // DuckDB reports one `main` per attached catalog (memory, system, ...),
    // so we just confirm the projection works and returns at least one row
    // named `main`.
    let rs = fetchDbSchemasMetaData($db, 'main');
    $rs.rows->at(0).value('TABLE_SCHEM')->cast(@String);
}
";

#[test]
fn fetch_db_schemas_metadata_finds_main() {
    let v = run_pure_function(FETCH_SCHEMAS, "first_main_schema_name__String_1_");
    assert_eq!(v, Value::String(SmolStr::new("main")));
}

// `loadCsvToDbTable` and `loadValuesToDbTable` need a real `Table` heap
// object whose `schema` (and Schema's `database`) chain is populated.
// That's most naturally produced by a `###Relational` Database
// declaration, which in turn needs the `RelationalDatabaseDSLPopulator`
// running on the evaluator's heap. Wiring that here is doable but
// awkward; the natural home for end-to-end coverage of these two
// natives is the Pure `<<test.Test>>` functions in
// `platform_store_relational_tests/load_values.pure` (commit 6), which
// the surveyor-driven test harness will exercise once the test repo is
// in place. Until then, the value-rendering pipeline is locked by the
// lib unit tests in `load_values.rs`.
