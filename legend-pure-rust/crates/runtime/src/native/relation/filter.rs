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

//! `filter(rel:Relation<T>[1], f:Function<{T[1]→Boolean[1]}>[1]):Relation<T>[1]`.
//!
//! Lambda-per-row template: invoke the predicate with `$x` bound to a
//! synthetic heap object that carries the row's column values as named
//! slots. Survivors are re-emitted as a fresh `TDS` whose `csv` slot
//! preserves the original CSV's header line plus the (verbatim) data
//! lines of surviving rows — no re-rendering, so quoting and inferred
//! types match the input exactly.
//!
//! This is the row-binding template that `sort`, `extend`, `groupBy`,
//! and any other lambda-per-row relation native will share.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_dsl_tds::csv::TypedCell;
use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::PureException;
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, unwrap_instance_value};

/// Pure
/// `filter<T>(rel:Relation<T>[1], f:Function<{T[1]→Boolean[1]}>[1])
///   :Relation<T>[1]`.
///
/// Per-row evaluation:
/// 1. Re-parse the receiver TDS (`read_parsed_tds`) so we have typed
///    column metadata + materialised cells.
/// 2. For each row, allocate a synthetic `Any`-classified heap object
///    (`row_tuple`) and `mutate_add` each column's value into a slot
///    named after the column. Empty cells are simply omitted, so
///    `$x.col` returns `Value::Unit` — matching `[0..1]` semantics.
/// 3. Invoke the lambda via `ctx.call_function(&f, &[row_tuple])` and
///    coerce the result to `Boolean`.
/// 4. Collect surviving row indices, reconstruct a CSV by slicing the
///    original CSV's lines (header + surviving data lines), and
///    allocate a fresh `TDS` heap instance with that CSV.
///
/// CSV reconstruction by line-slicing (not re-rendering typed cells)
/// preserves the original quoting/escaping shape, so a subsequent
/// `read_parsed_tds` re-derives equivalent inferred types and cell
/// values. Re-rendering would lose quote-style information (`'…'` vs
/// `"…"`) and could shift inferred column types when the filtered
/// subset's data narrows the inference (e.g. all-Integer rows
/// surviving from a Float column).
#[derive(Debug)]
pub struct Filter;

impl NativeFunction for Filter {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("filter (Relation)", args, 2)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);

        // -- Source TDS --------------------------------------------------
        let rel_value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&rel_value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("filter", &tds_obj, ctx)?;

        // -- Predicate lambda --------------------------------------------
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        // -- Per-row evaluation ------------------------------------------
        let mut survivors: Vec<usize> = Vec::with_capacity(parsed.rows.len());
        for (row_idx, row) in parsed.rows.iter().enumerate() {
            let row_tuple = build_row_tuple(&parsed.columns, row, ctx)?;
            let pred = ctx.call_function(&lambda_val, &[Value::Object(row_tuple)])?;
            if pred.as_boolean()? {
                survivors.push(row_idx);
            }
        }

        // -- Reconstruct CSV by line-slicing -----------------------------
        let new_csv = slice_csv_by_rows(parsed.csv.as_str(), &survivors);

        // -- Allocate the fresh TDS --------------------------------------
        let new_tds = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&new_tds, "csv", &[Value::String(new_csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(new_tds)))
    }

    fn signature(&self) -> &'static str {
        "filter(Relation<T>[1], Function<{T[1]->Boolean[1]}>[1]):Relation<T>[1]"
    }
}

/// Allocate a synthetic heap object representing one TDS row, with one
/// slot per non-empty cell named after its column. Classifier is
/// `meta::pure::metamodel::type::Any` — property access reads slots by
/// name regardless of classifier, and `Any` avoids accidentally
/// matching native-side classifier dispatch (e.g. `RelationType` /
/// `TDS`) that other natives use to recognise their own argument
/// shapes.
#[allow(clippy::result_large_err)]
fn build_row_tuple(
    columns: &[legend_pure_dsl_tds::csv::ParsedColumn],
    row: &[Option<TypedCell>],
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let handle = ctx.heap_mut().alloc_dynamic("meta::pure::metamodel::type::Any");
    for (col, cell) in columns.iter().zip(row.iter()) {
        let Some(cell) = cell else {
            continue;
        };
        let value = typed_cell_to_value(cell);
        ctx.heap_mut()
            .mutate_add(&handle, col.name.as_str(), &[value])
            .map_err(PureException::from)?;
    }
    Ok(handle)
}

/// Convert a [`TypedCell`] into a runtime [`Value`].
///
/// `Decimal`/`StrictDate`/`DateTime` map to `Value::String` for now;
/// the runtime's typed-cell handling for those is its own follow-up,
/// and the underlying storage in `TypedCell` is already a string.
/// `String` likewise — the smolstr is widened to `String`.
fn typed_cell_to_value(cell: &TypedCell) -> Value {
    match cell {
        TypedCell::Integer(i) => Value::Integer(*i),
        TypedCell::Float(f) => Value::Float(*f),
        TypedCell::Boolean(b) => Value::Boolean(*b),
        TypedCell::String(s)
        | TypedCell::Decimal(s)
        | TypedCell::StrictDate(s)
        | TypedCell::DateTime(s) => Value::String(SmolStr::new(s.as_str())),
    }
}

/// Slice the original CSV body to retain only the header line plus
/// the data lines whose 0-based index is in `surviving_row_indices`
/// (in ascending order). Joins the kept lines with `\n`, matching the
/// shape `read_parsed_tds` re-parses.
fn slice_csv_by_rows(csv: &str, surviving_row_indices: &[usize]) -> String {
    let trimmed = csv.trim_matches(|c: char| c == '\n' || c == '\r');
    let mut iter = trimmed.split('\n');
    let header = iter.next().unwrap_or("");
    let data_lines: Vec<&str> = iter.collect();

    let mut out = String::with_capacity(trimmed.len());
    out.push_str(header);
    for &idx in surviving_row_indices {
        if let Some(line) = data_lines.get(idx) {
            out.push('\n');
            out.push_str(line);
        }
    }
    out
}
