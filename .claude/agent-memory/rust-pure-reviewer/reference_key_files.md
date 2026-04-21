---
name: Reference — where things live
description: Fast lookup: the handful of files a reviewer ends up opening on every review. Absolute paths.
type: reference
---

# Reference — key files and what they answer

## Canonical docs (read first)

- `/Users/cocobey73/Projects/legend-pure/CLAUDE.md` — repo shape, Java stack build commands, M4→M3→M2→M1.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/CLAUDE.md` — Rust workspace guide, build commands, code conventions.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/ARCHITECTURE.md` — decision log, crate graph, derives.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/ARCHITECTURE.md` — runtime four-layer model, benchmark baselines.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/FUNCTION_DISPATCH.md` — dispatch algorithm + current error counts.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/BACKLOG.md` — P0–P3 tracked gaps; check before flagging missing features.
- `/Users/cocobey73/Projects/legend-pure/docs/reference/pure-language-reference.md` — the Pure language itself.
- `/Users/cocobey73/Projects/legend-pure/docs/architecture/compiler-pipeline.md` — Java pipeline (parse → first-pass → PostProcessor matcher loop → Validator → serialize → codegen).

## Java authoritative sources

- `/Users/cocobey73/Projects/legend-pure/legend-pure-core/legend-pure-m3-core/src/main/java/org/finos/legend/pure/m3/compiler/postprocessing/PostProcessor.java` — 185-line orchestrator; sub-pass ordering is the spec.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-core/legend-pure-m3-core/src/main/resources/platform/` — the canonical `.pure` platform library both stacks consume.

## Rust compiler hot spots

- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/pipeline.rs` — Pass 1 / 1.5 / 2a / 2b / 2.5 orchestrator; `compile()` entry point.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/model.rs` — `PureModel`, `ModelChunk`, `Element`, `DerivedIndexes`, package-tree resolution methods.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/resolve.rs` — `resolve_element_ptr` / `resolve_function_call`, `ImportScope`, type-ref resolution.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/lower.rs` — AST `Expression` → semantic `ValueSpec`; desugars operators, let, new, arrow.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/infer.rs` — bottom-up type inference pass 2.5.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/types.rs` — `TypeExpr`, `Multiplicity`, `Parameter`, `ValueSpec`, `ExprKind`.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/ids.rs` — `ElementId` (segmented), `PackageId`, `RelationId`.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/bootstrap.rs` — Chunk 0; well-known `ANY_ID`, `STRING_ID`, `INTEGER_ID`, etc.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/m3_parser.rs` — reads `m3.pure` instance-graph serialization directly into bootstrap arena.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/pure/src/validate.rs` — Pass 3 read-only checks on frozen model.

## Rust runtime hot spots

- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/eval.rs` — `Evaluator`, `eval()`, `eval_function_call()`, `dispatch_native()`, `call_user_function()`.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/value.rs` — `Value` enum; multiplicity coercions `to_one` / `to_zero_one` / `to_collection`.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/heap.rs` — `RuntimeHeap`, `HeapEntry::Dynamic|Typed`, `ObjectId`, `mutate_add`.
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/context.rs` — `VariableContext` (flat-map + undo-log).
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/native/` — `arithmetic`, `boolean`, `collection`, `comparison`, `lang`, `string`, `testing` (7 files, 41 impls).
- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/runtime/src/error.rs` — `PureRuntimeError` (internal) vs `PureException` (user-facing with `StackFrame`).

## Runtime design deep-dives

- `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/docs/runtime/` — `hybrid_compilation`, `memoization`, `mutateadd_mechanics`, `persistent_data_structures`, `performance_comparison`, `benchmarking_strategy`, `convergence_analysis`. **`convergence_analysis.md` is stale as of 2026-04-17 — claims expression lowering is missing but it exists.**
