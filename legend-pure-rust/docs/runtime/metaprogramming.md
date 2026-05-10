# Pure Meta-programming — Rust Port Assessment

> **Status (2026-05-09):** Surveyor **246/0/0**, PCT **465/465**. Rust is at parity with Java for every Pure-callable meta-programming primitive surveyed below, except for two `dynamicNew` overloads and three router-side wrappers that live in `legend-engine` (consumer code, not platform).
>
> The original RFC framing of this document — *can the Rust Arena/Index architecture support Pure metaprogramming?* — is settled: yes, it does. The original analysis is preserved as Appendix A (`MetaAccessor` proposal) and Appendix B (`mutateAdd` consumer taxonomy).

## Contents

1. [Parity matrix (Rust ↔ Java)](#1-parity-matrix-rust--java)
2. [Consumer-pressure ranking](#2-consumer-pressure-ranking)
3. [Router-readiness gap list](#3-router-readiness-gap-list)
4. [Platform-completeness reference](#4-platform-completeness-reference)
5. [Invariant: read-only meta-programming for Pure user code](#5-invariant-read-only-meta-programming-for-pure-user-code)
6. [Platform coverage owed (proposed test suite)](#6-platform-coverage-owed-proposed-test-suite)
7. [Appendix A — Original `MetaAccessor` proposal (historical)](#appendix-a--original-metaaccessor-proposal-historical)
8. [Appendix B — `mutateAdd` consumer taxonomy](#appendix-b--mutateadd-consumer-taxonomy)

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
| `dynamicNew(…, getterToOne, getterToMany, hiddenPayload, constraintsManager):Any[1]` | `dynamicNew.pure:26-27` | ❌ | — | The two **constraints-manager** overloads are unregistered. Surveyor today never exercises them; if router does, this is a router blocker. |
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
| `mutateAdd<T>(T[1], String[1], Any[*]):T[1]` | (legend-engine consumer-defined; not platform) | 🚫 | — | Deliberately not registered as a Pure native. Internal `RuntimeHeap::mutate_add` (`heap.rs:224`, 462) is used by `Reactivate` and `Copy`, but unreachable from Pure source. See §5 invariant. |
| `applyFunction(Function, Any[*])` | n/a | 🚫 | — | Java-internal reflective-array dispatch; never surfaced to Pure source. Subsumed by `eval`/`evaluate`. |

**Verdict:** 17 of 20 platform-callable rows ✅, 1 ❌ (the two `dynamicNew` constraints-manager overloads), 2 🚫 (`mutateAdd`, `applyFunction`). Locked by Surveyor 246/0/0 and PCT 465/465 with 9 manifest exclusions, none of which touch this surface.

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

- **`dynamicNew` 6-arg `constraintsManager` overloads** — declared `essential/lang/creation/dynamicNew.pure:26-27`, unregistered in Rust (`lang.rs:2252-2270` covers only 4 of 6 forms). Detection: `grep -rn "dynamicNew(.*constraints" legend-engine/`. If any router transform uses constraint-managed dynamicNew, this is a hard blocker; otherwise low-risk, defer until a failing test demands it. Tracked as a §6 item-5 test.

### 3.2 Consumer-defined wrappers (in scope)

These are platform-ready in mechanism (heap property access works) but the wrapper functions themselves live in legend-engine:

- **`RoutedValueSpecification.value` access** — the wrapping class is platform (17-line definition at `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/routing.pure:15`). Heap-property access to `.value` works today via the same path that powers `expressionSequence` reads. Status: ✅ mechanism in place, untested without legend-engine. §6 item 9 proposes a platform shadow test that locks this.
- **`byPassRouterInfo()`** — defined in `legend-engine/.../router/routing/router_routing.pure`; unwraps a `RoutedValueSpecification` to expose the inner VS. A 1–2 line walk over `.value` chains. Once legend-engine .pure is loadable in the Rust workspace, this becomes a Pure-defined function with no native dependency.
- **`byPassValueSpecificationWrapper()`** — defined in `legend-engine/.../router/store/cluster.pure`; same shape, strips a `StoreClusteredValueSpecification` wrapper. Same disposition.

### 3.3 Behavioural unknowns

- **`Reactivate` cross-chunk `ElementId` correctness** — believed working (the `meta.rs:2774` `reactivate_value` walker resolves through the model BiMap), but no router-PCT coverage exists. §6 item 1 (`testReactivate.pure`) closes this.
- **`copy` association-inverse symmetry under router rewrites** — `lang.rs:1018-1082` claims parity with Java `Copy.java:236`, but the platform has no test that exercises a router-shaped DMR cycle on a class with bidirectional Associations. §6 item 7 closes this.

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
- [native] `dynamicNew` (4 of 6 overloads — see §1 ❌) · `copy` · `eval` (0–7 args) · `evaluate` · `new` · `letFunction`

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

This invariant has exactly one failure mode: someone registers `mutateAdd` (or any equivalent reflective setter) as a Pure-callable native. That is now a code-review checkpoint — not an architectural unknown — and §6 item 8 proposes a negative test that would fail loudly if a reflective-mutation native ever ships.

---

## 6. Platform coverage owed (proposed test suite)

### Today's coverage (verified 2026-05-09)

The reflection surface is paradoxically the *least*-covered corner of the platform despite being the most consumer-pressured. Counts of `function <<test.Test>>` per native's `.pure` source file:

| Primitive | Platform `.pure` `<<test.Test>>` | Java `AbstractTest*` | Verdict |
|---|---:|---:|:---:|
| `deactivate` | **0** | 0 | ❌ |
| `reactivate` | **0** | 5 (scope/binding edges only) | ⚠️ |
| `evaluateAndDeactivate` | 2 | 0 | ⚠️ |
| `canReactivateDynamically` | 3 | 0 | ⚠️ |
| `openVariableValues` | 4 | 0 | ⚠️ |
| `dynamicNew` | 10 | 0 | ✅ |
| `eval` (0–7) | 7 | 0 | ✅ |
| `evaluate` | 10 | 0 | ✅ |
| `cast` | 11 | 0 | ✅ |
| `instanceOf` | 9 | 0 | ✅ |
| `subTypeOf` | 24 (incl. `_subTypeOf.pure`) | 0 | ✅ |
| `generalizations` | 15 | 0 | ✅ |
| `pathToElement` | 6 | 10 | ✅ |
| `elementToPath` | 11 | 0 | ✅ |

The DMR-cycle primitives (`deactivate`, `reactivate`, `evaluateAndDeactivate`) carry the heaviest consumer load (§2 ranks them #2 and #3 — 286 router calls for `evaluateAndDeactivate` alone) yet the thinnest direct test surface. `deactivate.pure` ships **zero** `<<test.Test>>` functions; `reactivate.pure` ships **zero** and the 5 Java `AbstractTestReactivate` `@Test`s only cover variable-scope edges (`testVariableScopeSuccess`, `testVariableScopeWithEmpty`, …), not round-trip identity, not DMR cycles, not cross-chunk reactivation.

### Authoring guidance

> [!IMPORTANT]
> **Tests must be Pure-side `<<test.Test>>` functions, run via PCT.** They live in platform `.pure` source, are picked up by both Java and Rust stacks (Surveyor walks the .pure tree, no Rust code needed), and stay backend-agnostic. Java `AbstractTest*` or Rust integration tests are *borderline* — acceptable only for assertions that genuinely cannot be expressed in Pure (e.g. proving a native is *not registered*, where the test must observe a compile-time error from outside the language). Default to Pure; reach for the harness only when Pure can't express the assertion.

### Proposals, prioritized by current gap

#### P0 — primitives with zero direct platform coverage

These ship today with no `<<test.Test>>` in the same file as the native; they are the highest-risk regressions.

1. **`testDeactivate.pure`** *(new file from zero, sibling to `deactivate.pure`)* — deactivate of literals, lambdas, `FunctionExpression`s, `VariableExpression`s, nested `^Class(prop=lambda)`. Assert `instanceOf(InstanceValue)` / `instanceOf(SimpleFunctionExpression)` discrimination on the result.
2. **`testReactivate.pure`** *(new file from zero, sibling to `reactivate.pure`)* — round-trip identity (`deactivate->reactivate == original`), reactivate with open-variable map, **reactivate of expressions referencing cross-chunk `ElementId`s** (closes §3.3 unknown 1). Borrowing the variable-scope shapes already covered by `AbstractTestReactivate.java` is fine, but the Pure form is canonical.
3. **`testDeactivateMutateReactivate.pure`** *(new file from zero — DMR pattern, dominant consumer shape, 286 router occurrences)* — take `{|$x + 1}`, deactivate, navigate `expressionSequence->at(0)->cast(@SimpleFunctionExpression).parametersValues`, swap the literal `1` for `2`, reactivate with `$x = 5`, assert result `7`. Locks the property-walk-on-VS path that Appendix A's `MetaAccessor` design speculated about.

#### P1 — primitives with thin direct coverage

Existing fixtures present but small (2–4 tests). Extend in-place rather than authoring new files.

4. **Extend `evaluateAndDeactivate.pure` fixtures** — current 2 tests grow to cover: operand evaluated-first ordering, type+multiplicity preservation across the boundary, nested `evaluateAndDeactivate(evaluateAndDeactivate(x))` idempotence.
5. **Extend `canReactivateDynamically.pure` fixtures** — current 3 grow to assert `false` on lambdas with truly unbound free vars vs. lambdas whose open-vars are bound by an enclosing `let`.
6. **Extend `openVariableValues.pure` fixtures** — current 4 grow to cover cross-scope captures, captures over `let` chains, captures of collection-typed bindings.
7. **VS-subtype property walks** *(new file `meta/reflect/testValueSpecificationProperties.pure`)* — `cast(@FunctionExpression).func`, `.parametersValues`, `cast(@LambdaFunction).expressionSequence`, `cast(@VariableExpression).name`. One test per VS subtype. Catches heap-property-access regressions before they hit consumers.

#### P2 — gaps already adjacent to working fixtures

8. **Extend `dynamicNew.pure` overload matrix** — current 10 tests grow to cover all 6 declared overloads explicitly; the two `constraintsManager` forms (§1 ❌ row) will fail today, pinning the gap to a concrete failing test rather than a hand-written matrix entry.
9. **Extend `getLowerBound.pure` / `getUpperBound.pure` / `isToOne.pure` / `isToMany.pure` / `hasUpperBound.pure` / `hasToOneUpperBound.pure` fixtures** — current 1–3 tests each grow to cover all five canonical shapes `[0..1]`, `[1]`, `[1..*]`, `[*]`, `[m..n]`.
10. **Extend `pathToElement.pure` / `elementToPath.pure` round-trips** — current fixtures cover happy paths; add `pathToElement(elementToPath($e)) == $e` over every `PackageableElement` kind (Class, Function, Profile, Association, Measure/Unit, primitive), plus `lenientPathToElement` returning `[]` on misses.
11. **`copy` association-inverse symmetry** *(new file `lang/creation/testCopyAssociationInverse.pure`)* — copy on a class with bidirectional Association, assert both ends still see each other after the copy. The Java `Copy.java:236` parity contract; `lang.rs:1018-1082` claims it but lacks an assertable platform test (closes §3.3 unknown 2).
12. **Router shadow** *(new file `meta/reflect/testRoutedValueSpecification.pure`)* — wrap an expression in `^RoutedValueSpecification(value = $vs)`, navigate `.value`, assert reactivate-through-wrapper works. Covers the platform-side router class without depending on legend-engine. `byPassRouterInfo` / `byPassValueSpecificationWrapper` shadows stay consumer-defined and are *not* added to platform.

#### Borderline — invariant lock-in (harness, not Pure)

13. **Negative test: `mutateAdd` not platform-callable.** This asserts an *absence* — a Pure-source `mutateAdd(...)` call should fail with `UnknownFunction` at compile time when the unclassified-functions extension is *not* loaded. Pure can express this via `assertError(|expr-that-references-mutateAdd, 'UnknownFunction')` if such testing primitives exist; otherwise it lives as a Rust integration test in `crates/runtime/tests/` (or Java parity equivalent). Either way, locks the §5 invariant — a regression that registers `mutateAdd` as a platform native breaks this test loudly.
14. **Negative test: `applyFunction` absent from platform.** Same shape as 13.

### How they run

Pure-side `<<test.Test>>` functions are picked up automatically:
- **Rust:** Surveyor walks the platform `.pure` tree; counts climb from 246/0/0 to (246 + N)/0/0 with no Rust code change.
- **Java:** the existing PCT runner discovers them through the same annotation.
- **CI:** both stacks fail loudly on regressions, no consumer-side coordination needed.

Borderline harness tests (#13, #14) live in their respective backend's test crate.

Sequencing: §6 is a *plan*. P0 first (3 new files), P1 next (extend 3 fixtures, author 1 new), then P2 / borderline as bandwidth allows.

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
