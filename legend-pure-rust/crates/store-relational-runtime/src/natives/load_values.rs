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

//! `meta::relational::metamodel::execute::loadValuesToDbTable` (×2 overloads).
//!
//! Inserts rows from a Pure-side nested list (`List<List<Any>>`) into an
//! *existing* DuckDB table. The single-element vs many-element overloads
//! share an implementation — they differ only in how the wrapper list
//! reaches the native (single object vs `Value::Collection` of objects).

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;

use crate::connection::DuckDBState;
use crate::dispatch::require_duckdb;

/// `loadValuesToDbTable(tableData:List<List<Any>>[*|1], table, dbConn) -> Nil`.
///
/// One impl serves both overloads; they share one body and one Rust type.
#[derive(Debug)]
pub struct LoadValuesToDbTable;

impl NativeFunction for LoadValuesToDbTable {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("loadValuesToDbTable", args, 3)?;
        let table_data = ctx.evaluate(&args[0])?.into_value();
        let table_value = ctx.evaluate(&args[1])?.into_value();
        let db_conn = ctx.evaluate(&args[2])?.into_value();

        require_duckdb("loadValuesToDbTable", &db_conn, ctx)?;
        let table_name = read_table_name("loadValuesToDbTable", &table_value, ctx)?;

        // Walk: tableData → outer List heap object(s) → row List heap objects → cells.
        let row_value_lists = collect_row_value_lists(&table_data, ctx)?;
        if row_value_lists.is_empty() {
            return Ok(Evaluated::new(Value::Unit));
        }

        let mut sql = format!(
            "INSERT INTO {target} VALUES ",
            target = quote_ident(&table_name)
        );
        let mut first = true;
        for row_cells in &row_value_lists {
            if !first {
                sql.push_str(", ");
            }
            first = false;
            sql.push('(');
            let mut col_first = true;
            for cell in row_cells {
                if !col_first {
                    sql.push_str(", ");
                }
                col_first = false;
                sql.push_str(&value_to_sql_literal(cell));
            }
            sql.push(')');
        }

        let state = ctx
            .extensions()
            .get_or_init::<DuckDBState, _>(DuckDBState::new)?;
        state.with_conn(|c| c.execute_batch(&sql))?;
        Ok(Evaluated::new(Value::Unit))
    }

    fn signature(&self) -> &'static str {
        "loadValuesToDbTable(List<List<Any>>[*|1], Table[1], DatabaseConnection[1]): Nil[0]"
    }
}

/// Read the `name` property off a `Table` heap row.
fn read_table_name(
    native: &str,
    table_value: &Value,
    ctx: &dyn EvalContextTrait,
) -> Result<String, PureException> {
    let Value::Object(handle) = table_value else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native}: expected a Table object, got {}",
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
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "{native}: Table.name is missing or not a String",
            )))
        })?;
    Ok(name.to_string())
}

/// Walk `tableData` (one or many `List<List<Any>>` heap objects) into a
/// nested `Vec<Vec<Value>>` of cells.
fn collect_row_value_lists(
    table_data: &Value,
    ctx: &dyn EvalContextTrait,
) -> Result<Vec<Vec<Value>>, PureException> {
    let mut all_rows: Vec<Vec<Value>> = Vec::new();
    let outer_lists: Vec<&Value> = match table_data {
        Value::Object(_) => vec![table_data],
        Value::Collection(items) => items.iter().collect(),
        Value::Unit => Vec::new(),
        other => {
            return Err(PureRuntimeError::EvaluationError(format!(
                "loadValuesToDbTable: tableData must be a List<List<Any>>, got {}",
                other.type_name()
            ))
            .into());
        }
    };
    for outer in outer_lists {
        let outer_handle = match outer {
            Value::Object(h) => h,
            other => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "loadValuesToDbTable: outer entry is not a List object (got {})",
                    other.type_name()
                ))
                .into());
            }
        };
        let row_lists = ctx
            .heap()
            .get_property_values(outer_handle, "values")
            .map_err(PureException::from)?;
        for row in &row_lists {
            let row_handle = match row {
                Value::Object(h) => h,
                other => {
                    return Err(PureRuntimeError::EvaluationError(format!(
                        "loadValuesToDbTable: row is not a List object (got {})",
                        other.type_name()
                    ))
                    .into());
                }
            };
            let cells = ctx
                .heap()
                .get_property_values(row_handle, "values")
                .map_err(PureException::from)?;
            all_rows.push(cells.iter().cloned().collect());
        }
    }
    Ok(all_rows)
}

/// Render a Pure `Value` as an inline SQL literal.
///
/// Primitive types map cleanly; anything we can't faithfully encode
/// falls back to a quoted string (matching Java Pure's permissive
/// rendering). `Value::Unit` becomes `NULL` so a missing cell still
/// produces a positional `INSERT` slot.
fn value_to_sql_literal(v: &Value) -> String {
    match v {
        Value::Unit => "NULL".to_string(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Decimal(d) => d.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Date(d) => format!("'{d}'"),
        other => format!("'{}'", format!("{other}").replace('\'', "''")),
    }
}

/// DuckDB-safe identifier quoting (double-quotes embedded double-quotes).
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
    use rust_decimal::Decimal;
    use smol_str::SmolStr;

    #[test]
    fn value_to_sql_literal_basic_types() {
        assert_eq!(value_to_sql_literal(&Value::Integer(7)), "7");
        assert_eq!(value_to_sql_literal(&Value::Boolean(true)), "TRUE");
        assert_eq!(value_to_sql_literal(&Value::Unit), "NULL");
        assert_eq!(
            value_to_sql_literal(&Value::String(SmolStr::new("a"))),
            "'a'"
        );
        assert_eq!(
            value_to_sql_literal(&Value::String(SmolStr::new("o'connor"))),
            "'o''connor'"
        );
        assert_eq!(
            value_to_sql_literal(&Value::Decimal(Decimal::new(1234, 2))),
            "12.34"
        );
    }
}
