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

//! `descending(c:ColSpec<T>[1]):SortInfo<T>[1]` — construct a `SortInfo`
//! with `direction = SortType.DESC`.
//!
//! Mirror of [`super::ascending`]; the only difference is the enum member
//! used for the `direction` slot. See that file for the rationale on
//! short-circuiting the Pure body.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::PureException;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::sort_info::alloc_sort_info;

/// Pure
/// `descending<T>(c:ColSpec<T>[1]):SortInfo<T>[1]`.
#[derive(Debug)]
pub struct Descending;

impl NativeFunction for Descending {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("descending", args, 1)?;
        Ok(Evaluated::new(Value::Object(alloc_sort_info(
            ctx, args, "DESC",
        )?)))
    }
}
