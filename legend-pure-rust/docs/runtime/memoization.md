# Memoization Strategy

## Overview

Pure functions without side effects that are called repeatedly with the same arguments
can be memoized — cache the result and return it on subsequent calls. This avoids
redundant computation for patterns common in Legend Engine (e.g., `elementToPath` called
~50K times with ~200 unique inputs).

## Purity Analysis

> **Critical rule:** A function is only memoizable if it AND all functions in its
> transitive call graph are free of `SideEffectFunction`-stereotyped functions.

Functions like `now()`, `today()`, `guid()` are non-deterministic. Any function that
transitively calls them is impure and must NOT be memoized.

```rust
pub enum Purity {
    Pure,     // Safe to memoize
    Impure,   // Calls a SideEffectFunction (directly or transitively)
    Unknown,  // Not yet analyzed
}
```

### Analysis Pass

Purity is **precomputed once** during model compilation — not checked at every call:

```rust
pub struct CompiledFunction {
    id: SmolStr,
    body: Vec<Expression>,
    purity: Purity,         // precomputed
    memoizable: bool,       // precomputed from purity + heuristics
}
```

The analysis runs a single pass over the call graph at load time:

```
Model load → build call graph → propagate SideEffectFunction transitively
           → mark purity on each function → apply heuristics → set `memoizable` flag
```

At runtime, checking memoizability is a single `bool` read (~1ns).

## Heuristics

Not all pure functions benefit from memoization. The cache lookup itself has a cost:

```
Cache check cost:
  Hash the key:         ~15ns (parameterless) to ~50ns (with args)
  HashMap lookup:       ~20ns
  Total per call:       ~35-70ns
```

Functions that execute in <50ns (trivial getters, arithmetic) should NOT be memoized —
the cache check costs more than re-evaluation.

```rust
fn should_memoize(func: &FunctionDef) -> bool {
    if func.purity != Purity::Pure { return false; }
    if func.body_expression_count() <= 2 { return false; }    // trivial
    if func.has_collection_params() { return false; }          // expensive to hash
    true
}
```

## Cache Strategies

### Keep-All (Recommended for Per-Query Executors)

```rust
memo: HashMap<MemoKey, Value>
```

- Lookup: ~20ns
- No eviction overhead
- Memory bounded by request lifetime (~2MB typical)
- Cache dropped when executor is dropped

### LRU (For Long-Lived Sessions: IDE, REPL)

```rust
memo: LruCache<MemoKey, Value>
```

- Lookup: ~50ns (extra bookkeeping)
- Bounded memory (configurable capacity)
- Keeps hot entries, evicts cold

### Configuration

```rust
pub enum MemoStrategy {
    Off,
    KeepAll { cache: HashMap<MemoKey, Value> },
    Bounded { cache: LruCache<MemoKey, Value>, capacity: usize },
}
```

## Cache Key

```rust
#[derive(Hash, Eq, PartialEq)]
pub struct MemoKey {
    function_id: SmolStr,
    args: Vec<ValueHash>,
}

#[derive(Hash, Eq, PartialEq)]
pub enum ValueHash {
    Integer(i64),
    Float(u64),          // f64.to_bits()
    Boolean(bool),
    String(SmolStr),
    Object(ObjectId),    // identity-based
    Unit,
}
```

## Expected Impact

```
Scenario: elementToPath called 50,000 times with ~200 unique inputs.

Without memo:  50,000 × 5μs = 250ms
With memo:     200 × 5μs + 49,800 × 35ns = 2.7ms

→ ~90x speedup for repetitive pure function calls
```
