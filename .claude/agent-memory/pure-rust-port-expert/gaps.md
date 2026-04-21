---
name: Gaps, risks, and underserved platform surface
description: Areas where the Rust port lags, where silent semantic drift is most likely, and which .pure platform surface most needs attention
type: project
---

**Highest-risk platform surface (stdlib features with thin or absent Rust support):**

1. **Metaprogramming family — `meta::pure::functions::lang::*`** (`essential/lang/creation/dynamicNew.pure`, `essential/lang/eval/evaluate.pure`, `essential/meta/reflect/canReactivateDynamically.pure`). The 29 `dynamicNew` compile errors and the `docs/runtime/metaprogramming.md` design doc both signal this is largely unbuilt. Parity gaps: runtime class reification, `GenericType` reflection, `deactivate`/`reactivate`, `evaluateAndDeactivate`. Missing support blocks any test using runtime reflection.

2. **Units and measures.** Units live as package-level elements with `Measure~Unit` name mangling (`crates/pure/src/pipeline.rs:370`) instead of being nested under Measure. No `convert(value, sourceUnit, targetUnit)` native, no conversion factor storage. All unit arithmetic tests in `AbstractTestMeasure` (28 tests catalogued) will fail.

3. **Qualified properties (derived properties) at runtime.** `eval_qualified_property` returns "not yet supported" (`crates/runtime/src/eval.rs:582`). Any class with `derivedProperty()` that's invoked runs aground immediately. Qualified properties ARE compiled (Pass 2b lowers their bodies via `lower_qualified_properties` in `pipeline.rs:1000`), so the gap is purely in the evaluator's dispatch.

4. **Constraints at validation time.** `Class { ... [ $this.age >= 0 ] }` constraint expressions are lowered (`crates/pure/src/pipeline.rs:1055`) but not evaluated at `new` time. Java enforces `ConstraintViolation` at instance creation. `BACKLOG.md` marks this P3 deferred.

5. **Islands: graph fetch (`#{...}#`), path expressions (`#>{...}#`), `###Mapping`, `###Relational`, `###Service` sections.** Parser treats them as `Expression::Island`; lowering returns `UnsupportedExpression` (`crates/pure/src/lower.rs:97-103`). Any Mapping DSL or graph-fetch test dead-ends. The `IslandPlugin`/`SectionPlugin` traits exist (`crates/parser/`), so the architecture is in place — but no plugin implementations beyond core.

6. **Relation / TDS.** `ExprKind::Column` is a placeholder. `TypeExpr::Relation(RelationId)` exists structurally but there's no evaluator path for column builders, window frames, `aggColSpec`, etc. `docs/runtime/persistent_data_structures.md` discusses a columnar layout but nothing is built.

7. **Incremental compilation.** No `UnloadWalk`/`UnloadUnbind` equivalent. IDE/Legend Studio use cases (hot reload, partial recompile on file edit) unsupported. `BACKLOG.md` marks P3.

8. **JNI execution bridge.** `crates/jni/src/lib.rs` is 22 lines with no JNI entry points. The Phase 6 plan (`convergence_analysis.md`) proposes `execute()` but it's not started. Consequence: no way for `legend-engine` to call into Rust yet, and no cross-engine PCT harness is possible.

9. **`legend test` CLI.** File exists (`crates/cli/src/commands/test.rs`) but is a skeleton — can't discover `<<test.Test>>` or `<<PCT.test>>` functions yet. Running the Java reference test corpus end-to-end via Rust is blocked.

**Risk areas where semantics could silently drift:**

- **Native `find_by_prefix` fallback** (`crates/runtime/src/native.rs:207`) — dispatches to alphabetically-first match when the FQN key doesn't resolve. A broken compile dispatch can mask itself as "works" with the wrong overload.
- **Partial parse acceptance** (`crates/runtime/tests/eval_tests.rs:129`) — tests currently proceed on parse error because platform has unsupported constructs; this pattern will hide regressions if copied to new tests without explicit Ok-only assertions.
- **Enum value as string** (`crates/runtime/src/eval.rs:593`) — any identity-sensitive enum test diverges silently.
- **Generic params treated as `Any`** (FUNCTION_DISPATCH.md) — can cause false overload matches for `map`, `fold`, `filter` generic variants.
- **Milestoning is absent** — classes with `<<temporal.*>>` compile but are missing synthetic properties; tests referencing `.businessDate(...)` will get UnresolvedElement, not a clear diagnostic.
- **`PureDate` timezone/precision edge cases** — 755-line `date.rs` needs dedicated cross-Java differential testing (DST, leap, month-end adjust, year-only precision).
- **Floating-point ordering** — `Value::PartialEq` compares floats by `to_bits()`. Java's `==` on primitives vs `BigDecimal::equals` have their own quirks; any `assertEquals` test with float/decimal is a candidate for drift.

**CLAUDE.md convention violations easy to hit:**
- Adding `unwrap()` or `expect()` in library code — CI gate `cargo lint-lib` catches, but local dev often does not until PR.
- Adding `serde` to `ast` — forbidden; protocol/JSON must stay in `protocol` crate.
- Emitting `tracing::info!` in hot paths — noisy; spec says `info+` only at call boundaries.
- Forgetting the copyright header on `.rs/.toml/.pure/.sh` — `scripts/check-copyright.sh` catches it.
- Missing `///` docs on public items — `#![deny(missing_docs)]` catches it.
- Desugaring new expression kinds into `FunctionCall` without updating types.rs docs and checking DESIGN.md §11 still reflects reality. The current `ExprKind` already violates this; don't further entrench it without a decision.

**Underserved `.pure` platform surface, ranked by breadth of breakage:**
1. `essential/meta/` and `essential/lang/` (metaprogramming, eval, dynamicNew, cast)
2. `essential/collection/iteration/` (map/filter/fold work for simple cases but not with generics, sortBy, groupBy, zip, take/drop, removeDuplicates)
3. `grammar/functions/meta/type/relation/` (column/relation — none implemented)
4. `grammar/milestoning.pure` + consumers (temporal rewrite)
5. `essential/lang/unit/` (unit conversion + cross-unit arithmetic)
6. `essential/date/` (variable-precision, adjust, `dateDiff`, hasDay/hasMonth)
7. `essential/meta/reflect/` (reactivate, deactivate, openVariableValues)

`crates/pure/BACKLOG.md` is the active tracker for known compiler gaps — ground-truth before picking work.
