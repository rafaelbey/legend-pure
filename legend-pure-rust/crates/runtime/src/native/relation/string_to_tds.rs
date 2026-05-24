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
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

/// Pure
/// `stringToTDS(s:String[1]):TDS<Any>[1]`.
///
/// Parses a CSV string into a runtime [`TDS`](crate::m3_paths::TDS) heap
/// instance whose `csv` slot carries the trimmed canonical CSV. The
/// shared parser and per-column type inference live in
/// [`legend_pure_dsl_tds::csv::parse_and_infer`] — same call the
/// compile-time `#TDS\n…\n#` lowerer makes — so a `#TDS#` literal and
/// a literal `stringToTDS('<csv>')` call produce equivalent runtime
/// instances. The compile-time difference is only the inferred type
/// parameter `T`: `#TDS#` lowers to `stringToTDS(csv)->cast(@TDS<…>)`,
/// supplying a typed `T`; a bare `stringToTDS` call stays at `TDS<Any>`.
///
/// The structured per-column / per-cell representation (`ParsedTDS`)
/// isn't stashed on the heap object yet — relation natives that
/// consume typed columns (`size`, `over`, `extend`, `sort`, …) re-call
/// `parse_and_infer` on demand via `super::shared::read_parsed_tds`.
/// That's deferred work; for now, the only Pure-observable slot is
/// `csv: String[1]`, which is what the metaclass declares.
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

        let tds_handle = ctx.heap_mut().alloc_dynamic(m3_paths::TDS);
        ctx.heap_mut()
            .mutate_add(&tds_handle, "csv", &[Value::String(parsed.csv.into())])
            .map_err(PureException::from)?;
        Ok(Evaluated::new(Value::Object(tds_handle)))
    }
}
