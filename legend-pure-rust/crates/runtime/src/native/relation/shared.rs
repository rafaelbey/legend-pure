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

//! Shared helpers used by multiple relation natives.
//!
//! Two clusters live here:
//!
//! 1. **Heap-walking utilities** — `single_object_slot`,
//!    `unwrap_instance_value`, `read_col_spec_array_columns`. Mirror the
//!    Java reflection chain `instance._slot()._slot()…`. Used by
//!    natives that consume `RelationType` / `ColSpec` / `ColSpecArray`
//!    metaclass instances at runtime.
//!
//! 2. **TDS row access** — `read_parsed_tds`. A `TDS` heap object only
//!    carries its `csv: String[1]` slot at runtime (the metaclass
//!    doesn't declare anything else). Natives that need typed
//!    columns or row data re-parse the canonical CSV via
//!    `legend_pure_dsl_tds::csv::parse_and_infer`. Reparsing is
//!    correct but wasteful; a future change can stash the parsed
//!    structure on the heap object as a native-data slot.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_dsl_tds::csv::{ParsedColumn, ParsedTDS, TypedCell};

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::EvalContextTrait;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Heap-walking utilities
// ---------------------------------------------------------------------------

/// Walk `csa.classifierGenericType.typeArguments[0].rawType.columns` and
/// return the column values. Mirrors Java's
/// `((ColSpecArrayInstance)…)._classifierGenericType()._typeArguments()
///   .getFirst()._rawType()._columns()` chain.
#[allow(clippy::result_large_err)]
pub(super) fn read_col_spec_array_columns(
    cs_obj: ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<Value>, PureException> {
    let cgt = single_object_slot(cs_obj, "classifierGenericType", ctx)?;
    let type_args = ctx
        .heap()
        .get_property_values(&cgt, "typeArguments")
        .map_err(PureException::from)?;
    let first_ta = type_args
        .iter()
        .find_map(|v| match v {
            Value::Object(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "addColumns: ColSpecArray.classifierGenericType.typeArguments[0] missing or not an Object"
                    .into(),
            ))
        })?;
    let raw = single_object_slot(first_ta, "rawType", ctx)?;
    let cols = ctx
        .heap()
        .get_property_values(&raw, "columns")
        .map_err(PureException::from)?;
    Ok(cols.iter().cloned().collect())
}

/// Read a single Object out of `obj.<slot>`.
#[allow(clippy::result_large_err)]
pub(super) fn single_object_slot(
    obj: ObjectHandle,
    slot: &str,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let values = ctx
        .heap()
        .get_property_values(&obj, slot)
        .map_err(PureException::from)?;
    values
        .iter()
        .find_map(|v| match v {
            Value::Object(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "relation native: slot '{slot}' missing or not an Object"
            )))
        })
}

/// Unwrap `Value::Object(InstanceValue).values` if needed; otherwise
/// require a bare `Value::Object`. Mirrors `lang.rs::unwrap_instance_value_*`.
#[allow(clippy::result_large_err)]
pub(super) fn unwrap_instance_value(
    value: &Value,
    instance_value_id: Option<legend_pure_parser_pure::ids::ElementId>,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let Value::Object(obj) = value else {
        return Err(PureException::from(PureRuntimeError::type_mismatch(
            "Object", value,
        )));
    };
    let obj = obj.clone();
    if let Some(iv_id) = instance_value_id {
        let classifier = ctx
            .heap()
            .classifier(&obj)
            .map_err(PureException::from)?
            .clone();
        let resolved = m3_paths::resolve(ctx.model(), &classifier);
        if resolved == Some(iv_id) {
            let inner = ctx
                .heap()
                .get_property_values(&obj, "values")
                .map_err(PureException::from)?;
            if let Some(Value::Object(unwrapped)) = inner.iter().next() {
                return Ok(unwrapped.clone());
            }
        }
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// TDS row access
// ---------------------------------------------------------------------------

/// Read the canonical CSV from a `TDS` heap object and re-parse it
/// into a [`ParsedTDS`] (columns + typed rows).
///
/// The runtime currently stores only the `csv: String[1]` slot on a
/// TDS heap instance — the structured per-column / per-cell
/// representation produced by `parse_and_infer` at allocation time
/// is not retained. Each consumer (`size`, `columns`, future
/// `filter`/`sort`/`extend`/…) re-parses on demand. That's wasteful
/// but functionally correct; a follow-up can cache the parsed shape
/// on the heap entry once we settle on the cache shape.
///
/// The argument `tds_obj` must be a `TDS` heap object (classifier
/// `meta::pure::metamodel::relation::TDS`); callers are expected to
/// have unwrapped any `InstanceValue` wrapper via
/// [`unwrap_instance_value`] beforehand.
#[allow(clippy::result_large_err)]
pub(super) fn read_parsed_tds(
    fn_name: &'static str,
    tds_obj: &ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ParsedTDS, PureException> {
    let csv_values = ctx
        .heap()
        .get_property_values(tds_obj, "csv")
        .map_err(PureException::from)?;
    let csv = csv_values
        .iter()
        .find_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "{fn_name}: TDS.csv slot is missing or not a String"
            )))
        })?;
    legend_pure_dsl_tds::csv::parse_and_infer(csv.as_str(), &[]).map_err(|e| {
        PureException::from(PureRuntimeError::EvaluationError(format!(
            "{fn_name}: parse_and_infer({csv:?}) failed: {e}"
        )))
    })
}

// ---------------------------------------------------------------------------
// Canonical CSV reconstruction (TypedCell → source-form text)
// ---------------------------------------------------------------------------

/// Convenience wrapper: render an entire [`ParsedTDS`] back to canonical
/// CSV form by feeding its `columns` + `rows` through
/// [`render_csv_from_columns_and_rows`].
pub(super) fn render_canonical_csv(parsed: &ParsedTDS) -> String {
    render_csv_from_columns_and_rows(&parsed.columns, &parsed.rows)
}

/// Reconstruct the canonical CSV string from a column header list and a
/// row matrix of typed cells. Output round-trips through
/// [`legend_pure_dsl_tds::csv::parse_and_infer`] — i.e. parsing the
/// returned string yields a `ParsedTDS` whose `columns` (matched on
/// `name` + `type_tag`) and `rows` are equal to the inputs (modulo
/// multiplicity, which the parser re-derives from the data).
///
/// Header line uses `, ` separators (mirroring `dsl-tds::lower::reconstruct_csv`);
/// row cells likewise. `\n` terminates lines.
///
/// Per-cell rendering rules:
///
/// - `None` → `''` — quoted empty, recognised by `RawCell::is_empty`.
/// - `Integer(i)` → decimal, no leading `+`.
/// - `Float(f)` → must include a `.` so reparse classifies the column
///   as `Float` (not `Integer`). We use `{:?}` which always emits `1.0`
///   form for whole-valued floats.
/// - `Decimal(s)` → emitted verbatim (already carries `D`/`d` suffix).
/// - `Boolean(b)` → `true` / `false`.
/// - `String(s)` → wrapped in single quotes; any internal `'` escaped
///   as `\'`.
/// - `StrictDate(s)`, `DateTime(s)` → verbatim (already source-form).
pub(super) fn render_csv_from_columns_and_rows(
    columns: &[ParsedColumn],
    rows: &[Vec<Option<TypedCell>>],
) -> String {
    let mut buf = String::new();
    let header: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    buf.push_str(&header.join(", "));
    for row in rows {
        buf.push('\n');
        let cells: Vec<String> = row.iter().map(render_cell).collect();
        buf.push_str(&cells.join(", "));
    }
    buf
}

/// Render a single [`TypedCell`] (or `None`) to its canonical CSV form.
fn render_cell(cell: &Option<TypedCell>) -> String {
    match cell {
        None => "''".to_string(),
        Some(TypedCell::Integer(i)) => i.to_string(),
        Some(TypedCell::Float(f)) => {
            // `{:?}` on f64 always renders a decimal point (`1.0`,
            // `-3.14`, `2e10`). Plain `{}` would emit `1` for whole
            // values and the column would re-classify as Integer.
            format!("{f:?}")
        }
        Some(TypedCell::Decimal(s)) => s.to_string(),
        Some(TypedCell::Boolean(true)) => "true".to_string(),
        Some(TypedCell::Boolean(false)) => "false".to_string(),
        Some(TypedCell::String(s)) => {
            // Single-quote wrap; escape internal `'` as `\'`. The
            // parser's `parse_csv_line` consumes `\\<quote>` as the
            // literal quote inside a quoted segment, and the cell
            // materialiser strips the leading backslash via
            // `unescape_string`.
            let escaped = s.replace('\'', "\\'");
            format!("'{escaped}'")
        }
        Some(TypedCell::StrictDate(s)) => s.to_string(),
        Some(TypedCell::DateTime(s)) => s.to_string(),
    }
}
