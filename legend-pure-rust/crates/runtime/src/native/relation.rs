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

//! Native functions over `RelationType` / `Column` / `ColSpecArray`.
//!
//! Mirrors `legend-pure-runtime/.../natives/essentials/meta/type/relation`.
//! All M3 identification is by ElementId (`m3_paths::resolve`), never
//! classifier-string matching — see `feedback_no_classifier_string_compare`.

// Helpers take `ObjectHandle` by value; with `Rc<RefCell<HeapEntry>>`
// that's an O(1) refcount bump rather than a meaningful copy.
#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::m3_paths;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// addColumns
// ---------------------------------------------------------------------------

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

        // -- Source: RelationType — read its columns ---------------------
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

        // -- ColSpec: navigate classifierGenericType chain ---------------
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

        // -- Allocate the fresh RelationType ----------------------------
        let new_rt = ctx.heap_mut().alloc_dynamic(m3_paths::RELATION_TYPE);
        if !merged.is_empty() {
            ctx.heap_mut()
                .mutate_add(&new_rt, "columns", &merged)
                .map_err(PureException::from)?;
        }
        Ok(Evaluated::new(Value::Object(new_rt)))
    }

    fn signature(&self) -> &'static str {
        "addColumns(RelationType<Any>[1], ColSpecArray<Any>[1]):RelationType<Any>[1]"
    }
}

/// Walk `csa.classifierGenericType.typeArguments[0].rawType.columns` and
/// return the column values. Mirrors Java's
/// `((ColSpecArrayInstance)…)._classifierGenericType()._typeArguments()
///   .getFirst()._rawType()._columns()` chain.
#[allow(clippy::result_large_err)]
fn read_col_spec_array_columns(
    cs_obj: ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<Value>, PureException> {
    let cgt = single_object_slot(cs_obj, "classifierGenericType", ctx)?;
    let type_args = ctx
        .heap()
        .get_property_values(&cgt, "typeArguments")
        .map_err(PureException::from)?;
    let first_ta = type_args
        .iter()
        .find_map(|v| match v {
            Value::Object(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(
                "addColumns: ColSpecArray.classifierGenericType.typeArguments[0] missing or not an Object"
                    .into(),
            ))
        })?;
    let raw = single_object_slot(first_ta, "rawType", ctx)?;
    let cols = ctx
        .heap()
        .get_property_values(&raw, "columns")
        .map_err(PureException::from)?;
    Ok(cols.iter().cloned().collect())
}

/// Read a single Object out of `obj.<slot>`.
#[allow(clippy::result_large_err)]
fn single_object_slot(
    obj: ObjectHandle,
    slot: &str,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let values = ctx
        .heap()
        .get_property_values(&obj, slot)
        .map_err(PureException::from)?;
    values
        .iter()
        .find_map(|v| match v {
            Value::Object(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "addColumns: slot '{slot}' missing or not an Object"
            )))
        })
}

/// Unwrap `Value::Object(InstanceValue).values` if needed; otherwise
/// require a bare `Value::Object`. Mirrors `lang.rs::unwrap_instance_value_*`.
#[allow(clippy::result_large_err)]
fn unwrap_instance_value(
    value: &Value,
    instance_value_id: Option<legend_pure_parser_pure::ids::ElementId>,
    ctx: &mut dyn EvalContextTrait,
) -> Result<ObjectHandle, PureException> {
    let Value::Object(obj) = value else {
        return Err(PureException::from(PureRuntimeError::type_mismatch(
            "Object", value,
        )));
    };
    let obj = obj.clone();
    if let Some(iv_id) = instance_value_id {
        let classifier = ctx
            .heap()
            .classifier(&obj)
            .map_err(PureException::from)?
            .clone();
        let resolved = m3_paths::resolve(ctx.model(), &classifier);
        if resolved == Some(iv_id) {
            let inner = ctx
                .heap()
                .get_property_values(&obj, "values")
                .map_err(PureException::from)?;
            if let Some(Value::Object(unwrapped)) = inner.iter().next() {
                return Ok(unwrapped.clone());
            }
        }
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register relation native functions into the registry under their
/// mangled Pure FQNs.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "addColumns_RelationType_1__ColSpecArray_1__RelationType_1_",
        AddColumns,
    );
}
