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

//! `distinct(rel:Relation<T>[1]):Relation<T>[1]` — deduplicate rows.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_dsl_tds::csv::ParsedTDS;
use legend_pure_parser_pure::types::ValueSpec;

use crate::error::PureException;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, render_canonical_csv, unwrap_instance_value};

/// Pure
/// `distinct<T>(rel:Relation<T>[1]):Relation<T>[1]`.
///
/// Drops duplicate rows from a TDS, preserving first-occurrence order.
/// Two rows are equal iff their columns hold the same `Option<TypedCell>`
/// values (so `None == None` — empty cells deduplicate alongside each
/// other, matching Java's `null == null` semantics for TDS distinct).
///
/// `TypedCell` carries `Float(f64)`, which precludes `Eq`/`Hash`, so we
/// scan the kept-rows accumulator linearly via `contains` rather than
/// hashing. Quadratic in row count, fine for the row-counts the runtime
/// sees today; if a future workload spikes, swap in an order-preserving
/// dedup (e.g. an `IndexSet` keyed on a wrapper that bit-eq-hashes
/// `f64`).
#[derive(Debug)]
pub struct Distinct;

impl NativeFunction for Distinct {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("distinct (Relation)", args, 1)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
        let value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("distinct", &tds_obj, ctx)?;

        // Order-preserving dedup. O(n²) by design — see struct doc.
        let mut deduped: Vec<Vec<Option<_>>> = Vec::with_capacity(parsed.rows.len());
        for row in &parsed.rows {
            if !deduped.iter().any(|kept| kept == row) {
                deduped.push(row.clone());
            }
        }

        let result = ParsedTDS {
            csv: parsed.csv.clone(),
            columns: parsed.columns.clone(),
            rows: deduped,
        };
        let new_csv = render_canonical_csv(&result);
        let tds_handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&tds_handle, "csv", &[Value::String(new_csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }
}
