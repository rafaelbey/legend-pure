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
//
//! `columns(rel:Relation<T>[1]):Column<T>[*]` — read TDS column metadata.
//!
//! Returns a Pure-side multi-value of `Column` heap objects, one per
//! column in the receiver TDS. Each Column object carries at least the
//! `name: String[1]` slot (the canonical PCT test reads `$t->columns().name`).
//! Re-parses the receiver's `csv` slot via [`super::shared::read_parsed_tds`]
//! to enumerate columns; allocator does not retain a parsed structure
//! on the heap object — see the note in `shared.rs`.
//!
//! The output multiplicity `[*]` is encoded as `Value::Collection`
//! (RRB-tree of `Value::Object`); a single-column TDS still returns a
//! one-element Collection (Pure's Many representation, not unwrapped to
//! a bare `Value::Object`).

#![allow(clippy::needless_pass_by_value)]

use im_rc::Vector as PVector;
use legend_pure_parser_pure::types::ValueSpec;

use crate::error::PureException;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_parsed_tds, unwrap_instance_value};

/// Pure
/// `columns<T>(rel:Relation<T>[1]):Column<T>[*]`.
#[derive(Debug)]
pub struct Columns;

impl NativeFunction for Columns {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("columns (Relation)", args, 1)?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
        let value = ctx.evaluate(&args[0])?.into_value();
        let tds_obj = unwrap_instance_value(&value, instance_value_id, ctx)?;
        let parsed = read_parsed_tds("columns", &tds_obj, ctx)?;

        let mut out: PVector<Value> = PVector::new();
        for col in &parsed.columns {
            let column_obj = ctx.heap_mut().alloc_dynamic(m3_paths::COLUMN);
            ctx.heap_mut()
                .mutate_add(&column_obj, "name", &[Value::String(col.name.clone())])
                .map_err(PureException::from)?;
            ctx.heap_mut()
                .mutate_add(&column_obj, "nameWildCard", &[Value::Boolean(false)])
                .map_err(PureException::from)?;
            out.push_back(Value::Object(column_obj));
        }
        Ok(Evaluated::new(Value::Collection(Box::new(out))))
    }

    fn signature(&self) -> &'static str {
        "columns(Relation<T>[1]):Column<T>[*]"
    }
}
