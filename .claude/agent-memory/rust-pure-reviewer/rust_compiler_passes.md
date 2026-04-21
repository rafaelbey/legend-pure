---
name: Rust compiler passes
description: The multi-pass pipeline in crates/pure/src/pipeline.rs — what each pass assumes and what it produces
type: project
---

## Pipeline, in order (see `crates/pure/src/pipeline.rs`)

1. **Bootstrap** — `bootstrap::create_bootstrap_chunk()` seeds Chunk 0 (Any, Nil, String, Integer, Float, Decimal, Boolean, Byte, Number, Date, StrictDate, DateTime, StrictTime). Currently built at runtime; BACKLOG P2 wants it compile-time.
2. **Pass 1 — declare**: Assign `ElementId`s, allocate `Element` shells with placeholder `Any` types, build package tree. Functions are stored under their **mangled name** (`f.mangled_name()`). Duplicates for non-functions produce `DuplicateElement`; function overloads are allowed.
3. **Pass 1.5 — topo sort**: Only **hard dependencies** (class supertypes) go into the DAG. Kahn's algorithm; cyclic inheritance is a compilation error. Soft deps (property types, param types) are NOT in the DAG, which is what allows `Person ↔ Company` cycles.
4. **Pass 2a — define signatures**: Hydrate everything *except* function bodies. All function parameters and return types get resolved, so `PureModel` has complete signatures.
5. **Pass 2b — define bodies**: Compile `Expression` bodies. Function parameters are seeded into `ResolutionContext.variable_types` before body lowering so dispatch can use them.
6. **Pass 2.5 — infer**: Bottom-up type inference over `ValueSpec` trees. Clones each function/QP body, infers in place, writes back. (Clone-and-restore is the borrow-checker workaround, not a perf choice.)
7. **Freeze**: `rebuild_derived_indexes()` computes specializations + association property reverse indexes.
8. **Pass 3 — validate**: Read-only checks.

## Non-obvious invariants

- **Pass 2a/2b split is load-bearing.** Merging them breaks type-based overload dispatch (2b consumes signatures written by 2a).
- **`ElementNode.name` for functions = mangled name**, not simple name. `Function.function_name` holds the simple name. `resolve_element_ptr` matches the mangled name (exact identity); `resolve_function_call` matches `function_name` and dispatches by arg count + type.
- **Chunk 0 is bootstrap**; Chunk N for N>0 is user code. `ElementId::InstanceId { chunk_id, local_idx }` is segmented so adding a chunk cannot invalidate existing IDs.
- `Pass 2.5` only runs on "the last chunk" — bootstrap bodies are assumed empty. Verify this assumption holds if platform sources ever get compiled into chunks >0 alongside user code.
- **Unit elements are populated in Pass 1** (`allocate_unit_shells`) and `pass_define_signatures` explicitly skips them — there's a SAFETY INVARIANT comment about AST coordinates pointing to the parent Measure.

## Why: reviewer needs this

- A PR that changes pass ordering or merges passes is breaking dispatch correctness, not a refactor.
- A PR that changes what counts as "hard" vs "soft" dependency changes the set of cyclic data models that compile.
- Pass 2.5's clone-and-restore pattern is a known cost — flag it in perf reviews but understand why it exists before suggesting a rewrite.

## How to apply

- When reviewing changes in `pipeline.rs`, trace the mutation sequence — who writes what, in what pass, and who reads it later.
- When reviewing `resolve.rs`, distinguish which resolver entry point is being changed (element_ptr vs function_call).
