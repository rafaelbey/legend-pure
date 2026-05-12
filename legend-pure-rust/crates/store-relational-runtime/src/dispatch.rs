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

//! Backend dispatch for relational-store natives.
//!
//! Each native reads `DatabaseConnection.type` from the heap and
//! resolves a [`RelationalBackend`] implementation via
//! [`resolve_backend`]. The native then calls trait methods on the
//! returned `&dyn RelationalBackend` — no `match` on a `Backend` enum
//! in every native body — so adding a new engine is one new
//! `impl RelationalBackend` plus one match arm in [`resolve_backend`].
//!
//! Two backends ship today:
//! * [`DuckDBBackend`] — in-process via the bundled `duckdb` crate.
//! * [`H2Backend`] — Java sub-process via PG-wire + the sync
//!   `postgres` crate (see [`crate::h2_server`]).

use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::native::EvalContextTrait;
use legend_pure_runtime::value::Value;

use crate::connection::{DuckDBState, H2State};
use crate::resultset;

/// Backend SPI implemented by [`DuckDBBackend`] and [`H2Backend`].
///
/// Every relational native goes through trait methods on this rather
/// than a per-native `match` on a backend enum — adding a new engine
/// is one new `impl` plus one arm in [`resolve_backend`].
pub trait RelationalBackend: Send + Sync {
    /// Human-readable engine label, used in error messages.
    fn name(&self) -> &'static str;

    /// Name of the `meta::relational::runtime::DatabaseType` enum
    /// member this backend matches (e.g. `"DuckDB"`, `"H2"`). Used by
    /// `createTempTable` when calling the user's DDL-builder lambda
    /// so the lambda can branch on engine-specific syntax.
    fn database_type_member(&self) -> &'static str;

    /// Run a read-only SQL statement and build a populated `ResultSet`
    /// heap object. Shared backbone of every native that returns a
    /// ResultSet (`executeInDb`, `fetchDb*MetaData`).
    ///
    /// # Errors
    /// Propagates engine-specific errors mapped to `PureException`.
    fn run_sql_to_result_set(
        &self,
        ctx: &mut dyn EvalContextTrait,
        sql: &str,
    ) -> Result<Value, PureException>;

    /// Execute a SQL batch with no result (DDL / DML / multi-statement).
    /// Used by `createTempTable`, `dropTempTable`, `loadCsvToDbTable`,
    /// `loadValuesToDbTable`.
    ///
    /// # Errors
    /// Propagates engine-specific errors mapped to `PureException`.
    fn execute_batch(&self, ctx: &mut dyn EvalContextTrait, sql: &str)
    -> Result<(), PureException>;

    /// Build the SQL that loads `path` (a CSV file with a header row)
    /// into `target_table`, optionally capped at `row_limit`. The SQL
    /// dialect differs between engines (DuckDB's `read_csv_auto` vs
    /// H2's `CSVREAD`), so the trait owns the rendering.
    fn load_csv_sql(&self, target_table: &str, path: &str, row_limit: Option<i64>) -> String;
}

/// DuckDB backend impl.
#[derive(Debug, Default)]
pub struct DuckDBBackend;

/// H2 backend impl (PG-wire compat through a Java sub-process).
#[derive(Debug, Default)]
pub struct H2Backend;

impl RelationalBackend for DuckDBBackend {
    fn name(&self) -> &'static str {
        "DuckDB"
    }
    fn database_type_member(&self) -> &'static str {
        "DuckDB"
    }
    fn run_sql_to_result_set(
        &self,
        ctx: &mut dyn EvalContextTrait,
        sql: &str,
    ) -> Result<Value, PureException> {
        resultset::run_sql_to_result_set_duckdb(ctx, sql)
    }
    fn execute_batch(
        &self,
        ctx: &mut dyn EvalContextTrait,
        sql: &str,
    ) -> Result<(), PureException> {
        let state = ctx
            .extensions()
            .get_or_init::<DuckDBState, _>(DuckDBState::new)?;
        state.with_conn(|c| c.execute_batch(sql))?;
        Ok(())
    }
    fn load_csv_sql(&self, target_table: &str, path: &str, row_limit: Option<i64>) -> String {
        let mut sql = format!(
            "INSERT INTO {target} SELECT * FROM read_csv_auto('{path}', header=true)",
            target = quote_ident(target_table),
            path = escape_sql_string(path),
        );
        if let Some(n) = row_limit {
            use std::fmt::Write;
            let _ = write!(&mut sql, " LIMIT {n}");
        }
        sql
    }
}

impl RelationalBackend for H2Backend {
    fn name(&self) -> &'static str {
        "H2"
    }
    fn database_type_member(&self) -> &'static str {
        "H2"
    }
    fn run_sql_to_result_set(
        &self,
        ctx: &mut dyn EvalContextTrait,
        sql: &str,
    ) -> Result<Value, PureException> {
        resultset::run_sql_to_result_set_h2(ctx, sql)
    }
    fn execute_batch(
        &self,
        ctx: &mut dyn EvalContextTrait,
        sql: &str,
    ) -> Result<(), PureException> {
        let state = ctx
            .extensions()
            .get_or_init::<H2State, _>(H2State::from_global_config)?;
        state.with_client(|c| c.batch_execute(sql))?;
        Ok(())
    }
    fn load_csv_sql(&self, target_table: &str, path: &str, row_limit: Option<i64>) -> String {
        // H2's `CSVREAD` returns a table-valued result with the file's
        // header columns. Wrap in a SELECT … LIMIT to cap rows.
        let inner = format!(
            "SELECT * FROM CSVREAD('{path}')",
            path = escape_sql_string(path),
        );
        let limited = match row_limit {
            Some(n) => format!("{inner} LIMIT {n}"),
            None => inner,
        };
        format!(
            "INSERT INTO {target} {limited}",
            target = quote_ident(target_table)
        )
    }
}

/// Process-wide singleton backend instances. Returning `&'static`
/// lets the trait object survive past the `resolve_backend` call site
/// without lifetime annotations on every consumer.
static DUCKDB_BACKEND: DuckDBBackend = DuckDBBackend;
static H2_BACKEND: H2Backend = H2Backend;

/// Resolve the backend trait object for a native call.
///
/// Reads `DatabaseConnection.type` from the heap and matches the enum
/// member name against the registered backends.
///
/// # Errors
/// Returns a [`PureException`] when:
/// * `connection` is not a heap object, OR
/// * its `type` property is not an `EnumValue` of `DatabaseType`, OR
/// * the enum member matches no registered backend.
pub fn resolve_backend(
    native_name: &str,
    connection: &Value,
    ctx: &dyn EvalContextTrait,
) -> Result<&'static dyn RelationalBackend, PureException> {
    let Value::Object(obj) = connection else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: expected a DatabaseConnection object, got {:?}",
            connection.type_name()
        ))
        .into());
    };
    let type_values = ctx
        .heap()
        .get_property_values(obj, "type")
        .map_err(PureException::from)?;
    let Some(first) = type_values.front() else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseConnection.type is unset; cannot route to a backend"
        ))
        .into());
    };
    match first {
        Value::EnumValue { member, .. } if member.as_str() == "DuckDB" => Ok(&DUCKDB_BACKEND),
        Value::EnumValue { member, .. } if member.as_str() == "H2" => Ok(&H2_BACKEND),
        Value::EnumValue { member, .. } => Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseType.{member} is not supported yet \
             (DuckDB and H2 are the only backends implemented)"
        ))
        .into()),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseConnection.type is not an EnumValue (got {:?})",
            other.type_name()
        ))
        .into()),
    }
}

// ---------------------------------------------------------------------
// Shared SQL helpers (used by trait impls)
// ---------------------------------------------------------------------

/// Double-quote an identifier, escaping embedded quotes by doubling.
/// Both DuckDB and H2 accept this form.
pub(crate) fn quote_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}

/// Single-quote escape for an embedded SQL string literal.
pub(crate) fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}
