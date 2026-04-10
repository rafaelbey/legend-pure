# Multiplicity & Iteration Strategy

## Problem

Pure has a multiplicity system that is integral to the type system:

```
Integer[1]     -- exactly one value (scalar)
Integer[0..1]  -- optional value
Integer[*]     -- zero or more values (collection)
Integer[1..*]  -- one or more values
Integer[2..5]  -- between 2 and 5 values
```

Two design questions:

1. **How does the runtime represent values across multiplicities?** Does a
   scalar `Integer[1]` differ from a collection `Integer[*]` at the value level?

2. **Can we use lazy iteration for chained `map`/`filter`?** Java's Eclipse
   Collections has `LazyIterable`. Rust has zero-cost `Iterator` chains.

---

## How Java Does It

### Multiplicity at Runtime — Everything is a List

In the Java interpreter, **there is no scalar representation**. Every value
is wrapped in a `ValueSpecification` which holds a `values` property containing
a `ListIterable<CoreInstance>`:

```java
// To get a single value from a function result:
CoreInstance result = executeFunction(...);
CoreInstance value = result.getValueForMetaPropertyToOne(M3Properties.values);

// To get a collection:
ListIterable<? extends CoreInstance> values =
    result.getValueForMetaPropertyToMany(M3Properties.values);
```

- A scalar `Integer[1]` is a list of size 1
- An optional `Integer[0..1]` is a list of size 0 or 1
- A collection `Integer[*]` is a list of size N

The `wrapValueSpecification` factory creates these uniformly:

```java
// Single value → wraps in a 1-element list
ValueSpecificationBootstrap.wrapValueSpecification(instance, executable, processorSupport);

// Collection → wraps as-is
ValueSpecificationBootstrap.wrapValueSpecification(list, executable, processorSupport);

// Empty → wraps as empty list
ValueSpecificationBootstrap.wrapValueSpecification(Lists.immutable.empty(), executable, processorSupport);
```

### Map / Filter — Eager, Not Lazy

**Key finding: The Java interpreter does NOT use lazy iteration for `map`
and `filter`.** Both are fully eager:

```java
// Map.java — eager collection into MutableList
MutableList<CoreInstance> results = Lists.mutable.with();
collection.forEach(instance -> {
    CoreInstance subResult = functionExecution.executeFunction(...);
    results.addAllIterable(subResult.getValues());  // flatten results
});
return wrapValueSpecification(results, ...);
```

```java
// Filter.java — eager collection into MutableList
MutableList<CoreInstance> results = Lists.mutable.with();
for (CoreInstance instance : collection) {
    if (accept(predicate, instance, ...)) {
        results.add(instance);
    }
}
return wrapValueSpecification(results, ...);
```

The `asLazy()` call I found is only used for internal setup (extension
registration), not for Pure expression evaluation.

**Why no laziness?** The interpreter evaluates lambdas via `executeFunction()`,
which is a full recursive call. Java's `LazyIterable.collectLazy()` could
defer evaluation, but each element still requires a full function execution
with variable context setup — the overhead of lazy vs eager dispatch is
negligible compared to that per-element cost.

---

## Proposed Rust Design

### Multiplicity Representation: Uniform Collection Model

Like Java, we treat everything as a collection at the representation level.
But unlike Java's heavyweight `ValueSpecification` wrapper, we make this
**zero-cost for the scalar case** using our existing `Value` enum:

```
Pure Multiplicity    →   Rust Runtime Representation
─────────────────────────────────────────────────────
Integer[1]           →   Value::Integer(42)
Integer[0..1]        →   Value::Integer(42) | Value::Unit
Integer[*]           →   Value::Collection(PVector<Value>)
String[0..0]         →   Value::Unit
```

**Key insight: A scalar value IS a single value.** We don't wrap it in a
collection. `Value::Integer(42)` is both a valid `Integer[1]` and a valid
element inside a `Value::Collection`.

This means:

| Operation | Java | Rust |
|---|---|---|
| Pass `Integer[1]` to function | Wrap in 1-element list | Pass `Value::Integer` directly |
| Return `Integer[1]` from function | Unwrap from list | Return `Value::Integer` directly |
| `$x + 1` | Unwrap from ValueSpec, compute, wrap | Compute directly — no wrapping |
| `[1, 2, 3]->map(...)` | Iterate list, wrap each | Iterate PVector, no wrapping |

**No allocation for scalars. No unwrapping overhead.**

### Multiplicity Coercion

The evaluator needs a few conversion helpers:

```rust
impl Value {
    /// Coerce a value to a collection (for functions expecting [*]).
    /// A scalar becomes a 1-element PVector. A collection stays as-is.
    /// Unit becomes an empty PVector.
    pub fn to_collection(&self) -> PVector<Value> {
        match self {
            Value::Collection(v) => v.clone(),  // O(1) — persistent
            Value::Unit => PVector::new(),
            other => {
                let mut v = PVector::new();
                v.push_back(other.clone());
                v
            }
        }
    }

    /// Coerce a collection to a scalar (for functions expecting [1]).
    /// Errors if collection has != 1 element.
    pub fn to_one(&self) -> Result<&Value, PureRuntimeError> {
        match self {
            Value::Collection(v) if v.len() == 1 => Ok(&v[0]),
            Value::Collection(v) => Err(PureRuntimeError::MultiplicityViolation {
                expected: "[1]".into(),
                actual: v.len(),
            }),
            Value::Unit => Err(PureRuntimeError::MultiplicityViolation {
                expected: "[1]".into(),
                actual: 0,
            }),
            other => Ok(other),  // already scalar
        }
    }

    /// Coerce to optional — returns None for Unit, Some for scalar, error for [*].
    pub fn to_zero_one(&self) -> Result<Option<&Value>, PureRuntimeError> {
        match self {
            Value::Unit => Ok(None),
            Value::Collection(v) if v.is_empty() => Ok(None),
            Value::Collection(v) if v.len() == 1 => Ok(Some(&v[0])),
            Value::Collection(v) => Err(PureRuntimeError::MultiplicityViolation {
                expected: "[0..1]".into(),
                actual: v.len(),
            }),
            other => Ok(Some(other)),
        }
    }

    /// Whether this value is "empty" (Unit or empty collection).
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Unit | Value::Collection(v) if v.is_empty())
    }
}
```

### Map/Filter: Where to Apply Laziness

The Java interpreter is **fully eager** for `map` and `filter`. We have
three options:

#### Option 1: Eager (match Java) — RECOMMENDED FOR V1

```rust
fn native_map(
    &mut self,
    collection: &PVector<Value>,
    lambda: &Lambda,
    source: &SourceInfo,
) -> Result<Value, PureException> {
    let mut results = PVector::new();
    for item in collection.iter() {
        let result = self.apply_lambda(lambda, &[item.clone()], source)?;
        // map flattens: if lambda returns [*], extend results
        match result {
            Value::Collection(inner) => {
                for v in inner.iter() {
                    results.push_back(v.clone());
                }
            }
            Value::Unit => {} // skip empty
            v => results.push_back(v),
        }
        results.push_back(result);
    }
    Ok(Value::Collection(results))
}
```

**Pros**: Simple, correct, matches Java behavior exactly.
**Cons**: Creates intermediate PVector for each stage in chains.

#### Option 2: Rust Iterator Fusion — FUTURE OPTIMIZATION

Rust's iterator trait enables zero-cost chaining:

```rust
// collection->filter(x | $x > 0)->map(x | $x * 2)
//
// Instead of: filter → PVector → map → PVector
// We could:   filter+map → single PVector

collection.iter()
    .filter(|v| eval_predicate(v))    // no intermediate collection
    .map(|v| eval_transform(v))       // fused with filter
    .collect::<PVector<Value>>()      // single allocation
```

**But there's a catch**: Each `eval_predicate` and `eval_transform` call
involves the evaluator (mutable borrow) and variable context. Rust's borrow
checker makes it difficult to close over `&mut self` in iterator adaptors.

This optimization requires **recognizing the pattern at the IR level** —
when the evaluator sees `filter().map()`, it could fuse them into a single
loop. This is a compiler optimization, not an interpreter change.

#### Option 3: Lazy Pull-Based Evaluation — FUTURE RESEARCH

A more advanced approach where `map` and `filter` return lazy `Value::LazySeq`
that only materializes when consumed:

```rust
enum Value {
    // ... existing variants ...
    LazySeq(Box<dyn Iterator<Item = Value>>),  // deferred evaluation
}
```

This would fuse `collection->filter(...)->map(...)->first()` into evaluating
only until the first matching+mapped element. But it introduces lifetime
complexity and is incompatible with `forbid(unsafe_code)` in some designs.

### Recommendation

**Start with Option 1 (eager, match Java).** It's correct, simple, and
sufficient. The real performance gain comes from our native types
(`Decimal`, `PureDate`) and O(1) variable lookup — not from lazy iteration.

If profiling reveals that intermediate collection allocation is a bottleneck
(unlikely for most Pure workloads), we can add iterator fusion as a targeted
optimization later.

---

## Design Decision Summary

| Aspect | Java | Rust |
|---|---|---|
| **Scalar representation** | Wrapped in 1-element list | Direct `Value::Integer`, no wrapping |
| **Optional [0..1]** | 0 or 1-element list | `Value` or `Value::Unit` |
| **Collection [*]** | List of values | `Value::Collection(PVector)` |
| **Empty [0..0]** | Empty list | `Value::Unit` |
| **map implementation** | Eager `MutableList` | Eager `PVector` (V1) |
| **filter implementation** | Eager `MutableList` | Eager `PVector` (V1) |
| **Lazy iteration** | Not used in interpreter | Future optimization path |
| **Scalar pass overhead** | Wrap + unwrap per call | Zero — no wrapping needed |

### Key Advantage: No Boxing Tax

In Java, even passing a simple `Integer[1]` through the interpreter requires:
1. Create `ValueSpecification` instance
2. Set `values` property to a 1-element list
3. At the receiving site, call `getValueForMetaPropertyToOne`
4. Unwrap the `CoreInstance` to get the actual value

In Rust, `Value::Integer(42)` flows through the evaluator directly.
The evaluator matches on the `Value` enum — no wrapping, unwrapping,
or heap allocation. For tight loops, this eliminates millions of
intermediate allocations per second.

---

## Compiled Functions: Iterator Fusion Unlocked

The interpreter's limitation with lazy iteration is a **borrow checker
constraint**, not a fundamental one. In the interpreter, `eval_lambda`
requires `&mut self`, so two closures in an iterator chain can't both
borrow the evaluator:

```rust
// INTERPRETER — won't compile: two &mut self closures
collection.iter()
    .filter(|v| self.eval_lambda(&predicate, v))   // &mut self
    .map(|v| self.eval_lambda(&transform, v))      // &mut self — CONFLICT!
    .collect()
```

**Compiled functions don't have this problem.** When we generate Rust code
for a Pure function, the lambdas become actual Rust closures that capture
no mutable evaluator state — because Pure functions are **pure**.

### Example: filter→map chain

Pure source:

```pure
function process(items: Integer[*]): Integer[*]
{
    $items->filter(x | $x > 0)->map(x | $x * 2)
}
```

#### Interpreter (eager, two passes):

```rust
// Pass 1: filter → intermediate PVector
let filtered = native_filter(&mut self, &items, predicate_lambda)?;
// Pass 2: map → final PVector
let result = native_map(&mut self, &filtered, transform_lambda)?;
```

Two full iterations, one intermediate `PVector` allocation.

#### Compiled (Value-level, fused single pass):

```rust
fn process(items: &PVector<Value>) -> PVector<Value> {
    items.iter()
        .filter(|v| matches!(v, Value::Integer(i) if *i > 0))
        .map(|v| match v {
            Value::Integer(i) => Value::Integer(i * 2),
            _ => unreachable!(),
        })
        .collect()
}
```

One pass, zero intermediate collections. LLVM fuses the filter and map
into a single loop automatically.

#### Compiled (type-specialized, zero boxing):

```rust
// With type-specialized codegen — all Value boxing eliminated
fn process_specialized(items: &[i64]) -> Vec<i64> {
    items.iter()
        .copied()
        .filter(|&x| x > 0)
        .map(|x| x * 2)
        .collect()
}
```

Zero boxing, native `i64` arithmetic, SIMD-eligible by LLVM. This is
orders of magnitude faster than even Java's compiled path.

### Why This Works: Purity = Borrow Safety

Pure's guarantees map perfectly to Rust's ownership model:

| Pure Property | Rust Benefit |
|---|---|
| No side effects | Closures capture nothing mutable → iterator fusion works |
| Immutable values | `&` references everywhere → no borrow conflicts |
| No shared mutable state | No `&mut self` needed in compiled closures |
| Referential transparency | LLVM can freely reorder, inline, vectorize |

The borrow checker only fights us in the interpreter because there's a
single mutable `Evaluator` struct. Compiled Pure functions are standalone —
the borrow checker becomes an ally that enforces purity at the Rust level.

### Three-Tier Performance Comparison

For `$items->filter(x | $x > 0)->map(x | $x * 2)` with 100K elements:

| Tier | Passes | Intermediate allocs | Per-element cost | Boxing |
|---|---|---|---|---|
| **Interpreted** | 2 | 1 PVector | ~50ns (eval dispatch) | `Value` enum |
| **Compiled (Value)** | 1 (fused) | 0 | ~5ns (match + branch) | `Value` enum |
| **Compiled (Specialized)** | 1 (fused) | 0 | ~0.5ns (SIMD-eligible) | None — raw `i64` |

The compiled path with type specialization is ~100x faster than
interpretation for collection-heavy workloads. This is the payoff
of the hybrid architecture — hot paths get compiled, cold paths
stay interpreted for flexibility.

### What Enables This

1. **Pure's type system** — The compiler knows `Integer[*]` at compile time,
   enabling type-specialized codegen without runtime checks.

2. **Pure's purity** — No side effects means closures are pure → iterator
   chains compose freely → LLVM can optimize aggressively.

3. **Rust's iterator trait** — Zero-cost abstraction. `filter().map().collect()`
   compiles to the same machine code as a hand-written `for` loop.

4. **The hybrid evaluator** — Functions start interpreted (flexibility),
   then hot paths are compiled (performance) based on profiling or
   annotations. Both tiers share the same `Value` type, so they interop
   seamlessly.
