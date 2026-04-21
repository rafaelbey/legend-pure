---
name: Port status snapshot April 2026
description: What's working in the Rust port today and what remains open, based on April 2026 repo state
type: project
---

**Snapshot date:** 2026-04-17, branch `legend-pure-rust` @ commit `0f3eab2e2be`.

**What works today:**
- Full lexer + recursive-descent parser for Pure (incl. type params, generic new syntax `^Type<Args>(...)`, slice `[start:stop:step]`, copy-from-variable, tilde unit refs, multi-statement lambdas).
- Composer (AST → grammar text) with multi-file parallel compose via rayon.
- Protocol v1 JSON conversion (AST ↔ protocol), snapshot-tested with `insta`.
- Semantic layer: declaration → topo sort → Pass 2a/2b → Pass 2.5 inference → validation.
- Type-based function dispatch with multiplicity-aware narrowing and variable-type tracking (params, let, lambda — scoped).
- Runtime interpreter for the "grammar tier" — literals, arithmetic, comparison, boolean, string concat/length/substring, let, if (deferred-arg), map/filter/fold via lambdas, user-defined function calls (incl. recursion). See `crates/runtime/tests/eval_tests.rs`.
- CLI commands: `parse`, `emit`, `compile`, `check`, `init`, `completions`; `test` and `plan` are stubs.

**Numbers** (`crates/pure/FUNCTION_DISPATCH.md`, `BACKLOG.md`):
- 243 compile errors when compiling the platform (down from 538 baseline, 55% reduction).
- Breakdown: 161 `AmbiguousImport`, 75 `UnresolvedElement`, 6 `ParseFailure`, 1 `DuplicateElement`.
- Top offenders: `elementToPath` (30), `dynamicNew` (29), `map` (21), `assertEquals` (12), `assert` (10), `abs` (8), `plus` (8) — mostly M3 metaclass types, generic multiplicity params, and overload messaging.

**What's NOT there yet:**
- Runtime stdlib has only ~6 native families (arithmetic/comparison/boolean/string/collection/lang/testing). Java has 173 native functions; convergence_analysis.md lists them in Grammar (30) / Essential (80) / Extension (63) tiers.
- JNI crate (`crates/jni/src/lib.rs`) is 22 lines — essentially empty. No execution bridge from Java into Rust yet.
- `legend test` CLI doesn't execute tests; `legend plan` is unimplemented.
- `QualifiedPropertyAccess` in the evaluator returns "not yet supported" (`crates/runtime/src/eval.rs:582`).
- Island expression lowering (`#>{}#`, `#{}#`) is unimplemented — compile errors for any graph fetch / path expression.
- Incremental compilation is not started.
- No compiled-engine-style codegen (hybrid compilation docs exist but only as design).
- Constraint evaluation at validation time is deferred (P3).

**Existing test counts** (as of 2026-04):
- Integration tests in pure: `crates/pure/tests/integration_tests.rs` (1515 lines).
- Runtime eval tests: `crates/runtime/tests/eval_tests.rs` (545 lines).
- Java reference has 1492 @Test methods in 232 classes, 873 containing inline Pure payloads — the porting target documented in `legend-pure-rust/test_catalog.md`.

**Recent activity trajectory** (git log on branch):
- Feb–Mar 2026: Parser features (unit refs, primitive constraints, slice, dotted keys, negative-year dates, generic-new, copy-from-variable, lambda shadowing).
- Late Mar: Runtime native dispatch, FQN-based native lookup.
- Apr: Compiler work focused on function dispatch — variable type tracking (`ea306eb7a9d`), narrowest-match dispatch, BACKLOG/docs updates. Active focus is reducing the 243-error backlog on the platform compile.

Platform `.pure` source has active edits on this branch (mostly additive tests in `assert.pure`, new `dynamicNew.pure` overloads, extended `copy.pure`/`new.pure`, small date-extract tweaks) — the Rust team is expanding the parity contract alongside the port.
