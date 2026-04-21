---
name: Review-approach guidelines for this codebase
description: How reviews should be calibrated for a team new to Rust working on a parity-driven rewrite
type: feedback
---

# Review-approach guidelines

**Lead with semantic parity, then Rust idioms, then micro-perf.** The team's biggest risk is behavior drift from the Java semantic oracle. Ambiguous overload resolution, missing native functions, and silent divergence in how multiplicity gets collapsed in edge cases are higher-impact findings than an unnecessary `.clone()` in a cold path. Flag allocations and hot-path issues, but don't bury them under the parity work that the dispatch error-count (`crates/pure/BACKLOG.md`) says is still open.

**Why:** The team is new to Rust and the dispatch/parity work is the critical path. Pedagogical reviews that over-index on Rust idioms will distract from the actual correctness frontier. Conversely, letting perf regressions slip because "it's about parity right now" will bake in structural problems that are expensive to fix once the semantic gap closes.

## How to apply

- When reviewing a PR, sort findings by: **(1)** correctness / semantic parity; **(2)** structural invariants from `project_rust_architecture_invariants` memory; **(3)** Rust idioms; **(4)** micro-perf.
- For Rust-idiom feedback, **explain the mechanism** (allocation, cache line, branch) and link the Rust Performance Book / Nomicon, not just "this is not idiomatic". The team learns from mechanism, not labels.
- Cross-check claims against the source before repeating doc promises. Several Rust docs (`convergence_analysis.md`, `IMPLEMENTATION_PLAN.md`) have bit-rotted.
- Treat `crates/pure/BACKLOG.md` as authoritative for what's tracked vs. what's a real miss. Don't re-flag P0–P3 items the team already knows about unless the PR is the one that was supposed to fix them.
- Challenge every `clone()`, `to_string()`, `Box::new(...)` on hot paths (evaluator, dispatch, resolution). Be quieter in cold paths (CLI, diagnostics, Pass 1 allocation).
- `Arc<PureModel>` + per-thread `Executor` is the threading model. Any suggestion that would require `PureModel` to be `!Sync`, or `Evaluator` to be `Send`, needs to justify the change against that model.
- Prefer concrete proposals with benchmark harnesses (criterion) over "consider optimizing".
