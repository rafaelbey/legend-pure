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

//! Helper shared by `ascending` / `descending`: allocate a fresh
//! `meta::pure::functions::relation::SortInfo` heap instance with
//! `column` populated from the caller's `ColSpec` argument and
//! `direction` populated with the named `SortType` enum member.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::EvalContextTrait;
use crate::value::Value;

use super::shared::unwrap_instance_value;

/// Allocate a fresh `SortInfo` heap object, populate `column` from
/// `args[0]` (a `ColSpec` value spec) and `direction` from the named
/// member of the `SortType` enumeration, then return its handle.
#[allow(clippy::result_large_err)]
pub(super) fn alloc_sort_info(
    ctx: &mut dyn EvalContextTrait,
    args: &[ValueSpec],
    direction_member: &'static str,
) -> Result<ObjectHandle, PureException> {
    let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);
    let sort_type_id = m3_paths::resolve(ctx.model(), m3_paths::SORT_TYPE).ok_or_else(|| {
        PureException::from(PureRuntimeError::EvaluationError(
            "ascending/descending: SortType enumeration not resolvable".into(),
        ))
    })?;

    let col_value = ctx.evaluate(&args[0])?.into_value();
    let col_obj = unwrap_instance_value(&col_value, instance_value_id, ctx)?;

    let direction = Value::EnumValue {
        enum_id: sort_type_id,
        member: direction_member.into(),
    };

    let sort_info = ctx.heap_mut().alloc_dynamic(m3_paths::SORT_INFO);
    ctx.heap_mut()
        .mutate_add(&sort_info, "column", &[Value::Object(col_obj)])
        .map_err(PureException::from)?;
    ctx.heap_mut()
        .mutate_add(&sort_info, "direction", &[direction])
        .map_err(PureException::from)?;
    Ok(sort_info)
}
