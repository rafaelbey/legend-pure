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

//! `rename(rel:Relation<T>[1], old:ColSpec<Z⊆T>[1], new:ColSpec<V>[1])
//!   :Relation<T-Z+V>[1]` — relabel a single TDS column.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_dsl_tds::csv::ParsedColumn;
use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, render_csv_from_columns_and_rows, unwrap_instance_value};

/// Pure
/// `rename<T,Z,K,V>(r:Relation<T>[1], old:ColSpec<Z=(?:K)⊆T>[1],
///                  new:ColSpec<V=(?:K)>[1]):Relation<T-Z+V>[1]`.
///
/// Renames a single column in a TDS. The `old` ColSpec selects an
/// existing column by name; the `new` ColSpec carries the replacement
/// name. Cell data is preserved verbatim — only the column header
/// changes. Column position is preserved (the rename happens in place
/// in the column list), matching the platform PCT
/// `testSimpleRenameShared` expectation that the renamed column keeps
/// its slot in `'#TDS\n   val,newStr\n…'`.
///
/// Errors when:
/// - `old` names a column not present on the relation,
/// - `new`'s name already exists on a different column (collision).
///
/// Mirrors the pattern of [`super::distinct`] / [`super::drop`]: unwrap
/// receiver → reparse canonical CSV → mutate the parsed column list →
/// re-render via [`render_csv_from_columns_and_rows`] → allocate a
/// fresh `TDS` heap object carrying the new CSV.
#[derive(Debug)]
pub struct Rename;

impl NativeFunction for Rename {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("rename (Relation, ColSpec, ColSpec)", args, 3)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);

        // -- Receiver: TDS -----------------------------------------------
        let rel_value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&rel_value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("rename", &tds_obj, ctx)?;

        // -- ColSpec args: read the `name` slot directly -----------------
        let from_value = ctx.evaluate(&args[1])?.into_value();
        let from_obj = unwrap_instance_value(&from_value, instance_value_id, ctx)?;
        let from_name = read_col_spec_name("rename (old)", &from_obj, ctx)?;

        let to_value = ctx.evaluate(&args[2])?.into_value();
        let to_obj = unwrap_instance_value(&to_value, instance_value_id, ctx)?;
        let to_name = read_col_spec_name("rename (new)", &to_obj, ctx)?;

        // -- Locate source column ---------------------------------------
        let source_idx = parsed
            .columns
            .iter()
            .position(|c| c.name == from_name)
            .ok_or_else(|| {
                PureException::from(PureRuntimeError::EvaluationError(format!(
                    "rename: column '{from_name}' not found in relation"
                )))
            })?;

        // -- Reject collision with another column -----------------------
        if from_name != to_name
            && parsed
                .columns
                .iter()
                .enumerate()
                .any(|(i, c)| i != source_idx && c.name == to_name)
        {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("rename: target column name '{to_name}' already exists in relation"),
            )));
        }

        // -- Mutate the column entry in place ---------------------------
        let mut updated_columns: Vec<ParsedColumn> = parsed.columns.clone();
        updated_columns[source_idx].name = to_name;

        // -- Render and allocate fresh TDS ------------------------------
        let csv = render_csv_from_columns_and_rows(&updated_columns, &parsed.rows);
        let tds_handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&tds_handle, "csv", &[Value::String(csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }

    fn signature(&self) -> &'static str {
        "rename(Relation<T>[1], ColSpec<Z>[1], ColSpec<V>[1]):Relation<T-Z+V>[1]"
    }
}

/// Read a single-column `ColSpec`'s `name:String[1]` slot. The runtime
/// allocator (`relation::alloc_col_spec_literal`) populates this slot
/// directly with the column name carried by the `~name` source-form
/// literal.
#[allow(clippy::result_large_err)]
fn read_col_spec_name(
    fn_name: &'static str,
    cs_obj: &ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<SmolStr, PureException> {
    let names = ctx
        .heap()
        .get_property_values(cs_obj, "name")
        .map_err(PureException::from)?;
    names
        .iter()
        .find_map(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "{fn_name}: ColSpec.name slot is missing or not a String"
            )))
        })
}
