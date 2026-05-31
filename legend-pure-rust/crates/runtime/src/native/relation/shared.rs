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

use legend_pure_dsl_tds::csv::{ColumnType, ParsedColumn, ParsedTDS, TypedCell};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::Multiplicity;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::{ObjectHandle, RuntimeHeap};
use crate::m3_paths;
use crate::native::EvalContextTrait;
use crate::relation::alloc_multiplicity;
use crate::value::Value;

/// Walk `csa.classifierGenericType.typeArguments[0].rawType.columns` and
/// return the column values. Mirrors Java's
/// `((ColSpecArrayInstance)…)._classifierGenericType()._typeArguments()
///   .getFirst()._rawType()._columns()` chain.
#[allow(clippy::result_large_err)]
pub fn read_col_spec_array_columns(
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
pub fn single_object_slot(
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
pub fn unwrap_instance_value(
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

/// Reconstruct a [`ParsedTDS`] from a `TDS` heap object.
///
/// `rows` is the source of truth: a `TDS` instance carries a `rows`
/// slot (one row tuple per row, built by [`build_row_tuple`]) plus its
/// column `RelationType` on `classifierGenericType`
/// (`classifierGenericType.typeArguments[0].rawType.columns`). The
/// canonical `csv` property is *derived* from these via the `tdsToCsv`
/// native — it is no longer a stored slot.
///
/// This walk is the exact inverse of [`alloc_tds_from_parsed`]:
///
/// - **columns** — read each `Column`'s `name`, type element
///   (`classifierGenericType.typeArguments[1].rawType`) and
///   `multiplicityArguments[0]`, mapping back to [`ParsedColumn`]
///   (see [`read_tds_columns`]).
/// - **rows** — for each row tuple, read each column-named slot and
///   re-tag the stored [`Value`] as a [`TypedCell`] using the column's
///   type (see [`value_to_typed_cell`]). A missing slot is an empty
///   (`None`) cell, matching [`build_row_tuple`]'s skip-empty rule.
///
/// `ParsedTDS::csv` is rendered from the recovered columns + rows so
/// consumers that read it observe the same canonical string the
/// derived `csv` property would.
///
/// The argument `tds_obj` must be a `TDS` heap object (classifier
/// `meta::pure::metamodel::relation::TDS`); callers are expected to
/// have unwrapped any `InstanceValue` wrapper via
/// [`unwrap_instance_value`] beforehand.
#[allow(clippy::result_large_err)]
pub fn read_parsed_tds(
    fn_name: &'static str,
    tds_obj: &ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ParsedTDS, PureException> {
    let columns = read_tds_columns(fn_name, tds_obj, ctx)?;

    let row_values = ctx
        .heap()
        .get_property_values(tds_obj, "rows")
        .map_err(PureException::from)?;
    let mut rows: Vec<Vec<Option<TypedCell>>> = Vec::with_capacity(row_values.len());
    for v in row_values.iter() {
        let Value::Object(row_obj) = v else {
            continue;
        };
        let mut cells: Vec<Option<TypedCell>> = Vec::with_capacity(columns.len());
        for col in &columns {
            let slot_vals = ctx
                .heap()
                .get_property_values(row_obj, col.name.as_str())
                .map_err(PureException::from)?;
            let cell = slot_vals
                .iter()
                .next()
                .and_then(|val| value_to_typed_cell(val, &col.type_tag));
            cells.push(cell);
        }
        rows.push(cells);
    }

    let csv = render_csv_from_columns_and_rows(&columns, &rows);
    Ok(ParsedTDS { csv, columns, rows })
}

/// Read a `TDS` heap object's column metadata off its attached
/// `RelationType`. Walks
/// `tds.classifierGenericType.typeArguments[0].rawType.columns` (the
/// shape [`alloc_tds_from_parsed`] writes, identical to the chain in
/// [`read_col_spec_array_columns`]) and recovers each column's name,
/// [`ColumnType`] and [`Multiplicity`].
#[allow(clippy::result_large_err)]
pub fn read_tds_columns(
    fn_name: &'static str,
    tds_obj: &ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<ParsedColumn>, PureException> {
    let missing = |what: &str| {
        PureException::from(PureRuntimeError::EvaluationError(format!(
            "{fn_name}: TDS.{what} missing — TDS heap object has no column RelationType"
        )))
    };

    let cgt = first_object_in_slot(ctx.heap(), tds_obj, "classifierGenericType")
        .ok_or_else(|| missing("classifierGenericType"))?;
    let rt_gt = first_object_in_slot(ctx.heap(), &cgt, "typeArguments")
        .ok_or_else(|| missing("classifierGenericType.typeArguments[0]"))?;
    let relation_type =
        first_object_in_slot(ctx.heap(), &rt_gt, "rawType").ok_or_else(|| missing("rawType"))?;
    let col_values = ctx
        .heap()
        .get_property_values(&relation_type, "columns")
        .map_err(PureException::from)?;

    let mut columns: Vec<ParsedColumn> = Vec::with_capacity(col_values.len());
    for v in col_values.iter() {
        let Value::Object(col_obj) = v else {
            continue;
        };
        let name = ctx
            .heap()
            .get_property_values(col_obj, "name")
            .map_err(PureException::from)?
            .iter()
            .find_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // Column.classifierGenericType: typeArguments=[null, <type GT>],
        // multiplicityArguments=[<Multiplicity>] — see `alloc_tds_column`.
        let col_cgt = first_object_in_slot(ctx.heap(), col_obj, "classifierGenericType");
        let type_tag = col_cgt
            .as_ref()
            .and_then(|cgt| first_object_in_slot(ctx.heap(), cgt, "typeArguments"))
            .and_then(|type_gt| {
                ctx.heap()
                    .get_property_values(&type_gt, "rawType")
                    .ok()?
                    .iter()
                    .find_map(|x| match x {
                        Value::Element(e) => Some(*e),
                        _ => None,
                    })
            })
            .map_or(ColumnType::String, |elem| {
                element_id_to_column_type(ctx.model(), elem)
            });
        let multiplicity = col_cgt
            .as_ref()
            .and_then(|cgt| first_object_in_slot(ctx.heap(), cgt, "multiplicityArguments"))
            .map_or(Multiplicity::ZeroOrMany, |m| {
                read_multiplicity(ctx.heap(), &m)
            });

        columns.push(ParsedColumn {
            name,
            type_tag,
            multiplicity,
        });
    }
    Ok(columns)
}

/// First `Value::Object` in `obj.<slot>`, or `None`.
fn first_object_in_slot(
    heap: &RuntimeHeap,
    obj: &ObjectHandle,
    slot: &str,
) -> Option<ObjectHandle> {
    heap.get_property_values(obj, slot)
        .ok()?
        .iter()
        .find_map(|v| match v {
            Value::Object(o) => Some(o.clone()),
            _ => None,
        })
}

/// Recover a [`Multiplicity`] from a `Multiplicity` heap object — the
/// inverse of [`alloc_multiplicity`]. Reads
/// `lowerBound.value` / `upperBound.value`; absent `upperBound` is the
/// open upper end (`*`).
fn read_multiplicity(heap: &RuntimeHeap, mult_obj: &ObjectHandle) -> Multiplicity {
    let read_bound = |slot: &str| -> Option<i64> {
        let bound = first_object_in_slot(heap, mult_obj, slot)?;
        heap.get_property_values(&bound, "value")
            .ok()?
            .iter()
            .find_map(|v| match v {
                Value::Integer(i) => Some(*i),
                _ => None,
            })
    };
    let lower = read_bound("lowerBound").unwrap_or(0);
    let upper = read_bound("upperBound");
    match (lower, upper) {
        (1, Some(1)) => Multiplicity::PureOne,
        (0, Some(1)) => Multiplicity::ZeroOrOne,
        (1, None) => Multiplicity::OneOrMany,
        (0, None) => Multiplicity::ZeroOrMany,
        (l, u) => Multiplicity::Range {
            lower: u32::try_from(l).unwrap_or(0),
            upper: u.map(|x| u32::try_from(x).unwrap_or(0)),
        },
    }
}

/// Map a column's stored type element back to its [`ColumnType`] — the
/// inverse of [`column_type_to_element_id`]. The seven primitives map
/// exactly (so `Decimal`/`StrictDate`/`DateTime` columns re-tag stored
/// `String` cells correctly via [`value_to_typed_cell`]). Any other
/// element becomes [`ColumnType::Other`] carrying just the bare class
/// name — the compiled model doesn't surface a simple package path, and
/// `Other` columns store their cells as plain strings, so the package
/// only affects `toString` display of explicitly-annotated non-primitive
/// columns.
fn element_id_to_column_type(model: &PureModel, id: ElementId) -> ColumnType {
    match model.element_name(id).as_str() {
        "Integer" => ColumnType::Integer,
        "Float" => ColumnType::Float,
        "Decimal" => ColumnType::Decimal,
        "Boolean" => ColumnType::Boolean,
        "String" => ColumnType::String,
        "StrictDate" | "Date" => ColumnType::StrictDate,
        "DateTime" => ColumnType::DateTime,
        other => ColumnType::Other {
            package: None,
            name: SmolStr::new(other),
        },
    }
}

/// Re-tag a stored row-slot [`Value`] as a [`TypedCell`] using the
/// column's [`ColumnType`] — the inverse of [`typed_cell_to_value`].
/// `Decimal`/`StrictDate`/`DateTime` are stored as `Value::String`, so
/// the column type is what distinguishes them from a plain `String`.
#[must_use]
pub fn value_to_typed_cell(value: &Value, column_type: &ColumnType) -> Option<TypedCell> {
    match value {
        Value::Integer(i) => Some(TypedCell::Integer(*i)),
        Value::Float(f) => Some(TypedCell::Float(*f)),
        Value::Boolean(b) => Some(TypedCell::Boolean(*b)),
        Value::String(s) => Some(match column_type {
            ColumnType::Decimal => TypedCell::Decimal(s.clone()),
            ColumnType::StrictDate => TypedCell::StrictDate(s.clone()),
            ColumnType::DateTime => TypedCell::DateTime(s.clone()),
            _ => TypedCell::String(s.clone()),
        }),
        _ => None,
    }
}

/// Convenience wrapper: render an entire [`ParsedTDS`] back to canonical
/// CSV form by feeding its `columns` + `rows` through
/// [`render_csv_from_columns_and_rows`].
pub fn render_canonical_csv(parsed: &ParsedTDS) -> String {
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
pub fn render_csv_from_columns_and_rows(
    columns: &[ParsedColumn],
    rows: &[Vec<Option<TypedCell>>],
) -> String {
    let mut buf = String::new();
    let header: Vec<String> = columns.iter().map(render_column_header).collect();
    buf.push_str(&header.join(","));
    for row in rows {
        buf.push('\n');
        let cells: Vec<String> = row.iter().map(|c| render_cell(c.as_ref())).collect();
        buf.push_str(&cells.join(","));
    }
    buf
}

/// `name[:Type][[mult]]` — always emits the type tag; appends the
/// multiplicity only when it differs from the column-default `[0..1]`.
/// Mirrors the user-stated rule "always include column type; include
/// multiplicity only when not [0..1]".
fn render_column_header(col: &ParsedColumn) -> String {
    let mut out = col.name.to_string();
    out.push(':');
    out.push_str(col.type_tag.pure_type_name());
    let mult_str = render_multiplicity_suffix(&col.multiplicity);
    if let Some(s) = mult_str {
        out.push_str(&s);
    }
    out
}

/// Render a multiplicity bracket suffix (`[1]`, `[*]`, `[1..*]`,
/// `[m..n]`), returning `None` for the column-default `[0..1]` which is
/// elided from the header.
fn render_multiplicity_suffix(mult: &Multiplicity) -> Option<String> {
    match mult {
        Multiplicity::ZeroOrOne => None,
        Multiplicity::PureOne => Some("[1]".to_string()),
        Multiplicity::OneOrMany => Some("[1..*]".to_string()),
        Multiplicity::ZeroOrMany => Some("[*]".to_string()),
        Multiplicity::Range { lower, upper: None } => Some(format!("[{lower}..*]")),
        Multiplicity::Range {
            lower,
            upper: Some(u),
        } => {
            if *lower == *u {
                Some(format!("[{lower}]"))
            } else {
                Some(format!("[{lower}..{u}]"))
            }
        }
        Multiplicity::Variable(name) => Some(format!("[{name}]")),
    }
}

/// Render a single [`TypedCell`] (or `None`) to its canonical CSV form.
///
/// `None` (absent cell) renders as the empty string — both `null` literal
/// input and bare-empty input round-trip through this unified canonical
/// form per the user-stated rule.
#[allow(clippy::match_same_arms)] // Decimal/StrictDate/DateTime each produce a `SmolStr.to_string()` arm — keep them separate so future formatting tweaks per type are local edits
fn render_cell(cell: Option<&TypedCell>) -> String {
    match cell {
        None => String::new(),
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
            // Bare verbatim — strings round-trip unquoted in the
            // canonical CSV. The parser still accepts the legacy
            // single-quoted form for backwards compatibility; the
            // renderer always emits the plain form. (Strings carrying
            // commas, newlines, or leading/trailing whitespace are
            // out of scope for this canonical form.)
            s.to_string()
        }
        Some(TypedCell::StrictDate(s)) => s.to_string(),
        Some(TypedCell::DateTime(s)) => s.to_string(),
    }
}

/// Allocate a synthetic heap object representing one TDS row, with one
/// slot per non-empty cell named after its column. Classifier is
/// `meta::pure::metamodel::type::Any` — property access reads slots by
/// name regardless of classifier, and `Any` avoids accidentally
/// matching native-side classifier dispatch (e.g. `RelationType` /
/// `TDS`) that other natives use to recognise their own argument
/// shapes.
///
/// Used by every lambda-per-row relation native (`filter` today;
/// `extend`, `sort`, `groupBy` follow the same pattern). The lambda
/// then invokes via `ctx.call_function(&lambda, &[Value::Object(handle)])`
/// and reads the row's columns through standard `PropertyCall` slot
/// lookup at runtime.
#[allow(clippy::result_large_err)]
pub fn build_row_tuple(
    columns: &[ParsedColumn],
    row: &[Option<TypedCell>],
    relation_type: Option<&ObjectHandle>,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS_TUPLE);
    for (col, cell) in columns.iter().zip(row.iter()) {
        let Some(cell) = cell else {
            continue;
        };
        let value = typed_cell_to_value(cell);
        ctx.heap_mut()
            .mutate_add(&handle, col.name.as_str(), &[value])
            .map_err(PureException::from)?;
    }
    // Java parity: each row's `classifierGenericType.rawType` is set to
    // the parent TDS's structural `RelationType`. That's what makes
    // Pure-level `$row.colName` Column-application + structural
    // `$row->instanceOf((cols))` resolve against `T` (the row type)
    // rather than against `TDSTuple` itself. Skipped when no
    // RelationType is supplied (legacy callers without a TDS context).
    if let Some(rt) = relation_type {
        let row_gt = ctx.heap_mut().alloc_dynamic(m3_paths::GENERIC_TYPE);
        ctx.heap_mut()
            .mutate_add(&row_gt, "rawType", &[Value::Object(rt.clone())])
            .map_err(PureException::from)?;
        ctx.heap_mut()
            .mutate_add(&handle, "classifierGenericType", &[Value::Object(row_gt)])
            .map_err(PureException::from)?;
    }
    Ok(handle)
}

/// Convert a [`TypedCell`] into a runtime [`Value`].
///
/// `Decimal`/`StrictDate`/`DateTime` map to `Value::String` for now;
/// the runtime's typed-cell handling for those is its own follow-up,
/// and the underlying storage in `TypedCell` is already a string.
/// `String` likewise — the smolstr is widened.
pub fn typed_cell_to_value(cell: &TypedCell) -> Value {
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

/// Materialise a `TDS` heap object from a [`ParsedTDS`] — the single
/// place the runtime TDS shape is written. The inverse of
/// [`read_parsed_tds`]. Used by `stringToTDS` and by every relation
/// native that produces a derived TDS.
///
/// Two things land on the object:
///
/// 1. **`rows`** — one row tuple per [`ParsedTDS::rows`] entry, built by
///    [`build_row_tuple`] (column-named slots, empty cells skipped). This
///    is the source of truth.
/// 2. **`classifierGenericType`** — the inferred column `RelationType`,
///    encoded as `GenericType{rawType=TDS, typeArguments=[GenericType{
///    rawType=RelationType{columns=[Column…]}}]}`. Mirrors Java's
///    `TDSExtension.parse`, which sets `classifierGenericType` even for a
///    bare `stringToTDS` call, and is what lets [`read_parsed_tds`]
///    recover per-column names/types/multiplicities (so the derived `csv`
///    and downstream natives render typed cells correctly).
///
/// Model-dependent element ids are resolved up front (the `model()`
/// borrow is released before the `heap_mut()` mutation phase begins —
/// `EvalContextTrait` ties both to `&self`).
#[allow(clippy::result_large_err)]
pub fn alloc_tds_from_parsed(
    ctx: &mut dyn EvalContextTrait,
    parsed: &ParsedTDS,
) -> Result<ObjectHandle, PureException> {
    // --- resolve model-dependent ids (model borrow released here) ---
    let column_class = m3_paths::resolve(ctx.model(), m3_paths::COLUMN);
    let tds_class = m3_paths::resolve(ctx.model(), m3_paths::TDS);
    let col_type_ids: Vec<Option<ElementId>> = parsed
        .columns
        .iter()
        .map(|c| column_type_to_element_id(ctx.model(), &c.type_tag))
        .collect();

    // --- allocate + mutate (heap only) ---
    let tds = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);

    // Hoist the column `RelationType` allocation ahead of the row
    // tuples so each row tuple can capture it as its
    // `classifierGenericType.rawType` (Java parity: TDSExtension's
    // classifier-override). Without this the row's only static-type
    // record is its `Any` classifier, and `$row->instanceOf((cols))`
    // has nothing structural to compare against.
    let mut col_objs: Vec<Value> = Vec::with_capacity(parsed.columns.len());
    for (i, col) in parsed.columns.iter().enumerate() {
        let col_obj = alloc_tds_column(
            ctx.heap_mut(),
            &col.name,
            col_type_ids[i],
            column_class,
            &col.multiplicity,
        )?;
        col_objs.push(Value::Object(col_obj));
    }
    let relation_type = ctx.heap_mut().alloc_dynamic(m3_paths::RELATION_TYPE);
    if !col_objs.is_empty() {
        ctx.heap_mut()
            .mutate_add(&relation_type, "columns", &col_objs)
            .map_err(PureException::from)?;
    }

    // rows: source of truth — each row carries the shared
    // `RelationType` as its classifierGenericType.rawType.
    let mut row_objs: Vec<Value> = Vec::with_capacity(parsed.rows.len());
    for row in &parsed.rows {
        let row_obj = build_row_tuple(&parsed.columns, row, Some(&relation_type), ctx)?;
        row_objs.push(Value::Object(row_obj));
    }
    if !row_objs.is_empty() {
        ctx.heap_mut()
            .mutate_add(&tds, "rows", &row_objs)
            .map_err(PureException::from)?;
    }
    let rt_gt = ctx.heap_mut().alloc_dynamic(m3_paths::GENERIC_TYPE);
    ctx.heap_mut()
        .mutate_add(&rt_gt, "rawType", &[Value::Object(relation_type)])
        .map_err(PureException::from)?;
    let tds_gt = ctx.heap_mut().alloc_dynamic(m3_paths::GENERIC_TYPE);
    ctx.heap_mut()
        .mutate_add(
            &tds_gt,
            "rawType",
            &[tds_class.map_or(Value::Unit, Value::Element)],
        )
        .map_err(PureException::from)?;
    ctx.heap_mut()
        .mutate_add(&tds_gt, "typeArguments", &[Value::Object(rt_gt)])
        .map_err(PureException::from)?;
    ctx.heap_mut()
        .mutate_add(&tds, "classifierGenericType", &[Value::Object(tds_gt)])
        .map_err(PureException::from)?;
    Ok(tds)
}

/// Allocate a single `Column` heap object for a TDS's inferred
/// `RelationType`. Mirrors [`crate::relation::alloc_column`] but takes a
/// pre-resolved `Column` classifier id + column type element id, so it
/// needs only `&mut RuntimeHeap` (no `&PureModel`) and can run inside a
/// native after the `model()` borrow is released.
#[allow(clippy::result_large_err)]
fn alloc_tds_column(
    heap: &mut RuntimeHeap,
    name: &SmolStr,
    type_elem: Option<ElementId>,
    column_class: Option<ElementId>,
    multiplicity: &Multiplicity,
) -> Result<ObjectHandle, PureException> {
    let mult_obj = alloc_multiplicity(heap, multiplicity)?;

    let inner_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(
        &inner_gt,
        "rawType",
        &[type_elem.map_or(Value::Unit, Value::Element)],
    )
    .map_err(PureException::from)?;

    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(
        &outer_gt,
        "rawType",
        &[column_class.map_or(Value::Unit, Value::Element)],
    )
    .map_err(PureException::from)?;
    // typeArguments[0] is the implicit source-RelationType slot (left
    // null, matching `_Column`); slot[1] carries the column's type GT.
    heap.mutate_add(
        &outer_gt,
        "typeArguments",
        &[Value::Unit, Value::Object(inner_gt)],
    )
    .map_err(PureException::from)?;
    heap.mutate_add(
        &outer_gt,
        "multiplicityArguments",
        &[Value::Object(mult_obj)],
    )
    .map_err(PureException::from)?;

    let column = heap.alloc_dynamic(m3_paths::COLUMN);
    heap.mutate_add(&column, "name", &[Value::String(name.clone())])
        .map_err(PureException::from)?;
    heap.mutate_add(&column, "nameWildCard", &[Value::Boolean(false)])
        .map_err(PureException::from)?;
    heap.mutate_add(&column, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(column)
}

/// Resolve a [`ColumnType`] to its Pure type element id — the inverse of
/// [`element_id_to_column_type`]. Primitives resolve by their bare name
/// (auto-imported); an `Other` annotation resolves by its `::`-joined
/// package path plus class name. `None` if the model can't resolve it
/// (then the column's type GT carries no `rawType` element).
fn column_type_to_element_id(model: &PureModel, column_type: &ColumnType) -> Option<ElementId> {
    let mut segments: Vec<SmolStr> = match column_type.pure_type_package() {
        Some(pkg) => pkg.split("::").map(SmolStr::new).collect(),
        None => Vec::new(),
    };
    segments.push(SmolStr::new(column_type.pure_type_name()));
    model.resolve_by_path(&segments)
}
