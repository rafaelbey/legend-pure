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

//! Marshal a DuckDB result set into a Pure
//! `meta::relational::metamodel::execute::ResultSet` heap object.
//!
//! The DuckDB → Pure cell mapping mirrors Java Pure's
//! `ExecuteInDb.createPureResultSetFromDatabaseResultSet` (lines
//! 247-429) so a Pure program reading
//! `$rs.rows->at(0).value('col')` sees the same Value variant across
//! both stacks. Anything we don't (yet) have a typed Pure value for
//! degrades to `Value::String` of `{:?}`, mirroring Java's `Types.OTHER`
//! branch.

use duckdb::types::{TimeUnit, ValueRef};
use legend_pure_runtime::date::PureDate;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::m3_paths;
use legend_pure_runtime::native::EvalContextTrait;
use legend_pure_runtime::value::Value;
use rust_decimal::Decimal;
use smol_str::SmolStr;

use crate::connection::{DuckDBState, H2State};

/// Run a read-only SQL statement against the per-Evaluator DuckDB
/// connection and build a populated `ResultSet` heap object.
///
/// Shared backend for the DuckDB arm of every native that returns a
/// `ResultSet` (`executeInDb`, `fetchDb*MetaData`). Keeping it on one
/// code path keeps the DuckDB→Pure marshalling decisions in one place.
///
/// # Errors
/// Propagates DuckDB errors (mapped to [`PureException`] via
/// [`crate::connection::map_duckdb_err`]) and heap-mutation errors.
pub fn run_sql_to_result_set_duckdb(
    ctx: &mut dyn EvalContextTrait,
    sql: &str,
) -> Result<Value, PureException> {
    let sql_null_handle = ctx.heap_mut().alloc_dynamic(m3_paths::RELATIONAL_SQL_NULL);
    let null_cell = Value::Object(sql_null_handle);

    let start = std::time::Instant::now();
    let (column_names, rows) = {
        let state = ctx
            .extensions()
            .get_or_init::<DuckDBState, _>(DuckDBState::new)?;
        state.with_conn(|c| {
            let mut stmt = c.prepare(sql)?;
            let mut rows = stmt.query([])?;
            let (col_count, column_names) = rows
                .as_ref()
                .map(|s| (s.column_count(), s.column_names()))
                .unwrap_or_default();
            let mut all_rows: Vec<Vec<Value>> = Vec::new();
            while let Some(row) = rows.next()? {
                let mut row_values: Vec<Value> = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    let r: ValueRef<'_> = row.get_ref(i)?;
                    row_values.push(cell_to_value(r, &null_cell));
                }
                all_rows.push(row_values);
            }
            Ok((column_names, all_rows))
        })?
    };
    let elapsed_ns = i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX);
    build(ctx, &column_names, rows, elapsed_ns)
}

/// Run a read-only SQL statement against the per-Evaluator H2 client
/// and build a populated `ResultSet` heap object. The H2 counterpart
/// of [`run_sql_to_result_set_duckdb`]; same shape, different engine.
///
/// # Errors
/// Propagates H2 (PG-wire) errors mapped to [`PureException`] via
/// [`crate::connection::map_postgres_err`].
pub fn run_sql_to_result_set_h2(
    ctx: &mut dyn EvalContextTrait,
    sql: &str,
) -> Result<Value, PureException> {
    let sql_null_handle = ctx.heap_mut().alloc_dynamic(m3_paths::RELATIONAL_SQL_NULL);
    let null_cell = Value::Object(sql_null_handle);

    let start = std::time::Instant::now();
    let (column_names, rows) = {
        let state = ctx
            .extensions()
            .get_or_init::<H2State, _>(H2State::from_global_config)?;
        state.with_client(|c| {
            // For an empty query (e.g. DDL surfacing through this path
            // by accident), `query` is still well-defined and returns
            // zero rows + the projection's column metadata.
            let pg_rows = c.query(sql, &[])?;
            let column_names: Vec<String> = pg_rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            let col_count = column_names.len();
            let mut all_rows: Vec<Vec<Value>> = Vec::with_capacity(pg_rows.len());
            for row in &pg_rows {
                let mut row_values: Vec<Value> = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    row_values.push(cell_to_value_pg(row, i, &null_cell));
                }
                all_rows.push(row_values);
            }
            Ok((column_names, all_rows))
        })?
    };
    let elapsed_ns = i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX);
    build(ctx, &column_names, rows, elapsed_ns)
}

/// Read one cell from a PG-wire row.
///
/// Covers the primitive types H2 surfaces by default through PG-wire
/// (bool, ints, floats, text, bytea). NUMERIC and date/time types
/// fall through to a string representation in v1 — the `postgres`
/// crate's typed-decoding for those requires extra feature flags
/// (`with-time-0_3` / a decimal feature) we haven't pulled in. The
/// fallback path is documented; growing the table is a follow-up.
fn cell_to_value_pg(row: &postgres::Row, idx: usize, null_cell: &Value) -> Value {
    use postgres::types::Type;
    let col = &row.columns()[idx];
    let pg_type = col.type_();
    match *pg_type {
        Type::BOOL => match row.try_get::<_, Option<bool>>(idx) {
            Ok(Some(b)) => Value::Boolean(b),
            _ => null_cell.clone(),
        },
        Type::INT2 => match row.try_get::<_, Option<i16>>(idx) {
            Ok(Some(i)) => Value::Integer(i64::from(i)),
            _ => null_cell.clone(),
        },
        Type::INT4 => match row.try_get::<_, Option<i32>>(idx) {
            Ok(Some(i)) => Value::Integer(i64::from(i)),
            _ => null_cell.clone(),
        },
        Type::INT8 => match row.try_get::<_, Option<i64>>(idx) {
            Ok(Some(i)) => Value::Integer(i),
            _ => null_cell.clone(),
        },
        Type::FLOAT4 => match row.try_get::<_, Option<f32>>(idx) {
            Ok(Some(f)) => Value::Float(f64::from(f)),
            _ => null_cell.clone(),
        },
        Type::FLOAT8 => match row.try_get::<_, Option<f64>>(idx) {
            Ok(Some(f)) => Value::Float(f),
            _ => null_cell.clone(),
        },
        Type::VARCHAR | Type::TEXT | Type::BPCHAR | Type::NAME => {
            match row.try_get::<_, Option<&str>>(idx) {
                Ok(Some(s)) => Value::String(SmolStr::new(s)),
                _ => null_cell.clone(),
            }
        }
        Type::BYTEA => match row.try_get::<_, Option<&[u8]>>(idx) {
            Ok(Some(b)) => {
                use std::fmt::Write;
                let mut s = String::with_capacity(b.len() * 2);
                for byte in b {
                    let _ = write!(&mut s, "{byte:02x}");
                }
                Value::String(SmolStr::new(s))
            }
            _ => null_cell.clone(),
        },
        // Composite / not-yet-typed: stringify the PG type name. Same
        // permissive treatment as the DuckDB cell_to_value's catchall.
        _ => Value::String(SmolStr::new(format!("<{pg_type}>"))),
    }
}

/// Build a `ResultSet` heap row populated from a freshly-fetched
/// DuckDB query.
///
/// `column_names` and `rows` are owned because they were collected
/// **inside** the `with_conn` closure (DuckDB borrows reset at scope
/// exit); `null_cell` is the shared `SQLNull` handle pre-allocated by
/// the caller before [`legend_pure_runtime::native::EvalContextTrait`]
/// got re-borrowed mutably.
///
/// # Errors
/// Propagates heap-mutation errors (currently unreachable on a
/// dynamically-allocated row, but kept for symmetry with the heap API).
pub fn build(
    ctx: &mut dyn EvalContextTrait,
    column_names: &[String],
    rows: Vec<Vec<Value>>,
    execution_ns: i64,
) -> Result<Value, PureException> {
    let rs = ctx
        .heap_mut()
        .alloc_dynamic(m3_paths::RELATIONAL_RESULT_SET);

    let col_name_values: Vec<Value> = column_names
        .iter()
        .map(|n| Value::String(SmolStr::new(n)))
        .collect();
    ctx.heap_mut()
        .mutate_add(&rs, "columnNames", &col_name_values)
        .map_err(PureException::from)?;
    ctx.heap_mut()
        .mutate_add(
            &rs,
            "executionTimeInNanoSecond",
            &[Value::Integer(execution_ns)],
        )
        .map_err(PureException::from)?;
    ctx.heap_mut()
        .mutate_add(
            &rs,
            "connectionAcquisitionTimeInNanoSecond",
            &[Value::Integer(0)],
        )
        .map_err(PureException::from)?;

    let rs_value = Value::Object(rs.clone());
    let parent_slice = std::slice::from_ref(&rs_value);
    let mut row_values: Vec<Value> = Vec::with_capacity(rows.len());
    for row in rows {
        let row_handle = ctx.heap_mut().alloc_dynamic(m3_paths::RELATIONAL_ROW);
        ctx.heap_mut()
            .mutate_add(&row_handle, "parent", parent_slice)
            .map_err(PureException::from)?;
        ctx.heap_mut()
            .mutate_add(&row_handle, "values", &row)
            .map_err(PureException::from)?;
        row_values.push(Value::Object(row_handle));
    }
    ctx.heap_mut()
        .mutate_add(&rs, "rows", &row_values)
        .map_err(PureException::from)?;

    Ok(Value::Object(rs))
}

/// Translate one DuckDB [`ValueRef`] into a Pure [`Value`].
///
/// Mirrors Java Pure's `ExecuteInDb.createPureResultSetFromDatabaseResultSet`
/// JDBC-types → Pure-types table; the comments above each branch tag
/// the Java line for cross-stack diffability.
///
/// Infallible — every variant has a defined Pure value. Composite or
/// not-yet-typed variants degrade to `Value::String`, mirroring Java's
/// `Types.OTHER` branch.
#[must_use]
pub fn cell_to_value(r: ValueRef<'_>, null_cell: &Value) -> Value {
    match r {
        // ExecuteInDb.java:262
        ValueRef::Null => null_cell.clone(),
        // ExecuteInDb.java:268
        ValueRef::Boolean(b) => Value::Boolean(b),
        // ExecuteInDb.java:275 (Integer/BigInt/SmallInt/TinyInt)
        ValueRef::TinyInt(i) => Value::Integer(i64::from(i)),
        ValueRef::SmallInt(i) => Value::Integer(i64::from(i)),
        ValueRef::Int(i) => Value::Integer(i64::from(i)),
        ValueRef::BigInt(i) => Value::Integer(i),
        // Unsigned: widen and demote to Decimal on overflow.
        ValueRef::UTinyInt(u) => Value::Integer(i64::from(u)),
        ValueRef::USmallInt(u) => Value::Integer(i64::from(u)),
        ValueRef::UInt(u) => Value::Integer(i64::from(u)),
        ValueRef::UBigInt(u) => {
            i64::try_from(u).map_or_else(|_| Value::Decimal(Decimal::from(u)), Value::Integer)
        }
        // ExecuteInDb.java:349 — DuckDB HUGEINT path. Java keeps it as
        // Integer when it fits in long; the Rust port mirrors that, but
        // anything that overflows i64 demotes to Decimal so the value is
        // not lost. Documented divergence; see plan §Risks.
        ValueRef::HugeInt(i) => i64::try_from(i).map_or_else(
            |_| Value::Decimal(Decimal::from_i128_with_scale(i, 0)),
            Value::Integer,
        ),
        // ExecuteInDb.java:316 (Float/Real/Double)
        ValueRef::Float(f) => Value::Float(f64::from(f)),
        ValueRef::Double(d) => Value::Float(d),
        // ExecuteInDb.java:327 (Decimal)
        ValueRef::Decimal(d) => Value::Decimal(d),
        // ExecuteInDb.java:295 (VARCHAR/CHAR)
        ValueRef::Text(b) => match std::str::from_utf8(b) {
            Ok(s) => Value::String(SmolStr::new(s)),
            Err(_) => Value::String(SmolStr::new(format!("<invalid utf-8: {b:?}>"))),
        },
        // ExecuteInDb.java:384 — hex-encode like Java BinaryUtils.encodeHex.
        ValueRef::Blob(b) => Value::String(SmolStr::new(encode_hex(b))),
        // ExecuteInDb.java:374 (DATE) — days since Unix epoch → strict_date.
        ValueRef::Date32(days) => date32_to_value(days),
        // ExecuteInDb.java:362 (TIMESTAMP)
        ValueRef::Timestamp(unit, n) => timestamp_to_value(unit, n),
        // ExecuteInDb.java:407 (Types.OTHER catchall — Time, Interval, …)
        ValueRef::Time64(unit, n) => Value::String(SmolStr::new(format!("Time64({unit:?},{n})"))),
        ValueRef::Interval {
            months,
            days,
            nanos,
        } => Value::String(SmolStr::new(format!("P{months}M{days}DT{nanos}N"))),
        // Composite types: not yet typed on the Pure side. Stringify so
        // tests can at least assert string equality against a stable form.
        other => Value::String(SmolStr::new(format!("{other:?}"))),
    }
}

/// `i32` days since the Unix epoch → Pure `StrictDate`.
fn date32_to_value(days: i32) -> Value {
    use jiff::Span;
    use jiff::civil::Date;
    let epoch = Date::constant(1970, 1, 1);
    let span: Span = Span::new().days(i64::from(days));
    match epoch.checked_add(span).ok() {
        Some(d) => {
            let date: Date = d;
            match PureDate::strict_date(date.year(), date.month(), date.day()).ok() {
                Some(p) => Value::Date(p),
                None => Value::String(SmolStr::new(format!("Date32:{days}"))),
            }
        }
        None => Value::String(SmolStr::new(format!("Date32:{days}"))),
    }
}

/// DuckDB `Timestamp(unit, n)` → Pure `DateTime` (UTC).
fn timestamp_to_value(unit: TimeUnit, n: i64) -> Value {
    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    let ts: Option<Timestamp> = match unit {
        TimeUnit::Second => Timestamp::from_second(n).ok(),
        TimeUnit::Millisecond => Timestamp::from_millisecond(n).ok(),
        TimeUnit::Microsecond => Timestamp::from_microsecond(n).ok(),
        TimeUnit::Nanosecond => Timestamp::from_nanosecond(i128::from(n)).ok(),
    };
    match ts {
        Some(t) => {
            let zoned = t.to_zoned(TimeZone::UTC);
            Value::Date(PureDate::from_civil_datetime(zoned.datetime()))
        }
        None => Value::String(SmolStr::new(format!("Timestamp({unit:?},{n})"))),
    }
}

/// Hex-encode bytes (lowercase), mirroring Java `BinaryUtils.encodeHex`.
fn encode_hex(b: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        let _ = write!(&mut s, "{byte:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encoding_matches_java_buil() {
        // BinaryUtils.encodeHex([0xDE, 0xAD, 0xBE, 0xEF]) == "deadbeef"
        assert_eq!(encode_hex(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }

    #[test]
    fn date32_zero_is_unix_epoch() {
        let v = date32_to_value(0);
        let Value::Date(p) = v else {
            panic!("expected Date, got {v:?}")
        };
        // 1970-01-01
        let dt = p.inner_datetime();
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn timestamp_microseconds_unix_epoch() {
        let v = timestamp_to_value(TimeUnit::Microsecond, 0);
        let Value::Date(p) = v else {
            panic!("expected Date, got {v:?}")
        };
        let dt = p.inner_datetime();
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
        assert_eq!(dt.hour(), 0);
    }

    fn run_smoke<F>(f: F)
    where
        F: FnOnce(&duckdb::Connection),
    {
        let c = duckdb::Connection::open_in_memory().expect("open");
        c.execute_batch("SET TimeZone='UTC';").ok();
        f(&c);
    }

    fn fetch_one(c: &duckdb::Connection, sql: &str, null: &Value) -> Vec<Value> {
        let mut stmt = c.prepare(sql).expect("prepare");
        let mut rows = stmt.query([]).expect("query");
        let n = rows.as_ref().map_or(0, duckdb::Statement::column_count);
        let row = rows.next().expect("next").expect("row");
        (0..n)
            .map(|i| {
                let r: ValueRef<'_> = row.get_ref(i).expect("get_ref");
                cell_to_value(r, null)
            })
            .collect()
    }

    #[test]
    fn cell_to_value_integer_and_string() {
        run_smoke(|c| {
            let null = Value::String(SmolStr::new("<SQLNull>"));
            let values = fetch_one(c, "SELECT 42::INTEGER, 'hi'::VARCHAR", &null);
            assert_eq!(values[0], Value::Integer(42));
            assert_eq!(values[1], Value::String(SmolStr::new("hi")));
        });
    }

    #[test]
    fn cell_to_value_null_clones_sentinel() {
        run_smoke(|c| {
            let null = Value::String(SmolStr::new("<SQLNull>"));
            let values = fetch_one(c, "SELECT NULL", &null);
            assert_eq!(values[0], null);
        });
    }
}
