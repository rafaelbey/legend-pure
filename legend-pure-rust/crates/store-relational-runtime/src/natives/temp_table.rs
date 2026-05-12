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

//! `meta::relational::metamodel::execute::{createTempTable, dropTempTable}`.
//!
//! `createTempTable` accepts a user-supplied lambda that builds the
//! `CREATE TABLE` DDL given (`tableName`, `columns`, `DatabaseType.DuckDB`).
//! We invoke the lambda via [`legend_pure_runtime::native::EvalContextTrait::call_function`]
//! and run the resulting DDL against the per-Evaluator DuckDB connection.
//!
//! Two overloads share one body via [`do_create_temp_table`]:
//!  * `(name, cols, sql, dbConn)`            — 4 args
//!  * `(name, cols, sql, relyOnFinally, dbConn)` — 5 args; the boolean is
//!    accepted for signature parity but not yet acted on (auto-drop on
//!    function exit needs a teardown hook that doesn't exist yet —
//!    deferred follow-up).

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::m3_paths;
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

use crate::connection::DuckDBState;
use crate::dispatch::{SUPPORTED_BACKEND, require_duckdb};

/// `createTempTable(name, cols, sqlBuilder, dbConn) -> Nil`.
#[derive(Debug)]
pub struct CreateTempTable;

impl NativeFunction for CreateTempTable {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("createTempTable", args, 4)?;
        let name = ctx
            .evaluate(&args[0])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();
        let cols = ctx.evaluate(&args[1])?.into_value();
        let sql_lambda = ctx.evaluate(&args[2])?.into_value();
        let db_conn = ctx.evaluate(&args[3])?.into_value();
        do_create_temp_table(name, cols, sql_lambda, db_conn, false, ctx)
    }

    fn signature(&self) -> &'static str {
        "createTempTable(String[1], Column[*], Function[1], DatabaseConnection[1]): Nil[0]"
    }
}

/// `createTempTable(name, cols, sqlBuilder, relyOnFinally, dbConn) -> Nil`.
///
/// Same body as [`CreateTempTable`]. The `relyOnFinally` boolean is
/// accepted for signature parity but ignored — auto-drop on function
/// exit requires a teardown hook the evaluator does not yet expose,
/// flagged as a follow-up.
#[derive(Debug)]
pub struct CreateTempTableWithFinally;

impl NativeFunction for CreateTempTableWithFinally {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("createTempTable", args, 5)?;
        let name = ctx
            .evaluate(&args[0])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();
        let cols = ctx.evaluate(&args[1])?.into_value();
        let sql_lambda = ctx.evaluate(&args[2])?.into_value();
        let rely_on_finally = ctx
            .evaluate(&args[3])?
            .into_value()
            .as_boolean()
            .map_err(PureException::from)?;
        let db_conn = ctx.evaluate(&args[4])?.into_value();
        do_create_temp_table(name, cols, sql_lambda, db_conn, rely_on_finally, ctx)
    }

    fn signature(&self) -> &'static str {
        "createTempTable(String[1], Column[*], Function[1], Boolean[1], DatabaseConnection[1]): Nil[0]"
    }
}

/// `dropTempTable(name, dbConn) -> Nil`.
#[derive(Debug)]
pub struct DropTempTable;

impl NativeFunction for DropTempTable {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("dropTempTable", args, 2)?;
        let name = ctx
            .evaluate(&args[0])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();
        let db_conn = ctx.evaluate(&args[1])?.into_value();
        require_duckdb("dropTempTable", &db_conn, ctx)?;

        let state = ctx
            .extensions()
            .get_or_init::<DuckDBState, _>(DuckDBState::new)?;
        let sql = format!("DROP TABLE {}", quote_ident(&name));
        state.with_conn(|c| c.execute_batch(&sql))?;
        Ok(Evaluated::new(Value::Unit))
    }

    fn signature(&self) -> &'static str {
        "dropTempTable(String[1], DatabaseConnection[1]): Nil[0]"
    }
}

/// Shared body for both `createTempTable` overloads.
fn do_create_temp_table(
    name: String,
    cols: Value,
    sql_lambda: Value,
    db_conn: Value,
    _rely_on_finally: bool,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Evaluated, PureException> {
    require_duckdb("createTempTable", &db_conn, ctx)?;

    // Materialise the `DatabaseType.DuckDB` enum literal the lambda
    // expects as its third argument.
    let db_type_id = m3_paths::resolve(ctx.model(), m3_paths::DATABASE_TYPE).ok_or_else(|| {
        PureException::from(PureRuntimeError::EvaluationError(
            "createTempTable: meta::relational::runtime::DatabaseType not resolvable".into(),
        ))
    })?;
    let db_type_arg = Value::EnumValue {
        enum_id: db_type_id,
        member: SmolStr::new(SUPPORTED_BACKEND),
    };

    let ddl = ctx
        .call_function(
            &sql_lambda,
            &[Value::String(SmolStr::new(&name)), cols, db_type_arg],
        )?
        .as_string()
        .map_err(PureException::from)?
        .to_string();

    let state = ctx
        .extensions()
        .get_or_init::<DuckDBState, _>(DuckDBState::new)?;
    state.with_conn(|c| c.execute_batch(&ddl))?;
    Ok(Evaluated::new(Value::Unit))
}

/// DuckDB-safe identifier quoting: double-quotes embedded double-quotes,
/// wraps in double-quotes. Defensive against names with hyphens or
/// embedded quotes; cheap enough to run unconditionally.
fn quote_ident(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_ident_simple() {
        assert_eq!(quote_ident("foo"), r#""foo""#);
    }

    #[test]
    fn quote_ident_embedded_quote_is_doubled() {
        // 'fo"o' → "fo""o"
        assert_eq!(quote_ident(r#"fo"o"#), r#""fo""o""#);
    }
}
