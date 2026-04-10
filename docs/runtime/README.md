# Legend Pure Runtime Design Documents

This directory contains architectural and design deep-dives for the `legend-pure-runtime` crate. The runtime executes compiled `PureModel`s using a high-performance interpreter, zero-overhead call stacks, and persistent data structures.

## Core Architecture

Ensure you read the high-level overview in [`crates/runtime/ARCHITECTURE.md`](../../crates/runtime/ARCHITECTURE.md) before diving into these detailed documents.

### Memory & State Management

- **[Persistent Data Structures](persistent_data_structures.md)** — Explains the use of HAMT (Persistent Hash Maps) and RRB trees (Persistent Vectors) to power the runtime's collections with `O(log N)` structural sharing, eliminating copying.
- **[Context Strategy & Benchmarks](context_strategy_benchmarks.md)** — Details the `VariableContext` flat-undo log design, turning variable lookups from an `O(D)` walk up a scope chain into an `O(1)` HashMap lookup.
- **[MutateAdd Mechanics](mutateadd_mechanics.md)** — Strategies for `mutateAdd` and deep-copy semantics in the pure object heap.

### Execution & Performance

- **[Hybrid Compilation](hybrid_compilation.md)** — How the runtime seamlessly blends interpreted expressions with AOT-compiled Rust functions for maximum performance.
- **[Performance Comparison](performance_comparison.md)** — Baseline benchmarks evaluating interpretation overhead between the legacy Java engine, compiled Rust, and interpreted Rust.
- **[Benchmarking Strategy](benchmarking_strategy.md)** — Our suite for continuously measuring interpreted throughput at the structural, algorithmic, and heap levels.
- **[Memoization](memoization.md)** — Implementing function purity checks and cache strategies to avoid redundant evaluation.

### Language Semantics

- **[Multiplicity & Iteration](multiplicity_and_iteration.md)** — How Pure's `[1]`, `[0..1]`, and `[*]` multiplicities are modeled in Rust without Java's boxing tax. This also explains the compiled iterator fusion advantages.
- **[Error Location & Diagnostics](error_location_design.md)** — The "lazy call stack" architecture: zero overhead on the happy path, full AST `SourceInfo` stack generation upon failure via `map_err`.
- **[Metaprogramming Analysis](metaprogramming_analysis.md)** — Implementing `meta::pure::router::routing::metaAccessor` and `deactivate`/`reactivate` dynamically at runtime.
- **[Convergence Analysis](convergence_analysis.md)** — Steps matching the legacy Pure IDE AST and the active Engine syntax parser capabilities.
- **[Architecture Deep Questions](architecture_deep_questions.md)** — Bootstrap problems, Thread-Safety rules, Polyglot dispatch contexts, Java FFI, and parser boundaries.
