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

//! `meta::relational::metamodel::execute::loadCsvToDbTable`.
//!
//! Loads a CSV file's rows into an *existing* relational table. The
//! engine-specific SQL (DuckDB's `read_csv_auto` vs H2's `CSVREAD`) is
//! owned by the [`RelationalBackend::load_csv_sql`] trait method, so
//! this native just resolves the backend and asks it for the SQL.
//!
//! [`RelationalBackend::load_csv_sql`]: crate::dispatch::RelationalBackend::load_csv_sql

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;

use crate::dispatch::resolve_backend;

/// `loadCsvToDbTable(filePath, table, dbConn, numberOfRows[0..1]) -> Nil`.
#[derive(Debug)]
pub struct LoadCsvToDbTable;

impl NativeFunction for LoadCsvToDbTable {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("loadCsvToDbTable", args, 4)?;
        let path = ctx
            .evaluate(&args[0])?
            .into_value()
            .as_string()
            .map_err(PureException::from)?
            .to_string();
        let table_value = ctx.evaluate(&args[1])?.into_value();
        let db_conn = ctx.evaluate(&args[2])?.into_value();
        let row_limit_value = ctx.evaluate(&args[3])?.into_value();

        let backend = resolve_backend("loadCsvToDbTable", &db_conn, ctx)?;
        let table_name = read_table_name(&table_value, ctx)?;
        let row_limit = first_integer_or_none(&row_limit_value);
        let sql = backend.load_csv_sql(&table_name, &path, row_limit);
        backend.execute_batch(ctx, &sql)?;
        Ok(Evaluated::new(Value::Unit))
    }

    fn signature(&self) -> &'static str {
        "loadCsvToDbTable(String[1], Table[1], DatabaseConnection[1], Integer[0..1]): Nil[0]"
    }
}

/// Pull the table's `name` slot off a `Table` heap object.
fn read_table_name(
    table_value: &Value,
    ctx: &dyn EvalContextTrait,
) -> Result<String, PureException> {
    let Value::Object(handle) = table_value else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "loadCsvToDbTable: expected a Table object, got {}",
            table_value.type_name()
        ))
        .into());
    };
    let names = ctx
        .heap()
        .get_property_values(handle, "name")
        .map_err(PureException::from)?;
    let name = names
        .front()
        .and_then(|v| v.as_string().ok())
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "loadCsvToDbTable: Table.name is missing or not a String".into(),
            ))
        })?;
    Ok(name.to_string())
}

/// Drain a `[0..1]` Integer slot, returning `Some(n)` only on a single
/// concrete `Value::Integer(n)`.
fn first_integer_or_none(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(n) => Some(*n),
        Value::Collection(items) => items.front().and_then(|item| {
            if let Value::Integer(n) = item {
                Some(*n)
            } else {
                None
            }
        }),
        _ => None,
    }
}
