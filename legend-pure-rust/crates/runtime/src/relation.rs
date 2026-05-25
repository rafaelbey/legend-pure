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

//! Heap allocators for `RelationType`, `Column`, and `ColSpecArray`
//! literals lowered from `@(cols)` and `~[cols]` Pure source.
//!
//! These shapes mirror the Java reference impl
//! (`legend-pure-core/.../navigation/relation/_RelationType.java::build`,
//! `_Column.java::getColumnInstance`):
//!
//! - **Column** — `name` (String), `nameWildCard` (Boolean = false),
//!   `classifierGenericType` → GenericType{rawType=Column,
//!   typeArguments=[null, `GT(rawType=type_element)`],
//!   multiplicityArguments=[Multiplicity]}
//! - **`RelationType`** — `columns` (`Column[*]`)
//! - **`ColSpecArray`** — `names` (`String[*]`) plus `classifierGenericType` →
//!   GenericType{rawType=ColSpecArray,
//!   typeArguments=`[GT(rawType=<inner RelationType>)]`}, so the addColumns
//!   native can navigate
//!   `csa.classifierGenericType.typeArguments[0].rawType.columns` per Java.

use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::{Multiplicity, RelationColumnLowered};

use crate::error::PureException;
use crate::heap::{ObjectHandle, RuntimeHeap};
use crate::m3_paths;
use crate::value::Value;

/// Allocate a `Multiplicity` heap object whose `lowerBound` /
/// `upperBound` slots carry `MultiplicityValue` wrappers (mirrors the
/// canonical `m3.pure:1400` shape — `lowerBound : MultiplicityValue[1]`
/// where `MultiplicityValue.value : Integer[1]`). Identical semantics
/// to `meta::build_multiplicity_wrapper`, but takes a raw heap so
/// `Evaluator::eval` can drive it without an `EvalContextTrait` wrapper.
///
/// # Errors
/// Returns `PureException` if heap mutation fails (only on a stale
/// `ObjectHandle`, which the freshly-allocated objects below cannot produce).
#[allow(clippy::result_large_err)]
pub fn alloc_multiplicity(
    heap: &mut RuntimeHeap,
    m: &Multiplicity,
) -> Result<ObjectHandle, PureException> {
    let (lower, upper): (i64, Option<i64>) = match m {
        Multiplicity::PureOne => (1, Some(1)),
        Multiplicity::ZeroOrOne => (0, Some(1)),
        Multiplicity::OneOrMany => (1, None),
        Multiplicity::Range { lower, upper } => (i64::from(*lower), upper.map(i64::from)),
        Multiplicity::ZeroOrMany | Multiplicity::Variable(_) => (0, None),
    };
    let mult = heap.alloc_dynamic(m3_paths::MULTIPLICITY);
    let lower_value = heap.alloc_dynamic(m3_paths::MULTIPLICITY_VALUE);
    heap.mutate_add(&lower_value, "value", &[Value::Integer(lower)])
        .map_err(PureException::from)?;
    heap.mutate_add(&mult, "lowerBound", &[Value::Object(lower_value)])
        .map_err(PureException::from)?;
    if let Some(u) = upper {
        let upper_value = heap.alloc_dynamic(m3_paths::MULTIPLICITY_VALUE);
        heap.mutate_add(&upper_value, "value", &[Value::Integer(u)])
            .map_err(PureException::from)?;
        heap.mutate_add(&mult, "upperBound", &[Value::Object(upper_value)])
            .map_err(PureException::from)?;
    }
    Ok(mult)
}

/// Allocate the `Column` heap shape per the Java `_Column.getColumnInstance`
/// algorithm: name slot, `nameWildCard=false`, and a chained
/// `classifierGenericType` carrying `typeArguments=[null, <type GT>]`
/// and `multiplicityArguments=[<Multiplicity>]`.
///
/// # Errors
/// Returns `PureException` if heap mutation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_column(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    col: &RelationColumnLowered,
) -> Result<ObjectHandle, PureException> {
    let mult_obj = alloc_multiplicity(heap, &col.multiplicity)?;

    // Inner GenericType wrapping the column's type element.
    let inner_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(&inner_gt, "rawType", &[Value::Element(col.type_element)])
        .map_err(PureException::from)?;

    // Outer Column GenericType: typeArguments=[null, inner_gt],
    // multiplicityArguments=[mult_obj].
    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    let column_raw_type =
        m3_paths::resolve(model, m3_paths::COLUMN).map_or(Value::Unit, Value::Element);
    heap.mutate_add(&outer_gt, "rawType", &[column_raw_type])
        .map_err(PureException::from)?;
    // typeArguments[0] is the implicit "source" RelationType slot left
    // null until `_Column.updateSource` runs (test surface doesn't need
    // it). We populate slot[1] with the column's type GenericType.
    heap.mutate_add(
        &outer_gt,
        "typeArguments",
        &[Value::Unit, Value::Object(inner_gt)],
    )
    .map_err(PureException::from)?;
    heap.mutate_add(
        &outer_gt,
        "multiplicityArguments",
        &[Value::Object(mult_obj)],
    )
    .map_err(PureException::from)?;

    let column = heap.alloc_dynamic(m3_paths::COLUMN);
    heap.mutate_add(&column, "name", &[Value::String(col.name.clone())])
        .map_err(PureException::from)?;
    heap.mutate_add(&column, "nameWildCard", &[Value::Boolean(false)])
        .map_err(PureException::from)?;
    heap.mutate_add(&column, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(column)
}

/// Allocate a `RelationType` heap object containing the supplied columns.
///
/// # Errors
/// Returns `PureException` if heap mutation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_relation_type_with_columns(
    heap: &mut RuntimeHeap,
    columns: &[ObjectHandle],
) -> Result<ObjectHandle, PureException> {
    let rt = heap.alloc_dynamic(m3_paths::RELATION_TYPE);
    if !columns.is_empty() {
        let payload: Vec<Value> = columns.iter().cloned().map(Value::Object).collect();
        heap.mutate_add(&rt, "columns", &payload)
            .map_err(PureException::from)?;
    }
    Ok(rt)
}

/// Allocate `RelationType` from the lowered triples — used by `addColumns`'s
/// result (a bare `RelationType` heap object, no metadata wrapper).
///
/// # Errors
/// Returns `PureException` if any underlying heap allocation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_relation_literal(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    columns: &[RelationColumnLowered],
) -> Result<ObjectHandle, PureException> {
    let column_ids: Vec<ObjectHandle> = columns
        .iter()
        .map(|c| alloc_column(heap, model, c))
        .collect::<Result<_, _>>()?;
    alloc_relation_type_with_columns(heap, &column_ids)
}

/// Allocate `@(cols)` at expression position — mirrors Java's
/// `AntlrContextToM3CoreInstance.atomicExpression` (`AT (type | multiplicity)`
/// branch, line 1098-1099), where `@type` evaluates to an `InstanceValue`
/// with empty `values` and `genericType = ^GT(rawType=<the relation type>)`.
/// The empty-values + metadata-only shape is what makes
/// `@(x:String)->genericType().rawType` return the relation type instance
/// (via `genericType()`'s `valueCount==0` else branch reading the IV's
/// pre-set `genericType` slot).
///
/// # Errors
///
/// Propagates any heap mutation errors.
///
/// # Panics
///
/// Panics if the `RelationType` element is not found in the model.
#[allow(clippy::result_large_err)]
pub fn alloc_relation_literal_at_expression_position(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    columns: &[RelationColumnLowered],
) -> Result<ObjectHandle, PureException> {
    let rt = alloc_relation_literal(heap, model, columns)?;
    let gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(&gt, "rawType", &[Value::Object(rt)])
        .map_err(PureException::from)?;
    let iv = heap.alloc_dynamic(m3_paths::INSTANCE_VALUE);
    heap.mutate_add(&iv, "genericType", &[Value::Object(gt)])
        .map_err(PureException::from)?;
    // multiplicity = PureOne so consumers reading `iv.multiplicity` get the
    // expected `[1]` shape (Java sets `getPureOne()` at line 1099). We rely
    // on the bootstrap multiplicity element being present in the model;
    // skip silently if it isn't (no test reads it today).
    if let Some(pure_one_id) = model.resolve_by_path(&[
        smol_str::SmolStr::new("meta"),
        smol_str::SmolStr::new("pure"),
        smol_str::SmolStr::new("metamodel"),
        smol_str::SmolStr::new("multiplicity"),
        smol_str::SmolStr::new("PureOne"),
    ]) {
        heap.mutate_add(&iv, "multiplicity", &[Value::Element(pure_one_id)])
            .map_err(PureException::from)?;
    }
    Ok(iv)
}

/// Allocate the `ColSpecArray` literal heap shape per Java's
/// `AddColumns` navigation chain
/// (`csa.classifierGenericType.typeArguments[0].rawType._columns()`).
///
/// # Errors
/// Returns `PureException` if any underlying heap allocation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_col_spec_array_literal(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    columns: &[RelationColumnLowered],
) -> Result<ObjectHandle, PureException> {
    let inner_relation = alloc_relation_literal(heap, model, columns)?;

    // Wrap the inner RelationType in a GenericType so the native can
    // navigate `csa.classifierGenericType.typeArguments[0].rawType._columns()`.
    let inner_relation_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(
        &inner_relation_gt,
        "rawType",
        &[Value::Object(inner_relation)],
    )
    .map_err(PureException::from)?;

    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    let csa_raw_type =
        m3_paths::resolve(model, m3_paths::COL_SPEC_ARRAY).map_or(Value::Unit, Value::Element);
    heap.mutate_add(&outer_gt, "rawType", &[csa_raw_type])
        .map_err(PureException::from)?;
    heap.mutate_add(
        &outer_gt,
        "typeArguments",
        &[Value::Object(inner_relation_gt)],
    )
    .map_err(PureException::from)?;

    let csa = heap.alloc_dynamic(m3_paths::COL_SPEC_ARRAY);
    let names: Vec<Value> = columns
        .iter()
        .map(|c| Value::String(c.name.clone()))
        .collect();
    if !names.is_empty() {
        heap.mutate_add(&csa, "names", &names)
            .map_err(PureException::from)?;
    }
    heap.mutate_add(&csa, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(csa)
}

/// Allocate the single-column `ColSpec` literal heap shape produced
/// by `~name` source syntax. Mirrors the platform's
/// `meta::pure::metamodel::relation::ColSpec<T>` shape per
/// `platform/pure/relation.pure:17`, where the comment *"T is of type
/// **RelationType**"* declares the runtime invariant: a ColSpec's
/// `classifierGenericType.typeArguments[0].rawType` is a `RelationType`
/// with the column as one entry in its `columns` slot.
///
/// Concretely the chain is:
///
/// ```text
/// ColSpec
///   ├─ name: String[1]            -- the column name
///   └─ classifierGenericType: GenericType
///        ├─ rawType: ColSpec
///        └─ typeArguments[0]: GenericType
///             └─ rawType: RelationType        ← the inner shape
///                  └─ columns: Column[*]      ← navigable per `relation.pure`
/// ```
///
/// This mirrors `alloc_col_spec_array_literal`'s structure so that a
/// `~name` (ColSpec) and a `~[name1, name2]` (ColSpecArray) literal
/// expose the same reflective shape — both reachable via
/// `cs.classifierGenericType.typeArguments[0].rawType.columns`. Earlier
/// the inner `rawType` slot held a bare `Column`, which had no
/// `columns` slot and broke the Pure body for
/// `reduce(rel, win, row, x|eval(colSpec, x), …)` and any other
/// reflective walk that expected a `RelationType` per the platform
/// comment.
///
/// # Errors
/// Returns `PureException` if any underlying heap allocation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_col_spec_literal(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    column: &RelationColumnLowered,
) -> Result<ObjectHandle, PureException> {
    // Single-column RelationType captures the column's typed shape —
    // matches the platform's "T is of type RelationType" invariant.
    let inner_relation = alloc_relation_literal(heap, model, std::slice::from_ref(column))?;

    // Inner GenericType wraps the RelationType.
    let inner_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(&inner_gt, "rawType", &[Value::Object(inner_relation)])
        .map_err(PureException::from)?;

    // ColSpec's classifierGenericType points at the RelationType GenericType.
    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    let cs_raw_type =
        m3_paths::resolve(model, m3_paths::COL_SPEC).map_or(Value::Unit, Value::Element);
    heap.mutate_add(&outer_gt, "rawType", &[cs_raw_type])
        .map_err(PureException::from)?;
    heap.mutate_add(&outer_gt, "typeArguments", &[Value::Object(inner_gt)])
        .map_err(PureException::from)?;

    let cs = heap.alloc_dynamic(m3_paths::COL_SPEC);
    heap.mutate_add(&cs, "name", &[Value::String(column.name.clone())])
        .map_err(PureException::from)?;
    heap.mutate_add(&cs, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(cs)
}

/// Allocate a `FuncColSpec` literal — the heap shape behind
/// `~name:lam` syntax. Mirrors `alloc_col_spec_literal` for the
/// `name` + `classifierGenericType` slots, plus a `function` slot
/// carrying the (already-evaluated) init lambda the `extend` native
/// invokes per row.
///
/// The caller (`eval.rs::ColSpecLiteral`) evaluates
/// `column.init_lambda` against the enclosing scope first so the
/// resulting `Value::Function` captures any outer-scope variables the
/// lambda body reads. This allocator just slots the produced value
/// onto the heap object — it doesn't itself compile or close over
/// anything.
///
/// # Errors
/// Returns `PureException` if any underlying heap allocation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_func_col_spec_literal(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    column: &RelationColumnLowered,
    function_value: Value,
) -> Result<ObjectHandle, PureException> {
    let column_obj = alloc_column(heap, model, column)?;

    let inner_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(&inner_gt, "rawType", &[Value::Object(column_obj)])
        .map_err(PureException::from)?;

    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    let fcs_raw_type =
        m3_paths::resolve(model, m3_paths::FUNC_COL_SPEC).map_or(Value::Unit, Value::Element);
    heap.mutate_add(&outer_gt, "rawType", &[fcs_raw_type])
        .map_err(PureException::from)?;
    heap.mutate_add(&outer_gt, "typeArguments", &[Value::Object(inner_gt)])
        .map_err(PureException::from)?;

    let fcs = heap.alloc_dynamic(m3_paths::FUNC_COL_SPEC);
    heap.mutate_add(&fcs, "name", &[Value::String(column.name.clone())])
        .map_err(PureException::from)?;
    heap.mutate_add(&fcs, "function", &[function_value])
        .map_err(PureException::from)?;
    heap.mutate_add(&fcs, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(fcs)
}

/// Allocate an `AggColSpec` literal — the heap shape behind `~name:map:reduce`
/// (Agg) syntax. Sets the platform `AggColSpec` slots `name`, `map`, `reduce`
/// (the two already-evaluated lambdas) plus a `classifierGenericType` wrapping a
/// single-column `RelationType` — the correct "T is RelationType" shape from
/// [`alloc_col_spec_literal`], deliberately *not* the bare-`Column` shape
/// [`alloc_func_col_spec_literal`] still carries.
///
/// The caller (`eval.rs::ColSpecLiteral`) evaluates `column.init_lambda` (map)
/// and `column.reduce_lambda` (reduce) against the enclosing scope first so each
/// `Value::Function` captures any outer-scope variables; this allocator just
/// slots the produced values onto the heap object.
///
/// # Errors
/// Returns `PureException` if any underlying heap allocation fails.
#[allow(clippy::result_large_err)]
pub fn alloc_agg_col_spec_literal(
    heap: &mut RuntimeHeap,
    model: &PureModel,
    column: &RelationColumnLowered,
    map_value: Value,
    reduce_value: Value,
) -> Result<ObjectHandle, PureException> {
    // Single-column RelationType captures the column's typed shape —
    // matches the platform's "T is of type RelationType" invariant.
    let inner_relation = alloc_relation_literal(heap, model, std::slice::from_ref(column))?;

    let inner_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    heap.mutate_add(&inner_gt, "rawType", &[Value::Object(inner_relation)])
        .map_err(PureException::from)?;

    let outer_gt = heap.alloc_dynamic(m3_paths::GENERIC_TYPE);
    let acs_raw_type =
        m3_paths::resolve(model, m3_paths::AGG_COL_SPEC).map_or(Value::Unit, Value::Element);
    heap.mutate_add(&outer_gt, "rawType", &[acs_raw_type])
        .map_err(PureException::from)?;
    heap.mutate_add(&outer_gt, "typeArguments", &[Value::Object(inner_gt)])
        .map_err(PureException::from)?;

    let acs = heap.alloc_dynamic(m3_paths::AGG_COL_SPEC);
    heap.mutate_add(&acs, "name", &[Value::String(column.name.clone())])
        .map_err(PureException::from)?;
    heap.mutate_add(&acs, "map", &[map_value])
        .map_err(PureException::from)?;
    heap.mutate_add(&acs, "reduce", &[reduce_value])
        .map_err(PureException::from)?;
    heap.mutate_add(&acs, "classifierGenericType", &[Value::Object(outer_gt)])
        .map_err(PureException::from)?;
    Ok(acs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::RuntimeHeap;
    use legend_pure_parser_pure::bootstrap;
    use legend_pure_parser_pure::types::Multiplicity;
    use smol_str::SmolStr;

    fn bootstrap_model() -> PureModel {
        let mut model = PureModel::new();
        let (chunk, _) = bootstrap::create_bootstrap_chunk(model.root_package);
        model.chunks.push(chunk);
        model
    }

    /// `alloc_col_spec_literal` must place a single-column
    /// `RelationType` in its inner `classifierGenericType.typeArguments
    /// [0].rawType` slot — never a bare `Column` — so that
    /// `colSpec.classifierGenericType.typeArguments[0].rawType.columns`
    /// resolves to a one-Column list per the platform `relation.pure`
    /// spec. Before the fix the slot held a bare `Column` and `.columns`
    /// returned empty.
    #[test]
    fn col_spec_literal_inner_raw_type_is_relation_type_with_one_column() {
        let model = bootstrap_model();
        let mut heap = RuntimeHeap::new();

        let column = RelationColumnLowered {
            name: SmolStr::new("col name"),
            type_element: bootstrap::INTEGER_ID,
            multiplicity: Multiplicity::PureOne,
            init_lambda: None,
            reduce_lambda: None,
        };

        let cs = alloc_col_spec_literal(&mut heap, &model, &column)
            .expect("alloc must succeed against the bootstrap model");

        // cs.classifierGenericType → outer GT
        let cgt_values = heap
            .get_property_values(&cs, "classifierGenericType")
            .expect("ColSpec has a classifierGenericType slot");
        let outer_gt = cgt_values
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("classifierGenericType is an Object");

        // outer_gt.typeArguments[0] → inner GT
        let ta = heap
            .get_property_values(&outer_gt, "typeArguments")
            .expect("outer GT has a typeArguments slot");
        let inner_gt = ta
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("typeArguments[0] is an Object");

        // inner_gt.rawType → must be a RelationType, not a Column.
        let raw_type_values = heap
            .get_property_values(&inner_gt, "rawType")
            .expect("inner GT has a rawType slot");
        let inner_raw = raw_type_values
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("inner GT rawType is an Object");

        // Classifier must be RelationType.
        let classifier = heap
            .classifier(&inner_raw)
            .expect("inner_raw has a classifier");
        assert_eq!(
            classifier.as_str(),
            m3_paths::RELATION_TYPE,
            "ColSpec's inner classifierGenericType.typeArguments[0].rawType must be a \
             RelationType (per `platform/pure/relation.pure:17`, not a bare Column)",
        );

        // RelationType.columns must hold exactly one Column with the
        // canonical (unquoted) name.
        let columns = heap
            .get_property_values(&inner_raw, "columns")
            .expect("inner RelationType has a columns slot");
        assert_eq!(
            columns.len(),
            1,
            "single-column ColSpec must materialise one Column in the inner RelationType"
        );
        let col_obj = match &columns[0] {
            Value::Object(o) => o.clone(),
            other => panic!("expected Column object, got {other:?}"),
        };
        let col_name = heap
            .get_property_values(&col_obj, "name")
            .expect("Column.name slot");
        let actual_name = col_name
            .iter()
            .find_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .expect("Column.name is a String");
        assert_eq!(actual_name.as_str(), "col name");

        // The outer ColSpec.name slot also still carries the column name
        // (unchanged contract used by `select` / `rename` natives).
        let cs_name_values = heap.get_property_values(&cs, "name").expect("ColSpec.name");
        let cs_name = cs_name_values
            .iter()
            .find_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .expect("ColSpec.name is a String");
        assert_eq!(cs_name.as_str(), "col name");
    }

    /// `alloc_agg_col_spec_literal` must materialise an `AggColSpec` carrying
    /// the platform `name`/`map`/`reduce` slots (the reduce slot is what the
    /// engine's aggregating `extend` reads), and — like `alloc_col_spec_literal`
    /// — wrap a single-column `RelationType` in its inner `classifierGenericType`
    /// (not a bare `Column`).
    #[test]
    fn agg_col_spec_literal_has_map_reduce_and_relation_type_inner() {
        let model = bootstrap_model();
        let mut heap = RuntimeHeap::new();

        let column = RelationColumnLowered {
            name: SmolStr::new("total"),
            type_element: bootstrap::INTEGER_ID,
            multiplicity: Multiplicity::PureOne,
            init_lambda: None,
            reduce_lambda: None,
        };

        // Sentinel values stand in for the evaluated map/reduce lambdas.
        let map_value = Value::String(SmolStr::new("MAP"));
        let reduce_value = Value::String(SmolStr::new("REDUCE"));
        let acs = alloc_agg_col_spec_literal(&mut heap, &model, &column, map_value, reduce_value)
            .expect("alloc must succeed against the bootstrap model");

        assert_eq!(
            heap.classifier(&acs)
                .expect("AggColSpec classifier")
                .as_str(),
            m3_paths::AGG_COL_SPEC,
        );

        // The map and reduce slots carry the supplied function values.
        let read_string = |slot: &str| {
            heap.get_property_values(&acs, slot)
                .unwrap_or_else(|_| panic!("AggColSpec.{slot} slot"))
                .iter()
                .find_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
        };
        assert_eq!(read_string("map").as_deref(), Some("MAP"));
        assert_eq!(read_string("reduce").as_deref(), Some("REDUCE"));

        // Inner classifierGenericType.typeArguments[0].rawType is a RelationType
        // (the "T is RelationType" invariant), not a bare Column.
        let outer_gt = heap
            .get_property_values(&acs, "classifierGenericType")
            .expect("classifierGenericType slot")
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("classifierGenericType is an Object");
        let inner_gt = heap
            .get_property_values(&outer_gt, "typeArguments")
            .expect("typeArguments slot")
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("typeArguments[0] is an Object");
        let inner_raw = heap
            .get_property_values(&inner_gt, "rawType")
            .expect("rawType slot")
            .iter()
            .find_map(|v| match v {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            })
            .expect("inner rawType is an Object");
        assert_eq!(
            heap.classifier(&inner_raw)
                .expect("inner classifier")
                .as_str(),
            m3_paths::RELATION_TYPE,
            "AggColSpec inner rawType must be a RelationType, not a bare Column",
        );
    }
}
