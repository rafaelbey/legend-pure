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

//! `tdsToCsv<T>(tds:TDS<T>[1]):String[1]`.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::PureException;
use crate::m3_paths;
use crate::native::relation::shared::{
    read_parsed_tds, render_canonical_csv, unwrap_instance_value,
};
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

/// Pure `tdsToCsv<T>(tds:TDS<T>[1]):String[1]`.
///
/// Renders a `TDS`'s canonical CSV (header line + one line per row) from
/// its `rows` + column `RelationType`. Backs the `TDS.csv()` qualified
/// property — `csv` is derived from `rows` (the source of truth), not a
/// stored slot.
///
/// Reads the structured shape via [`read_parsed_tds`] and renders it with
/// [`render_canonical_csv`], so the output round-trips through
/// `parse_and_infer` (and matches what a `#TDS#` literal's reconstructed
/// CSV would be, modulo canonicalisation of cell values).
#[derive(Debug)]
pub struct TdsToCsv;

impl NativeFunction for TdsToCsv {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("tdsToCsv", args, 1)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
        let value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("tdsToCsv", &tds_obj, ctx)?;
        let csv = render_canonical_csv(&parsed);
        Ok(Evaluated::new(Value::String(csv.into())))
    }
}
