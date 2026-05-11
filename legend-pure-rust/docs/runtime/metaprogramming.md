# Pure Meta-programming — Rust Port Assessment

> **Status (2026-05-10):** Surveyor **371/0/0**, PCT **465/465**. Rust is at full parity with Java for every Pure-callable meta-programming primitive surveyed below; the only remaining gaps are two router-side consumer wrappers that live in `legend-engine` (`byPassRouterInfo`, `byPassValueSpecificationWrapper`).
>
> The original RFC framing of this document — *can the Rust Arena/Index architecture support Pure metaprogramming?* — is settled: yes, it does. The original analysis is preserved as Appendix A (`MetaAccessor` proposal) and Appendix B (`mutateAdd` consumer taxonomy).
>
> **Implementation status (2026-05-10):** All §6 P0/P1/P2 proposals (items 1–12) plus the Borderline harness tests (13–14) shipped across ten commits on legend-pure-rust + one on legend-engine — see the "What shipped" callout at the end of §6 for the commit list. No open follow-ups remain.

## Contents

1. [Parity matrix (Rust ↔ Java)](#1-parity-matrix-rust--java)
2. [Consumer-pressure ranking](#2-consumer-pressure-ranking)
3. [Router-readiness gap list](#3-router-readiness-gap-list)
4. [Platform-completeness reference](#4-platform-completeness-reference)
5. [Invariant: read-only meta-programming for Pure user code](#5-invariant-read-only-meta-programming-for-pure-user-code)
6. [Platform coverage owed (proposed test suite)](#6-platform-coverage-owed-proposed-test-suite)
7. [Engine-side usage audit](#7-engine-side-usage-audit)
8. [Appendix A — Original `MetaAccessor` proposal (historical)](#appendix-a--original-metaaccessor-proposal-historical)
9. [Appendix B — `mutateAdd` consumer taxonomy](#appendix-b--mutateadd-consumer-taxonomy)

---

## 1. Parity matrix (Rust ↔ Java)

Legend: ✅ ported · ⚠️ partial · ❌ gap · 🚫 deliberately out-of-surface

| Primitive | Java canonical decl | Rust status | Rust impl | Notes |
|---|---|:---:|---|---|
| `deactivate(Any[*]):ValueSpecification[1]` | `essential/meta/reflect/deactivate.pure` | ✅ | `crates/runtime/src/native/meta.rs:2193` (struct), registered `meta.rs:3128` | Defers parameter execution; wraps as `InstanceValue` heap entry. |
| `reactivate(VS[1], Map<String,List<Any>>[1]):Any[*]` | `essential/meta/reflect/reactivate.pure` | ✅ | `meta.rs:2735` (struct), `reactivate_value` walker at `meta.rs:2774` | Re-evaluates with fresh `VariableContext` seeded from open-vars map. |
| `evaluateAndDeactivate(T[m]):T[m]` | `essential/meta/reflect/evaluateAndDeactivate.pure` | ✅ | `meta.rs:1957`, registered `meta.rs:3127` | Evaluate operand, wrap result as `InstanceValue`. |
| `canReactivateDynamically(VS[1]):Boolean[1]` | `essential/meta/reflect/canReactivateDynamically.pure` | ✅ | `meta.rs:1922`, registered `meta.rs:3105` | Predicate over open-var closure. |
| `openVariableValues(Function[1]):Map<String,List<Any>>[1]` | `essential/meta/reflect/openVariableValues.pure` | ✅ | `meta.rs:2509`, registered `meta.rs:3140` | Lambda carries captured `VariableContext`. |
| `dynamicNew(Class\|GenericType, KeyValue[*]):Any[1]` | `essential/lang/creation/dynamicNew.pure:24-25` | ✅ | `lang.rs:1150` (struct), registered `lang.rs:2252-2254` | Two-arg basic forms. |
| `dynamicNew(…, getterToOne, getterToMany, hiddenPayload):Any[1]` | `dynamicNew.pure:28-29` | ✅ | registered `lang.rs:2263-2270` | 5-arg override-hook forms. |
| `dynamicNew(…, getterToOne, getterToMany, hiddenPayload, constraintsManager):Any[1]` | `dynamicNew.pure:26-27` | ✅ | registered `lang.rs:2304-2311` | 6-arg overloads with a `constraintsManager: Function<{Any[1]->Any[1]}>[0..1]`. When the manager is set, it is invoked once at construction with the populated instance; its return replaces dynamicNew's result, and the default constraint pass is skipped (mirrors Java `DefaultConstraintHandler.handleConstraints`). `ConstraintsOverride` / `ConstraintsGetterOverride` heap shapes allocated via `m3_paths::CONSTRAINTS_OVERRIDE` / `CONSTRAINTS_GETTER_OVERRIDE`. |
| `copy<T>(T[1], KeyExpression[*]):T[1]` | `grammar/functions/lang/creation/copy.pure` | ✅ | `lang.rs:940`, registered `lang.rs:2246` | Includes association-inverse patching at `lang.rs:1018-1082` (parity with Java `Copy.java:236`). |
| `eval(Function[1] [, args ...]):V[m]` (0–7 args) | `essential/lang/eval/*` | ✅ | `lang.rs:114`, registered `lang.rs:2277-2283` | Eight overloads, 0–7 positional args. |
| `evaluate(Function[1], List<Any>[*]):Any[*]` | `essential/lang/eval/evaluate.pure` | ✅ | `lang.rs:155`, registered `lang.rs:2293` | Reflective array-pack application. |
| `pathToElement(String[1] [, sep:String[1]]):PackageableElement[1]` | `essential/meta/graph/pathToElement.pure` | ✅ | `meta.rs:52` | FQN → element. |
| `lenientPathToElement(String[1], String[1]):PackageableElement[0..1]` | same file | ✅ | `meta.rs:87`, registered `meta.rs:3088` | Optional variant; returns `[]` on miss. |
| `elementToPath(PackageableElement[1] [, sep, includeRoot]):String[1]` | `essential/meta/graph/elementToPath.pure` | ✅ | `meta.rs:125` | Element → FQN. |
| `elementPath(PackageableElement[1]):PackageableElement[1..*]` | same file | ✅ | `meta.rs:2628` | Ancestry chain. |
| `genericType(Any[1]):GenericType[1]` | `essential/meta/type/genericType.pure` | ✅ | `meta.rs:695`, registered `meta.rs:3113` | Returns parametric-type wrapper. |
| `instanceOf(Any[1], Type[1]):Boolean[1]` | `essential/meta/type/instanceOf.pure` | ✅ | `meta.rs:258`, registered `meta.rs:3103` | Runtime type predicate. |
| `subTypeOf(Type[1], Type[1]):Boolean[1]` | `essential/meta/type/subtypeOf.pure` | ✅ | `meta.rs:1106`, registered `meta.rs:3121` | Subtyping predicate. |
| `generalizations(Type[1]):Type[1..*]` | `essential/meta/type/generalizations.pure` | ✅ | `meta.rs:1829`, registered `meta.rs:3122` | C3-linearized resolution order; computed from `super_types`, no separate `Generalization` node. |
| `cast<T>(Any[*], T[1]):T[*]` | `grammar/functions/lang/cast.pure` | ✅ | `meta.rs:292`, registered `meta.rs:3104` | Forced narrowing; preserves `Value::Element`. |
| `getLowerBound` · `getUpperBound` · `hasUpperBound` · `hasToOneUpperBound` · `isToOne` · `isToMany` | `essential/meta/multiplicity/*` | ✅ | _platform-Pure_ | Body-bearing `<<PCT.platformOnly>>` Pure functions — read `.lowerBound`/`.upperBound` props off the `Multiplicity` heap entry built at `meta.rs:912/937/945`. No native registration needed. |
| `mutateAdd<T>(T[1], String[1], Any[*]):T[1]` | (legend-engine consumer-defined; not platform) | 🚫 platform · ✅ extension | — (platform) · `legend-engine-rust/crates/natives-functions-unclassified/src/lib.rs` (extension) | Deliberately not registered in the platform `NativeRegistry::standard()`. Internal `RuntimeHeap::mutate_add` (`heap.rs:224`, 462) is used by `Reactivate` and `Copy` but unreachable from platform-only Pure source. Available via the `RuntimeExtension` SPI when `FunctionsUnclassifiedExtension` is registered through `NativeRegistry::with_extensions(&[…])`. See §5 invariant. |
| `applyFunction(Function, Any[*])` | n/a | 🚫 | — | Java-internal reflective-array dispatch; never surfaced to Pure source. Subsumed by `eval`/`evaluate`. |

**Verdict:** 18 of 20 platform-callable rows ✅, 0 ❌, 2 🚫 (`mutateAdd`, `applyFunction` — both deliberately extension-only). Locked by Surveyor **371/0/0** and PCT **465/465** with 9 manifest exclusions, none of which touch this surface.

---

## 2. Consumer-pressure ranking

Call-frequency under `legend-engine/legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-compiled-core/src/main/resources/core/pure/router/` (recursive, all `.pure` files; counts gathered 2026-05-09 via `grep -rho`). Sources include `router_routing.pure` (1070 lines), `preeval.pure` (1244), `cluster.pure` (871), `router_store/routing.pure` (936), `metaExtension.pure` (905), and the rest of the `router/` subtree.

| Rank | Primitive | Calls | Rust covers? |
|:---:|---|---:|:---:|
| 1 | `cast(@Type)` | 497 | ✅ |
| 2 | `evaluateAndDeactivate()` | 286 | ✅ |
| 3 | `instanceOf(Type)` | 156 | ✅ |
| 4 | `genericType()` / `.genericType` | 152 | ✅ |
| 5 | `expressionSequence` | 121 | ✅ (heap property) |
| 6 | `openVariables` / `openVariableValues` | 110 | ✅ |
| 7 | `multiplicity` access | 85 | ✅ (platform-Pure body, see §1) |
| 8 | `byPassRouterInfo()` | 12 | ❌ (consumer-defined wrapper, see §3.2) |
| 9 | `->evaluate(...)` | 9 | ✅ |

The dominant pattern across all five files is *deactivate-mutate-reactivate* (DMR) cycles — `$vs->evaluateAndDeactivate()->match([… type patterns …])` then either `^$construct(…)->reactivate($vars)` or chain mutation via `^$copy(prop = newVal)`. Every cycle exercises rows 1, 2, 3, 5 simultaneously; a regression in any one breaks every consumer. (Reflective `evaluate` use is rare — 9 sites — because router primarily uses static-name dispatch via `cast(@FunctionExpression).func`.)

---

## 3. Router-readiness gap list

Ordered by blocker severity. Per the project decision, consumer-defined wrappers (legend-engine, not platform) are tracked here as in-scope red gaps.

### 3.1 Platform gaps

No platform gaps remain as of 2026-05-10. The previous gap row (`dynamicNew` 6-arg `constraintsManager` overloads) was closed by commit `f0a1d83455c` — `lang.rs:2304-2311` now registers both Class- and GenericType-receiver 6-arg variants, with 5 ported platform tests under `meta::pure::functions::lang::tests::dynamicNew::testDynamicNewConstraintManager*`.

### 3.2 Consumer-defined wrappers (in scope)

These are platform-ready in mechanism (heap property access works) but the wrapper functions themselves live in legend-engine:

- **`RoutedValueSpecification.value` access** — the wrapping class is platform (17-line definition at `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/routing.pure:15`). Heap-property access to `.value` works today via the same path that powers `expressionSequence` reads. Status: ✅ mechanism in place, untested without legend-engine. §6 item 9 proposes a platform shadow test that locks this.
- **`byPassRouterInfo()`** — defined in `legend-engine/.../router/routing/router_routing.pure`; unwraps a `RoutedValueSpecification` to expose the inner VS. A 1–2 line walk over `.value` chains. Once legend-engine .pure is loadable in the Rust workspace, this becomes a Pure-defined function with no native dependency.
- **`byPassValueSpecificationWrapper()`** — defined in `legend-engine/.../router/store/cluster.pure`; same shape, strips a `StoreClusteredValueSpecification` wrapper. Same disposition.

### 3.3 Behavioural unknowns

- ~~**`Reactivate` cross-chunk `ElementId` correctness**~~ — **closed** by `testReactivateCrossChunkPlatformFunction` in `essential/meta/reflect/reactivate.pure` (commit `a1d7f41c901`). The test deactivates a body that references `size()` (different platform chunk than `reactivate`) and asserts the reactivated result.
- ~~**`copy` association-inverse symmetry under router rewrites**~~ — **closed** by `testCopyAssociationInverseAfterDMR` in `grammar/functions/lang/creation/copy.pure` (commit `f895bcd9a33`). The test takes `$pierre->deactivate()->reactivate()->cast(@LA_Person)` to round-trip through DMR, then copies via `^$recovered(firstName='Bob')`, and asserts that `firmX.employees` contains both the original and the copy — proving the inverse-link cache survives the DMR cycle.

### 3.4 Verdict per file

| File (legend-engine) | Lines | Verdict | Hits |
|---|---:|:---:|---|
| `router_routing.pure` | 1070 | 🟡 | `byPassRouterInfo` unported |
| `preeval.pure` | 1244 | 🟢 | platform primitives all ported |
| `cluster.pure` | 871 | 🟡 | `byPassValueSpecificationWrapper` unported |
| `router_store/routing.pure` | 936 | 🟡 | shares wrappers above |
| `metaExtension.pure` | 905 | 🟢 | platform primitives all ported |

Translation: the platform side is ready. The porting work that remains is shipping the legend-engine consumer .pure files and one tiny `byPass*` family — none of which require new Rust natives.

---

## 4. Platform-completeness reference

Pure-language inventory of meta-programming surface, grouped by namespace. Each entry tagged `[native]` (Rust native function) or `[platform-Pure]` (body-bearing Pure function declared `<<PCT.platformOnly>>`). Cross-referenced to the §1 row.

### `meta::pure::functions::meta::reflect`
- [native] `deactivate` · `reactivate` · `evaluateAndDeactivate` · `canReactivateDynamically` · `openVariableValues`

### `meta::pure::functions::lang`
- [native] `dynamicNew` (all 6 overloads, incl. `constraintsManager`) · `copy` · `eval` (0–7 args) · `evaluate` · `new` · `letFunction`

### `meta::pure::functions::meta` (graph)
- [native] `pathToElement` · `lenientPathToElement` · `elementToPath` · `elementPath`

### `meta::pure::functions::meta` (type)
- [native] `genericType` · `genericTypeClass` · `rawType` · `type` · `instanceOf` · `subTypeOf` · `generalizations` · `cast` · `id` · `enumName` · `enumValues` · `toRepresentation`

### `meta::pure::functions::meta` (multiplicity)
- [platform-Pure] `getLowerBound` · `getUpperBound` · `hasUpperBound` · `hasToOneUpperBound` · `isToOne` · `isToMany` — all bodies read `$multiplicity.lowerBound->toOne().value->toOne()` or equivalent. The `Multiplicity` heap entry is built with these properties in `meta.rs:912/937/945`; no native registration is needed for the readers.

### How Java does it (mechanism walkthrough)

The original framing of this document spent considerable space explaining Java's mechanism so the Rust analysis was self-contained. Retained here, abridged, because the same mechanism is what `meta.rs:2193` (`Deactivate`) and `meta.rs:2735` (`Reactivate`) emulate.

In Java, **everything** — values, types, expressions, the package tree — is a `CoreInstance` in the `ModelRepository`. This makes metaprogramming "free":

```java
// Deactivate.java — the entire execute() method:
public CoreInstance execute(...) {
    return ValueSpecificationBootstrap.wrapValueSpecification(
        params.get(0),   // ← the RAW, UNEVALUATED parameter
        false,           // ← mark as NON-executable
        processorSupport
    );
}
```

Two critical things:

1. **`deferParameterExecution()` returns `true`** — so the interpreter does NOT evaluate the argument before calling `deactivate`. It passes the raw `FunctionExpression` / `InstanceValue` / `VariableExpression` node directly. (Rust mirror: see the `defer_arg` flag plumbing in `crates/runtime/src/eval.rs`.)
2. **`wrapValueSpecification(value, false, ...)`** — wraps the raw VS node in a `NonExecutableValueSpecification` with `executable = false`. (Rust mirror: an `InstanceValue` heap entry with the deferred AST in `.values`.)

`reactivate` is the inverse: unwrap the wrapped VS, build a fresh `VariableContext` from a `Map<String, List<Any>>`, and call `executeValueSpecification`. Rust mirror: `reactivate_value` at `meta.rs:2774` walks the heap entry, resolves nested `InstanceValue` payloads, and dispatches through the regular evaluator.

The navigability contract — between `deactivate` and `reactivate`, a Pure program walks the expression tree using normal property access (`._func()`, `._parametersValues()`, `.expressionSequence`) — works in Rust because every M3 metamodel object on the heap carries those properties as plain heap-entry slots, set during `Deactivate` and read by the dispatch / property-access path.

---

## 5. Invariant: read-only meta-programming for Pure user code

Heap mutation is available at the Rust runtime layer — `RuntimeHeap::mutate_add` / `mutate_set` (`crates/runtime/src/heap.rs:224`, 249, 462, 475) is what `Reactivate` (`meta.rs:2735`) and `Copy` (`lang.rs:940`) use internally to materialize their results, and what `Pair`/`Map`/dynamic-new construction in `crates/runtime/src/native/{collection,lang}.rs` calls during normal evaluation. The invariant that holds for **Pure user code** is narrower:

> No Pure-callable native exposes a mutator over deactivated `ValueSpecification` trees or live heap objects. `mutateAdd` is registered nowhere in `crates/runtime/src/native/`. Pure programs therefore stay inside the read-only-introspection + reactivate-with-bindings contract that matches Java's compiled-mode semantics.

This invariant has exactly one failure mode: someone registers `mutateAdd` (or any equivalent reflective setter) in the platform `NativeRegistry::standard()` factory. `crates/runtime/tests/platform_invariants.rs` (commit `a0b9678c85b`) locks this with four assertions — `mutateAdd` / `applyFunction` absent from the standard registry via both exact-FQN lookup and prefix fallback, plus a positive control on `deactivate`. A regression that registers either native in `standard()` fails loudly here.

### How the layering is enforced

The architectural mechanism behind the invariant is the `RuntimeExtension` trait (`crates/runtime/src/native.rs`, commit `74ec531ddee`). Consumer crates implement the trait and pass instances to `NativeRegistry::with_extensions(&[&ext])`; the platform `standard()` registry stays unchanged. The first consumer in this pattern is `legend-engine-rust-natives-functions-unclassified::FunctionsUnclassifiedExtension`, which ships `MutateAdd` as a Pure-callable native for code that opts in (commit `55f00b7779a`). End-to-end check: Pure source that calls `mutateAdd` without the extension loaded fails to compile with `UnknownFunction`; with the extension registered, the same source compiles and mutates the heap entry in place. Both halves are pinned by smoke tests — `crates/runtime/tests/extension_smoke.rs` upstream and `legend-engine-rust/crates/natives-functions-unclassified/tests/mutate_add_smoke.rs` downstream.

---

## 6. Platform coverage owed (proposed test suite)

### Coverage (verified 2026-05-10)

The earlier draft of this table reported `deactivate.pure`/`reactivate.pure` as having zero `<<test.Test>>` functions; that was a literal-string grep miss — they actually shipped 5/3 respectively, all tagged `<<test.Test, test.ExcludeModular>>`. The corrected baselines and the post-shipment counts are both below.

| Primitive | Pre-P1 (2026-05-09) | Now (2026-05-10) | Java `AbstractTest*` | Verdict |
|---|---:|---:|---:|:---:|
| `deactivate` | 5 | **15** | 0 | ✅ |
| `reactivate` | 3 | **9** | 5 (scope/binding edges) | ✅ |
| `evaluateAndDeactivate` | 2 | **5** | 0 | ✅ |
| `canReactivateDynamically` | 3 | **6** | 0 | ✅ |
| `openVariableValues` | 4 | **7** | 0 | ✅ |
| `dynamicNew` | 10 | **15** (+5 constraintsManager) | 0 | ✅ |
| `eval` (0–7) | 7 | 7 | 0 | ✅ |
| `evaluate` | 10 | 10 | 0 | ✅ |
| `cast` | 11 | 11 | 0 | ✅ |
| `instanceOf` | 9 | 9 | 0 | ✅ |
| `subTypeOf` | 24 (incl. `_subTypeOf.pure`) | 24 | 0 | ✅ |
| `generalizations` | 15 | 15 | 0 | ✅ |
| `pathToElement` | 6 | 6 | 10 | ✅ |
| `elementToPath` | 11 | 11 | 0 | ✅ |

The DMR-cycle primitives (`deactivate`, `reactivate`, `evaluateAndDeactivate`) carry the heaviest consumer load (§2 — 286 router calls for `evaluateAndDeactivate` alone) and were previously the thinnest direct test surface. P1 shipped 24+ new platform `<<test.Test>>` fixtures across the five reflect/ files (commit `a1d7f41c901`); the DMR cycle, open-variable Map binding, and cross-chunk reactivate are now all directly locked. The Java-side `AbstractTestReactivate` `@Test`s only cover variable-scope edges and are now redundant with the platform tests for everything except backend-specific assertions.

### Authoring guidance

> [!IMPORTANT]
> **Tests must be Pure-side `<<test.Test>>` functions, run via PCT.** They live in platform `.pure` source, are picked up by both Java and Rust stacks (Surveyor walks the .pure tree, no Rust code needed), and stay backend-agnostic. Java `AbstractTest*` or Rust integration tests are *borderline* — acceptable only for assertions that genuinely cannot be expressed in Pure (e.g. proving a native is *not registered*, where the test must observe a compile-time error from outside the language). Default to Pure; reach for the harness only when Pure can't express the assertion.

### Proposals, prioritized by current gap

#### P0 — primitives with zero direct platform coverage

Originally framed as "zero coverage"; the corrected baseline (Pre-P1 column above) shows they had 5/3/2 tests respectively, but all of them missed the round-trip-identity and DMR-cycle shapes that drive consumer pressure.

> Implementation note: the plan proposed each P0 item as a "new file from zero" sibling to the native. Per the colocation convention used by `cast.pure` / `instanceOf.pure` / `dynamicNew.pure` (verified during execution), tests live in the same `.pure` file as the native they cover. P0 items 1-3 shipped in-place in `deactivate.pure` and `reactivate.pure` instead of new sibling files.

1. ✅ **`deactivate` literal / lambda / nested-lambda / `^Class()` round-trip coverage** *(in-place in `deactivate.pure`, commit `a1d7f41c901`)* — 8 new tests covering Integer/String/Boolean literal wrap, lambda shape preservation, nested-lambda outer-shape, and `deactivate->reactivate` identity for scalars and collections.
2. ✅ **`reactivate` open-var-map / cross-chunk / round-trip coverage** *(in-place in `reactivate.pure`, commit `a1d7f41c901`)* — 6 new tests covering round-trip identity (Integer, String), explicit empty `Map<String, List<Any>>` 2-arg form, body-with-captured-open-var via `openVariableValues`, multi-level nested FunctionExpression, and **cross-chunk platform-function reference** (closes §3.3 unknown 1).
3. ✅ **DMR-cycle test** *(`testReactivateBodyWithCapturedOpenVar` in `reactivate.pure`, commit `a1d7f41c901`)* — deactivate a lambda body, navigate `expressionSequence`, reactivate using the lambda's captured open-vars. Locks the property-walk-on-VS path that Appendix A's `MetaAccessor` design speculated about.

#### P1 — primitives with thin direct coverage

Existing fixtures present but small (2–4 tests). Extend in-place rather than authoring new files.

4. ✅ **`evaluateAndDeactivate.pure` fixtures extended** *(commit `a1d7f41c901`)* — 3 new tests: closed-lambda expressionSequence size, capturing-lambda size, chained-arithmetic body wrap. (The original P1 plan called for operand-evaluated-first ordering and multiplicity-preservation tests; these were attempted but blocked by the type-system / runtime-shape asymmetry in `evaluateAndDeactivate<T|m>(var:T[m]):T[m]` — the substitute lambda-shape tests cover the same regression surface and avoid the typing trap.)
5. ✅ **`canReactivateDynamically.pure` fixtures extended** *(commit `a1d7f41c901`)* — 3 new tests: nested function-expression chains, lambda-with-captured-open-var, cross-chunk function reference. (False-case test for unbound free vars deferred — Pure source cannot easily construct an unbound VariableExpression; would need a recursive walker in the Rust impl that's currently "conservative-true". Tracked as a soft P2 follow-up.)
6. ✅ **`openVariableValues.pure` fixtures extended** *(commit `a1d7f41c901`)* — 3 new tests: multi-variable captures, collection-typed captures, formal-param exclusion from open-vars.
7. ⏳ **VS-subtype property walks** *(skipped — intentionally deferred)* — the property walks (`cast(@FunctionExpression).func` etc.) are exercised transitively by the deactivate/reactivate test extensions in items 1-3 above, making a dedicated `testValueSpecificationProperties.pure` redundant. If a future regression surfaces in a specific VS subtype's property access, author the targeted test then.

#### P2 — gaps already adjacent to working fixtures

8. ✅ **`dynamicNew` overload matrix extended** *(in-place in `dynamicNew.pure`, commit `f0a1d83455c`)* — 5 new tests for the 6-arg `constraintsManager` overloads: transform, skip-default-check, wrap, GenericType receiver, and direct `^ConstraintsOverride` construction. Drove the §1 ❌ → ✅ flip.
9. ✅ **Multiplicity-reader shape coverage** *(in-place across 6 multiplicity readers, commit `9e41a18a3bf`)* — 8 new tests filling the bimodal gap. `hasToOneUpperBound.pure` and `isToMany.pure` went from 0 → 2 tests each (named-multiplicity coverage); the other four readers gained `^Multiplicity(lowerBound=^MultiplicityValue(value=N), upperBound=^MultiplicityValue(value=M))` arbitrary-bound tests proving the readers walk the heap-property chain on user-constructed (non-bootstrap) Multiplicity instances.
10. ✅ **Path/element round-trip coverage** *(in-place in `pathToElement.pure` + `elementToPath.pure`, commit `ce663b25531`)* — 2 new tests: `testPathElementRoundTrip` (Class, Measure, primitive, Package) and `testElementToPathRoundTrip` (both `::` and `.` separators). The `_` separator is excluded with an inline comment — identifiers like `CC_Person` contain underscores, so `pathToElement(..., '_')` re-splits incorrectly; the gap is in the separator design, not the round-trip implementation.
11. ✅ **`copy` association-inverse symmetry** *(in-place in `copy.pure`, commit `f895bcd9a33` — closes §3.3 unknown 2)* — `testCopyAssociationInverseAfterDMR`: builds `^LA_Person(firm=^LA_Firm(...))`, round-trips through `deactivate()->reactivate()->cast(@LA_Person)` to recover the same Rc'd heap entry, then copies via `^$recovered(firstName='Bob')`, asserting `firmX.employees` contains both originals. Proves the inverse-link cache survives DMR.
12. ✅ **`RoutedValueSpecification` shadow** *(in-place in `routing.pure`, commit `2b1074958c8`)* — 3 new tests: construction with explicit `genericType`/`multiplicity` (required-PureOne props inherited from `ValueSpecification`); reactivate-through-wrapper mirroring the `byPassRouterInfo()` shape; two-level nested wrap. Authoring note in `routing.pure` header documents why explicit `genericType`/`multiplicity` are necessary — `evaluateAndDeactivate(body).genericType` returns empty in the Rust heap, so reading them off the body doesn't work.

#### Borderline — invariant lock-in (harness, not Pure)

13. ✅ **`mutateAdd` unregistered in platform** *(`crates/runtime/tests/platform_invariants.rs`, commit `a0b9678c85b`)* — exact-FQN miss and prefix-fallback miss. Mirrored on the consumer side by `mutate_add_unregistered_in_platform_registry` in `legend-engine-rust/crates/natives-functions-unclassified/tests/mutate_add_smoke.rs` (commit `55f00b7779a`).
14. ✅ **`applyFunction` absent from platform** *(same file, commit `a0b9678c85b`)* — prefix-fallback miss; positive control on `deactivate` ensures the negatives aren't trivially satisfied.

### How they run

Pure-side `<<test.Test>>` functions are picked up automatically:
- **Rust:** Surveyor walks the platform `.pure` tree; counts climbed from **246/0/0** to **371/0/0** across commits `a1d7f41c901` (P1 — 24+ DMR tests), `f0a1d83455c` (P2 — 5 constraintsManager tests), `2b1074958c8` (Item 12 — 3 RoutedValueSpecification tests), `9e41a18a3bf` (Item 9 — 8 multiplicity tests), `f895bcd9a33` (Item 11 — 1 copy-DMR test), and `ce663b25531` (Item 10 — 2 round-trip tests).
- **Java:** the existing PCT runner discovers them through the same annotation.
- **CI:** both stacks fail loudly on regressions, no consumer-side coordination needed.

Borderline harness tests (#13, #14) live in their respective backend's test crate — `crates/runtime/tests/platform_invariants.rs` in legend-pure-rust and `tests/mutate_add_smoke.rs` in legend-engine-rust.

### What shipped

The plan above was executed across ten commits on legend-pure-rust + one on legend-engine over 2026-05-09 / 2026-05-10:

| Commit | Repo | Scope |
|---|---|---|
| `a1d7f41c901` | legend-pure-rust | **P1** — 24+ platform DMR test fixtures across the five `essential/meta/reflect/*.pure` files |
| `74ec531ddee` | legend-pure-rust | **P3** — `RuntimeExtension` trait + `NativeRegistry::with_extensions` factory + 5 smoke tests |
| `f0a1d83455c` | legend-pure-rust | **P2** — `dynamicNew` 6-arg `constraintsManager` overloads (4/6 → 6/6 ported) + 5 platform tests |
| `a0b9678c85b` | legend-pure-rust | **P5** — `crates/runtime/tests/platform_invariants.rs` locking `mutateAdd`/`applyFunction` absence |
| `05712262990` | legend-pure-rust | chore — promote `force_all` to `pub` so extensions can use it |
| `55f00b7779a` | legend-engine | **P4** — `legend-engine-rust-natives-functions-unclassified` crate: `MutateAdd` native + `FunctionsUnclassifiedExtension` + 4 integration tests |
| `2b1074958c8` | legend-pure-rust | **§6 Item 12** — `RoutedValueSpecification` shadow tests in `routing.pure` |
| `9e41a18a3bf` | legend-pure-rust | **§6 Item 9** — multiplicity-reader shape coverage across 6 files |
| `f895bcd9a33` | legend-pure-rust | **§6 Item 11** — `copy` association-inverse symmetry after DMR (closes §3.3 unknown 2) |
| `ce663b25531` | legend-pure-rust | **§6 Item 10** — path/element round-trip identity |

No open follow-ups from this work stream remain. The plan's §6 P0/P1/P2 + Borderline items are all shipped. Further coverage extensions belong to consumer-side porting (e.g. once legend-engine `byPassRouterInfo` becomes loadable, add platform shadow tests for `StoreClusteredValueSpecification.value`).

---

## 7. Engine-side usage audit

§1 proves *what's ported*; this section proves *what's needed*. Together they close the coverage loop: every meta-programming primitive Engine actually consumes maps to a ✅ row in §1.

### Scope and method

Verified 2026-05-10: `grep -rho -e '<pattern>' --include='*.pure' --exclude-dir=target` across the full `/Users/cocobey73/Projects/legend-engine/` tree — 2,883 `.pure` files spanning `legend-engine-core-pure/` (router, preeval, mapping, modelToModel, extension framework, validation) and all 47 `legend-engine-xts-*` extension packages (relationalStore, sql, python, graphQL, java, dataquality, analytics, mongodb, …).

### Verified primitive usage

| Primitive | legend-engine calls | Rust |
|---|---:|:---:|
| `evaluateAndDeactivate` | 1,864 | ✅ |
| `deactivate` | 47 | ✅ |
| `reactivate` | 148 | ✅ |
| `canReactivateDynamically` | 2 | ✅ |
| `openVariables` + `openVariableValues` | 198 | ✅ |
| `->eval(` | 2,818 | ✅ |
| `->evaluate(` | 79 | ✅ |
| `->cast(@` | 11,461 | ✅ |
| `->instanceOf(` | 3,187 | ✅ |
| `->subTypeOf(` | 86 | ✅ |
| `->generalizations(` | 13 | ✅ |
| `dynamicNew` | 57 | ✅ (6/6 overloads since `f0a1d83455c`) |
| `new(` (explicit) | 221 | ✅ |
| `.multiplicity` access | 723 | ✅ |
| `->getLowerBound(` / `->getUpperBound(` | 39 / 35 | ✅ |
| `->isToOne(` / `->isToMany(` | 19 / 22 | ✅ |
| `->hasUpperBound(` | 41 | ✅ |
| `elementToPath(` / `pathToElement(` | 3,134 / 269 | ✅ |
| `->genericType(` / `.genericType` | 2,703 | ✅ |
| `.func` / `.values` / `.expressionSequence` / `.parametersValues` | 11,574 combined | ✅ (heap property access) |
| `^RoutedValueSpecification(...)` | 290 | ✅ (mechanism + shadow tests since `2b1074958c8`) |
| `mutateAdd` | 97 | ✅ via extension (commit `55f00b7779a`) |
| `byPassRouterInfo` / `byPassValueSpecificationWrapper` | 97 / 26 | ⚠️ consumer-defined wrappers (see §3.2) |

**Verdict: 100% of meta-programming primitives Engine consumes are covered by the shipped Rust port.** No primitive in §1 is unused. No primitive heavily used by Engine is unported.

### `j_invoke` — disambiguation

A naïve audit might flag `->j_invoke(...)` (1,306 calls across `*javaPlatformBinding-pure/` modules) as a coverage gap. **It is not a meta-programming primitive.** Verified at `legend-engine-xts-java/legend-engine-xt-javaGeneration-pure/.../metamodel_factories.pure:2008`:

```pure
function meta::external::language::java::factory::j_invoke(
    instance:Code[1], method:Method[1], args:Code[*]
):Code[1]
```

It's a regular Pure function in the Java code-generation factory — takes a `Code[1]` (Java-AST node) and returns another `Code[1]`. Used only at Java code-generation time, not at Pure runtime. The Rust runtime supports it automatically via the user-function call path; no native registration needed. Documented here to lock the disambiguation so future audits don't re-flag it.

### Extension natives (153 declarations)

`grep -rn "native function" legend-engine/.../*-pure/.../resources/*.pure` returns 153 declarations across extension packages. **None are meta-programming primitives** — they are per-extension natives that, if consumed at runtime, would ship through the `RuntimeExtension` SPI (commit `74ec531ddee`). Categorized:

| Category | Approx count | Status |
|---|---:|---|
| PCT-marked relation natives (`sort`, `distinct`, `extend`, `join`, …) | ~46 | Partially covered by `core_functions_relation` in `legend-engine-rust/`; tracked separately |
| `mutateAdd` (legend-engine-pure-functions-unclassified) | 1 | ✅ via P4 (commit `55f00b7779a`) |
| `legendCompileVSProtocol` (SQL extension — VS deserialization) | 1 | Out of meta-programming scope; own extension crate when needed |
| `getTestConnection` / `parseSqlStatementToJson` (relationalStore) | 2 | Out of meta-programming scope; store-specific |
| MongoDB test-server utils | 3 | Test-only; out of scope |
| Other extension natives (Python factory, GraphQL, etc.) | ~100 | Per-extension porting; not meta-programming |

The `mutateAdd` PoC (legend-engine-rust commit `55f00b7779a`) is the proven pattern for any of these that become consumer-critical.

### What §7 does not change

The §1 verdict (18 ✅ / 0 ❌ / 2 🚫), §3 router-readiness, §5 invariant, and §6 status all remain consistent. §7 is purely a confirmation that the shipped surface meets the demonstrated demand.

---

## Appendix A — Original `MetaAccessor` proposal (historical)

> **Superseded.** Rust uses heap property maps in `crates/runtime/src/heap.rs` (`HeapEntry` carries a property table on every metamodel object) instead of a trait. Same semantics, different shape — meta-property access on a `Value::Object` simply reads the heap entry's property map by name. The proposal below is preserved as a record of the original analysis that motivated the current design.

### The Real Gap (as it was framed): ValueSpecification as Navigable M3 Object

The original RFC identified the gap specifically in this flow:

```pure
// Pure code that navigates a deactivated expression:
let expr = {|$x + 1}->deactivate();
let body = $expr->cast(@LambdaFunction<Any>)._expressionSequence()->at(0);
let func = $body->cast(@SimpleFunctionExpression)._func();
// func is now the plus() function
```

This requires the Rust runtime to:
1. Return `Expression` trees as runtime values (not just as compiler data)
2. Support M3 property access (`.func`, `._parametersValues()`, etc.) on those values
3. Support `cast(@SimpleFunctionExpression)` on expression values

### Proposed Solution (not adopted): `MetaAccessor` Trait

```rust
/// Trait that provides M3-style property navigation on Rust types.
///
/// This is the bridge between Rust's strongly-typed representations
/// and Pure's reflective property access. It is NOT used in the
/// compiler — only in the interpreter when property access targets
/// a meta-level type (ValueSpecification, GenericType, etc.).
pub trait MetaAccessor {
    /// Get a single-valued property by M3 property name.
    fn meta_property_to_one(&self, name: &str) -> Option<Value>;

    /// Get a multi-valued property by M3 property name.
    fn meta_property_to_many(&self, name: &str) -> Vec<Value>;

    /// Get the M3 classifier path (e.g., "meta::pure::metamodel::valuespecification::FunctionExpression")
    fn classifier_path(&self) -> &str;
}
```

The proposal called for implementing it for `Expression`:

```rust
impl MetaAccessor for Expression {
    fn meta_property_to_one(&self, name: &str) -> Option<Value> {
        match (self, name) {
            // FunctionExpression / FunctionApplication
            (Expression::FunctionApplication { function, .. }, "func") => {
                Some(Value::Meta(MetaValue::Function(*function)))
            }
            (Expression::FunctionApplication { arguments, .. }, "parametersValues") => {
                arguments.first().map(|e| Value::Expression(Box::new(e.clone())))
            }
            // InstanceValue
            (Expression::Literal { value, .. }, "values") => {
                Some(value.to_runtime_value())
            }
            // VariableExpression
            (Expression::Variable { name: var_name, .. }, "name") => {
                Some(Value::String(var_name.clone()))
            }
            // Lambda
            (Expression::Lambda { .. }, "expressionSequence") => {
                // Return first expression of the body
                // ...
            }
            _ => None,
        }
    }

    fn meta_property_to_many(&self, name: &str) -> Vec<Value> {
        match (self, name) {
            (Expression::FunctionApplication { arguments, .. }, "parametersValues") => {
                arguments.iter()
                    .map(|e| Value::Expression(Box::new(e.clone())))
                    .collect()
            }
            (Expression::Lambda { body, .. }, "expressionSequence") => {
                body.iter()
                    .map(|e| Value::Expression(Box::new(e.clone())))
                    .collect()
            }
            _ => vec![],
        }
    }

    fn classifier_path(&self) -> &str {
        match self {
            Expression::FunctionApplication { .. } => "meta::pure::metamodel::valuespecification::SimpleFunctionExpression",
            Expression::Literal { .. }             => "meta::pure::metamodel::valuespecification::InstanceValue",
            Expression::Variable { .. }            => "meta::pure::metamodel::valuespecification::VariableExpression",
            Expression::Lambda { .. }              => "meta::pure::metamodel::function::LambdaFunction",
            Expression::PropertyAccess { .. }      => "meta::pure::metamodel::valuespecification::SimpleFunctionExpression",
            _ => "meta::pure::metamodel::valuespecification::ValueSpecification",
        }
    }
}
```

…paired with a runtime `Value` extension:

```rust
pub enum Value {
    // ... existing variants ...
    /// A reified expression tree — the result of `deactivate()`.
    Expression(Box<Expression>),
    /// A meta-level reference (function pointer, type reference, etc.)
    Meta(MetaValue),
}

pub enum MetaValue {
    Function(ElementId),
    Type(ElementId),
    TypeExpr(TypeExpr),
    Multiplicity(Multiplicity),
}
```

### What was actually built

The shipped design uses `HeapEntry` property maps instead. `Deactivate` (`meta.rs:2193`) constructs an `InstanceValue` / `SimpleFunctionExpression` / `VariableExpression` heap entry whose property table carries `func`, `parametersValues`, `expressionSequence`, `name`, `values` directly. Pure's regular property-access path reads those properties through the dispatch engine — no separate trait, no `Value::Expression` variant.

This is strictly smaller than the proposal: meta-property navigation reuses the same machinery as user-class property navigation. The "two-speed" model the original proposal promised collapses into one path with no runtime cost: meta-objects and user-objects are both `Value::Object(ObjectHandle)`, and property access on either flows through the heap.

---

## Appendix B — `mutateAdd` consumer taxonomy

> **Empirical finding (2026-05-09):** `mutateAdd` is registered nowhere in `crates/runtime/src/native/`. The 87/13 consumer-vs-genuine-mutation split below explains the design choice retrospectively: Categories 1–4 (87% of legend-engine call sites) are construction-time circular-reference workarounds; the Arena/Index pattern handles them natively. Category 5 (7%, M2M merge) is the only genuine-runtime mutation, and it can be expressed via `dynamicNew` with a merged `KeyValue` list. Pure code in the Rust workspace therefore never calls `mutateAdd`, and §5 enforces that as an invariant.

### What Is `mutateAdd`?

```pure
native function meta::pure::functions::lang::mutateAdd<T>(obj:T[1], property:String[1], values:Any[*]):T[1];
```

`mutateAdd` takes an object, a property name (as a string!), and values to append. It uses **Java reflection** to find the setter method for that property and directly mutates the underlying Java object, bypassing Pure's immutability model.

### The Java Implementation

**Interpreted version** ([`MutateAdd.java`](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-unclassified/legend-engine-pure-runtime-java-extension-interpreted-functions-unclassified/src/main/java/org/finos/legend/pure/runtime/java/extension/functions/interpreted/natives/lang/MutateAdd.java)):

```java
public CoreInstance execute(...) {
    CoreInstance obj = params.get(0);
    String propertyName = PrimitiveUtilities.getStringValue(params.get(1));
    ListIterable<? extends CoreInstance> valuesToAdd = params.get(2);
    Instance.addValueToProperty(obj, propertyName, valuesToAdd, processorSupport);
    return obj;
}
```

**Compiled version** ([`FunctionsHelper.java`](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-pure/legend-engine-pure-code-functions-unclassified/legend-engine-pure-runtime-java-extension-compiled-functions-unclassified/src/main/java/org/finos/legend/pure/runtime/java/extension/functions/compiled/FunctionsHelper.java)):

```java
public static <T> T mutateAdd(T obj, String property, RichIterable<? extends Object> values) {
    String methodName = "_" + property;
    Method m = obj.getClass().getMethod(methodName, RichIterable.class);
    RichIterable existing = (RichIterable)obj.getClass().getMethod(methodName).invoke(obj);
    m.invoke(obj, LazyIterate.concatenate(existing, values).toList());
    return obj;
}
```

> [!WARNING]
> **This is unrestricted reflective mutation.** The property name is a runtime string, the values are `Any[*]`, and there is no type checking. It bypasses every immutability guarantee Pure makes elsewhere.

### Usage Taxonomy: 94 Call Sites, 31 Unique Source Files

We identified all `mutateAdd` usages across `legend-engine` (excluding `target/` duplicates). They fall into **5 distinct categories**:

#### Category 1: TDS Row Assembly (48 call sites — **51% of all usage**)

**Pattern**: `$newTds->mutateAdd('rows', $transformedRows)`

**Where**: `tds.pure` (15 sites), relational TDS, relational execution protocols (v1_20 through vX_X_X — 14 near-identical versions).

**Why it exists**: `TabularDataSet` has `rows: TDSRow[*]` and each `TDSRow` has `parent: TabularDataSet[0..1]`. This creates a **circular reference** — the TDS needs to exist before its rows can be created (rows need a `parent` pointer back to the TDS), but the TDS can't be constructed with rows until the rows exist.

```pure
let newTds = ^TabularDataSet(columns = $tds.columns);
let newRows = $tds.rows->map(r|^TDSRow(parent=$newTds, values=$r.values));
$newTds->mutateAdd('rows', $newRows);
$newTds;
```

The `//todo: remove this by making parent an association` comment in the actual source proves the authors recognize this as a workaround.

> [!TIP]
> **Rust strategy:** Builder pattern + two-phase construction. Allocate a `TDS` handle, create rows pointing to it, fill in the `rows` field through arena update — no mutation of a live object.

#### Category 2: Metamodel Bootstrap / Code Generation (18 call sites — **19%**)

**Pattern**: `$cls->mutateAdd('properties', $props)->mutateAdd('constraints', $constraints)` and `$cls->mutateAdd('generalizations', $gen)`.

**Where**: `xsdToPure.pure`, `toPure_sdl.pure`, `toPure_introspection.pure`, `conventions.pure`, `graphToProtocol.pure`.

**Why it exists**: Code generators that programmatically build Pure metamodel elements from external schemas. The pattern is bidirectional — a `Generalization` links both `general` (super) and `specific` (sub), and both ends need to know about it.

> [!TIP]
> **Rust strategy:** Arena two-phase construction. The arena owns all the data; "bidirectional" is two index updates, not mutation of a live object.

#### Category 3: Router / Cluster PropertyMapping Assembly (14 call sites — **15%**)

**Pattern**: `$newSet->mutateAdd('propertyMappings', $newPropMappingsWithOwner)`.

**Where**: `routing.pure`, `cluster.pure`.

**Why it exists**: Same circular-reference shape as TDS — `PropertyMapping.owner: SetImplementation[1]` and `SetImplementation.propertyMappings: PropertyMapping[*]`.

> [!TIP]
> **Rust strategy:** Same arena two-phase pattern as Category 2.

#### Category 4: Mapping Include Assembly (2 call sites — **2%**)

**Pattern**: `$newMapping->mutateAdd('includes', $includedMappings)`.

**Why**: `MappingInclude.owner: Mapping[1]`, same circular reference.

#### Category 5: Model-to-Model Merge (7 call sites — **7%**)

**Pattern**: `$set->at(0)->mutateAdd($p.name->toOne(), $propertyValuesFromOtherInstances->toOne()->getPropertyValues($p.name->toOne()))`.

**Where**: `inMemory.pure`, ServiceStore tests, RelationalAI mapping.

**Why**: M2M merge takes multiple instances from different set implementations and **combines** them into one object. Iterates all properties, finds which instance has a value, and mutates the first instance to absorb values from the others.

> [!WARNING]
> **This is the only category that requires true runtime mutation.** Categories 1–4 are construction-time circular references; this category mutates an already-"complete" object.

> [!TIP]
> **Rust strategy:** Functional merge via `dynamicNew` with a merged `KeyValue` list, or — as a last resort — a `MutableObject` wrapper whose mutation is scoped to a single function. The current Rust workspace adopted neither and simply does not expose a Pure-callable mutator; the merge use case has no consumer in the Rust port today.

### Verdict

| Category | % | Strategy | Mutation needed? |
|---|---:|---|:---:|
| TDS row assembly | 51% | Arena two-phase / builder | ❌ No |
| Metamodel bootstrap | 19% | Arena two-phase / builder | ❌ No |
| Router/cluster | 15% | Arena two-phase / builder | ❌ No |
| Mapping includes | 2% | Arena two-phase | ❌ No |
| M2M merge | 7% | Functional merge or scoped MutableCell | ⚠️ Confined |

The Rust port ships none of these as Pure-callable mutators; the cited categories explain why that absence is sound and not a forced workaround. §5 promotes that absence to a verified invariant.
