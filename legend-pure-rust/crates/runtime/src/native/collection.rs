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

//! Collection native functions: `size`, `isEmpty`, `at`, `first`, `last`,
//! `range`, `take`, `drop`, `concatenate`, `map`, `filter`, `fold`.

use im_rc::Vector as PVector;
use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::{MapState, Value, ValueKey};

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

/// Pure `isEmpty(Any[*]): Boolean[1]`
#[derive(Debug)]
pub struct IsEmpty;

impl NativeFunction for IsEmpty {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("isEmpty", &values, 1)?;
        Ok(Evaluated::new(Value::Boolean(values[0].is_empty())))
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

/// Pure `last(Any[*]): Any[0..1]` — last element or Unit.
#[derive(Debug)]
pub struct Last;

impl NativeFunction for Last {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("last", &values, 1)?;
        let result = match &values[0] {
            Value::Collection(v) => v.back().cloned().unwrap_or(Value::Unit),
            Value::Unit => Value::Unit,
            other => other.clone(),
        };
        Ok(Evaluated::new(result))
    }
}

/// Pure `init<T>(set:T[*]):T[*]` — all but the last element.
///
/// Returns an empty collection for `Unit`, the input unchanged for a
/// single scalar (since there's no "last to drop" conceptually), and
/// every element except the final one for a multi-element collection.
/// The platform docstring is "the initial segment of the collection".
#[derive(Debug)]
pub struct Init;

impl NativeFunction for Init {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("init", &values, 1)?;
        let result = match &values[0] {
            Value::Collection(v) => {
                if v.is_empty() {
                    Value::Unit
                } else {
                    let mut trimmed = (**v).clone();
                    trimmed.pop_back();
                    let as_vec: Vec<Value> = trimmed.iter().cloned().collect();
                    Value::from_vec(as_vec)
                }
            }
            // Scalar / Unit: no "last" to drop; mirror Java Pure's
            // behaviour of returning the empty segment.
            _ => Value::Unit,
        };
        Ok(Evaluated::new(result))
    }
}

/// Pure `tail<T>(set:T[*]):T[*]`
///
/// Returns everything but the first element. Mirror of [`Init`] (which
/// drops the last). Empty for `[]` or single-element collections.
#[derive(Debug)]
pub struct Tail;

impl NativeFunction for Tail {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("tail", &values, 1)?;
        let result = match &values[0] {
            Value::Collection(v) => {
                if v.is_empty() {
                    Value::Unit
                } else {
                    let mut trimmed = (**v).clone();
                    trimmed.pop_front();
                    let as_vec: Vec<Value> = trimmed.iter().cloned().collect();
                    Value::from_vec(as_vec)
                }
            }
            // Single scalar / Unit: nothing-after-the-first → empty.
            _ => Value::Unit,
        };
        Ok(Evaluated::new(result))
    }
}

/// Pure `zip<T,U>(set1:T[*], set2:U[*]):Pair<T,U>[*]`
///
/// Pairwise combines two collections, truncating to the shorter length.
/// Each pair allocates a heap `Pair` object with `first` / `second`
/// slots, mirroring the shape produced by [`KeyValues`] and consumed by
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

/// Pure `values<U,V>(m:Map<U,V>[1]):V[*]`
///
/// Returns the values stored in the map as a flat collection. Iteration
/// order follows the HAMT — deterministic per run, not insertion-ordered.
/// Mirror of [`Keys`].
#[derive(Debug)]
pub struct Values;

impl NativeFunction for Values {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("values", &values, 1)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let snapshot: Vec<Value> = {
            let state = m.borrow();
            state.entries.values().cloned().collect()
        };
        Ok(Evaluated::new(Value::from_vec(snapshot)))
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

/// Pure `concatenate(Any[*], Any[*]): Any[*]` — concatenate two collections.
#[derive(Debug)]
pub struct Concatenate;

impl NativeFunction for Concatenate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("concatenate", &values, 2)?;
        let mut a = values[0].to_collection();
        let b = values[1].to_collection();
        a.append(b);
        Ok(Evaluated::new(Value::Collection(Box::new(a))))
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

/// Pure `filter(T[*], Function<{T[1]->Boolean[1]}>[1]): T[*]`
///
/// Filters a collection using a boolean predicate.
#[derive(Debug)]
pub struct Filter;

impl NativeFunction for Filter {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("filter", args, 2)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        let mut results = Vec::with_capacity(source.len());
        for item in &source {
            let pred = ctx.call_function(&lambda_val, std::slice::from_ref(item))?;
            if pred.as_boolean()? {
                results.push(item.clone());
            }
        }

        Ok(Evaluated::new(Value::from_vec(results)))
    }
}

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

/// Pure `forAll<T>(value:T[*], func:Function<{T[1]->Boolean[1]}>[1]):Boolean[1]`
///
/// Returns `true` iff every element satisfies the predicate. For an empty
/// input vacuously returns `true`.
#[derive(Debug)]
pub struct ForAll;

impl NativeFunction for ForAll {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("forAll", args, 2)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();

        for item in &source {
            let pred = ctx.call_function(&lambda_val, std::slice::from_ref(item))?;
            if !pred.as_boolean()? {
                return Ok(Evaluated::new(Value::Boolean(false)));
            }
        }

        Ok(Evaluated::new(Value::Boolean(true)))
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

/// Pure `toOne<T>(values:T[*]):T[1]` / `toOne<T>(values:T[0..1]):T[1]`
///
/// Asserts the collection contains exactly one element and returns that
/// element as a scalar. Produces [`PureRuntimeError::MultiplicityViolation`]
/// when the input is empty or has multiple elements.
#[derive(Debug)]
pub struct ToOne;

impl NativeFunction for ToOne {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // 1-arg form: cast to [1]; 2-arg form: same with custom error message.
        // The platform declaration in toOne.pure has both shapes; the message
        // arg is informational only — we don't currently surface it.
        if values.is_empty() || values.len() > 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "toOne: expected 1 or 2 argument(s), got {}",
                values.len()
            ))
            .into());
        }
        Ok(Evaluated::new(values[0].to_one()?.clone()))
    }
}

/// Pure `toOneMany<T>(values:T[*]):T[1..*]` /
/// `toOneMany<T>(values:T[*], message:String[1]):T[1..*]`
///
/// Casts the collection to `[1..*]` — passes through any non-empty
/// collection (or scalar wrapped as a singleton); errors on empty.
/// Mirror of [`ToOne`] but with the upper bound relaxed to `*`.
#[derive(Debug)]
pub struct ToOneMany;

impl NativeFunction for ToOneMany {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        if values.is_empty() || values.len() > 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "toOneMany: expected 1 or 2 argument(s), got {}",
                values.len()
            ))
            .into());
        }
        let coll = values[0].to_collection();
        if coll.is_empty() {
            return Err(PureRuntimeError::MultiplicityViolation {
                expected: "[1..*]".into(),
                actual: 0,
            }
            .into());
        }
        // Pass through unchanged — the input is already a valid collection
        // / scalar / single-element wrapper. The cast to [1..*] is purely a
        // multiplicity-typing assertion; the value shape doesn't change.
        Ok(Evaluated::new(values[0].clone()))
    }
}

/// Pure `toMultiplicity<T|z>(source:T[*], object:Any[z]):T[z]`
///
/// Narrows a `T[*]` collection to whatever static multiplicity `z` the
/// second argument carries at the call site. The second arg is a sentinel
/// — its value is unused; only its compile-time multiplicity drives the
/// narrowing. The runtime validates that `source.size()` falls within the
/// inferred bounds and returns the same values at the narrowed multiplicity,
/// or raises a Pure exception on cardinality mismatch.
///
/// Error text matches Java parity verbatim:
/// `"Cannot cast a collection of size N to multiplicity [BOUNDS]"` —
/// see `AbstractTestToMultiplicity.java` in the upstream test tree.
#[derive(Debug)]
pub struct ToMultiplicity;

impl NativeFunction for ToMultiplicity {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args_spec("toMultiplicity", args, 2)?;
        let target_mult = args[1]
            .type_info
            .as_deref()
            .map(|ti| ti.multiplicity.clone())
            .ok_or_else(|| {
                PureException::from(PureRuntimeError::EvaluationError(
                    "toMultiplicity: second argument is missing static type info".into(),
                ))
            })?;
        let (lower, upper) = multiplicity_bounds(&target_mult);
        let values = force_all(args, ctx)?;
        let coll = values[0].to_collection();
        let size = coll.len();
        let in_lower = (size as u32) >= lower;
        let in_upper = upper.is_none_or(|u| (size as u32) <= u);
        if !(in_lower && in_upper) {
            return Err(PureRuntimeError::EvaluationError(format!(
                "Cannot cast a collection of size {size} to multiplicity {}",
                format_multiplicity_bounds(&target_mult),
            ))
            .into());
        }
        Ok(Evaluated::new(values[0].clone()))
    }
}

/// Variant-by-variant `(lower, upper)` extraction for [`pure::Multiplicity`].
/// `upper = None` means unbounded (`*`). `Variable(_)` treated as `[*]`
/// (unbound) — at runtime a parametric mult should have been substituted to
/// a concrete variant by dispatch, but keep the fallback non-panicking.
fn multiplicity_bounds(m: &legend_pure_parser_pure::types::Multiplicity) -> (u32, Option<u32>) {
    use legend_pure_parser_pure::types::Multiplicity as M;
    match m {
        M::PureOne => (1, Some(1)),
        M::ZeroOrOne => (0, Some(1)),
        M::ZeroOrMany | M::Variable(_) => (0, None),
        M::OneOrMany => (1, None),
        M::Range { lower, upper } => (*lower, *upper),
    }
}

/// Java-parity rendering of a multiplicity for the `toMultiplicity` error
/// message. Matches `org.finos.legend.pure.m3.tests.function.base.multiplicity.AbstractTestToMultiplicity`
/// expectations exactly: `[1]`, `[0..1]`, `[1..*]`, `[*]`, `[N]`, `[N..M]`,
/// `[N..*]`.
fn format_multiplicity_bounds(m: &legend_pure_parser_pure::types::Multiplicity) -> String {
    use legend_pure_parser_pure::types::Multiplicity as M;
    match m {
        M::PureOne => "[1]".into(),
        M::ZeroOrOne => "[0..1]".into(),
        M::ZeroOrMany => "[*]".into(),
        M::OneOrMany => "[1..*]".into(),
        M::Variable(name) => format!("[{name}]"),
        M::Range { lower, upper } => match upper {
            Some(u) if u == lower => format!("[{lower}]"),
            Some(u) => format!("[{lower}..{u}]"),
            None => format!("[{lower}..*]"),
        },
    }
}

/// `expect_args` analogue that runs on the raw `&[ValueSpec]` rather than
/// post-`force_all` values. Needed when the native reads `type_info` off the
/// spec before forcing — `force_all` clones values out and discards the
/// surrounding `ValueSpec` headers.
fn expect_args_spec(name: &str, args: &[ValueSpec], expected: usize) -> Result<(), PureException> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(PureRuntimeError::EvaluationError(format!(
            "{name}: expected {expected} argument(s), got {}",
            args.len()
        ))
        .into())
    }
}

/// Pure `isNotEmpty(Any[*]):Boolean[1]`
///
/// Counterpart of `isEmpty` — returns `true` when the collection has at least
/// one element. Also `true` for any scalar value.
#[derive(Debug)]
pub struct IsNotEmpty;

impl NativeFunction for IsNotEmpty {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("isNotEmpty", &values, 1)?;
        Ok(Evaluated::new(Value::Boolean(!values[0].is_empty())))
    }
}

/// Pure `contains<T>(T[*], Any[1]): Boolean[1]`
///
/// Returns `true` when the collection contains an element equal to the
/// given needle, using [`Value`] equality. Empty collections always return
/// `false`. A scalar input is treated as a singleton collection.
#[derive(Debug)]
pub struct Contains;

impl NativeFunction for Contains {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("contains", &values, 2)?;
        let haystack = values[0].to_collection();
        let needle = &values[1];
        let ctx_ref: &dyn EvalContextTrait = ctx;
        Ok(Evaluated::new(Value::Boolean(haystack.iter().any(|v| {
            crate::native::equality::values_equal(ctx_ref, v, needle)
        }))))
    }
}

/// Pure `reverse<T|m>(T[m]): T[m]`
///
/// Returns the input collection in reverse order. A scalar input is
/// returned unchanged (a single-element reversal is a no-op). Empty
/// input yields [`Value::Unit`].
#[derive(Debug)]
pub struct Reverse;

impl NativeFunction for Reverse {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("reverse", &values, 1)?;
        let source = values[0].to_collection();
        let reversed: Vec<Value> = source.iter().rev().cloned().collect();
        Ok(Evaluated::new(Value::from_vec(reversed)))
    }
}

/// Pure `indexOf<T>(T[*], T[1]): Integer[1]`
///
/// Returns the 0-based index of the first element equal to the needle,
/// or `-1` when the needle is not found. Uses [`Value`] equality.
#[derive(Debug)]
pub struct IndexOf;

impl NativeFunction for IndexOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("indexOf", &values, 2)?;
        let haystack = values[0].to_collection();
        let needle = &values[1];
        let ctx_ref: &dyn EvalContextTrait = ctx;
        let idx = haystack
            .iter()
            .position(|v| crate::native::equality::values_equal(ctx_ref, v, needle));
        let result = match idx {
            #[allow(clippy::cast_possible_wrap)]
            Some(i) => Value::Integer(i as i64),
            None => Value::Integer(-1),
        };
        Ok(Evaluated::new(result))
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

/// Pure `add<T|m>(T[m], T[1]): T[$1_MANY$]`
/// Pure `add<T|m>(T[m], Integer[1], T[1]): T[$1_MANY$]`
///
/// 2-arg form appends to the end; 3-arg form inserts at the given index
/// (0-based, clamped to `[0, len]`). Scalar inputs are promoted; empty
/// inputs yield a singleton.
#[derive(Debug)]
pub struct Add;

impl NativeFunction for Add {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        match values.len() {
            2 => {
                let mut coll = values[0].to_collection();
                coll.push_back(values[1].clone());
                let as_vec: Vec<Value> = coll.into_iter().collect();
                Ok(Evaluated::new(Value::from_vec(as_vec)))
            }
            3 => {
                let mut coll = values[0].to_collection();
                let raw_index = values[1].as_integer()?;
                let len = i64::try_from(coll.len()).unwrap_or(i64::MAX);
                let index = raw_index.clamp(0, len);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                coll.insert(index as usize, values[2].clone());
                let as_vec: Vec<Value> = coll.into_iter().collect();
                Ok(Evaluated::new(Value::from_vec(as_vec)))
            }
            n => Err(PureRuntimeError::EvaluationError(format!(
                "add: expected 2 or 3 argument(s), got {n}"
            ))
            .into()),
        }
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

/// Pure `removeAllOptimized<T>(T[*], T[*]): T[*]`
///
/// Returns a new collection containing the elements of the first argument
/// that are **not** present in the second argument. Uses [`Value`] equality
/// for membership. Preserves the original ordering of the first argument
/// and keeps duplicates that don't match any element in the second argument.
#[derive(Debug)]
pub struct RemoveAllOptimized;

impl NativeFunction for RemoveAllOptimized {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("removeAllOptimized", &values, 2)?;
        let source = values[0].to_collection();
        let to_remove = values[1].to_collection();

        let ctx_ref: &dyn EvalContextTrait = ctx;
        let out: Vec<Value> = source
            .iter()
            .filter(|v| {
                !to_remove
                    .iter()
                    .any(|r| crate::native::equality::values_equal(ctx_ref, r, v))
            })
            .cloned()
            .collect();
        Ok(Evaluated::new(Value::from_vec(out)))
    }
}

/// Coerce a runtime [`Value`] into a hashable [`ValueKey`].
///
/// Primitives + dates map to their direct `ValueKey` variant. Heap objects
/// default to identity (`ValueKey::Object(ObjectHandle)`) unless the owning
/// class annotates one or more properties with `<<equality.Key>>` — in
/// that case the key extracts those properties' values and becomes
/// `ValueKey::ObjectByEqualityKeys`, so instances with equal annotated
/// fields collide in the same bucket even across distinct `ObjectHandle`s.
/// Collections, maps, and functions never compare meaningfully and raise
/// an explicit error.
#[allow(clippy::result_large_err)]
fn value_to_key(v: &Value, ctx: &dyn EvalContextTrait) -> Result<ValueKey, PureException> {
    match v {
        Value::Boolean(b) => Ok(ValueKey::Boolean(*b)),
        Value::Integer(i) => Ok(ValueKey::Integer(*i)),
        Value::Decimal(d) => Ok(ValueKey::Decimal(*d)),
        Value::String(s) => Ok(ValueKey::String(s.clone())),
        Value::Date(d) => Ok(ValueKey::Date(*d)),
        Value::StrictTime(t) => Ok(ValueKey::StrictTime(*t)),
        Value::EnumValue { enum_id, member } => Ok(ValueKey::EnumValue {
            enum_id: *enum_id,
            member: member.clone(),
        }),
        Value::Object(id) => object_equality_key(id.clone(), ctx),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "Map key must be a hashable primitive or object identity, got {}",
            other.type_name()
        ))
        .into()),
    }
}

/// Resolve a heap object into a `ValueKey`.
///
/// Walks the object's classifier to its owning Class, then looks for
/// `<<equality.Key>>`-annotated properties. If any exist, extract their
/// current values and build `ObjectByEqualityKeys`; otherwise fall back
/// to identity equality via `ValueKey::Object(ObjectHandle)`.
#[allow(clippy::result_large_err)]
fn object_equality_key(
    id: ObjectHandle,
    ctx: &dyn EvalContextTrait,
) -> Result<ValueKey, PureException> {
    let classifier = ctx.heap().classifier(&id)?.to_string();
    let Some(class_id) = crate::m3_paths::resolve(ctx.model(), &classifier) else {
        return Ok(ValueKey::Object(id));
    };

    // Canonical `<<equality.Key>>` property list (profile identity +
    // stereotype label both checked in the shared helper).
    let equality_props = crate::native::equality::equality_key_properties(ctx.model(), class_id);

    if equality_props.is_empty() {
        return Ok(ValueKey::Object(id));
    }

    let mut fields: Vec<(SmolStr, ValueKey)> = Vec::with_capacity(equality_props.len());
    for prop_name in equality_props {
        let values = ctx.heap().get_property_values(&id, prop_name.as_str())?;
        let field_key = match values.iter().next() {
            Some(v) => value_to_key(v, ctx)?,
            // Absent annotated field — represent as a sentinel string so
            // two instances both missing the field still collide.
            None => ValueKey::String(SmolStr::new_static("")),
        };
        fields.push((prop_name, field_key));
    }
    Ok(ValueKey::ObjectByEqualityKeys { class_id, fields })
}

/// Inverse of [`value_to_key`] — recover a [`Value`] shape from a stored
/// key so `keys()` can emit the collection.
fn key_to_value(k: &ValueKey) -> Value {
    match k {
        ValueKey::Boolean(b) => Value::Boolean(*b),
        ValueKey::Integer(i) => Value::Integer(*i),
        ValueKey::Decimal(d) => Value::Decimal(*d),
        ValueKey::String(s) => Value::String(s.clone()),
        ValueKey::Date(d) => Value::Date(*d),
        ValueKey::StrictTime(t) => Value::StrictTime(*t),
        ValueKey::Object(id) => Value::Object(id.clone()),
        ValueKey::ObjectByEqualityKeys { .. } => {
            // No reified heap instance corresponds uniquely to a
            // value-keyed entry; callers that need the original object
            // (e.g. `keys()` for introspection) see a placeholder. This
            // matches the Java runtime's "keyed by fields, not instance"
            // semantic — the original instance isn't recoverable from a
            // stored key.
            Value::Unit
        }
        ValueKey::EnumValue { enum_id, member } => Value::EnumValue {
            enum_id: *enum_id,
            member: member.clone(),
        },
    }
}

/// Pure `newMap<U,V>(pairs:Pair<U,V>[*]):Map<U,V>[1]`
///
/// Fold a collection of `Pair` heap objects (each with `first` / `second`
/// properties) into an `im_rc::HashMap`-backed `Value::Map`. The second
/// overload (`newMap(pairs, p:Property[*])`) takes a property-list used
/// as an equality key by the Java runtime — we accept and ignore it for
/// now because Rust's `ValueKey` already drives equality from the value.
#[derive(Debug)]
pub struct NewMap;

impl NativeFunction for NewMap {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // `args[0]` is the pair collection; `args[1]` (optional) is the
        // custom-key property list, not yet honoured.
        if values.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "newMap: expected at least 1 argument (the pairs)".into(),
            )
            .into());
        }
        let pairs = values[0].to_collection();
        let mut entries = im_rc::HashMap::new();
        for pair in &pairs {
            let Value::Object(obj_id) = pair else {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "newMap: expected Pair<U,V>, got {}",
                    pair.type_name()
                ))
                .into());
            };
            let first_vals = ctx.heap().get_property_values(&obj_id.clone(), "first")?;
            let second_vals = ctx.heap().get_property_values(&obj_id.clone(), "second")?;
            let Some(k) = first_vals.iter().next() else {
                return Err(PureRuntimeError::EvaluationError(
                    "newMap: Pair.first is empty".into(),
                )
                .into());
            };
            let Some(v) = second_vals.iter().next() else {
                return Err(PureRuntimeError::EvaluationError(
                    "newMap: Pair.second is empty".into(),
                )
                .into());
            };
            let key = value_to_key(k, ctx)?;
            entries.insert(key, v.clone());
        }
        Ok(Evaluated::new(Value::Map(new_map_cell(entries, 0))))
    }
}

/// Wrap a freshly-built [`MapState`] in the `Rc<RefCell<…>>` cell that
/// `Value::Map` carries. Keeps the construction site for new map
/// identities centralised — every `put` / `putAll` / `replaceAll` / etc.
/// allocates a fresh cell so Pure semantics ("`put` returns a new map")
/// stays intact, while `getIfAbsentPutWithKey` mutates an existing cell.
fn new_map_cell(
    entries: im_rc::HashMap<ValueKey, Value>,
    get_if_absent_counter: i64,
) -> std::rc::Rc<std::cell::RefCell<MapState>> {
    std::rc::Rc::new(std::cell::RefCell::new(MapState {
        entries,
        get_if_absent_counter,
    }))
}

/// Pure `get<U,V>(m:Map<U,V>[1], key:U[1]):V[0..1]`
///
/// Returns the value bound to `key` or [`Value::Unit`] when the map has
/// no mapping.
#[derive(Debug)]
pub struct Get;

impl NativeFunction for Get {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("get", &values, 2)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let key = value_to_key(&values[1], ctx)?;
        match m.borrow().entries.get(&key) {
            Some(v) => Ok(Evaluated::new(v.clone())),
            None => Ok(Evaluated::new(Value::Unit)),
        }
    }
}

/// Pure `put<U,V>(m:Map<U,V>[1], key:U[1], value:V[1]):Map<U,V>[1]`
///
/// Returns a **new** map (fresh `Rc<RefCell<MapState>>`) carrying every
/// entry of `m` plus `key -> value`. Mirrors Java Pure's `Put.java`:
///
/// ```java
/// MapCoreInstance newMap = new MapCoreInstance(map, true, processorSupport);
/// newMap.getMap().put(key, value);
/// return newMap;
/// ```
///
/// `m` is untouched — HAMT structural sharing keeps the entry copy
/// O(log N), and the returned cell has `get_if_absent_counter` reset
/// to zero (Java's `MapCoreInstance` copy ctor does the same when
/// `copyData=true` would normally clone the stats — but `Put` never
/// reads the stats so reset is the simplest correct choice; tests that
/// observe stats use `getIfAbsentPutWithKey`, which mutates in place
/// and never goes through `Put`).
#[derive(Debug)]
pub struct Put;

impl NativeFunction for Put {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("put", &values, 3)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let key = value_to_key(&values[1], ctx)?;
        let mut updated = m.borrow().entries.clone();
        updated.insert(key, values[2].clone());
        Ok(Evaluated::new(Value::Map(new_map_cell(updated, 0))))
    }
}

/// Pure `keys<U,V>(m:Map<U,V>[1]):U[*]`
///
/// Returns the map's keys as a collection. Ordering follows the
/// underlying HAMT's iteration order — deterministic per run, but not
/// insertion-ordered. Platform tests assert set-equality (`assertEquals`
/// with a single key or sorted sets), not a specific order.
#[derive(Debug)]
pub struct Keys;

impl NativeFunction for Keys {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("keys", &values, 1)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let state = m.borrow();
        let items: Vec<Value> = state.entries.keys().map(key_to_value).collect();
        Ok(Evaluated::new(Value::from_vec(items)))
    }
}

/// Pure `keyValues<U,V>(m:Map<U,V>[1]):Pair<U,V>[*]`
///
/// Materialise the map's entries as a collection of `Pair<U,V>` heap
/// objects (classifier `meta::pure::functions::collection::Pair`) with
/// `first` = key, `second` = value. Ordering follows the HAMT's
/// iteration order — deterministic per run, not insertion-ordered.
#[derive(Debug)]
pub struct KeyValues;

impl NativeFunction for KeyValues {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("keyValues", &values, 1)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        // Snapshot the entries so we don't hold a borrow on the cell while
        // mutating the heap (the heap is independent of the map cell, but
        // keeping the borrow tight protects against any future re-entry).
        let snapshot: Vec<(ValueKey, Value)> = {
            let state = m.borrow();
            state
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut items: Vec<Value> = Vec::with_capacity(snapshot.len());
        for (k, v) in &snapshot {
            let obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::PAIR);
            ctx.heap_mut()
                .mutate_add(&obj, "first", &[key_to_value(k)])?;
            ctx.heap_mut()
                .mutate_add(&obj, "second", std::slice::from_ref(v))?;
            items.push(Value::Object(obj));
        }
        Ok(Evaluated::new(Value::from_vec(items)))
    }
}

/// Pure `putAll<U,V>(m:Map<U,V>[1], pairs:Pair<U,V>[*]):Map<U,V>[1]` /
/// `putAll<U,V>(m:Map<U,V>[1], o:Map<U,V>[1]):Map<U,V>[1]`
///
/// Returns a fresh map with all entries of `m` plus every entry from the
/// second argument (collection of `Pair` objects or another `Map`). The
/// second argument's bindings overwrite colliding keys in `m`.
#[derive(Debug)]
pub struct PutAll;

impl NativeFunction for PutAll {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("putAll", &values, 2)?;
        let Value::Map(base) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        // Clone the base entries; HAMT structural sharing keeps this O(1).
        let mut updated: im_rc::HashMap<ValueKey, Value> = base.borrow().entries.clone();
        match &values[1] {
            Value::Map(other) => {
                // Snapshot the other map's entries before iterating so we
                // never hold a borrow on `other` across the insertion loop
                // (and so `other == base` aliasing stays sound).
                let other_entries: Vec<(ValueKey, Value)> = other
                    .borrow()
                    .entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (k, v) in other_entries {
                    updated.insert(k, v);
                }
            }
            other => {
                for pair in &other.to_collection() {
                    let (k, v) = pair_first_second(ctx, pair, "putAll")?;
                    updated.insert(value_to_key(&k, ctx)?, v);
                }
            }
        }
        Ok(Evaluated::new(Value::Map(new_map_cell(updated, 0))))
    }
}

/// Pure `replaceAll<U,V>(m:Map<U,V>[1], pairs:Pair<U,V>[*]):Map<U,V>[1]`
///
/// Returns a fresh map containing only the entries produced by folding
/// `pairs`. Any existing keys on `m` that aren't in the new pair list are
/// dropped. Mirrors Java Pure's `^Map<U,V>(_func=$m._func)` reset
/// semantics — the resulting map preserves the original's custom-key
/// function (when one exists) but discards every stored binding.
#[derive(Debug)]
pub struct ReplaceAll;

impl NativeFunction for ReplaceAll {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("replaceAll", &values, 2)?;
        let Value::Map(_) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let mut updated: im_rc::HashMap<ValueKey, Value> = im_rc::HashMap::new();
        for pair in &values[1].to_collection() {
            let (k, v) = pair_first_second(ctx, pair, "replaceAll")?;
            updated.insert(value_to_key(&k, ctx)?, v);
        }
        Ok(Evaluated::new(Value::Map(new_map_cell(updated, 0))))
    }
}

/// Pure
/// `getIfAbsentPutWithKey<U,V>(m:Map<U,V>[1], key:U[1], func:Function<{U[1]->V[0..1]}>[1]):V[0..1]`
///
/// Returns the value at `key` if present. Otherwise increments the map's
/// `getIfAbsentCounter`, evaluates `func(key)`, stores the result under
/// `key` in the same shared `MapState`, and returns it. The map is
/// observably mutated for every binding that points at the same
/// `Rc<RefCell<MapState>>` — mirrors Java Pure's `MapCoreInstance` which
/// holds an in-place mutable map alongside its `PureMapStats`
/// (see `GetIfAbsentPutWithKey.java`).
///
/// **Borrow discipline:** the borrow on the cell is released *before*
/// `ctx.call_function` runs the lambda, so a re-entrant call into
/// natives that read or mutate the same map (`get`, `put`,
/// `getIfAbsentPutWithKey` recursively, …) does not panic. The counter
/// is incremented up front to match Java's ordering — Eclipse
/// Collections' `getIfAbsentPutWithKey` calls the lambda after entering
/// the absent branch, and the `incrementGetIfAbsentCounter()` call sits
/// at the top of that lambda body.
#[derive(Debug)]
pub struct GetIfAbsentPutWithKey;

impl NativeFunction for GetIfAbsentPutWithKey {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() != 3 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "getIfAbsentPutWithKey: expected 3 arguments, got {}",
                args.len()
            ))
            .into());
        }
        let m_val = ctx.evaluate(&args[0])?.into_value();
        let key_val = ctx.evaluate(&args[1])?.into_value();
        let func_val = ctx.evaluate(&args[2])?.into_value();
        let Value::Map(m) = &m_val else {
            return Err(PureRuntimeError::type_mismatch("Map", &m_val).into());
        };
        let key = value_to_key(&key_val, ctx)?;

        // Read-only check first; drop the borrow before any reentrant
        // call so a lambda may safely touch the same map.
        if let Some(existing) = {
            let state = m.borrow();
            state.entries.get(&key).cloned()
        } {
            return Ok(Evaluated::new(existing));
        }

        // Java order: bump the stats counter, then evaluate the lambda,
        // then insert. The counter increment lives in the lambda body in
        // Java (see `GetIfAbsentPutWithKey.java`), but Pure code can
        // never observe the intermediate state because lambda evaluation
        // runs to completion before the next statement.
        m.borrow_mut().get_if_absent_counter += 1;

        let result = ctx.call_function(&func_val, &[key_val])?;

        // Re-borrow to install the new entry. Pure's signature is
        // `V[0..1]`, so a `Value::Unit` result represents "absent" and
        // we still record it (matches Java: `getIfAbsentPutWithKey`
        // unconditionally stores the lambda's return).
        m.borrow_mut().entries.insert(key, result.clone());

        Ok(Evaluated::new(result))
    }
}

/// Pure `getMapStats<U,V>(m:Map<U,V>[1]):MapStats[0..1]`
///
/// Materialises the map's mutation stats as a
/// `meta::pure::functions::collection::MapStats` heap object with a
/// single `getIfAbsentCounter:Integer[1]` property. Mirrors Java Pure's
/// `MapCoreInstance.getStats()` plus the
/// `wrapValueSpecification(stats, …)` wrapping in the interpreted
/// engine. Returns the stats object even when the counter is zero —
/// the test reads `.getIfAbsentCounter` directly on the result and
/// would NPE on an empty multiplicity.
#[derive(Debug)]
pub struct GetMapStats;

impl NativeFunction for GetMapStats {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("getMapStats", &values, 1)?;
        let Value::Map(m) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Map", &values[0]).into());
        };
        let map = m.borrow();
        let counter = map.get_if_absent_counter;
        let size = map.entries.len() as i64;
        drop(map);
        let obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::MAP_STATS);
        ctx.heap_mut()
            .mutate_add(&obj, "getIfAbsentCounter", &[Value::Integer(counter)])?;
        ctx.heap_mut()
            .mutate_add(&obj, "size", &[Value::Integer(size)])?;
        Ok(Evaluated::new(Value::Object(obj)))
    }
}

/// Pure `groupBy<X,K>(xs:X[*], f:Function<{X[1]->K[1]}>[1]):Map<K,List<X>>[1]`
///
/// For each item in `xs`, evaluate `f(item)` to produce a grouping key,
/// then bucket the items into a `Map<K, List<X>>` where each `List`
/// preserves the original order. The `List` is materialised as a
/// `meta::pure::functions::collection::List` heap object with the
/// grouped items stored in its `values` property — matching the shape
/// other natives consume via `List.values`.
#[derive(Debug)]
pub struct GroupBy;

impl NativeFunction for GroupBy {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() != 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "groupBy: expected 2 arguments, got {}",
                args.len()
            ))
            .into());
        }
        let xs = ctx.evaluate(&args[0])?.into_value();
        let f = ctx.evaluate(&args[1])?.into_value();
        let items = xs.to_collection();

        // Preserve insertion order of keys so `.keys()` output is
        // reproducible. im_rc::HashMap doesn't guarantee order, so we
        // track order separately via a Vec<ValueKey> of first-seen keys.
        let mut groups: im_rc::HashMap<ValueKey, Vec<Value>> = im_rc::HashMap::new();
        let mut key_order: Vec<ValueKey> = Vec::new();
        for item in &items {
            let key_val = ctx.call_function(&f, std::slice::from_ref(item))?;
            let key = value_to_key(&key_val, ctx)?;
            if !groups.contains_key(&key) {
                key_order.push(key.clone());
            }
            groups.entry(key).or_default().push(item.clone());
        }

        let mut map: im_rc::HashMap<ValueKey, Value> = im_rc::HashMap::new();
        for key in key_order {
            let bucket = groups.remove(&key).unwrap_or_default();
            let list_obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::LIST);
            for v in &bucket {
                ctx.heap_mut()
                    .mutate_add(&list_obj, "values", std::slice::from_ref(v))?;
            }
            map.insert(key, Value::Object(list_obj));
        }
        Ok(Evaluated::new(Value::Map(new_map_cell(map, 0))))
    }
}

/// Read the `first` / `second` properties of a `Pair` heap object. Emits
/// a consistent error message tagged with the caller's native name so
/// failure traces point at the right native.
#[allow(clippy::result_large_err)]
fn pair_first_second(
    ctx: &mut dyn EvalContextTrait,
    pair: &Value,
    native_name: &'static str,
) -> Result<(Value, Value), PureException> {
    let Value::Object(obj_id) = pair else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: expected Pair<U,V>, got {}",
            pair.type_name()
        ))
        .into());
    };
    let first_vals = ctx.heap().get_property_values(&obj_id.clone(), "first")?;
    let second_vals = ctx.heap().get_property_values(&obj_id.clone(), "second")?;
    let Some(k) = first_vals.iter().next() else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: Pair.first is empty"
        ))
        .into());
    };
    let Some(v) = second_vals.iter().next() else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: Pair.second is empty"
        ))
        .into());
    };
    Ok((k.clone(), v.clone()))
}

/// Pure `replaceTreeNode(root:TreeNode[1], target:TreeNode[1], value:TreeNode[1]):TreeNode[1]`.
///
/// Walks the `root` tree (via `TreeNode.childrenData`) and produces a
/// new tree whose subtree-identical-to-`target` slot is replaced by
/// `value`. Identity comparison is `Rc::ptr_eq` (matches Java's `==`
/// in `ReplaceTreeNode.java:58, 85`). Errors when no replacement
/// occurs (Java throws `"Copy failed ... node not found!"`).
///
/// Persistent-copy traversal: each `TreeNode` in the result is a
/// fresh heap object, leaving `root` and all its children untouched
/// — Pure programs that retain `root` see the original tree intact
/// post-call. Non-`childrenData` properties are copied by value.
#[derive(Debug)]
pub struct ReplaceTreeNode;

impl NativeFunction for ReplaceTreeNode {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("replaceTreeNode", &values, 3)?;
        let source = match &values[0] {
            Value::Object(h) => h.clone(),
            other => return Err(PureRuntimeError::type_mismatch("TreeNode", other).into()),
        };
        let target = match &values[1] {
            Value::Object(h) => h.clone(),
            other => return Err(PureRuntimeError::type_mismatch("TreeNode", other).into()),
        };
        let replacement = match &values[2] {
            Value::Object(h) => h.clone(),
            other => return Err(PureRuntimeError::type_mismatch("TreeNode", other).into()),
        };

        // Root-is-target short-circuit (Java's `if (sourceTree ==
        // targetNode) newInstance = subTree`).
        if std::rc::Rc::ptr_eq(&source, &target) {
            return Ok(Evaluated::new(Value::Object(replacement)));
        }

        let (new_root, replaced) = clone_tree_node(&source, &target, &replacement, ctx)?;
        if !replaced {
            return Err(PureRuntimeError::EvaluationError(
                "replaceTreeNode: target node not found in source tree".into(),
            )
            .into());
        }
        Ok(Evaluated::new(Value::Object(new_root)))
    }
}

/// Recursive persistent-copy of `source` into a fresh `TreeNode`,
/// substituting any `childrenData` entry identity-equal to `target`
/// with `replacement`. Subtrees that don't contain `target` retain
/// their original handles — only the path-to-target gets fresh
/// clones. Returns the new (or reused) node + a `replaced` flag.
///
/// Identity preservation diverges from Java's interpreted runtime
/// (which always clones every level — see `ReplaceTreeNode.java:64`),
/// but matches Pure's persistent-data idiom: callers that retain
/// untouched subtrees see them as the same objects after the call.
/// The result is structurally equivalent to Java's output; identity-
/// asserting tests like `assertIs($child2, $newRoot.childrenData->at(1))`
/// now pass because `$child2` wasn't on the path-to-target.
fn clone_tree_node(
    source: &crate::heap::ObjectHandle,
    target: &crate::heap::ObjectHandle,
    replacement: &crate::heap::ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<(crate::heap::ObjectHandle, bool), PureException> {
    use std::rc::Rc;
    let property_names = ctx
        .heap()
        .property_names(source)
        .map_err(PureException::from)?;

    // Two-pass to enable subtree-reuse: first compute the new
    // childrenData list (with replacement / recursive clones), then
    // decide whether anything below this node actually changed. If
    // not, return the original handle and skip the allocation.
    let mut new_children: Option<Vec<Value>> = None;
    let mut replaced = false;
    for name in &property_names {
        if name.as_str() == "childrenData" {
            let src_values = ctx
                .heap()
                .get_property_values(source, name)
                .map_err(PureException::from)?;
            let mut built: Vec<Value> = Vec::with_capacity(src_values.len());
            for val in src_values.iter() {
                match val {
                    Value::Object(child_handle) if Rc::ptr_eq(child_handle, target) => {
                        built.push(Value::Object(replacement.clone()));
                        replaced = true;
                    }
                    Value::Object(child_handle) => {
                        let (new_child, child_replaced) =
                            clone_tree_node(child_handle, target, replacement, ctx)?;
                        if child_replaced {
                            replaced = true;
                        }
                        built.push(Value::Object(new_child));
                    }
                    // Non-Object entries in childrenData would be a
                    // type violation upstream — copy verbatim and let
                    // a separate diagnostic catch it.
                    other => built.push(other.clone()),
                }
            }
            new_children = Some(built);
        }
    }

    if !replaced {
        // No descendent contained `target` — return source as-is so
        // the caller sees the same handle for the unmodified subtree.
        return Ok((source.clone(), false));
    }

    let new_node = ctx.heap_mut().alloc_dynamic(crate::m3_paths::TREE_NODE);
    for name in &property_names {
        if name.as_str() == "childrenData" {
            if let Some(children) = &new_children
                && !children.is_empty()
            {
                ctx.heap()
                    .mutate_add(&new_node, name, children)
                    .map_err(PureException::from)?;
            }
        } else {
            let src_values = ctx
                .heap()
                .get_property_values(source, name)
                .map_err(PureException::from)?;
            let copied: Vec<Value> = src_values.iter().cloned().collect();
            if !copied.is_empty() {
                ctx.heap()
                    .mutate_add(&new_node, name, &copied)
                    .map_err(PureException::from)?;
            }
        }
    }
    Ok((new_node, true))
}

/// Register all collection native functions.
pub fn register(registry: &mut NativeRegistry) {
    // Non-lambda collection operations
    registry.register("size_Any_MANY__Integer_1_", Size);
    registry.register("isEmpty_Any_MANY__Boolean_1_", IsEmpty);
    registry.register("isNotEmpty_Any_MANY__Boolean_1_", IsNotEmpty);
    registry.register("at_T_MANY__Integer_1__T_1_", At);
    registry.register("first_T_MANY__T_$0_1$_", First);
    registry.register("last_T_MANY__T_$0_1$_", Last);
    registry.register("init_T_MANY__T_MANY_", Init);
    registry.register("tail_T_MANY__T_MANY_", Tail);
    registry.register("zip_T_MANY__U_MANY__Pair_MANY_", Zip);
    registry.register("values_Map_1__V_MANY_", Values);
    registry.register(
        "range_Integer_1__Integer_1__Integer_1__Integer_MANY_",
        Range,
    );
    registry.register("take_T_MANY__Integer_1__T_MANY_", Take);
    registry.register("drop_T_MANY__Integer_1__T_MANY_", Drop);
    registry.register("concatenate_T_MANY__T_MANY__T_MANY_", Concatenate);

    // Lambda-dependent collection operations
    registry.register("map_T_MANY__Function_1__V_MANY_", Map);
    registry.register("map_T_m__Function_1__V_m_", Map);
    registry.register("map_T_$0_1$__Function_1__V_$0_1$_", Map);
    registry.register("filter_T_MANY__Function_1__T_MANY_", Filter);
    registry.register("fold_T_MANY__Function_1__V_m__V_m_", Fold);
    registry.register("exists_T_MANY__Function_1__Boolean_1_", Exists);
    registry.register("forAll_T_MANY__Function_1__Boolean_1_", ForAll);
    registry.register("toOne_T_MANY__T_1_", ToOne);
    registry.register("toOne_T_$0_1$__T_1_", ToOne);
    registry.register("toOne_T_MANY__String_1__T_1_", ToOne);
    registry.register("toOneMany_T_MANY__T_$1_MANY$_", ToOneMany);
    registry.register("toOneMany_T_MANY__String_1__T_$1_MANY$_", ToOneMany);
    // toMultiplicity uses the generic mangled name with `z` preserved —
    // the second arg's static multiplicity (read off `args[1].type_info`)
    // drives the narrowing decision, not a specialised dispatch.
    registry.register("toMultiplicity_T_MANY__Any_z__T_z_", ToMultiplicity);
    registry.register("removeDuplicates_T_MANY__T_MANY_", RemoveDuplicates);
    registry.register(
        "removeDuplicates_T_MANY__Function_$0_1$__Function_$0_1$__T_MANY_",
        RemoveDuplicates,
    );

    // Additional non-lambda collection operations
    registry.register("contains_T_MANY__Any_1__Boolean_1_", Contains);
    registry.register("reverse_T_m__T_m_", Reverse);
    registry.register("indexOf_T_MANY__T_1__Integer_1_", IndexOf);
    registry.register("add_T_m__T_1__T_$1_MANY$_", Add);
    // Platform `add.pure` declares two overloads with `T[*]` (not the
    // earlier-registered `T[m]`): 2-arg `add(T[*], T[1])` and 3-arg
    // `add(T[*], Integer[1], T[1])`. The impl already dispatches on
    // arg-count for both shapes — register the platform keys verbatim
    // so exact-FQN dispatch finds the impl without falling back to
    // `find_by_prefix`.
    registry.register("add_T_MANY__T_1__T_$1_MANY$_", Add);
    registry.register("add_T_MANY__Integer_1__T_1__T_$1_MANY$_", Add);
    registry.register("slice_T_MANY__Integer_1__Integer_1__T_MANY_", Slice);
    registry.register("sort_T_m__Function_$0_1$__Function_$0_1$__T_m_", Sort);
    registry.register(
        "removeAllOptimized_T_MANY__T_MANY__T_MANY_",
        RemoveAllOptimized,
    );

    // Lambda-dependent additional operations
    registry.register("find_T_MANY__Function_1__T_$0_1$_", Find);

    // Map primitives
    registry.register("newMap_Pair_MANY__Map_1_", NewMap);
    // 2-arg overload: `newMap(Pair<U,V>[*], Property<U,Any|1>[*])`.
    // The Property arg is accepted for signature parity but not yet
    // honoured (per the `NewMap::execute` doc comment) — same impl.
    registry.register("newMap_Pair_MANY__Property_MANY__Map_1_", NewMap);

    registry.register(
        "replaceTreeNode_TreeNode_1__TreeNode_1__TreeNode_1__TreeNode_1_",
        ReplaceTreeNode,
    );
    registry.register("get_Map_1__U_1__V_$0_1$_", Get);
    registry.register("keys_Map_1__U_MANY_", Keys);
    registry.register("put_Map_1__U_1__V_1__Map_1_", Put);
    registry.register("keyValues_Map_1__Pair_MANY_", KeyValues);
    registry.register("putAll_Map_1__Map_1__Map_1_", PutAll);
    registry.register("putAll_Map_1__Pair_MANY__Map_1_", PutAll);
    registry.register("replaceAll_Map_1__Pair_MANY__Map_1_", ReplaceAll);
    registry.register(
        "getIfAbsentPutWithKey_Map_1__U_1__Function_1__V_$0_1$_",
        GetIfAbsentPutWithKey,
    );
    registry.register("groupBy_X_MANY__Function_1__Map_1_", GroupBy);
    registry.register("getMapStats_Map_1__MapStats_$0_1$_", GetMapStats);
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
    fn is_empty_true() {
        assert_eq!(
            IsEmpty
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn is_empty_false() {
        assert_eq!(
            IsEmpty
                .execute(&[lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
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
    fn last_of_collection() {
        assert_eq!(
            Last.execute(&[int_spec(&[10, 20, 30])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(30)
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
    fn concatenate_collections() {
        assert_eq!(
            Concatenate
                .execute(&[int_spec(&[1, 2]), int_spec(&[3, 4])], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[1, 2, 3, 4])
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

    #[test]
    fn is_not_empty_true_and_false() {
        assert_eq!(
            IsNotEmpty
                .execute(&[int_spec(&[1, 2])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
        assert_eq!(
            IsNotEmpty
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
        // Scalar counts as non-empty
        assert_eq!(
            IsNotEmpty
                .execute(&[lit_int(7)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn to_one_scalar_passthrough() {
        assert_eq!(
            ToOne
                .execute(&[lit_int(42)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(42)
        );
        assert_eq!(
            ToOne
                .execute(&[int_spec(&[42])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(42)
        );
    }

    #[test]
    fn to_one_empty_errors() {
        assert!(
            ToOne
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .is_err()
        );
        assert!(ToOne.execute(&[int_spec(&[])], &mut MockCtx).is_err());
    }

    #[test]
    fn to_one_multi_errors() {
        assert!(ToOne.execute(&[int_spec(&[1, 2])], &mut MockCtx).is_err());
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
    fn contains_finds_element() {
        assert_eq!(
            Contains
                .execute(&[int_spec(&[1, 2, 3]), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn contains_missing_element() {
        assert_eq!(
            Contains
                .execute(&[int_spec(&[1, 2, 3]), lit_int(99)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn contains_on_empty_is_false() {
        assert_eq!(
            Contains
                .execute(&[lit_collection(vec![]), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn contains_wrong_arg_count() {
        assert!(Contains.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
    }

    #[test]
    fn reverse_multi_element() {
        assert_eq!(
            Reverse
                .execute(&[int_spec(&[1, 2, 3])], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[3, 2, 1])
        );
    }

    #[test]
    fn reverse_single_element_is_noop() {
        assert_eq!(
            Reverse
                .execute(&[lit_int(7)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(7)
        );
    }

    #[test]
    fn reverse_empty_returns_unit() {
        assert_eq!(
            Reverse
                .execute(&[lit_collection(vec![])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn reverse_wrong_arg_count() {
        assert!(Reverse.execute(&[], &mut MockCtx).is_err());
    }

    #[test]
    fn index_of_found() {
        assert_eq!(
            IndexOf
                .execute(&[int_spec(&[10, 20, 30]), lit_int(20)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(1)
        );
    }

    #[test]
    fn index_of_not_found_returns_minus_one() {
        assert_eq!(
            IndexOf
                .execute(&[int_spec(&[10, 20, 30]), lit_int(99)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn index_of_empty_returns_minus_one() {
        assert_eq!(
            IndexOf
                .execute(&[lit_collection(vec![]), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn index_of_wrong_arg_count() {
        assert!(IndexOf.execute(&[lit_int(1)], &mut MockCtx).is_err());
    }

    // Happy-path `find` exercises the lambda-capable context; covered by
    // `eval_find_returns_first_match` in `tests/eval_tests.rs`.

    #[test]
    fn find_wrong_arg_count() {
        assert!(Find.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
    }

    #[test]
    fn add_appends_to_collection() {
        assert_eq!(
            Add.execute(&[int_spec(&[1, 2]), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn add_to_empty_yields_singleton() {
        assert_eq!(
            Add.execute(&[lit_collection(vec![]), lit_int(1)], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Integer(1)
        );
    }

    #[test]
    fn add_to_scalar_promotes_to_collection() {
        assert_eq!(
            Add.execute(&[lit_int(1), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[1, 2])
        );
    }

    #[test]
    fn add_wrong_arg_count() {
        assert!(Add.execute(&[lit_int(1)], &mut MockCtx).is_err());
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
    fn remove_all_optimized_removes_matching_elements() {
        assert_eq!(
            RemoveAllOptimized
                .execute(
                    &[int_spec(&[1, 2, 3, 2, 4]), int_spec(&[2, 4])],
                    &mut MockCtx,
                )
                .unwrap()
                .into_value(),
            int_collection(&[1, 3])
        );
    }

    #[test]
    fn remove_all_optimized_with_empty_removal_list_is_identity() {
        assert_eq!(
            RemoveAllOptimized
                .execute(
                    &[int_spec(&[1, 2, 3]), lit_collection(vec![])],
                    &mut MockCtx,
                )
                .unwrap()
                .into_value(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn remove_all_optimized_empty_source() {
        assert_eq!(
            RemoveAllOptimized
                .execute(&[lit_collection(vec![]), int_spec(&[1])], &mut MockCtx)
                .unwrap()
                .into_value(),
            Value::Unit
        );
    }

    #[test]
    fn remove_all_optimized_wrong_arg_count() {
        assert!(
            RemoveAllOptimized
                .execute(&[int_spec(&[1])], &mut MockCtx)
                .is_err()
        );
    }
}
