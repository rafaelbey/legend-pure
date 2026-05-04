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

//! `select(rel, ColSpec)` / `select(rel, ColSpecArray)` — project a TDS
//! down to a named subset of columns. Two overloads, each with its own
//! mangled key. Both share the same TDS-side projection logic; they
//! differ only in how they read the requested column-name list off the
//! second argument's heap shape.
//!
//! Single ColSpec literal carries the column name on its `name` slot
//! directly (`alloc_col_spec_literal` in `relation.rs:298`); the
//! ColSpecArray literal carries an ordered `names: String[*]` slot
//! (`alloc_col_spec_array_literal` in `relation.rs:256`). Either way,
//! we project `parsed.columns` and each `parsed.rows[i]` to the same
//! requested-name index list, render canonical CSV, and allocate a
//! fresh TDS heap object.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_dsl_tds::csv::ParsedTDS;
use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, unwrap_instance_value};

/// `select<T,Z>(r:Relation<T>[1], cs:ColSpec<Z⊆T>[1]):Relation<Z>[1]`.
///
/// One column. The column's name lives on the ColSpec's `name` slot.
#[derive(Debug)]
pub struct SelectColSpec;

impl NativeFunction for SelectColSpec {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("select (Relation, ColSpec)", args, 2)?;
        let names = read_col_spec_name(args, ctx)?;
        project_tds(args, &names, ctx)
    }

    fn signature(&self) -> &'static str {
        "select(Relation<T>[1], ColSpec<Z⊆T>[1]):Relation<Z>[1]"
    }
}

/// `select<T,Z>(r:Relation<T>[1], csa:ColSpecArray<Z⊆T>[1]):Relation<Z>[1]`.
///
/// Multiple columns. The names live on the ColSpecArray's `names` slot
/// (ordered, same order as the literal text).
#[derive(Debug)]
pub struct SelectColSpecArray;

impl NativeFunction for SelectColSpecArray {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("select (Relation, ColSpecArray)", args, 2)?;
        let names = read_col_spec_array_names(args, ctx)?;
        project_tds(args, &names, ctx)
    }

    fn signature(&self) -> &'static str {
        "select(Relation<T>[1], ColSpecArray<Z⊆T>[1]):Relation<Z>[1]"
    }
}

// ---------------------------------------------------------------------------
// Name extraction
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn read_col_spec_name(
    args: &[ValueSpec],
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<SmolStr>, PureException> {
    let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
    let cs_value = ctx.evaluate(&args[1])?.into_value();
    let cs_obj = unwrap_instance_value(&cs_value, instance_value_id, ctx)?;
    let name_values = ctx
        .heap()
        .get_property_values(&cs_obj, "name")
        .map_err(PureException::from)?;
    let name = name_values
        .iter()
        .find_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "select: ColSpec.name slot missing or not a String".into(),
            ))
        })?;
    Ok(vec![name])
}

#[allow(clippy::result_large_err)]
fn read_col_spec_array_names(
    args: &[ValueSpec],
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<SmolStr>, PureException> {
    let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
    let csa_value = ctx.evaluate(&args[1])?.into_value();
    let csa_obj = unwrap_instance_value(&csa_value, instance_value_id, ctx)?;
    let name_values = ctx
        .heap()
        .get_property_values(&csa_obj, "names")
        .map_err(PureException::from)?;
    if name_values.is_empty() {
        return Err(PureException::from(PureRuntimeError::EvaluationError(
            "select: ColSpecArray.names slot is empty".into(),
        )));
    }
    let mut out = Vec::with_capacity(name_values.len());
    for v in name_values.iter() {
        match v {
            Value::String(s) => out.push(s.clone()),
            other => {
                return Err(PureException::from(PureRuntimeError::EvaluationError(
                    format!("select: ColSpecArray.names entry is not a String: {other:?}"),
                )));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Projection (shared)
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn project_tds(
    args: &[ValueSpec],
    requested: &[SmolStr],
    ctx: &mut dyn EvalContextTrait,
) -> Result<Evaluated, PureException> {
    let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
    let rel_value = ctx.evaluate(&args[0])?.into_value();
    let tds_obj = unwrap_instance_value(&rel_value, instance_value_id, ctx)?;
    let parsed = read_parsed_tds("select", &tds_obj, ctx)?;

    // Resolve each requested name to its index in the source columns.
    // Order follows the request, not the source — matches Java semantics
    // (`select(~[other, val])` reorders).
    let mut indices: Vec<usize> = Vec::with_capacity(requested.len());
    for name in requested {
        let idx = parsed
            .columns
            .iter()
            .position(|c| c.name.as_str() == name.as_str())
            .ok_or_else(|| {
                let available: Vec<&str> =
                    parsed.columns.iter().map(|c| c.name.as_str()).collect();
                PureException::from(PureRuntimeError::EvaluationError(format!(
                    "select: column '{name}' not present in receiver; have {available:?}"
                )))
            })?;
        indices.push(idx);
    }

    let new_columns: Vec<_> = indices.iter().map(|&i| parsed.columns[i].clone()).collect();
    let new_rows: Vec<Vec<_>> = parsed
        .rows
        .iter()
        .map(|row| indices.iter().map(|&i| row[i].clone()).collect())
        .collect();

    let projected = ParsedTDS {
        csv: parsed.csv.clone(),
        columns: new_columns,
        rows: new_rows,
    };
    let new_csv = super::shared::render_canonical_csv(&projected);
    let new_tds: ObjectHandle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
    ctx.heap_mut()
        .mutate_add(&new_tds, "csv", &[Value::String(new_csv.into())])
        .map_err(PureException::from)?;
    Ok(Evaluated::new(Value::Object(new_tds)))
}
