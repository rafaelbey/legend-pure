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

//! `addColumns(RelationType<Any>[1], ColSpecArray<Any>[1]):RelationType<Any>[1]`.

#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use crate::value::Value;

use super::shared::{read_col_spec_array_columns, unwrap_instance_value};

/// Pure
/// `addColumns(source:RelationType<Any>[1], colSpec:ColSpecArray<Any>[1])
///   :RelationType<Any>[1]`.
///
/// Mirrors Java's `AddColumns.java:47-50`:
/// 1. Unwrap the source argument from any `InstanceValue` wrapper, take
///    its `columns` slot.
/// 2. Unwrap the colSpec argument, navigate
///    `classifierGenericType.typeArguments[0].rawType.columns` to get
///    the new columns the literal carries.
/// 3. Concatenate; allocate a fresh `RelationType` whose `columns` slot
///    holds the merged list.
///
/// The Java reference clones each column via `_Column.getColumnInstance`;
/// this implementation reuses the existing Column `ObjectIds`. The two
/// platform tests (`testAddColumns`, `testAddColumnsAfterEvaluateAndDeactivate`)
/// only read column metadata, never mutate it through aliased Column
/// references, so aliasing is safe. Promote to clone-on-merge if a
/// future test surface needs it.
#[derive(Debug)]
pub struct AddColumns;

impl NativeFunction for AddColumns {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("addColumns", args, 2)?;

        let relation_type_id =
            m3_paths::resolve(ctx.model(), m3_paths::RELATION_TYPE).ok_or_else(|| {
                PureException::from(PureRuntimeError::EvaluationError(
                    "addColumns: RelationType element not resolvable".into(),
                ))
            })?;
        let col_spec_array_id = m3_paths::resolve(ctx.model(), m3_paths::COL_SPEC_ARRAY)
            .ok_or_else(|| {
                PureException::from(PureRuntimeError::EvaluationError(
                    "addColumns: ColSpecArray element not resolvable".into(),
                ))
            })?;
        let instance_value_id = m3_paths::resolve(ctx.model(), m3_paths::INSTANCE_VALUE);

        let source_value = ctx.evaluate(&args[0])?.into_value();
        let source_obj = unwrap_instance_value(&source_value, instance_value_id, ctx)?;
        let source_classifier_id = ctx
            .heap()
            .classifier(&source_obj)
            .map_err(PureException::from)
            .and_then(|c| {
                m3_paths::resolve(ctx.model(), &c).ok_or_else(|| {
                    PureException::from(PureRuntimeError::EvaluationError(format!(
                        "addColumns: source classifier '{c}' not resolvable"
                    )))
                })
            })?;
        if source_classifier_id != relation_type_id {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                "addColumns: source argument is not a RelationType".into(),
            )));
        }
        let source_cols = ctx
            .heap()
            .get_property_values(&source_obj, "columns")
            .map_err(PureException::from)?;
        let mut merged: Vec<Value> = source_cols.iter().cloned().collect();

        let cs_value = ctx.evaluate(&args[1])?.into_value();
        let cs_obj = unwrap_instance_value(&cs_value, instance_value_id, ctx)?;
        let cs_classifier_id = ctx
            .heap()
            .classifier(&cs_obj)
            .map_err(PureException::from)
            .and_then(|c| {
                m3_paths::resolve(ctx.model(), &c).ok_or_else(|| {
                    PureException::from(PureRuntimeError::EvaluationError(format!(
                        "addColumns: colSpec classifier '{c}' not resolvable"
                    )))
                })
            })?;
        if cs_classifier_id != col_spec_array_id {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                "addColumns: colSpec argument is not a ColSpecArray".into(),
            )));
        }
        let inner_cols = read_col_spec_array_columns(cs_obj, ctx)?;
        merged.extend(inner_cols);

        let new_rt = ctx.heap_mut().alloc_dynamic(m3_paths::RELATION_TYPE);
        if !merged.is_empty() {
            ctx.heap_mut()
                .mutate_add(&new_rt, "columns", &merged)
                .map_err(PureException::from)?;
        }
        Ok(Evaluated::new(Value::Object(new_rt)))
    }
}
