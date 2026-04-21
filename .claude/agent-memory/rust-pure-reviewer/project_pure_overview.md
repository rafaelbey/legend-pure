---
name: Key divergences between Java and Rust implementations
description: Where the Rust rewrite has deliberately chosen a different shape than the Java reference, and why — so reviewers don't mis-flag them as bugs
type: project
---

# Key divergences Java → Rust

These are **intentional** structural differences, documented in `legend-pure-rust/ARCHITECTURE.md`'s decision log. Do not flag them as regressions; do flag any PR that silently re-converges without justification.

## Storage model
- **Java**: Mutable `CoreInstance` graph in `ModelRepository`. Every node has bidirectional back-references (5 patterns that mutate during post-processing). Generic traversal via property-name strings.
- **Rust**: `Arena<ElementNode>` + `Arena<Element>` per `ModelChunk`, plus a global `Arena<Package>`. `ElementId::InstanceId { chunk_id: u16, local_idx: u32 }` is `Copy`. Unidirectional edges; **derived indexes** (specializations, association-injected properties) computed in a single O(N) post-freeze scan. Parallel-read-safe after freeze.

## Incremental merge / chunking
- **Java**: Re-parse file(s); `IncrementalCompiler` unloads/reloads pieces via `UnloadWalk` / `UnloadUnbind`.
- **Rust**: Segmented `ElementId` means adding a chunk is O(1) — push chunk, link packages, done. No rewrite of existing IDs.

## Generalization node
- **Java**: `Class.generalizations: Generalization[*]` — a node type wrapping each supertype reference.
- **Rust**: Eliminated. `Class.super_types: Vec<TypeExpr>` directly. Reverse direction (`specializations`) is a derived index.

## AST type parameter support
- **Rust AST preserves `Class X<T>{}`** — the Java ANTLR grammar currently rejects this. Forward-compatible with generics-in-classes work.

## Expression shape
- **Java**: `ValueSpecification` hierarchy (`FunctionExpression`, `InstanceValue`, `VariableExpression`, `SimpleFunctionExpression`) with a lot of subclass churn.
- **Rust AST**: keeps `Arithmetic { op, left, right }`, `Comparison { op, left, right }`, etc. — NOT desugared.
- **Rust `pure` Layer 5**: `ValueSpec { kind: ExprKind, source_info, type_info }` where operators ARE desugared to `ExprKind::FunctionCall("plus", ...)`. `let` → `FunctionCall("letFunction", ...)`, `new` → `FunctionCall("new", ...)`. `Group(...)` is eliminated.
- **Rust protocol JSON**: normalized form matching Legend Protocol v1 (`{"_type": "func", "function": "plus", "parameters": [...]}`).
- Three shapes, three layers. All translation between AST ↔ Protocol lives in the `protocol` crate; nowhere else.

## Primitive values
- **Java**: Every value boxed into `ListIterable` / `CoreInstance`. Scalar `42` is a 1-element list.
- **Rust**: Unboxed. `Value::Integer(i64)` is a scalar. Multiplicity `[1]` vs `[0..1]` vs `[*]` is resolved by `to_one()` / `to_zero_one()` / `to_collection()` coercion methods on `Value`. Deliberately side-steps the allocation tax Java pays per-scalar.

## Runtime collections
- **Java**: Eclipse Collections `MutableList` — mutable, cloning on accumulator patterns causes O(N²) in fold+put.
- **Rust**: `im_rc::Vector` (RRB) and `im_rc::HashMap` (HAMT). Persistent, structural-sharing; O(log N) put. Measured 25× faster fold+put on 1K items (2.67ms std HashMap vs 0.1ms HAMT equivalent).

## Object heap
- **Java**: Objects live in the `CoreInstance` graph itself — same graph as type definitions. No separation.
- **Rust**: `RuntimeHeap` uses `SlotMap<ObjectId, HeapEntry>` distinct from `PureModel`. `ObjectId` is generational (`slotmap`), giving safe identity-preserving `mutateAdd`. `HeapEntry::Dynamic | Typed` enables future hybrid compilation without rework.

## Call stack
- **Java**: Eager push/pop `StackFrame` on every function call — allocation per frame on happy path.
- **Rust**: Lazy. `Evaluator` has no call-stack field; `map_err` appends `StackFrame`s only on the unwind. Zero happy-path overhead.

## Variable context
- **Java**: Scope chain (stack of `VariableContext` with parent pointer). `get()` walks chain up to root.
- **Rust**: Single flat `HashMap<SmolStr, Value>` + `Vec<Vec<UndoEntry>>` undo log. `get()` = O(1) single lookup. `pop_scope()` = O(K) undo replay where K is set-count in the scope. Benchmarked against scope-stack and HAMT snapshots; flat+undo won.

## Purity analysis / memoization
- **Java**: Runtime `@Memoized` annotations on specific native functions; pure analysis not systematic.
- **Rust**: Purity-gated memoization via transitive `SideEffectFunction` compile-time analysis + boolean check at runtime. (Design documented in `docs/runtime/memoization.md`; implementation tracked.)

## Bootstrap
- **Java**: Parses `m3.pure` through the same front-end pipeline; recursive.
- **Rust**: `crates/pure/src/m3_parser.rs` is a specialised instance-graph serialization parser (1396 lines) that directly allocates into the bootstrap arena — no intermediate AST. BACKLOG P2 wants to move this to compile-time via `build.rs`.

## Why this memory exists

**Why:** Reviewers (especially those fresh off the Java side) often mis-flag these as "Rust is doing it wrong." They aren't — they're documented design choices. But the reverse is also important: a PR that *reverts* one of these without justification should be pushed back on.

## How to apply

- When reviewing a PR that looks like it's re-introducing a Java pattern (e.g. bidirectional refs, mutable scope chain, boxing primitives, merging Pass 2a/2b), ask: "What's the reason to reconverge here?"
- When a reviewer from the Java world claims something is wrong, check this list before agreeing.
- When the Rust team suggests adopting a Java pattern for convenience, check whether the divergence's original justification still applies.
