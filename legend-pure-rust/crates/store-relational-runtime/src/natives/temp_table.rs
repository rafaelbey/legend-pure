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
//! `createTempTable` invokes a user-supplied lambda that returns the
//! DDL string, given (`tableName`, `columns`, `DatabaseType`). The
//! resolved backend's `database_type_member()` feeds the third
//! argument so lambdas can branch on engine-specific syntax (e.g.
//! DuckDB's `CREATE TABLE` vs H2's `CREATE LOCAL TEMPORARY TABLE`).
//!
//! Two overloads share one body via [`do_create_temp_table`]:
//!  * `(name, cols, sql, dbConn)`            — 4 args
//!  * `(name, cols, sql, relyOnFinally, dbConn)` — 5 args; the boolean
//!    is accepted for signature parity but not yet acted on (auto-
//!    drop on function exit needs a teardown hook the evaluator
//!    doesn't expose yet — tracked in BACKLOG).

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::m3_paths;
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

use crate::dispatch::{quote_ident, resolve_backend};

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
}

/// `createTempTable(name, cols, sqlBuilder, relyOnFinally, dbConn) -> Nil`.
///
/// Same body as [`CreateTempTable`]. The `relyOnFinally` boolean is
/// accepted for signature parity but ignored — auto-drop on function
/// exit requires a teardown hook the evaluator does not yet expose.
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
        let backend = resolve_backend("dropTempTable", &db_conn, ctx)?;
        let sql = format!("DROP TABLE {}", quote_ident(&name));
        backend.execute_batch(ctx, &sql)?;
        Ok(Evaluated::new(Value::Unit))
    }
}

/// Shared body for both `createTempTable` overloads.
///
/// Invokes the user's DDL-builder lambda with the table name, columns,
/// and the `DatabaseType` enum value matching the resolved backend
/// — so user code can branch on engine-specific DDL syntax. Then runs
/// the returned DDL through the trait's `execute_batch`.
fn do_create_temp_table(
    name: String,
    cols: Value,
    sql_lambda: Value,
    db_conn: Value,
    _rely_on_finally: bool,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Evaluated, PureException> {
    let backend = resolve_backend("createTempTable", &db_conn, ctx)?;
    let member = backend.database_type_member();

    let db_type_id = m3_paths::resolve(ctx.model(), m3_paths::DATABASE_TYPE).ok_or_else(|| {
        PureException::from(PureRuntimeError::EvaluationError(
            "createTempTable: meta::relational::runtime::DatabaseType not resolvable".into(),
        ))
    })?;
    let db_type_arg = Value::EnumValue {
        enum_id: db_type_id,
        member: SmolStr::new(member),
    };

    let ddl = ctx
        .call_function(
            &sql_lambda,
            &[Value::String(SmolStr::new(&name)), cols, db_type_arg],
        )?
        .as_string()
        .map_err(PureException::from)?
        .to_string();

    backend.execute_batch(ctx, &ddl)?;
    Ok(Evaluated::new(Value::Unit))
}
