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

//! `size(rel:Relation<T>[1]):Integer[1]` — number of rows in a TDS.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::PureException;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, unwrap_instance_value};

/// Template for any relation-on-TDS native: unwrap → read CSV →
/// reparse → consume the parsed structure.
///
/// `size` returns the row count. The runtime stores only the canonical
/// CSV on a TDS heap object, so we re-parse via
/// `legend_pure_dsl_tds::csv::parse_and_infer` and read
/// `parsed.rows.len()`. Reparsing is wasteful but functionally
/// correct; a follow-up that caches the parsed structure on the heap
/// will lift this cost across all relation natives at once.
#[derive(Debug)]
pub struct Size;

impl NativeFunction for Size {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("size (Relation)", args, 1)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
        let value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("size", &tds_obj, ctx)?;
        let n = i64::try_from(parsed.rows.len()).unwrap_or(i64::MAX);
        Ok(Evaluated::new(Value::Integer(n)))
    }
}
