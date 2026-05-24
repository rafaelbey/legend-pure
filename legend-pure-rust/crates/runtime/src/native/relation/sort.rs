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

//! `sort(rel:Relation<T>[1], sortInfo:SortInfo<X⊆T>[*]):Relation<T>[1]`.
//!
//! Multi-key stable sort over the rows of a TDS, driven by a list of
//! `SortInfo` heap objects each carrying a `column: ColSpec<T>[1]` slot
//! and a `direction: SortType[1]` enum slot (`ASC` / `DESC`). Earlier
//! entries in the list are major sort keys; equal-on-major comparisons
//! fall through to the next entry, and ties left after exhausting the
//! list keep first-occurrence (stable) order.
//!
//! The receiver TDS is reparsed via [`read_parsed_tds`]; the sorted
//! row matrix is rendered through the shared
//! [`render_csv_from_columns_and_rows`] helper (the same path
//! `concatenate`/`limit`/`drop` use), and the resulting canonical CSV
//! is stashed on a freshly allocated `TDS` heap object's `csv` slot.

#![allow(clippy::needless_pass_by_value)]

use std::cmp::Ordering;

use legend_pure_dsl_tds::csv::{ParsedTDS, TypedCell};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{
    read_parsed_tds, render_csv_from_columns_and_rows, single_object_slot, unwrap_instance_value,
};

/// Pure `sort<X,T>(rel:Relation<T>[1], sortInfo:SortInfo<X⊆T>[*]):Relation<T>[1]`.
#[derive(Debug)]
pub struct Sort;

impl NativeFunction for Sort {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("sort (Relation, SortInfo[*])", args, 2)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);

        let rel_value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&rel_value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("sort", &tds_obj, ctx)?;

        let sort_info_value = ctx.evaluate(&args[1])?.into_value();
        let sort_info_objs = collect_sort_info(&sort_info_value, instance_value_id, ctx)?;

        // Empty list → identity. Skip the row-shuffle and re-render.
        if sort_info_objs.is_empty() {
            return Ok(Evaluated::new(Value::Object(tds_obj)));
        }

        // Decode each SortInfo into (column-index, direction).
        let mut keys: Vec<(usize, Direction)> = Vec::with_capacity(sort_info_objs.len());
        for so_obj in &sort_info_objs {
            let key = decode_sort_info(so_obj, &parsed, ctx)?;
            keys.push(key);
        }

        let mut sorted = parsed.rows.clone();
        sorted.sort_by(|a, b| compare_rows(a, b, &keys));

        let new_csv = render_csv_from_columns_and_rows(&parsed.columns, &sorted);

        let tds_handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&tds_handle, "csv", &[Value::String(new_csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }
}

/// One sort key resolved against the source TDS: the column's index in
/// `ParsedTDS::columns` and its sort direction.
#[derive(Debug, Clone, Copy)]
enum Direction {
    Asc,
    Desc,
}

/// Flatten a `Value` representing a `SortInfo[*]` argument into a
/// `Vec<ObjectHandle>`. Accepts:
/// - `Value::Unit` — empty list → empty vec.
/// - `Value::Collection(v)` — recurse into each element.
/// - `Value::Object(...)` — single SortInfo (multiplicity-1 case; legal
///   because a `[*]` parameter accepts a single element).
/// - Other shapes are a type error.
#[allow(clippy::result_large_err)]
fn collect_sort_info(
    value: &Value,
    instance_value_id: Option<ElementId>,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<ObjectHandle>, PureException> {
    let mut out = Vec::new();
    push_sort_info(value, instance_value_id, ctx, &mut out)?;
    Ok(out)
}

#[allow(clippy::result_large_err)]
fn push_sort_info(
    value: &Value,
    instance_value_id: Option<ElementId>,
    ctx: &mut dyn EvalContextTrait,
    out: &mut Vec<ObjectHandle>,
) -> Result<(), PureException> {
    match value {
        Value::Unit => Ok(()),
        Value::Collection(v) => {
            for entry in v.iter() {
                push_sort_info(entry, instance_value_id, ctx, out)?;
            }
            Ok(())
        }
        Value::Object(_) => {
            let obj = unwrap_instance_value(value, instance_value_id, ctx)?;
            out.push(obj);
            Ok(())
        }
        other => Err(PureException::from(PureRuntimeError::type_mismatch(
            "SortInfo", other,
        ))),
    }
}

/// Decode a `SortInfo` heap object into the column index its `column`
/// slot points at (resolved against `parsed.columns` by name) and its
/// `direction` enum member (`ASC` / `DESC`).
#[allow(clippy::result_large_err)]
fn decode_sort_info(
    so_obj: &ObjectHandle,
    parsed: &ParsedTDS,
    ctx: &mut dyn EvalContextTrait,
) -> Result<(usize, Direction), PureException> {
    // direction: SortType[1] — read the enum member name.
    let direction_values = ctx
        .heap()
        .get_property_values(so_obj, "direction")
        .map_err(PureException::from)?;
    let direction = direction_values
        .iter()
        .find_map(|v| match v {
            Value::EnumValue { member, .. } => Some(member.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "sort: SortInfo.direction slot missing or not an EnumValue".into(),
            ))
        })?;
    let direction = match direction.as_str() {
        "ASC" => Direction::Asc,
        "DESC" => Direction::Desc,
        other => {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("sort: unsupported SortType member '{other}' (expected ASC / DESC)"),
            )));
        }
    };

    // column: ColSpec<T>[1] — read its `name` slot.
    let col_obj = single_object_slot(so_obj.clone(), "column", ctx)?;
    let name_values = ctx
        .heap()
        .get_property_values(&col_obj, "name")
        .map_err(PureException::from)?;
    let col_name = name_values
        .iter()
        .find_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "sort: SortInfo.column.name slot missing or not a String".into(),
            ))
        })?;

    let idx = parsed
        .columns
        .iter()
        .position(|c| c.name.as_str() == col_name.as_str())
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "sort: column '{col_name}' not present in source TDS"
            )))
        })?;
    Ok((idx, direction))
}

/// Lexicographic row comparator over a sequence of `(column-index, direction)`
/// keys. The first non-equal column drives the result; equal-on-all
/// returns `Ordering::Equal` and `sort_by` keeps stable order.
fn compare_rows(
    a: &[Option<TypedCell>],
    b: &[Option<TypedCell>],
    keys: &[(usize, Direction)],
) -> Ordering {
    for &(col_idx, dir) in keys {
        let lhs = a.get(col_idx).and_then(|c| c.as_ref());
        let rhs = b.get(col_idx).and_then(|c| c.as_ref());
        let raw = compare_cells(lhs, rhs);
        let directed = match dir {
            Direction::Asc => raw,
            Direction::Desc => raw.reverse(),
        };
        if directed != Ordering::Equal {
            return directed;
        }
    }
    Ordering::Equal
}

/// Compare two `Option<TypedCell>` values for ascending order.
///
/// `None` (empty cell) sorts before any `Some(...)`. Numeric variants
/// compare numerically; `Decimal`/`StrictDate`/`DateTime` use lexical
/// compare on their source-form storage (ISO-8601 dates and decimal
/// literals sort correctly lexicographically). Booleans use
/// `false < true`. Mixed-type cells (which shouldn't arise within a
/// single column of a `ParsedTDS`) fall back to a stable type-tag
/// ordering so the comparator remains total.
fn compare_cells(a: Option<&TypedCell>, b: Option<&TypedCell>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => match (a, b) {
            (TypedCell::Integer(x), TypedCell::Integer(y)) => x.cmp(y),
            (TypedCell::Float(x), TypedCell::Float(y)) => {
                x.partial_cmp(y).unwrap_or(Ordering::Equal)
            }
            (TypedCell::Integer(x), TypedCell::Float(y)) => {
                (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
            }
            (TypedCell::Float(x), TypedCell::Integer(y)) => {
                x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
            }
            (TypedCell::Boolean(x), TypedCell::Boolean(y)) => x.cmp(y),
            // SmolStr-backed cells: lexicographic compare on the inner
            // string. Distinct variants kept separate so the pattern
            // shows in profiles per type.
            (TypedCell::Decimal(x), TypedCell::Decimal(y))
            | (TypedCell::String(x), TypedCell::String(y))
            | (TypedCell::StrictDate(x), TypedCell::StrictDate(y))
            | (TypedCell::DateTime(x), TypedCell::DateTime(y)) => x.as_str().cmp(y.as_str()),
            // Mixed-variant fallback — shouldn't happen for a single
            // homogeneously-typed TDS column. Tag-order by discriminant
            // index so the comparator stays total.
            _ => type_tag(a).cmp(&type_tag(b)),
        },
    }
}

fn type_tag(cell: &TypedCell) -> u8 {
    match cell {
        TypedCell::Integer(_) => 0,
        TypedCell::Float(_) => 1,
        TypedCell::Decimal(_) => 2,
        TypedCell::Boolean(_) => 3,
        TypedCell::String(_) => 4,
        TypedCell::StrictDate(_) => 5,
        TypedCell::DateTime(_) => 6,
    }
}
