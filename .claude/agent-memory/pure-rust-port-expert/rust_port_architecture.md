---
name: Rust port architecture
description: Crate layering, key data model choices, and the load-bearing designs of the Rust rewrite
type: project
---

**Crate layers** (dep rule: lower never depends on higher):
```
cli, jni             Layer 7 — dev tools + Java FFI
runtime              Layer 6 — interpreter + heap
pure                 Layer 5 — semantic layer (PureModel)
protocol             Layer 3 — AST ↔ Legend Protocol v1 JSON
parser, compose      Layer 2 — recursive-descent parser / AST→text
lexer                Layer 1
ast, ast-derive      Layer 0 — data model + proc-macros
core-platform-pure   embeds platform *.pure at build time
stress               criterion benchmarks
```

Dep graph authority: `legend-pure-rust/ARCHITECTURE.md`.

**AST keeps no serde** — protocol/JSON conversion isolated in `protocol` crate. AST preserves `Arithmetic { op, left, right }` as a distinct variant; JSON normalizes to `{"_type": "func", "function": "plus", ...}`.

**Key data model choices (`crates/pure/DESIGN.md`):**
- Arena/index pattern: `ElementId { chunk_id: u16, local_idx: u32 }` — no `&` refs between nodes; chunked so model merging is O(1).
- Unidirectional canonical data + derived indexes (rebuilt via `rebuild_derived_indexes()`). Eliminates 5 Java bidirectional-mutation patterns (`_specializationsAdd`, `_propertiesFromAssociationsAdd`, etc.).
- No `Generalization` node: `Vec<ElementId>` for supertypes + inverted `specialization_index`.
- `TypeExpr` is the Rust equivalent of Java `GenericType` — variants `Named{element, type_arguments, value_arguments}`, `FunctionType`, `Relation`, `Generic(name)`, `AlgebraUnion`.
- `Element` enum (`Class | Enumeration | Function | Profile | Association | Measure | PrimitiveType | Unit`) replaces Java `CoreInstance` inheritance.
- Hard vs. soft dependencies: supertypes sorted topologically (cycles = error); property/param types can cycle.
- `SmolStr` everywhere for names; `#[forbid(unsafe_code)]` except `jni`.
- `SourceInfo` on every Pure node — non-negotiable for diagnostics + runtime errors.
- Freeze-then-query lifecycle — `PureModelBuilder` mutates, then model is immutable and read-parallel.

**Pipeline passes** (`crates/pure/src/pipeline.rs`):
1. Declaration — assign `ElementId`s, build package tree, allocate Unit shells. Functions use mangled names at declaration time; overloads allowed.
2. Topological sort (Kahn on supertypes). Cycles → `CyclicInheritance`.
3. **Pass 2a — Signatures**: hydrate everything except function bodies. After 2a all function signatures are visible to type-based dispatch.
4. **Pass 2b — Bodies**: compile expression bodies; seed `variable_types` from parameters before lowering.
5. Pass 2.5 — bottom-up type inference over bodies (`crates/pure/src/infer.rs`).
6. Freeze (`rebuild_derived_indexes`).
7. Validation (`crates/pure/src/validate.rs`).

**Resolution entry points** (never confuse):
- `resolve_element_ptr` — exact mangled name; for type refs, annotations, property access.
- `resolve_function_call` (`crates/pure/src/resolve.rs:565`) — simple name + overload dispatch; only for `FunctionApplication` and `ArrowFunction`. Filters by param count, then narrows by type + multiplicity scoring. See `crates/pure/FUNCTION_DISPATCH.md`.

**Runtime four-layer storage** (`crates/runtime/ARCHITECTURE.md`):
1. `Arc<PureModel>` — shared immutable model (`Send + Sync`).
2. `RuntimeHeap` — per-executor `SlotMap<ObjectId, HeapEntry>` where entries are `Dynamic(HashMap)` or `Typed(dyn TypedObject)`; generational IDs preserve identity across `mutateAdd`.
3. `VariableContext` — flat `HashMap` + undo-log for O(1) `get`/`set`, O(scope-size) `pop_scope`.
4. Extensions: `CompiledFunction`, `TypedObject`, `RuntimeEnv` traits.

**Value** (`Value` enum, `crates/runtime/src/value.rs`): unboxed primitives (`Integer(i64)`, `Float(f64)`, `Boolean`, `Decimal` = `rust_decimal`, `String` = `SmolStr`), `Date` = `PureDate` (jiff), `StrictTime` = `jiff::civil::Time`, `Object(ObjectId)`, `Collection(Box<PVector<Value>>)` = `im_rc`, `Map(Box<im_rc::HashMap>)`, `Lambda`, `Unit`. Scalar ≠ 1-element collection, unlike Java which boxes everything.

**Executor NOT `Send`** because `im_rc` uses `Rc`. Pattern: shared `Arc<PureModel>` across threads, per-thread executor.

Benchmark baselines (criterion, release): Integer match 1.2ns; PVector clone 1000 elems 6.1ns (structural share); dynamic property access 13.5ns; `mutateAdd` 29.4ns; HAMT put 10K items 1.05ms vs std HashMap clone-per-put 2.67ms (25× slower).
