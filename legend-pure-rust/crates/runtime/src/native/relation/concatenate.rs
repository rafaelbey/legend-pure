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

//! `concatenate(rel1:Relation<T>[1], rel2:Relation<T>[1]):Relation<T>[1]`.
//!
//! Vertically appends `rel2`'s rows to `rel1`'s rows. Both inputs must
//! agree on column count, names, and column types (multiplicity is
//! data-derived and may legitimately differ — e.g. `[1]` vs `[0..1]` —
//! so the output's multiplicities are re-inferred via `parse_and_infer`
//! when downstream natives reparse the result).

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, render_csv_from_columns_and_rows, unwrap_instance_value};

/// Pure
/// `concatenate<T>(rel1:Relation<T>[1], rel2:Relation<T>[1]):Relation<T>[1]`.
///
/// Validates that the two relations have a compatible schema (same
/// column count, identical column names in order, identical
/// [`ColumnType`](legend_pure_dsl_tds::csv::ColumnType) per column) and
/// returns a new TDS whose rows are `rel1.rows ++ rel2.rows`. The
/// canonical CSV of the result is rendered from `rel1`'s column header
/// followed by the concatenated row data; reparsing it via
/// `parse_and_infer` yields a `ParsedTDS` with the unioned data and
/// possibly relaxed multiplicities (e.g. `[1]` → `[0..1]` if the
/// concatenation introduces nulls).
#[derive(Debug)]
pub struct Concatenate;

impl NativeFunction for Concatenate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("concatenate (Relation)", args, 2)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);

        let v1 = ctx.evaluate(&args[0])?.into_value();
        let tds1 = unwrap_instance_value(&v1, instance_value_id, ctx)?;
        let p1 = read_parsed_tds("concatenate", &tds1, ctx)?;

        let v2 = ctx.evaluate(&args[1])?.into_value();
        let tds2 = unwrap_instance_value(&v2, instance_value_id, ctx)?;
        let p2 = read_parsed_tds("concatenate", &tds2, ctx)?;

        // Schema compatibility: column count, names, and types must
        // match. Multiplicity is data-derived and intentionally not
        // compared — two compatible TDS may differ on `[1]` vs `[0..1]`.
        if p1.columns.len() != p2.columns.len() {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "concatenate: schema mismatch — left has {} column(s), right has {}",
                    p1.columns.len(),
                    p2.columns.len(),
                ),
            )));
        }
        for (i, (l, r)) in p1.columns.iter().zip(p2.columns.iter()).enumerate() {
            if l.name != r.name {
                return Err(PureException::from(PureRuntimeError::EvaluationError(
                    format!(
                        "concatenate: column {} name mismatch — left '{}' vs right '{}'",
                        i, l.name, r.name,
                    ),
                )));
            }
            if l.type_tag != r.type_tag {
                return Err(PureException::from(PureRuntimeError::EvaluationError(
                    format!(
                        "concatenate: column '{}' type mismatch — left `{}` vs right `{}`",
                        l.name,
                        l.type_tag.pure_type_name(),
                        r.type_tag.pure_type_name(),
                    ),
                )));
            }
        }

        let mut rows = Vec::with_capacity(p1.rows.len() + p2.rows.len());
        rows.extend(p1.rows.iter().cloned());
        rows.extend(p2.rows.iter().cloned());

        let new_csv = render_csv_from_columns_and_rows(&p1.columns, &rows);
        let tds_handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&tds_handle, "csv", &[Value::String(new_csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }

    fn signature(&self) -> &'static str {
        "concatenate(Relation<T>[1], Relation<T>[1]):Relation<T>[1]"
    }
}
