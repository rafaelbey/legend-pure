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

use crate::error::PureRuntimeError;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
use crate::value::Value;

// ---------------------------------------------------------------------------
// size
// ---------------------------------------------------------------------------

/// Pure `size(Any[*]): Integer[1]` — number of elements in a collection.
#[derive(Debug)]
pub struct Size;

impl NativeFunction for Size {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("size", args, 1)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_possible_wrap)]
        Ok(Value::Integer(coll.len() as i64))
    }

    fn signature(&self) -> &'static str {
        "size(Any[*]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// isEmpty
// ---------------------------------------------------------------------------

/// Pure `isEmpty(Any[*]): Boolean[1]`
#[derive(Debug)]
pub struct IsEmpty;

impl NativeFunction for IsEmpty {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("isEmpty", args, 1)?;
        Ok(Value::Boolean(args[0].is_empty()))
    }

    fn signature(&self) -> &'static str {
        "isEmpty(Any[*]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// at
// ---------------------------------------------------------------------------

/// Pure `at(Any[*], Integer[1]): Any[1]` — element at index.
///
/// Pure uses 0-based indexing.
#[derive(Debug)]
pub struct At;

impl NativeFunction for At {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("at", args, 2)?;
        let coll = args[0].to_collection();
        let idx = args[1].as_integer()?;
        if idx < 0 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "at: index cannot be negative, got {idx}"
            )));
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let idx = idx as usize;
        coll.get(idx).cloned().ok_or_else(|| {
            PureRuntimeError::EvaluationError(format!(
                "at: index {idx} out of bounds for collection of size {}",
                coll.len()
            ))
        })
    }

    fn signature(&self) -> &'static str {
        "at(Any[*], Integer[1]): Any[1]"
    }
}

// ---------------------------------------------------------------------------
// first / last
// ---------------------------------------------------------------------------

/// Pure `first(Any[*]): Any[0..1]` — first element or Unit.
#[derive(Debug)]
pub struct First;

impl NativeFunction for First {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("first", args, 1)?;
        match &args[0] {
            Value::Collection(v) => Ok(v.front().cloned().unwrap_or(Value::Unit)),
            Value::Unit => Ok(Value::Unit),
            other => Ok(other.clone()), // scalar is its own first
        }
    }

    fn signature(&self) -> &'static str {
        "first(Any[*]): Any[0..1]"
    }
}

/// Pure `last(Any[*]): Any[0..1]` — last element or Unit.
#[derive(Debug)]
pub struct Last;

impl NativeFunction for Last {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("last", args, 1)?;
        match &args[0] {
            Value::Collection(v) => Ok(v.back().cloned().unwrap_or(Value::Unit)),
            Value::Unit => Ok(Value::Unit),
            other => Ok(other.clone()),
        }
    }

    fn signature(&self) -> &'static str {
        "last(Any[*]): Any[0..1]"
    }
}

// ---------------------------------------------------------------------------
// range
// ---------------------------------------------------------------------------

/// Pure `range(Integer[1], Integer[1], Integer[1]): Integer[*]`
///
/// `range(start, stop, step)` — generates integers from start (inclusive)
/// to stop (exclusive) with the given step.
#[derive(Debug)]
pub struct Range;

impl NativeFunction for Range {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("range", args, 3)?;
        let start = args[0].as_integer()?;
        let end_exclusive = args[1].as_integer()?;
        let increment = args[2].as_integer()?;
        if increment == 0 {
            return Err(PureRuntimeError::EvaluationError(
                "range: step cannot be zero".into(),
            ));
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
        Ok(Value::Collection(Box::new(result)))
    }

    fn signature(&self) -> &'static str {
        "range(Integer[1], Integer[1], Integer[1]): Integer[*]"
    }
}

// ---------------------------------------------------------------------------
// take / drop
// ---------------------------------------------------------------------------

/// Pure `take(Any[*], Integer[1]): Any[*]` — first N elements.
#[derive(Debug)]
pub struct Take;

impl NativeFunction for Take {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("take", args, 2)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = args[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Value::Collection(Box::new(coll.take(n))))
    }

    fn signature(&self) -> &'static str {
        "take(Any[*], Integer[1]): Any[*]"
    }
}

/// Pure `drop(Any[*], Integer[1]): Any[*]` — all elements after first N.
#[derive(Debug)]
pub struct Drop;

impl NativeFunction for Drop {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("drop", args, 2)?;
        let coll = args[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = args[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Value::Collection(Box::new(coll.skip(n))))
    }

    fn signature(&self) -> &'static str {
        "drop(Any[*], Integer[1]): Any[*]"
    }
}

// ---------------------------------------------------------------------------
// concatenate
// ---------------------------------------------------------------------------

/// Pure `concatenate(Any[*], Any[*]): Any[*]` — concatenate two collections.
#[derive(Debug)]
pub struct Concatenate;

impl NativeFunction for Concatenate {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("concatenate", args, 2)?;
        let mut a = args[0].to_collection();
        let b = args[1].to_collection();
        a.append(b);
        Ok(Value::Collection(Box::new(a)))
    }

    fn signature(&self) -> &'static str {
        "concatenate(Any[*], Any[*]): Any[*]"
    }
}

// ---------------------------------------------------------------------------
// map (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `map(T[*], Function<{T[1]->V[1]}>[1]): V[*]`
///
/// Applies a lambda to each element of a collection.
#[derive(Debug)]
pub struct Map;

impl NativeFunction for Map {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("map", args, 2)?;
        let source = args[0].to_collection();
        let lambda = &args[1];

        // Pure collections are flat — a lambda returning `V[*]` or `V[0..1]`
        // contributes zero, one, or many scalar elements to the result. We
        // flatten Collection/Unit outputs here so downstream `.property`
        // access sees scalars, not nested collections.
        let mut results: Vec<Value> = Vec::with_capacity(source.len());
        for item in &source {
            match ctx.eval_lambda(lambda, std::slice::from_ref(item))? {
                Value::Collection(inner) => {
                    for v in inner.iter() {
                        results.push(v.clone());
                    }
                }
                Value::Unit => {}
                other => results.push(other),
            }
        }

        Ok(Value::from_vec(results))
    }

    fn signature(&self) -> &'static str {
        "map(T[*], Function<{T[1]->V[1]}>[1]): V[*]"
    }
}

// ---------------------------------------------------------------------------
// filter (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `filter(T[*], Function<{T[1]->Boolean[1]}>[1]): T[*]`
///
/// Filters a collection using a boolean predicate.
#[derive(Debug)]
pub struct Filter;

impl NativeFunction for Filter {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("filter", args, 2)?;
        let source = args[0].to_collection();
        let lambda = &args[1];

        let mut results = Vec::with_capacity(source.len());
        for item in &source {
            let predicate_result = ctx.eval_lambda(lambda, std::slice::from_ref(item))?;
            if predicate_result.as_boolean()? {
                results.push(item.clone());
            }
        }

        Ok(Value::from_vec(results))
    }

    fn signature(&self) -> &'static str {
        "filter(T[*], Function<{T[1]->Boolean[1]}>[1]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// fold (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `fold(T[*], Function<{T[1],V[m]->V[m]}>[1], V[m]): V[m]`
///
/// Accumulates a collection into a single value using a binary function.
#[derive(Debug)]
pub struct Fold;

impl NativeFunction for Fold {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("fold", args, 3)?;
        let source = args[0].to_collection();
        let lambda = &args[1];
        let mut accumulator = args[2].clone();

        for item in &source {
            accumulator = ctx.eval_lambda(lambda, &[item.clone(), accumulator])?;
        }

        Ok(accumulator)
    }

    fn signature(&self) -> &'static str {
        "fold(T[*], Function<{T[1],V[m]->V[m]}>[1], V[m]): V[m]"
    }
}

// ---------------------------------------------------------------------------
// exists (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `exists<T>(value:T[*], func:Function<{T[1]->Boolean[1]}>[1]):Boolean[1]`
///
/// Returns `true` as soon as any element satisfies the predicate. For an
/// empty input returns `false`.
#[derive(Debug)]
pub struct Exists;

impl NativeFunction for Exists {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("exists", args, 2)?;
        let source = args[0].to_collection();
        let lambda = &args[1];

        for item in &source {
            let predicate_result = ctx.eval_lambda(lambda, std::slice::from_ref(item))?;
            if predicate_result.as_boolean()? {
                return Ok(Value::Boolean(true));
            }
        }

        Ok(Value::Boolean(false))
    }

    fn signature(&self) -> &'static str {
        "exists(T[*], Function<{T[1]->Boolean[1]}>[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// forAll (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `forAll<T>(value:T[*], func:Function<{T[1]->Boolean[1]}>[1]):Boolean[1]`
///
/// Returns `true` iff every element satisfies the predicate. For an empty
/// input vacuously returns `true`.
#[derive(Debug)]
pub struct ForAll;

impl NativeFunction for ForAll {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("forAll", args, 2)?;
        let source = args[0].to_collection();
        let lambda = &args[1];

        for item in &source {
            let predicate_result = ctx.eval_lambda(lambda, std::slice::from_ref(item))?;
            if !predicate_result.as_boolean()? {
                return Ok(Value::Boolean(false));
            }
        }

        Ok(Value::Boolean(true))
    }

    fn signature(&self) -> &'static str {
        "forAll(T[*], Function<{T[1]->Boolean[1]}>[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// removeDuplicates (non-lambda variant)
// ---------------------------------------------------------------------------

/// Pure `removeDuplicates<T>(value:T[*]):T[*]`
///
/// Returns a new collection with duplicate elements removed, preserving
/// the first occurrence order. Uses `PartialEq` on [`Value`] for comparison.
///
/// Also accepts two optional `Function[0..1]` hasher/equator arguments in the
/// richer signature (`removeDuplicates_T_MANY__Function_$0_1$__Function_$0_1$__T_MANY_`);
/// those arguments are ignored here — we fall back to value equality, matching
/// the default Pure semantics when no custom comparator is supplied.
#[derive(Debug)]
pub struct RemoveDuplicates;

impl NativeFunction for RemoveDuplicates {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        // Accept 1-arg and 3-arg variants. The 3-arg variant has optional
        // hash/equality lambdas that we currently ignore.
        if args.is_empty() || args.len() > 3 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "removeDuplicates: expected 1..=3 arguments, got {}",
                args.len()
            )));
        }
        let source = args[0].to_collection();

        let mut seen: Vec<Value> = Vec::with_capacity(source.len());
        let mut out: Vec<Value> = Vec::with_capacity(source.len());
        for item in &source {
            if !seen.iter().any(|s| s == item) {
                seen.push(item.clone());
                out.push(item.clone());
            }
        }

        Ok(Value::from_vec(out))
    }

    fn signature(&self) -> &'static str {
        "removeDuplicates(T[*]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// toOne
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toOne", args, 1)?;
        Ok(args[0].to_one()?.clone())
    }

    fn signature(&self) -> &'static str {
        "toOne(T[*]): T[1]"
    }
}

// ---------------------------------------------------------------------------
// isNotEmpty
// ---------------------------------------------------------------------------

/// Pure `isNotEmpty(Any[*]):Boolean[1]`
///
/// Counterpart of `isEmpty` — returns `true` when the collection has at least
/// one element. Also `true` for any scalar value.
#[derive(Debug)]
pub struct IsNotEmpty;

impl NativeFunction for IsNotEmpty {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("isNotEmpty", args, 1)?;
        Ok(Value::Boolean(!args[0].is_empty()))
    }

    fn signature(&self) -> &'static str {
        "isNotEmpty(Any[*]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// contains (collection)
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("contains", args, 2)?;
        let haystack = args[0].to_collection();
        let needle = &args[1];
        Ok(Value::Boolean(haystack.iter().any(|v| v == needle)))
    }

    fn signature(&self) -> &'static str {
        "contains(T[*], Any[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// reverse
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("reverse", args, 1)?;
        let source = args[0].to_collection();
        let reversed: Vec<Value> = source.iter().rev().cloned().collect();
        Ok(Value::from_vec(reversed))
    }

    fn signature(&self) -> &'static str {
        "reverse(T[m]): T[m]"
    }
}

// ---------------------------------------------------------------------------
// indexOf
// ---------------------------------------------------------------------------

/// Pure `indexOf<T>(T[*], T[1]): Integer[1]`
///
/// Returns the 0-based index of the first element equal to the needle,
/// or `-1` when the needle is not found. Uses [`Value`] equality.
#[derive(Debug)]
pub struct IndexOf;

impl NativeFunction for IndexOf {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("indexOf", args, 2)?;
        let haystack = args[0].to_collection();
        let needle = &args[1];
        let idx = haystack.iter().position(|v| v == needle);
        match idx {
            #[allow(clippy::cast_possible_wrap)]
            Some(i) => Ok(Value::Integer(i as i64)),
            None => Ok(Value::Integer(-1)),
        }
    }

    fn signature(&self) -> &'static str {
        "indexOf(T[*], T[1]): Integer[1]"
    }
}

// ---------------------------------------------------------------------------
// find (lambda-dependent)
// ---------------------------------------------------------------------------

/// Pure `find<T>(T[*], Function<{T[1]->Boolean[1]}>[1]): T[0..1]`
///
/// Returns the first element for which the predicate returns `true`, or
/// [`Value::Unit`] if no element satisfies the predicate.
#[derive(Debug)]
pub struct Find;

impl NativeFunction for Find {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("find", args, 2)?;
        let source = args[0].to_collection();
        let lambda = &args[1];

        for item in &source {
            let predicate_result = ctx.eval_lambda(lambda, std::slice::from_ref(item))?;
            if predicate_result.as_boolean()? {
                return Ok(item.clone());
            }
        }

        Ok(Value::Unit)
    }

    fn signature(&self) -> &'static str {
        "find(T[*], Function<{T[1]->Boolean[1]}>[1]): T[0..1]"
    }
}

// ---------------------------------------------------------------------------
// add (append)
// ---------------------------------------------------------------------------

/// Pure `add<T|m>(T[m], T[1]): T[$1_MANY$]`
///
/// Appends a single element to the end of a collection, producing a new
/// collection. Scalar inputs are promoted; empty inputs yield a singleton.
#[derive(Debug)]
pub struct Add;

impl NativeFunction for Add {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("add", args, 2)?;
        let mut coll = args[0].to_collection();
        coll.push_back(args[1].clone());
        let as_vec: Vec<Value> = coll.into_iter().collect();
        Ok(Value::from_vec(as_vec))
    }

    fn signature(&self) -> &'static str {
        "add(T[m], T[1]): T[$1_MANY$]"
    }
}

// ---------------------------------------------------------------------------
// slice
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("slice", args, 3)?;
        let source = args[0].to_collection();
        let start = args[1].as_integer()?;
        let end = args[2].as_integer()?;

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let start = start.max(0) as usize;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let end = end.max(0) as usize;
        let start = start.min(source.len());
        let end = end.min(source.len());

        if start >= end {
            return Ok(Value::Unit);
        }

        // PVector::slice takes Range; we iterate to keep it simple & avoid
        // accidentally moving the clone out.
        let out: Vec<Value> = source
            .iter()
            .skip(start)
            .take(end - start)
            .cloned()
            .collect();
        Ok(Value::from_vec(out))
    }

    fn signature(&self) -> &'static str {
        "slice(T[*], Integer[1], Integer[1]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// sort
// ---------------------------------------------------------------------------

/// Default comparator used by [`Sort`] when no user-supplied comparator
/// is provided. Compares values of the same primitive type using their
/// natural ordering. Cross-type comparisons produce an evaluation error.
fn cmp_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering, PureRuntimeError> {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => Ok(x.partial_cmp(y).unwrap_or(Ordering::Equal)),
        (Value::Decimal(x), Value::Decimal(y)) => Ok(x.cmp(y)),
        (Value::String(x), Value::String(y)) => Ok(x.cmp(y)),
        (Value::Boolean(x), Value::Boolean(y)) => Ok(x.cmp(y)),
        (Value::Date(x), Value::Date(y)) => Ok(x.cmp(y)),
        _ => Err(PureRuntimeError::EvaluationError(format!(
            "sort: cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// Pure `sort<T>(T[*]): T[*]` — default (natural) comparator.
///
/// Sorts the collection stably using [`cmp_values`] as the comparator. Only
/// homogeneous collections of comparable primitive types are supported;
/// mixed-type inputs produce an evaluation error.
#[derive(Debug)]
pub struct Sort;

impl NativeFunction for Sort {
    fn execute(
        &self,
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("sort", args, 1)?;
        let source = args[0].to_collection();
        let mut items: Vec<Value> = source.iter().cloned().collect();

        // Stable sort, bubbling up the first comparison error via a captured
        // Result slot. `sort_by` cannot short-circuit on its own.
        let mut cmp_err: Option<PureRuntimeError> = None;
        items.sort_by(|a, b| match cmp_values(a, b) {
            Ok(o) => o,
            Err(e) => {
                if cmp_err.is_none() {
                    cmp_err = Some(e);
                }
                std::cmp::Ordering::Equal
            }
        });
        if let Some(e) = cmp_err {
            return Err(e);
        }

        Ok(Value::from_vec(items))
    }

    fn signature(&self) -> &'static str {
        "sort(T[*]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// removeAllOptimized
// ---------------------------------------------------------------------------

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
        args: &[Value],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("removeAllOptimized", args, 2)?;
        let source = args[0].to_collection();
        let to_remove = args[1].to_collection();

        let out: Vec<Value> = source
            .iter()
            .filter(|v| !to_remove.iter().any(|r| r == *v))
            .cloned()
            .collect();
        Ok(Value::from_vec(out))
    }

    fn signature(&self) -> &'static str {
        "removeAllOptimized(T[*], T[*]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all collection native functions.
pub fn register(registry: &mut NativeRegistry) {
    // Non-lambda collection operations
    registry.register("size_Any_MANY__Integer_1_", Size);
    registry.register("isEmpty_Any_MANY__Boolean_1_", IsEmpty);
    registry.register("isNotEmpty_Any_MANY__Boolean_1_", IsNotEmpty);
    registry.register("at_T_MANY__Integer_1__T_1_", At);
    registry.register("first_T_MANY__T_$0_1$_", First);
    registry.register("last_T_MANY__T_$0_1$_", Last);
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
    registry.register("slice_T_MANY__Integer_1__Integer_1__T_MANY_", Slice);
    registry.register("sort_T_MANY__T_MANY_", Sort);
    registry.register(
        "removeAllOptimized_T_MANY__T_MANY__T_MANY_",
        RemoveAllOptimized,
    );

    // Lambda-dependent additional operations
    registry.register("find_T_MANY__Function_1__T_$0_1$_", Find);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NoOpEvalCtx;

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
            Size.execute(&[int_collection(&[1, 2, 3])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(3)
        );
    }

    #[test]
    fn size_of_scalar() {
        assert_eq!(
            Size.execute(&[Value::Integer(42)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn size_of_unit() {
        assert_eq!(
            Size.execute(&[Value::Unit], &mut NoOpEvalCtx).unwrap(),
            Value::Integer(0)
        );
    }

    #[test]
    fn is_empty_true() {
        assert_eq!(
            IsEmpty.execute(&[Value::Unit], &mut NoOpEvalCtx).unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn is_empty_false() {
        assert_eq!(
            IsEmpty
                .execute(&[Value::Integer(1)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn at_valid_index() {
        assert_eq!(
            At.execute(
                &[int_collection(&[10, 20, 30]), Value::Integer(1)],
                &mut NoOpEvalCtx
            )
            .unwrap(),
            Value::Integer(20)
        );
    }

    #[test]
    fn at_out_of_bounds() {
        assert!(
            At.execute(
                &[int_collection(&[10, 20]), Value::Integer(5)],
                &mut NoOpEvalCtx
            )
            .is_err()
        );
    }

    #[test]
    fn first_of_collection() {
        assert_eq!(
            First
                .execute(&[int_collection(&[10, 20, 30])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(10)
        );
    }

    #[test]
    fn first_of_empty() {
        assert_eq!(
            First.execute(&[Value::Unit], &mut NoOpEvalCtx).unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn last_of_collection() {
        assert_eq!(
            Last.execute(&[int_collection(&[10, 20, 30])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(30)
        );
    }

    #[test]
    fn range_ascending() {
        let r = Range
            .execute(
                &[Value::Integer(0), Value::Integer(5), Value::Integer(1)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, int_collection(&[0, 1, 2, 3, 4]));
    }

    #[test]
    fn range_with_step() {
        let r = Range
            .execute(
                &[Value::Integer(0), Value::Integer(10), Value::Integer(3)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, int_collection(&[0, 3, 6, 9]));
    }

    #[test]
    fn range_descending() {
        let r = Range
            .execute(
                &[Value::Integer(5), Value::Integer(0), Value::Integer(-1)],
                &mut NoOpEvalCtx,
            )
            .unwrap();
        assert_eq!(r, int_collection(&[5, 4, 3, 2, 1]));
    }

    #[test]
    fn take_elements() {
        assert_eq!(
            Take.execute(
                &[int_collection(&[1, 2, 3, 4, 5]), Value::Integer(3)],
                &mut NoOpEvalCtx
            )
            .unwrap(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn drop_elements() {
        assert_eq!(
            Drop.execute(
                &[int_collection(&[1, 2, 3, 4, 5]), Value::Integer(2)],
                &mut NoOpEvalCtx
            )
            .unwrap(),
            int_collection(&[3, 4, 5])
        );
    }

    #[test]
    fn concatenate_collections() {
        assert_eq!(
            Concatenate
                .execute(
                    &[int_collection(&[1, 2]), int_collection(&[3, 4])],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            int_collection(&[1, 2, 3, 4])
        );
    }

    #[test]
    fn type_mismatch_errors() {
        assert!(
            At.execute(
                &[int_collection(&[1, 2]), Value::String("1".into())],
                &mut NoOpEvalCtx
            )
            .is_err()
        );
        assert!(
            Range
                .execute(
                    &[
                        Value::String("1".into()),
                        Value::Integer(5),
                        Value::Integer(1)
                    ],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
        assert!(
            Take.execute(
                &[int_collection(&[1, 2]), Value::String("1".into())],
                &mut NoOpEvalCtx
            )
            .is_err()
        );
        assert!(
            Drop.execute(
                &[int_collection(&[1, 2]), Value::String("1".into())],
                &mut NoOpEvalCtx
            )
            .is_err()
        );
    }

    #[test]
    fn wrong_arg_count_errors() {
        assert!(Size.execute(&[], &mut NoOpEvalCtx).is_err());
        assert!(
            At.execute(&[int_collection(&[1])], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(
            Range
                .execute(&[Value::Integer(1), Value::Integer(5)], &mut NoOpEvalCtx)
                .is_err()
        );
        assert!(
            Take.execute(&[int_collection(&[1])], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn is_not_empty_true_and_false() {
        assert_eq!(
            IsNotEmpty
                .execute(&[int_collection(&[1, 2])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            IsNotEmpty
                .execute(&[Value::Unit], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
        // Scalar counts as non-empty
        assert_eq!(
            IsNotEmpty
                .execute(&[Value::Integer(7)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn to_one_scalar_passthrough() {
        assert_eq!(
            ToOne
                .execute(&[Value::Integer(42)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(42)
        );
        assert_eq!(
            ToOne
                .execute(&[int_collection(&[42])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(42)
        );
    }

    #[test]
    fn to_one_empty_errors() {
        assert!(ToOne.execute(&[Value::Unit], &mut NoOpEvalCtx).is_err());
        assert!(
            ToOne
                .execute(&[int_collection(&[])], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn to_one_multi_errors() {
        assert!(
            ToOne
                .execute(&[int_collection(&[1, 2])], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    #[test]
    fn remove_duplicates_preserves_first_occurrence() {
        let input = int_collection(&[1, 2, 1, 3, 2, 4]);
        let result = RemoveDuplicates
            .execute(&[input], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(result, int_collection(&[1, 2, 3, 4]));
    }

    #[test]
    fn remove_duplicates_on_empty() {
        assert_eq!(
            RemoveDuplicates
                .execute(&[Value::Unit], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn remove_duplicates_ignores_trailing_hasher_args() {
        // The 3-arg variant passes hash + equality lambdas that we currently ignore.
        let input = int_collection(&[1, 1, 2]);
        let result = RemoveDuplicates
            .execute(&[input, Value::Unit, Value::Unit], &mut NoOpEvalCtx)
            .unwrap();
        assert_eq!(result, int_collection(&[1, 2]));
    }

    // -----------------------------------------------------------------------
    // contains
    // -----------------------------------------------------------------------

    #[test]
    fn contains_finds_element() {
        assert_eq!(
            Contains
                .execute(
                    &[int_collection(&[1, 2, 3]), Value::Integer(2)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn contains_missing_element() {
        assert_eq!(
            Contains
                .execute(
                    &[int_collection(&[1, 2, 3]), Value::Integer(99)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn contains_on_empty_is_false() {
        assert_eq!(
            Contains
                .execute(&[Value::Unit, Value::Integer(1)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Boolean(false)
        );
    }

    #[test]
    fn contains_wrong_arg_count() {
        assert!(
            Contains
                .execute(&[int_collection(&[1])], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // reverse
    // -----------------------------------------------------------------------

    #[test]
    fn reverse_multi_element() {
        assert_eq!(
            Reverse
                .execute(&[int_collection(&[1, 2, 3])], &mut NoOpEvalCtx)
                .unwrap(),
            int_collection(&[3, 2, 1])
        );
    }

    #[test]
    fn reverse_single_element_is_noop() {
        assert_eq!(
            Reverse
                .execute(&[Value::Integer(7)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(7)
        );
    }

    #[test]
    fn reverse_empty_returns_unit() {
        assert_eq!(
            Reverse.execute(&[Value::Unit], &mut NoOpEvalCtx).unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn reverse_wrong_arg_count() {
        assert!(Reverse.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    // -----------------------------------------------------------------------
    // indexOf
    // -----------------------------------------------------------------------

    #[test]
    fn index_of_found() {
        assert_eq!(
            IndexOf
                .execute(
                    &[int_collection(&[10, 20, 30]), Value::Integer(20)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn index_of_not_found_returns_minus_one() {
        assert_eq!(
            IndexOf
                .execute(
                    &[int_collection(&[10, 20, 30]), Value::Integer(99)],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn index_of_empty_returns_minus_one() {
        assert_eq!(
            IndexOf
                .execute(&[Value::Unit, Value::Integer(1)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(-1)
        );
    }

    #[test]
    fn index_of_wrong_arg_count() {
        assert!(
            IndexOf
                .execute(&[Value::Integer(1)], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // find (lambda-dependent)
    // -----------------------------------------------------------------------

    // Happy-path `find` exercises the lambda-capable context; covered by
    // `eval_find_returns_first_match` in `tests/eval_tests.rs`.

    #[test]
    fn find_wrong_arg_count() {
        assert!(
            Find.execute(&[int_collection(&[1])], &mut NoOpEvalCtx)
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // add (append)
    // -----------------------------------------------------------------------

    #[test]
    fn add_appends_to_collection() {
        assert_eq!(
            Add.execute(
                &[int_collection(&[1, 2]), Value::Integer(3)],
                &mut NoOpEvalCtx
            )
            .unwrap(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn add_to_empty_yields_singleton() {
        assert_eq!(
            Add.execute(&[Value::Unit, Value::Integer(1)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(1)
        );
    }

    #[test]
    fn add_to_scalar_promotes_to_collection() {
        assert_eq!(
            Add.execute(&[Value::Integer(1), Value::Integer(2)], &mut NoOpEvalCtx)
                .unwrap(),
            int_collection(&[1, 2])
        );
    }

    #[test]
    fn add_wrong_arg_count() {
        assert!(Add.execute(&[Value::Integer(1)], &mut NoOpEvalCtx).is_err());
    }

    // -----------------------------------------------------------------------
    // slice
    // -----------------------------------------------------------------------

    #[test]
    fn slice_basic_range() {
        assert_eq!(
            Slice
                .execute(
                    &[
                        int_collection(&[10, 20, 30, 40, 50]),
                        Value::Integer(1),
                        Value::Integer(4)
                    ],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            int_collection(&[20, 30, 40])
        );
    }

    #[test]
    fn slice_empty_when_start_equals_end() {
        assert_eq!(
            Slice
                .execute(
                    &[
                        int_collection(&[1, 2, 3]),
                        Value::Integer(2),
                        Value::Integer(2)
                    ],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn slice_clamps_out_of_bounds() {
        assert_eq!(
            Slice
                .execute(
                    &[
                        int_collection(&[1, 2, 3]),
                        Value::Integer(1),
                        Value::Integer(99)
                    ],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            int_collection(&[2, 3])
        );
    }

    #[test]
    fn slice_type_mismatch_errors() {
        assert!(
            Slice
                .execute(
                    &[
                        int_collection(&[1, 2, 3]),
                        Value::String("1".into()),
                        Value::Integer(2)
                    ],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    #[test]
    fn slice_wrong_arg_count() {
        assert!(
            Slice
                .execute(
                    &[int_collection(&[1, 2, 3]), Value::Integer(0)],
                    &mut NoOpEvalCtx
                )
                .is_err()
        );
    }

    // -----------------------------------------------------------------------
    // sort
    // -----------------------------------------------------------------------

    #[test]
    fn sort_integers_ascending() {
        assert_eq!(
            Sort.execute(&[int_collection(&[3, 1, 2])], &mut NoOpEvalCtx)
                .unwrap(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn sort_single_element_is_noop() {
        assert_eq!(
            Sort.execute(&[Value::Integer(5)], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Integer(5)
        );
    }

    #[test]
    fn sort_empty_returns_unit() {
        assert_eq!(
            Sort.execute(&[Value::Unit], &mut NoOpEvalCtx).unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn sort_mixed_types_errors() {
        let mut v = PVector::new();
        v.push_back(Value::Integer(1));
        v.push_back(Value::String("oops".into()));
        let input = Value::Collection(Box::new(v));
        assert!(Sort.execute(&[input], &mut NoOpEvalCtx).is_err());
    }

    #[test]
    fn sort_wrong_arg_count() {
        assert!(Sort.execute(&[], &mut NoOpEvalCtx).is_err());
    }

    // -----------------------------------------------------------------------
    // removeAllOptimized
    // -----------------------------------------------------------------------

    #[test]
    fn remove_all_optimized_removes_matching_elements() {
        assert_eq!(
            RemoveAllOptimized
                .execute(
                    &[int_collection(&[1, 2, 3, 2, 4]), int_collection(&[2, 4])],
                    &mut NoOpEvalCtx
                )
                .unwrap(),
            int_collection(&[1, 3])
        );
    }

    #[test]
    fn remove_all_optimized_with_empty_removal_list_is_identity() {
        assert_eq!(
            RemoveAllOptimized
                .execute(&[int_collection(&[1, 2, 3]), Value::Unit], &mut NoOpEvalCtx)
                .unwrap(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn remove_all_optimized_empty_source() {
        assert_eq!(
            RemoveAllOptimized
                .execute(&[Value::Unit, int_collection(&[1])], &mut NoOpEvalCtx)
                .unwrap(),
            Value::Unit
        );
    }

    #[test]
    fn remove_all_optimized_wrong_arg_count() {
        assert!(
            RemoveAllOptimized
                .execute(&[int_collection(&[1])], &mut NoOpEvalCtx)
                .is_err()
        );
    }
}
