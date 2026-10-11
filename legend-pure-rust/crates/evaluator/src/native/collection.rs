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

//! Collection natives: the element accessors (`at`, `first`, `last`, `init`,
//! `tail`, `take`, `drop`, `slice`, `size`, `isEmpty`, `range`, `add`,
//! `concatenate`, `reverse`, `toOne`, `indexOf`), the lambda-driven
//! iterators (`map`, `filter`, `fold`, `exists`, `forAll`, `find`,
//! `removeDuplicates`, `sort`), `zip`, and the `Map` natives (`newMap`,
//! `put`, `putAll`, `get`, `keys`, `values`).
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.

use im_rc::Vector as PVector;
use legend_pure_metamodel::types::ValueSpec;

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

/// Pure `fold(T[*], Function<{T[1],V[m]->V[m]}>[1], V[m]): V[m]`
///
/// Accumulates a collection into a single value using a binary function.
#[derive(Debug)]
pub struct Fold;

impl NativeFunction for Fold {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fold", args, 3)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();
        let mut accumulator = ctx.evaluate(&args[2])?.into_value();

        for item in &source {
            accumulator = ctx.call_function(&lambda_val, &[item.clone(), accumulator])?;
        }

        Ok(Evaluated::new(accumulator))
    }
}

/// Pure `take(Any[*], Integer[1]): Any[*]` — first N elements.
#[derive(Debug)]
pub struct Take;

impl NativeFunction for Take {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("take", &values, 2)?;
        let coll = values[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = values[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Evaluated::new(Value::Collection(Box::new(coll.take(n)))))
    }
}

/// Pure `drop(Any[*], Integer[1]): Any[*]` — all elements after first N.
#[derive(Debug)]
pub struct Drop;

impl NativeFunction for Drop {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("drop", &values, 2)?;
        let coll = values[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = values[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Evaluated::new(Value::Collection(Box::new(coll.skip(n)))))
    }
}

/// Pure `zip<T,U>(set1:T[*], set2:U[*]):Pair<T,U>[*]`
///
/// Pairwise combines two collections, truncating to the shorter length.
/// Each pair allocates a heap `Pair` object with `first` / `second`
/// slots, mirroring the shape produced by `keyValues` and consumed by
/// `pair()` / Map iteration.
#[derive(Debug)]
pub struct Zip;

impl NativeFunction for Zip {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("zip", &values, 2)?;
        let xs = collection_as_vec(&values[0]);
        let ys = collection_as_vec(&values[1]);
        let n = xs.len().min(ys.len());
        let mut out: Vec<Value> = Vec::with_capacity(n);
        for i in 0..n {
            let obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::PAIR);
            ctx.heap_mut().mutate_add(&obj, "first", &[xs[i].clone()])?;
            ctx.heap_mut()
                .mutate_add(&obj, "second", &[ys[i].clone()])?;
            out.push(Value::Object(obj));
        }
        Ok(Evaluated::new(Value::from_vec(out)))
    }
}

/// View any value as a `Vec<Value>` — `Collection` flattens to its
/// elements; `Unit` is empty; a scalar is wrapped in a singleton.
fn collection_as_vec(v: &Value) -> Vec<Value> {
    match v {
        Value::Collection(c) => c.iter().cloned().collect(),
        Value::Unit => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Pure `first(Any[*]): Any[0..1]` — first element or Unit.
#[derive(Debug)]
pub struct First;

impl NativeFunction for First {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("first", &values, 1)?;
        let result = match &values[0] {
            Value::Collection(v) => v.front().cloned().unwrap_or(Value::Unit),
            Value::Unit => Value::Unit,
            other => other.clone(), // scalar is its own first
        };
        Ok(Evaluated::new(result))
    }
}

/// Pure `removeDuplicates<T,V>(col:T[*], key:Function<{T[1]->V[1]}>[0..1],
/// eql:Function<{V[1],V[1]->Boolean[1]}>[0..1]):T[*]`.
///
/// Removes duplicate elements, preserving first-occurrence order.
/// Both function arguments are optional (`[0..1]`). When `key` is
/// supplied each element is mapped through it before comparison;
/// when `eql` is supplied that function decides equality
/// (otherwise [`Value::eq`] is used).
///
/// The 1-arg `removeDuplicates(col)` and 2-arg
/// `removeDuplicates(col, eql)` shapes are platform `.pure`
/// wrappers that delegate to this 3-arg native with `[]` (empty)
/// for the omitted parameters — see
/// `legend-pure-core/.../removeDuplicates.pure:25-33`. The 1-arg
/// `removeDuplicatesBy(col, key)` similarly delegates here with
/// `[]` for `eql` (`removeDuplicatesBy.pure:18-21`).
#[derive(Debug)]
pub struct RemoveDuplicates;

impl NativeFunction for RemoveDuplicates {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Accept 1-arg (col only) and 3-arg (col, key, eql) variants.
        // Platform wrappers normalise to 3-arg before dispatch, but
        // direct callers may use the bare native.
        if values.is_empty() || values.len() == 2 || values.len() > 3 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "removeDuplicates: expected 1 or 3 argument(s), got {}",
                values.len()
            ))
            .into());
        }
        let source = values[0].to_collection();
        let key_fn: Option<Value> = values.get(1).filter(|v| !is_empty_optional(v)).cloned();
        let eql_fn: Option<Value> = values.get(2).filter(|v| !is_empty_optional(v)).cloned();

        // Pre-compute the comparison key for each input element.
        // When no `key` is provided the element itself is its key.
        let mut keys: Vec<Value> = Vec::with_capacity(source.len());
        for item in &source {
            let k = match &key_fn {
                Some(kf) => ctx.call_function(kf, std::slice::from_ref(item))?,
                None => item.clone(),
            };
            keys.push(k);
        }

        // Two-pass dedup: walk elements in order, keeping each whose
        // key isn't equal (under `eql_fn` if provided, else
        // `Value::eq`) to any already-kept element's key.
        let mut kept: Vec<usize> = Vec::with_capacity(source.len());
        for i in 0..source.len() {
            let mut is_dup = false;
            for &j in &kept {
                let eq = match &eql_fn {
                    Some(ef) => {
                        let result = ctx.call_function(ef, &[keys[j].clone(), keys[i].clone()])?;
                        result.as_boolean()?
                    }
                    None => keys[j] == keys[i],
                };
                if eq {
                    is_dup = true;
                    break;
                }
            }
            if !is_dup {
                kept.push(i);
            }
        }

        let out: Vec<Value> = kept.iter().map(|&i| source[i].clone()).collect();
        Ok(Evaluated::new(Value::from_vec(out)))
    }
}

/// Whether a `Value` should be treated as an absent optional argument
/// — i.e., the `[]` literal that the platform wrappers pass for
/// omitted `key`/`eql` parameters. Matches `Value::Unit` and any
/// empty collection.
fn is_empty_optional(v: &Value) -> bool {
    match v {
        Value::Unit => true,
        Value::Collection(items) => items.is_empty(),
        _ => false,
    }
}

/// Default comparator used by [`Sort`] when no user-supplied comparator
/// is provided. Routes through [`crate::native::comparison::compare_values`]
/// — Phase 1's total comparator that handles same-type primitives,
/// cross-numeric promotion (Integer/Float/Decimal lattice), and
/// cross-type pairs via stable per-variant ordinals.
///
/// Cross-type sorts (e.g. `['aaa', 2]`) used to error here; now they
/// produce a deterministic ordering — Integer before String, both
/// before Object/Collection — because the underlying comparator is
/// total. `assertSameElements(['aaa', 2], [2, 'aaa'])` and similar
/// mixed-type comparisons depend on this.
fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    crate::native::comparison::compare_values(a, b).cmp(&0)
}

/// Pure `sort<T,U|m>(col:T[m], key:Function[0..1], comp:Function[0..1]):T[m]`.
///
/// Sorts the collection stably. When `key` is provided (non-empty), each
/// element is first mapped through `key` and the resulting values drive
/// ordering — values stay associated with their original element. When
/// `comp` is provided, it returns an `Integer` (negative/zero/positive)
/// to order two items; otherwise, primitive natural order via
/// `cmp_values` is used. Both `key` and `comp` may independently be
/// `Unit` / `[]` to mean "not provided".
#[derive(Debug)]
pub struct Sort;

impl NativeFunction for Sort {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("sort", &values, 3)?;
        let source = values[0].to_collection();
        let key_fn = lambda_or_none(&values[1]);
        let comp_fn = lambda_or_none(&values[2]);

        // Pre-compute key(x) once per element when a key is supplied,
        // so we don't re-invoke the lambda during every comparison.
        let items: Vec<Value> = source.iter().cloned().collect();
        let keys: Vec<Value> = if let Some(k) = key_fn {
            let mut out = Vec::with_capacity(items.len());
            for it in &items {
                out.push(ctx.call_function(k, std::slice::from_ref(it))?);
            }
            out
        } else {
            items.clone()
        };

        // Indices into items/keys — sort those so we can carry a value
        // alongside its pre-computed key.
        let mut indices: Vec<usize> = (0..items.len()).collect();
        let mut cmp_err: Option<PureRuntimeError> = None;

        if let Some(cf) = comp_fn {
            indices.sort_by(|&i, &j| {
                if cmp_err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                match ctx.call_function(cf, &[keys[i].clone(), keys[j].clone()]) {
                    Ok(Value::Integer(n)) => n.cmp(&0),
                    Ok(other) => {
                        cmp_err = Some(PureRuntimeError::type_mismatch("Integer", &other));
                        std::cmp::Ordering::Equal
                    }
                    Err(e) => {
                        cmp_err = Some(PureRuntimeError::EvaluationError(format!("{e}")));
                        std::cmp::Ordering::Equal
                    }
                }
            });
        } else {
            indices.sort_by(|&i, &j| cmp_values(&keys[i], &keys[j]));
        }
        if let Some(e) = cmp_err {
            return Err(e.into());
        }

        let sorted: Vec<Value> = indices.into_iter().map(|i| items[i].clone()).collect();
        Ok(Evaluated::new(Value::from_vec(sorted)))
    }
}

/// Extract a `&Value::Function` from a lambda-valued argument, or `None` if
/// the arg is `Unit`/empty. Any other non-Unit value is treated as "not a
/// lambda" and returns `None` — matches the semantics of the optional
/// `Function[0..1]` parameter.
fn lambda_or_none(v: &Value) -> Option<&Value> {
    match v {
        Value::Function(_) => Some(v),
        _ => None,
    }
}

/// Pure `size(Any[*]): Integer[1]` — number of elements in a collection.
#[derive(Debug)]
pub struct Size;

impl NativeFunction for Size {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("size", &values, 1)?;
        let coll = values[0].to_collection();
        #[allow(clippy::cast_possible_wrap)]
        Ok(Evaluated::new(Value::Integer(coll.len() as i64)))
    }
}

/// Pure `range(Integer[1], Integer[1], Integer[1]): Integer[*]`
/// Pure `range(Integer[1], Integer[1]): Integer[*]` — step defaults to 1
/// Pure `range(Integer[1]): Integer[*]` — start defaults to 0, step to 1
///
/// Generates integers from `start` (inclusive) to `stop` (exclusive)
/// with the given `step`. Java's stride direction follows the sign of
/// `step` — positive step ascends, negative step descends.
#[derive(Debug)]
pub struct Range;

impl NativeFunction for Range {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        let (start, end_exclusive, increment) = match values.len() {
            1 => (0i64, values[0].as_integer()?, 1i64),
            2 => (values[0].as_integer()?, values[1].as_integer()?, 1i64),
            3 => (
                values[0].as_integer()?,
                values[1].as_integer()?,
                values[2].as_integer()?,
            ),
            n => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "range: expected 1, 2, or 3 argument(s), got {n}"
                ))
                .into());
            }
        };
        if increment == 0 {
            // Java's `Range.execute` throws "range step must not be 0";
            // platform `testRangeStepError` pins the exact text.
            return Err(
                PureRuntimeError::EvaluationError("range step must not be 0".into()).into(),
            );
        }
        let mut result = PVector::new();
        let mut i = start;
        if increment > 0 {
            while i < end_exclusive {
                result.push_back(Value::Integer(i));
                i += increment;
            }
        } else {
            while i > end_exclusive {
                result.push_back(Value::Integer(i));
                i += increment;
            }
        }
        Ok(Evaluated::new(Value::Collection(Box::new(result))))
    }
}

/// Pure `map(T[*], Function<{T[1]->V[1]}>[1]): V[*]`
///
/// Applies a lambda to each element of a collection.
#[derive(Debug)]
pub struct Map;

impl NativeFunction for Map {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("map", args, 2)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        // Pure collections are flat — a lambda returning `V[*]` or `V[0..1]`
        // contributes zero, one, or many scalar elements to the result. We
        // flatten Collection/Unit outputs here so downstream `.property`
        // access sees scalars, not nested collections.
        let mut results: Vec<Value> = Vec::with_capacity(source.len());
        for item in &source {
            let called = ctx.call_function(&lambda_val, std::slice::from_ref(item))?;
            match &called {
                Value::Collection(inner) => {
                    for v in inner.iter() {
                        results.push(v.clone());
                    }
                }
                Value::Unit => {}
                _ => results.push(called),
            }
        }

        Ok(Evaluated::new(Value::from_vec(results)))
    }
}

/// Pure `slice<T>(T[*], Integer[1], Integer[1]): T[*]`
///
/// Returns the sub-collection from `start` (inclusive) to `end` (exclusive).
/// Indices are clamped to the collection bounds: negative indices become 0,
/// and indices past the end become the length. If `start >= end` after
/// clamping, the result is empty.
#[derive(Debug)]
pub struct Slice;

impl NativeFunction for Slice {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("slice", &values, 3)?;
        let source = values[0].to_collection();
        let start = values[1].as_integer()?;
        let end = values[2].as_integer()?;

        // Java Pure's `Slice.execute` throws when the low bound exceeds
        // the high bound — `testSliceError` pins the exact text:
        // "The low bound (X) can't be higher than the high bound (Y) in
        // a slice operation". Empty result when start == end stays as
        // a soft return (Java behaves the same).
        if start > end {
            return Err(PureRuntimeError::EvaluationError(format!(
                "The low bound ({start}) can't be higher than the high bound ({end}) in a slice operation"
            ))
            .into());
        }

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let start = start.max(0) as usize;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let end = end.max(0) as usize;
        let start = start.min(source.len());
        let end = end.min(source.len());

        if start >= end {
            return Ok(Evaluated::new(Value::Unit));
        }

        // PVector::slice takes Range; we iterate to keep it simple & avoid
        // accidentally moving the clone out.
        let out: Vec<Value> = source
            .iter()
            .skip(start)
            .take(end - start)
            .cloned()
            .collect();
        Ok(Evaluated::new(Value::from_vec(out)))
    }
}

/// Pure `find<T>(T[*], Function<{T[1]->Boolean[1]}>[1]): T[0..1]`
///
/// Returns the first element for which the predicate returns `true`, or
/// [`Value::Unit`] if no element satisfies the predicate.
#[derive(Debug)]
pub struct Find;

impl NativeFunction for Find {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("find", args, 2)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        for item in &source {
            let pred = ctx.call_function(&lambda_val, std::slice::from_ref(item))?;
            if pred.as_boolean()? {
                return Ok(Evaluated::new(item.clone()));
            }
        }

        Ok(Evaluated::new(Value::Unit))
    }
}

/// Pure `at(Any[*], Integer[1]): Any[1]` — element at index.
///
/// Pure uses 0-based indexing.
#[derive(Debug)]
pub struct At;

impl NativeFunction for At {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("at", &values, 2)?;
        let coll = values[0].to_collection();
        let idx = values[1].as_integer()?;
        if idx < 0 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "at: index cannot be negative, got {idx}"
            ))
            .into());
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let idx = idx as usize;
        coll.get(idx).cloned().map(Evaluated::new).ok_or_else(|| {
            // Java-parity message: assertError tests in the platform
            // (testSimpleAssertError* in essential/tests/assertError.pure)
            // pin this exact phrasing via String equality on the error
            // text. Anything else fails the assertion silently.
            PureRuntimeError::EvaluationError(format!(
                "The system is trying to get an element at offset {idx} where the collection is of size {}",
                coll.len()
            ))
            .into()
        })
    }
}

/// Pure `exists<T>(value:T[*], func:Function<{T[1]->Boolean[1]}>[1]):Boolean[1]`
///
/// Returns `true` as soon as any element satisfies the predicate. For an
/// empty input returns `false`.
#[derive(Debug)]
pub struct Exists;

impl NativeFunction for Exists {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("exists", args, 2)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        for item in &source {
            let pred = ctx.call_function(&lambda_val, std::slice::from_ref(item))?;
            if pred.as_boolean()? {
                return Ok(Evaluated::new(Value::Boolean(true)));
            }
        }

        Ok(Evaluated::new(Value::Boolean(false)))
    }
}

/// Register the fished collection natives.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("fold_T_MANY__Function_1__V_m__V_m_", Fold);
    registry.register("take_T_MANY__Integer_1__T_MANY_", Take);
    registry.register("drop_T_MANY__Integer_1__T_MANY_", Drop);
    registry.register("zip_T_MANY__U_MANY__Pair_MANY_", Zip);
    registry.register("first_T_MANY__T_$0_1$_", First);
    registry.register(
        "removeDuplicates_T_MANY__Function_$0_1$__Function_$0_1$__T_MANY_",
        RemoveDuplicates,
    );
    registry.register("sort_T_m__Function_$0_1$__Function_$0_1$__T_m_", Sort);
    registry.register("size_Any_MANY__Integer_1_", Size);
    registry.register(
        "range_Integer_1__Integer_1__Integer_1__Integer_MANY_",
        Range,
    );
    registry.register("map_T_MANY__Function_1__V_MANY_", Map);
    registry.register("map_T_m__Function_1__V_m_", Map);
    registry.register("map_T_$0_1$__Function_1__V_$0_1$_", Map);
    registry.register("slice_T_MANY__Integer_1__Integer_1__T_MANY_", Slice);
    registry.register("find_T_MANY__Function_1__T_$0_1$_", Find);
    registry.register("at_T_MANY__Integer_1__T_1_", At);
    registry.register("exists_T_MANY__Function_1__Boolean_1_", Exists);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, lit_collection, lit_int, lit_str};

    /// Build a `ValueSpec` for an integer collection — mirrors the old `int_collection`
    /// helper but produces a `ValueSpec` suitable for `MockCtx`.
    fn int_spec(items: &[i64]) -> ValueSpec {
        lit_collection(items.iter().map(|&i| lit_int(i)).collect())
    }

    /// Build the expected `Value::Collection` for assertions.
    fn int_collection(items: &[i64]) -> Value {
        let mut v = PVector::new();
        for &i in items {
            v.push_back(Value::Integer(i));
        }
        Value::Collection(Box::new(v))
    }

    #[test]
    fn take_elements() {
        assert_eq!(
            Take.execute(&[int_spec(&[1, 2, 3, 4, 5]), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn drop_elements() {
        assert_eq!(
            Drop.execute(&[int_spec(&[1, 2, 3, 4, 5]), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[3, 4, 5])
        );
    }

    #[test]
    fn first_of_collection() {
        assert_eq!(
            First
                .execute(&[int_spec(&[10, 20, 30])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(10)
        );
    }

    #[test]
    fn first_of_empty() {
        assert_eq!(
            First
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn remove_duplicates_preserves_first_occurrence() {
        let input = int_spec(&[1, 2, 1, 3, 2, 4]);
        let result = RemoveDuplicates
            .execute(&[input], &mut MockCtx)
            .unwrap()
            .into_value();
        assert_eq!(result, int_collection(&[1, 2, 3, 4]));
    }

    #[test]
    fn remove_duplicates_on_empty() {
        assert_eq!(
            RemoveDuplicates
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn remove_duplicates_ignores_trailing_hasher_args() {
        // The 3-arg variant passes hash + equality lambdas that we currently ignore.
        let input = int_spec(&[1, 1, 2]);
        let result = RemoveDuplicates
            .execute(
                &[input, lit_collection(vec![]), lit_collection(vec![])],
                &mut MockCtx,
            )
            .unwrap()
            .into_value();
        assert_eq!(result, int_collection(&[1, 2]));
    }

    #[test]
    fn sort_integers_ascending() {
        assert_eq!(
            Sort.execute(
                &[
                    int_spec(&[3, 1, 2]),
                    lit_collection(vec![]),
                    lit_collection(vec![])
                ],
                &mut MockCtx,
            )
            .unwrap()
            .into_value(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn sort_single_element_is_noop() {
        assert_eq!(
            Sort.execute(
                &[lit_int(5), lit_collection(vec![]), lit_collection(vec![])],
                &mut MockCtx,
            )
            .unwrap()
            .into_value(),
            Value::Integer(5)
        );
    }

    #[test]
    fn sort_empty_returns_unit() {
        assert_eq!(
            Sort.execute(
                &[
                    lit_collection(vec![]),
                    lit_collection(vec![]),
                    lit_collection(vec![]),
                ],
                &mut MockCtx,
            )
            .unwrap()
            .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn sort_mixed_types_succeeds_with_total_order() {
        // Phase 5b: Sort routes through compare_values which is total
        // (cross-type pairs fall back to a stable per-variant ordinal),
        // so mixed Integer + String sorts no longer error. The platform
        // assertSameElements PCT tests rely on this — see the
        // testSuccessAssertSameElements case. The trade-off: previous
        // "explicit error on cross-type sort" behavior is gone, but it
        // was a Java-divergence — Java Pure's sort tolerates mixed types.
        let input = lit_collection(vec![lit_int(1), lit_str("oops")]);
        let r = Sort
            .execute(
                &[input, lit_collection(vec![]), lit_collection(vec![])],
                &mut MockCtx,
            )
            .unwrap()
            .into_value();
        let Value::Collection(c) = &r else {
            panic!("expected Collection, got {r:?}");
        };
        // Integer's type ordinal (1) is less than String's (4), so the
        // Integer comes first.
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Value::Integer(1));
        assert_eq!(c[1], Value::String("oops".into()));
    }

    #[test]
    fn sort_wrong_arg_count() {
        assert!(Sort.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn size_of_collection() {
        assert_eq!(
            Size.execute(&[int_spec(&[1, 2, 3])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(3)
        );
    }

    #[test]
    fn size_of_scalar() {
        assert_eq!(
            Size.execute(&[lit_int(42)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(1)
        );
    }

    #[test]
    fn size_of_unit() {
        assert_eq!(
            Size.execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(0)
        );
    }

    #[test]
    fn range_ascending() {
        let r = Range
            .execute(&[lit_int(0), lit_int(5), lit_int(1)], &mut MockCtx)
            .unwrap()
            .into_value();
        assert_eq!(r, int_collection(&[0, 1, 2, 3, 4]));
    }

    #[test]
    fn range_with_step() {
        let r = Range
            .execute(&[lit_int(0), lit_int(10), lit_int(3)], &mut MockCtx)
            .unwrap()
            .into_value();
        assert_eq!(r, int_collection(&[0, 3, 6, 9]));
    }

    #[test]
    fn range_descending() {
        let r = Range
            .execute(&[lit_int(5), lit_int(0), lit_int(-1)], &mut MockCtx)
            .unwrap()
            .into_value();
        assert_eq!(r, int_collection(&[5, 4, 3, 2, 1]));
    }

    #[test]
    fn slice_basic_range() {
        assert_eq!(
            Slice
                .execute(
                    &[int_spec(&[10, 20, 30, 40, 50]), lit_int(1), lit_int(4)],
                    &mut MockCtx,
                )
                .unwrap()
                .into_value(),
            int_collection(&[20, 30, 40])
        );
    }

    #[test]
    fn slice_empty_when_start_equals_end() {
        assert_eq!(
            Slice
                .execute(
                    &[int_spec(&[1, 2, 3]), lit_int(2), lit_int(2)],
                    &mut MockCtx,
                )
                .unwrap()
                .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn slice_clamps_out_of_bounds() {
        assert_eq!(
            Slice
                .execute(
                    &[int_spec(&[1, 2, 3]), lit_int(1), lit_int(99)],
                    &mut MockCtx,
                )
                .unwrap()
                .into_value(),
            int_collection(&[2, 3])
        );
    }

    #[test]
    fn slice_type_mismatch_errors() {
        assert!(
            Slice
                .execute(
                    &[int_spec(&[1, 2, 3]), lit_str("1"), lit_int(2)],
                    &mut MockCtx,
                )
                .is_err()
        );
    }

    #[test]
    fn slice_wrong_arg_count() {
        assert!(
            Slice
                .execute(&[int_spec(&[1, 2, 3]), lit_int(0)], &mut MockCtx)
                .is_err()
        );
    }

    // Happy-path `find` exercises the lambda-capable context; covered by
    // `find_returns_the_first_match_or_nothing` in `tests/collection.rs`.

    #[test]
    fn find_wrong_arg_count() {
        assert!(Find.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
    }

    #[test]
    fn at_valid_index() {
        assert_eq!(
            At.execute(&[int_spec(&[10, 20, 30]), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(20)
        );
    }

    #[test]
    fn at_out_of_bounds() {
        assert!(
            At.execute(&[int_spec(&[10, 20]), lit_int(5)], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn type_mismatch_errors() {
        assert!(
            At.execute(&[int_spec(&[1, 2]), lit_str("1")], &mut MockCtx)
                .is_err()
        );
        assert!(
            Range
                .execute(&[lit_str("1"), lit_int(5), lit_int(1)], &mut MockCtx)
                .is_err()
        );
        assert!(
            Take.execute(&[int_spec(&[1, 2]), lit_str("1")], &mut MockCtx)
                .is_err()
        );
        assert!(
            Drop.execute(&[int_spec(&[1, 2]), lit_str("1")], &mut MockCtx)
                .is_err()
        );
    }

    #[test]
    fn wrong_arg_count_errors() {
        assert!(Size.execute(&[], &mut MockCtx).is_err());
        assert!(At.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
        // Range now accepts 1, 2, or 3 args (Phase 3) — only 0 or >3 errors.
        assert!(Range.execute(&[], &mut MockCtx).is_err());
        assert!(
            Range
                .execute(
                    &[lit_int(1), lit_int(2), lit_int(3), lit_int(4)],
                    &mut MockCtx,
                )
                .is_err()
        );
        assert!(Take.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
    }
}
