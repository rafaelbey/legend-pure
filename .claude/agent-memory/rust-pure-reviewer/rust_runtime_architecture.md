---
name: Rust runtime architecture
description: Four-layer storage, evaluator loop, native dispatch, thread safety, and known benchmark baselines
type: project
---

## Four-layer storage (see `crates/runtime/ARCHITECTURE.md`)

1. **`Arc<PureModel>`** — immutable, compiled model. `Send + Sync`, shared across threads.
2. **`RuntimeHeap`** — `SlotMap<ObjectId, HeapEntry>`. Per-executor, **not `Send`** because `im-rc` uses `Rc`. `HeapEntry = Dynamic(RuntimeObject) | Typed(Box<dyn TypedObject>)`; both produce `Value::Object(ObjectId)` so callers don't discriminate.
3. **`VariableContext`** — flat `HashMap<SmolStr, Value>` + `Vec<Vec<UndoEntry>>` undo log. `get()` = single HashMap lookup regardless of scope depth. Benchmarked against scope-stack and HAMT snapshots; flat+undo won.
4. **Extension points** — `TypedObject`, `CompiledFunction`, `RuntimeEnv` traits for hybrid compilation and polyglot dispatch.

## Value design (`crates/runtime/src/value.rs`)

- `Value::{Boolean, Integer, Float, Decimal, String, Date, StrictTime, Object, Collection, Map, Lambda, Unit}`
- Primitives unboxed (contrast: Java boxes everything as List).
- `Decimal` = `rust_decimal::Decimal` (128-bit, Copy, ~5ns arithmetic).
- `Date` / `StrictTime` = `jiff`-backed; Copy.
- `Collection` = `Box<im_rc::Vector<Value>>` (RRB tree; O(log N) structural sharing). `Map` = `Box<im_rc::HashMap<ValueKey, Value>>` (HAMT).
- `ValueKey` deliberately narrower than `Value` — only hashable variants (no Float, no Collection, no Lambda).
- `PartialEq` uses `f64::to_bits()` for Float equality — note this diverges from NaN-compare semantics (NaN == NaN under `to_bits`).

## Evaluator (`crates/runtime/src/eval.rs`)

- **Lazy call stack**: no call-stack field. Happy path is zero-overhead `Result::Ok` return. On error, each `map_err` in the recursive `eval()` chain appends a `StackFrame` to the `PureException`.
- **Dispatch order in `eval_function_call`**: exact FQN native → prefix-based simple-name native fallback → user function by ElementId → `FunctionNotFound`. The prefix fallback (`find_by_prefix`) matters because the compiler often lowers operators with simple names before FQN resolution.
- **`defer_execution`** on `NativeFunction` controls short-circuiting (`if`, lazy lambdas). Deferred natives get `Value::Lambda` wrappers of unevaluated expressions.
- `call_user_function` clones `func.parameters`, `func.body`, and `node.source_info` before pushing scope — known cost, tolerated because of borrow-checker pressure.

## Thread safety contract

- `PureModel` is `Send + Sync` (`Arc`-shared).
- `RuntimeHeap` + `Evaluator` are **not** `Send` (because of `im-rc`).
- Intended pattern: one model, one executor per thread. Do not suggest making the runtime `Send` without switching `im-rc` to `im` (the `Arc`-backed variant).

## Benchmark baselines (release mode, criterion)

| Op | Time |
|---|---|
| Integer create + match | 1.2 ns |
| Float create + match | 1.8 ns |
| SmolStr clone (short) | 4.0 ns |
| PVector clone (1000 elems) | 6.1 ns |
| Property access (dynamic) | 13.5 ns |
| Object allocation | 13.5 ns |
| mutateAdd | 29.4 ns |
| HAMT put (10K items) | 1.05 ms |
| std HashMap clone-per-put (1K) | 2.67 ms (25× slower than HAMT) |

## Why: reviewer needs this

- Any PR that attempts to make the runtime `Send` needs to acknowledge the `im-rc` vs `im` tradeoff.
- Any PR that moves the call-stack from lazy to eager must be benchmarked — zero-overhead happy path is explicit design intent.
- Native function prefix fallback is *load-bearing* for current dispatch — removing it breaks operators.

## How to apply

- Ask for criterion results on any PR touching hot paths (`eval`, `dispatch_native`, `get_property`, `mutate_add`).
- Push back on unnecessary `Send` bounds; the team has made a deliberate per-thread-executor choice.
