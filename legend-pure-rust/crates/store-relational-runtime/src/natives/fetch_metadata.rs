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

//! `meta::relational::metamodel::execute::fetchDb*MetaData` — 5
//! introspection natives backed by DuckDB's `information_schema`.
//!
//! Each native runs a `SELECT … FROM information_schema.<view> WHERE …`
//! query with the supplied wildcard patterns (`[0..1] String`) applied
//! as `LIKE` filters, and returns the result as a populated `ResultSet`.
//! The column **names** in the returned ResultSet are exactly the
//! `information_schema` column names (uppercase per SQL spec) — the
//! upstream JDBC `DatabaseMetaData.getTables` etc. methods return the
//! same names, so test assertions written against either engine
//! transfer over.

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;

use std::fmt::Write;

use crate::dispatch::resolve_backend;

// ---------------------------------------------------------------------
// fetchDbTablesMetaData
// ---------------------------------------------------------------------

/// `fetchDbTablesMetaData(dbConn, schemaPattern[0..1], tablePattern[0..1]) -> ResultSet`.
#[derive(Debug)]
pub struct FetchDbTablesMetaData;

impl NativeFunction for FetchDbTablesMetaData {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fetchDbTablesMetaData", args, 3)?;
        let db_conn = ctx.evaluate(&args[0])?.into_value();
        let schema_pattern = optional_string(ctx.evaluate(&args[1])?.into_value())?;
        let table_pattern = optional_string(ctx.evaluate(&args[2])?.into_value())?;

        let mut sql = String::from(
            "SELECT table_catalog AS TABLE_CAT, \
                    table_schema  AS TABLE_SCHEM, \
                    table_name    AS TABLE_NAME, \
                    table_type    AS TABLE_TYPE \
             FROM information_schema.tables",
        );
        let mut wheres: Vec<String> = Vec::new();
        if let Some(p) = &schema_pattern {
            wheres.push(format!("table_schema LIKE '{}'", escape_sql_string(p)));
        }
        if let Some(p) = &table_pattern {
            wheres.push(format!("table_name LIKE '{}'", escape_sql_string(p)));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY table_schema, table_name");
        let backend = resolve_backend("fetchDbTablesMetaData", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "fetchDbTablesMetaData(DatabaseConnection[1], String[0..1], String[0..1]): ResultSet[1]"
    }
}

// ---------------------------------------------------------------------
// fetchDbColumnsMetaData
// ---------------------------------------------------------------------

/// `fetchDbColumnsMetaData(dbConn, schemaPattern[0..1], tablePattern[0..1], columnPattern[0..1]) -> ResultSet`.
#[derive(Debug)]
pub struct FetchDbColumnsMetaData;

impl NativeFunction for FetchDbColumnsMetaData {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fetchDbColumnsMetaData", args, 4)?;
        let db_conn = ctx.evaluate(&args[0])?.into_value();
        let schema_pattern = optional_string(ctx.evaluate(&args[1])?.into_value())?;
        let table_pattern = optional_string(ctx.evaluate(&args[2])?.into_value())?;
        let column_pattern = optional_string(ctx.evaluate(&args[3])?.into_value())?;

        let mut sql = String::from(
            "SELECT table_catalog          AS TABLE_CAT, \
                    table_schema           AS TABLE_SCHEM, \
                    table_name             AS TABLE_NAME, \
                    column_name            AS COLUMN_NAME, \
                    data_type              AS TYPE_NAME, \
                    ordinal_position       AS ORDINAL_POSITION, \
                    is_nullable            AS IS_NULLABLE, \
                    column_default         AS COLUMN_DEF \
             FROM information_schema.columns",
        );
        let mut wheres: Vec<String> = Vec::new();
        if let Some(p) = &schema_pattern {
            wheres.push(format!("table_schema LIKE '{}'", escape_sql_string(p)));
        }
        if let Some(p) = &table_pattern {
            wheres.push(format!("table_name LIKE '{}'", escape_sql_string(p)));
        }
        if let Some(p) = &column_pattern {
            wheres.push(format!("column_name LIKE '{}'", escape_sql_string(p)));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY table_schema, table_name, ordinal_position");
        let backend = resolve_backend("fetchDbColumnsMetaData", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "fetchDbColumnsMetaData(DatabaseConnection[1], String[0..1], String[0..1], String[0..1]): ResultSet[1]"
    }
}

// ---------------------------------------------------------------------
// fetchDbSchemasMetaData
// ---------------------------------------------------------------------

/// `fetchDbSchemasMetaData(dbConn, schemaPattern[0..1]) -> ResultSet`.
#[derive(Debug)]
pub struct FetchDbSchemasMetaData;

impl NativeFunction for FetchDbSchemasMetaData {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fetchDbSchemasMetaData", args, 2)?;
        let db_conn = ctx.evaluate(&args[0])?.into_value();
        let schema_pattern = optional_string(ctx.evaluate(&args[1])?.into_value())?;

        let mut sql = String::from(
            "SELECT schema_name  AS TABLE_SCHEM, \
                    catalog_name AS TABLE_CATALOG \
             FROM information_schema.schemata",
        );
        if let Some(p) = &schema_pattern {
            let _ = write!(
                &mut sql,
                " WHERE schema_name LIKE '{}'",
                escape_sql_string(p)
            );
        }
        sql.push_str(" ORDER BY schema_name");
        let backend = resolve_backend("fetchDbSchemasMetaData", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "fetchDbSchemasMetaData(DatabaseConnection[1], String[0..1]): ResultSet[1]"
    }
}

// ---------------------------------------------------------------------
// fetchDbPrimaryKeysMetaData
// ---------------------------------------------------------------------

/// `fetchDbPrimaryKeysMetaData(dbConn, schemaPattern[0..1], tableName) -> ResultSet`.
#[derive(Debug)]
pub struct FetchDbPrimaryKeysMetaData;

impl NativeFunction for FetchDbPrimaryKeysMetaData {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fetchDbPrimaryKeysMetaData", args, 3)?;
        let db_conn = ctx.evaluate(&args[0])?.into_value();
        let schema_pattern = optional_string(ctx.evaluate(&args[1])?.into_value())?;
        let table_name = ctx
            .evaluate(&args[2])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();

        // Primary-key columns: join table_constraints (constraint_type =
        // 'PRIMARY KEY') with key_column_usage to get per-column positions.
        let mut sql = format!(
            "SELECT kcu.table_catalog       AS TABLE_CAT, \
                    kcu.table_schema        AS TABLE_SCHEM, \
                    kcu.table_name          AS TABLE_NAME, \
                    kcu.column_name         AS COLUMN_NAME, \
                    kcu.ordinal_position    AS KEY_SEQ, \
                    tc.constraint_name      AS PK_NAME \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_schema = tc.constraint_schema \
              AND kcu.constraint_name   = tc.constraint_name \
              AND kcu.table_name        = tc.table_name \
             WHERE tc.constraint_type = 'PRIMARY KEY' \
               AND tc.table_name LIKE '{}'",
            escape_sql_string(&table_name)
        );
        if let Some(p) = &schema_pattern {
            let _ = write!(
                &mut sql,
                " AND tc.table_schema LIKE '{}'",
                escape_sql_string(p)
            );
        }
        sql.push_str(" ORDER BY kcu.ordinal_position");
        let backend = resolve_backend("fetchDbPrimaryKeysMetaData", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "fetchDbPrimaryKeysMetaData(DatabaseConnection[1], String[0..1], String[1]): ResultSet[1]"
    }
}

// ---------------------------------------------------------------------
// fetchDbImportedKeysMetaData
// ---------------------------------------------------------------------

/// `fetchDbImportedKeysMetaData(dbConn, schemaPattern[0..1], tableName) -> ResultSet`.
///
/// Lists foreign-key relationships *into* `tableName` — i.e. rows where
/// the referenced table is the named one. Mirrors JDBC's
/// `DatabaseMetaData.getImportedKeys` column shape, projected from
/// DuckDB's `information_schema.referential_constraints` +
/// `key_column_usage`.
#[derive(Debug)]
pub struct FetchDbImportedKeysMetaData;

impl NativeFunction for FetchDbImportedKeysMetaData {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fetchDbImportedKeysMetaData", args, 3)?;
        let db_conn = ctx.evaluate(&args[0])?.into_value();
        let schema_pattern = optional_string(ctx.evaluate(&args[1])?.into_value())?;
        let table_name = ctx
            .evaluate(&args[2])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();

        let mut sql = format!(
            "SELECT kcu.table_catalog       AS FKTABLE_CAT, \
                    kcu.table_schema        AS FKTABLE_SCHEM, \
                    kcu.table_name          AS FKTABLE_NAME, \
                    kcu.column_name         AS FKCOLUMN_NAME, \
                    kcu.ordinal_position    AS KEY_SEQ, \
                    rc.constraint_name      AS FK_NAME, \
                    rc.unique_constraint_schema AS PKTABLE_SCHEM, \
                    rc.unique_constraint_name   AS PK_NAME, \
                    rc.update_rule          AS UPDATE_RULE, \
                    rc.delete_rule          AS DELETE_RULE \
             FROM information_schema.referential_constraints rc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_schema = rc.constraint_schema \
              AND kcu.constraint_name   = rc.constraint_name \
             WHERE kcu.table_name LIKE '{}'",
            escape_sql_string(&table_name)
        );
        if let Some(p) = &schema_pattern {
            let _ = write!(
                &mut sql,
                " AND kcu.table_schema LIKE '{}'",
                escape_sql_string(p)
            );
        }
        sql.push_str(" ORDER BY kcu.ordinal_position");
        let backend = resolve_backend("fetchDbImportedKeysMetaData", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "fetchDbImportedKeysMetaData(DatabaseConnection[1], String[0..1], String[1]): ResultSet[1]"
    }
}

// ---------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------

/// Extract a `[0..1]` `String` pattern. `Value::Unit`, empty collection,
/// and an absent slot all yield `None`.
fn optional_string(v: Value) -> Result<Option<String>, PureException> {
    match v {
        Value::Unit => Ok(None),
        Value::String(s) => Ok(Some(s.to_string())),
        Value::Collection(items) => match items.front() {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.to_string())),
            Some(other) => Err(
                legend_pure_runtime::error::PureRuntimeError::EvaluationError(format!(
                    "expected String[0..1], got {}",
                    other.type_name()
                ))
                .into(),
            ),
        },
        other => Err(
            legend_pure_runtime::error::PureRuntimeError::EvaluationError(format!(
                "expected String[0..1], got {}",
                other.type_name()
            ))
            .into(),
        ),
    }
}

/// Single-quote escape for an embedded SQL string literal.
fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}
