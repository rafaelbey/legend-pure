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

//! `stringToTDS(s:String[1]):TDS<Any>[1]`.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::native::relation::shared::alloc_tds_from_parsed;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

/// Pure
/// `stringToTDS(s:String[1]):TDS<Any>[1]`.
///
/// Parses a CSV string into a runtime [`TDS`](crate::m3_paths::TDS) heap
/// instance. The shared parser and per-column type inference live in
/// [`legend_pure_dsl_tds::csv::parse_and_infer`] — same call the
/// compile-time `#TDS\n…\n#` lowerer makes — so a `#TDS#` literal and
/// a literal `stringToTDS('<csv>')` call produce equivalent runtime
/// instances. The compile-time difference is only the inferred type
/// parameter `T`: `#TDS#` lowers to `stringToTDS(csv)->cast(@TDS<…>)`,
/// supplying a typed `T`; a bare `stringToTDS` call stays at `TDS<Any>`.
///
/// The resulting heap object carries the typed `rows` (source of truth)
/// plus the inferred column `RelationType` on `classifierGenericType` —
/// see [`alloc_tds_from_parsed`]. The `csv` property is *derived* from
/// `rows` by the `tdsToCsv` native (which backs `TDS.csv()`), no longer
/// a stored slot. Attaching the inferred `RelationType` even for a bare
/// `stringToTDS` call mirrors Java's `TDSExtension.parse`.
#[derive(Debug)]
pub struct StringToTDS;

impl NativeFunction for StringToTDS {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("stringToTDS", args, 1)?;
        let csv_value = ctx.evaluate(&args[0])?.into_value();
        let csv_str = csv_value.as_string()?.clone();

        let parsed =
            legend_pure_dsl_tds::csv::parse_and_infer(csv_str.as_str(), &[]).map_err(|e| {
                PureException::from(PureRuntimeError::EvaluationError(format!(
                    "stringToTDS: {e}"
                )))
            })?;

        let tds_handle = alloc_tds_from_parsed(ctx, &parsed)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }
}
