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

use crate::error::{PureException, PureRuntimeError};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::{Value, ValueKey};

// ---------------------------------------------------------------------------
// size
// ---------------------------------------------------------------------------

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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("isEmpty", &values, 1)?;
        Ok(Evaluated::new(Value::Boolean(values[0].is_empty())))
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
            PureRuntimeError::EvaluationError(format!(
                "at: index {idx} out of bounds for collection of size {}",
                coll.len()
            ))
            .into()
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("range", &values, 3)?;
        let start = values[0].as_integer()?;
        let end_exclusive = values[1].as_integer()?;
        let increment = values[2].as_integer()?;
        if increment == 0 {
            return Err(
                PureRuntimeError::EvaluationError("range: step cannot be zero".into()).into(),
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
            match ctx.call_function(&lambda_val, std::slice::from_ref(item))? {
                Value::Collection(inner) => {
                    for v in inner.iter() {
                        results.push(v.clone());
                    }
                }
                Value::Unit => {}
                other => results.push(other),
            }
        }

        Ok(Evaluated::new(Value::from_vec(results)))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        // Accept 1-arg and 3-arg variants. The 3-arg variant has optional
        // hash/equality lambdas that we currently ignore.
        if values.is_empty() || values.len() > 3 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "removeDuplicates: expected 1..=3 arguments, got {}",
                values.len()
            ))
            .into());
        }
        let source = values[0].to_collection();

        let mut seen: Vec<Value> = Vec::with_capacity(source.len());
        let mut out: Vec<Value> = Vec::with_capacity(source.len());
        for item in &source {
            if !seen.iter().any(|s| s == item) {
                seen.push(item.clone());
                out.push(item.clone());
            }
        }

        Ok(Evaluated::new(Value::from_vec(out)))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toOne", &values, 1)?;
        Ok(Evaluated::new(values[0].to_one()?.clone()))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("isNotEmpty", &values, 1)?;
        Ok(Evaluated::new(Value::Boolean(!values[0].is_empty())))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("contains", &values, 2)?;
        let haystack = values[0].to_collection();
        let needle = &values[1];
        Ok(Evaluated::new(Value::Boolean(
            haystack.iter().any(|v| v == needle),
        )))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("reverse", &values, 1)?;
        let source = values[0].to_collection();
        let reversed: Vec<Value> = source.iter().rev().cloned().collect();
        Ok(Evaluated::new(Value::from_vec(reversed)))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("indexOf", &values, 2)?;
        let haystack = values[0].to_collection();
        let needle = &values[1];
        let idx = haystack.iter().position(|v| v == needle);
        let result = match idx {
            #[allow(clippy::cast_possible_wrap)]
            Some(i) => Value::Integer(i as i64),
            None => Value::Integer(-1),
        };
        Ok(Evaluated::new(result))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("add", &values, 2)?;
        let mut coll = values[0].to_collection();
        coll.push_back(values[1].clone());
        let as_vec: Vec<Value> = coll.into_iter().collect();
        Ok(Evaluated::new(Value::from_vec(as_vec)))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("slice", &values, 3)?;
        let source = values[0].to_collection();
        let start = values[1].as_integer()?;
        let end = values[2].as_integer()?;

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
        // Enum values sort by the member's declaration order within the
        // enumeration (matching Java Pure's enum ordering). Values from
        // different enumerations are unordered — surface as an explicit
        // error rather than producing a misleading cross-enum ordering.
        (
            Value::EnumValue {
                enum_id: e1,
                member: m1,
                ..
            },
            Value::EnumValue {
                enum_id: e2,
                member: m2,
                ..
            },
        ) => {
            if e1 == e2 {
                Ok(m1.cmp(m2))
            } else {
                Err(PureRuntimeError::EvaluationError(format!(
                    "sort: cannot compare EnumValues from different enumerations"
                )))
            }
        }
        // Heap objects sort by allocation order (ObjectId). This is a
        // stable but semantically arbitrary ordering — meaningful only
        // when the caller cares about determinism, not Pure-level object
        // identity. Tests that need property-based ordering must pass a
        // key or comparator function.
        (Value::Object(a), Value::Object(b)) => {
            use std::cmp::Ordering as O;
            let av = slotmap::Key::data(a).as_ffi();
            let bv = slotmap::Key::data(b).as_ffi();
            Ok(if av == bv {
                O::Equal
            } else if av < bv {
                O::Less
            } else {
                O::Greater
            })
        }
        (Value::Element(a), Value::Element(b)) => Ok(format!("{a}").cmp(&format!("{b}"))),
        _ => Err(PureRuntimeError::EvaluationError(format!(
            "sort: cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// Pure `sort<T,U|m>(col:T[m], key:Function[0..1], comp:Function[0..1]):T[m]`.
///
/// Sorts the collection stably. When `key` is provided (non-empty), each
/// element is first mapped through `key` and the resulting values drive
/// ordering — values stay associated with their original element. When
/// `comp` is provided, it returns an `Integer` (negative/zero/positive)
/// to order two items; otherwise, primitive natural order via
/// [`cmp_values`] is used. Both `key` and `comp` may independently be
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
                out.push(ctx.call_function(k, &[it.clone()])?);
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
            indices.sort_by(|&i, &j| match cmp_values(&keys[i], &keys[j]) {
                Ok(o) => o,
                Err(e) => {
                    if cmp_err.is_none() {
                        cmp_err = Some(e);
                    }
                    std::cmp::Ordering::Equal
                }
            });
        }
        if let Some(e) = cmp_err {
            return Err(e.into());
        }

        let sorted: Vec<Value> = indices.into_iter().map(|i| items[i].clone()).collect();
        Ok(Evaluated::new(Value::from_vec(sorted)))
    }

    fn signature(&self) -> &'static str {
        "sort<T,U|m>(T[m], Function<{T[1]->U[1]}>[0..1], Function<{U[1],U[1]->Integer[1]}>[0..1]): T[m]"
    }
}

/// Extract a `&Value::Function` from a lambda-valued argument, or `None` if
/// the arg is `Unit`/empty. Any other non-Unit value is treated as "not a
/// lambda" and returns `None` — matches the semantics of the optional
/// `Function[0..1]` parameter.
fn lambda_or_none(v: &Value) -> Option<&Value> {
    match v {
        Value::Unit => None,
        Value::Function(_) => Some(v),
        _ => None,
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("removeAllOptimized", &values, 2)?;
        let source = values[0].to_collection();
        let to_remove = values[1].to_collection();

        let out: Vec<Value> = source
            .iter()
            .filter(|v| !to_remove.iter().any(|r| r == *v))
            .cloned()
            .collect();
        Ok(Evaluated::new(Value::from_vec(out)))
    }

    fn signature(&self) -> &'static str {
        "removeAllOptimized(T[*], T[*]): T[*]"
    }
}

// ---------------------------------------------------------------------------
// Map primitives: newMap / get / keys
// ---------------------------------------------------------------------------

/// Coerce a runtime [`Value`] into a hashable [`ValueKey`].
///
/// Only types listed in `ValueKey` can be used as map keys — primitives,
/// dates, and heap objects (keyed by identity). Collections, maps, and
/// functions never compare meaningfully and raise an explicit error rather
/// than silently misbehaving. Mirrors the Java runtime's "key types must be
/// `equality-safe`" constraint.
fn value_to_key(v: &Value) -> Result<ValueKey, PureException> {
    match v {
        Value::Boolean(b) => Ok(ValueKey::Boolean(*b)),
        Value::Integer(i) => Ok(ValueKey::Integer(*i)),
        Value::Decimal(d) => Ok(ValueKey::Decimal(*d)),
        Value::String(s) => Ok(ValueKey::String(s.clone())),
        Value::Date(d) => Ok(ValueKey::Date(*d)),
        Value::StrictTime(t) => Ok(ValueKey::StrictTime(*t)),
        Value::Object(id) => Ok(ValueKey::Object(*id)),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "Map key must be a hashable primitive or object identity, got {}",
            other.type_name()
        ))
        .into()),
    }
}

/// Inverse of [`value_to_key`] — recover the original [`Value`] shape from
/// a stored key so `keys()` can emit the collection.
fn key_to_value(k: &ValueKey) -> Value {
    match k {
        ValueKey::Boolean(b) => Value::Boolean(*b),
        ValueKey::Integer(i) => Value::Integer(*i),
        ValueKey::Decimal(d) => Value::Decimal(*d),
        ValueKey::String(s) => Value::String(s.clone()),
        ValueKey::Date(d) => Value::Date(*d),
        ValueKey::StrictTime(t) => Value::StrictTime(*t),
        ValueKey::Object(id) => Value::Object(*id),
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
        let mut map = im_rc::HashMap::new();
        for pair in pairs.iter() {
            let Value::Object(obj_id) = pair else {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "newMap: expected Pair<U,V>, got {}",
                    pair.type_name()
                ))
                .into());
            };
            let first_vals = ctx.heap().get_property_values(*obj_id, "first")?;
            let second_vals = ctx.heap().get_property_values(*obj_id, "second")?;
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
            let key = value_to_key(k)?;
            map.insert(key, v.clone());
        }
        Ok(Evaluated::new(Value::Map(Box::new(map))))
    }

    fn signature(&self) -> &'static str {
        "newMap<U,V>(pairs:Pair<U,V>[*]):Map<U,V>[1]"
    }
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
        let key = value_to_key(&values[1])?;
        match m.get(&key) {
            Some(v) => Ok(Evaluated::new(v.clone())),
            None => Ok(Evaluated::new(Value::Unit)),
        }
    }

    fn signature(&self) -> &'static str {
        "get<U,V>(m:Map<U,V>[1], key:U[1]):V[0..1]"
    }
}

/// Pure `put<U,V>(m:Map<U,V>[1], key:U[1], value:V[1]):Map<U,V>[1]`
///
/// Returns a new map with `key -> value` added (or replaced). The
/// original map is untouched — HAMT structural sharing keeps this O(log N).
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
        let key = value_to_key(&values[1])?;
        let mut updated = (**m).clone();
        updated.insert(key, values[2].clone());
        Ok(Evaluated::new(Value::Map(Box::new(updated))))
    }

    fn signature(&self) -> &'static str {
        "put<U,V>(m:Map<U,V>[1], key:U[1], value:V[1]):Map<U,V>[1]"
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
        let items: Vec<Value> = m.keys().map(key_to_value).collect();
        Ok(Evaluated::new(Value::from_vec(items)))
    }

    fn signature(&self) -> &'static str {
        "keys<U,V>(m:Map<U,V>[1]):U[*]"
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
    registry.register("sort_T_m__Function_$0_1$__Function_$0_1$__T_m_", Sort);
    registry.register(
        "removeAllOptimized_T_MANY__T_MANY__T_MANY_",
        RemoveAllOptimized,
    );

    // Lambda-dependent additional operations
    registry.register("find_T_MANY__Function_1__T_$0_1$_", Find);

    // Map primitives
    registry.register("newMap_Pair_MANY__Map_1_", NewMap);
    registry.register("get_Map_1__U_1__V_$0_1$_", Get);
    registry.register("keys_Map_1__U_MANY_", Keys);
    registry.register("put_Map_1__U_1__V_1__Map_1_", Put);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, force_all, lit_collection, lit_int, lit_str};

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
        assert!(
            Range
                .execute(&[lit_int(1), lit_int(5)], &mut MockCtx)
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

    // -----------------------------------------------------------------------
    // contains
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // reverse
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // indexOf
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // find (lambda-dependent)
    // -----------------------------------------------------------------------

    // Happy-path `find` exercises the lambda-capable context; covered by
    // `eval_find_returns_first_match` in `tests/eval_tests.rs`.

    #[test]
    fn find_wrong_arg_count() {
        assert!(Find.execute(&[int_spec(&[1])], &mut MockCtx).is_err());
    }

    // -----------------------------------------------------------------------
    // add (append)
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // slice
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // sort
    // -----------------------------------------------------------------------

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
    fn sort_mixed_types_errors() {
        let input = lit_collection(vec![lit_int(1), lit_str("oops")]);
        assert!(
            Sort.execute(
                &[input, lit_collection(vec![]), lit_collection(vec![])],
                &mut MockCtx,
            )
            .is_err()
        );
    }

    #[test]
    fn sort_wrong_arg_count() {
        assert!(Sort.execute(&[], &mut MockCtx).is_err());
    }

    // -----------------------------------------------------------------------
    // removeAllOptimized
    // -----------------------------------------------------------------------

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
