# Pure Compiler — Backlog & Deferred Work

Tracking document for known gaps, deferred tasks, and future improvements
in the Pure compiler crate. Each item includes rationale for deferral and
dependencies, so future contributors know what's safe to pick up.

## Priority Legend

- **P0** — Blocking correctness; next up
- **P1** — Important for completeness; near-term
- **P2** — Architectural improvement; medium-term
- **P3** — Nice-to-have; long-term

---

## Function Dispatch

> See [FUNCTION_DISPATCH.md](./FUNCTION_DISPATCH.md) for the full design.

| Item | Priority | Status | Notes |
|---|---|---|---|
| Param count filtering | P0 | ✅ Done | `f.parameters.len() == arg_count` |
| Pass 2a/2b split (signatures before bodies) | P0 | ✅ Done | Signatures resolved before bodies |
| Type-compatible matching | P0 | ✅ Done | Exact + subtype scoring via `is_type_compatible` |
| Multiplicity narrowing | P0 | ✅ Done | `is_multiplicity_compatible` + specificity scoring |
| Subtype matching (`Integer` → `Number`) | P0 | ✅ Done | `is_subtype` walks super_types chain |
| Variable type tracking | P0 | ✅ Done | Function params, let bindings, lambda params tracked in `ResolutionContext.variable_types`. |
| Generic params treated as `Any` | P1 | ⚠️ Compromise | Correct for now; causes false matches |
| Generic unification (`Z` propagation) | P2 | 🔲 Deferred | Complex; Java uses `TypeInferenceObserver` |
| Lambda parameter type inference | P2 | 🔲 Deferred | Infer from expected `Function<{...}>` type |
| Numeric coercion (`Integer` → `Float`) | P3 | 🔲 Deferred | Java has implicit widening |
| Return type influence on dispatch | P3 | 🔲 Deferred | Expected return type narrows candidates |

---

## Bootstrap / Chunk 0

| Item | Priority | Status | Notes |
|---|---|---|---|
| Move chunk 0 to compile-time | P2 | 🔲 Deferred | Currently built at runtime via `create_bootstrap_chunk()`. Could be a `const` or `lazy_static` built from m3.pure at compile time (build.rs or proc-macro). Saves ~1ms startup per compilation. |
| M3 parser completeness | P1 | 🔲 Partial | `m3_parser.rs` handles classes/enums/associations from m3.pure. Missing: constraints, qualified properties, derived properties. |
| M3 property types | P1 | 🔲 Partial | M3 class properties are registered with placeholder `Any` types. Should resolve actual types from m3.pure definitions. |

---

## Units & Measures

| Item | Priority | Status | Notes |
|---|---|---|---|
| Unit as child of Measure | P1 | 🔲 Deferred | Units are currently promoted to package-level elements (`Element::Unit`). In M3, they're children of their parent Measure. Resolution uses `Measure~Unit` naming convention which works but is non-canonical. |
| Unit conversion functions | P2 | 🔲 Deferred | `convert(value, sourceUnit, targetUnit)` — needs conversion factor storage. |
| Canonical unit references | P1 | 🔲 Deferred | Parser emits `MeasureName~UnitName` for unit refs in expressions. This works for resolution but should follow M3's `Measure.canonicalUnit` pattern. |

---

## Expression Lowering

| Item | Priority | Status | Notes |
|---|---|---|---|
| Root package `::` references | P1 | 🔲 Open | 16 errors. `::meta::pure::...` paths starting with `::` need root-anchored resolution. |
| Lambda variable scope | P1 | 🔲 Open | 6+ errors. Lambda params (`e`, `d`, etc.) not in variable scope during body lowering. |
| Package-as-value references | P1 | 🔲 Open | 5+ errors. Qualified package paths used in expression context (e.g., `meta::pure::functions::meta`). |
| `^ClassName(typeArgs)(props)` constructor | P1 | 🔲 Open | Parse failures. Dual-paren new syntax for classes with type variable constructors. |
| Variable type tracking | P0 | 🔲 Next | Needed for type dispatch. `HashMap<SmolStr, TypeExpr>` in lowering context. |

---

## Type System

| Item | Priority | Status | Notes |
|---|---|---|---|
| Type hierarchy walk (subtype check) | P0 | ✅ Done | `is_subtype(child, parent)` following `super_types` chain. |
| Multiplicity compatibility | P0 | ✅ Done | `is_multiplicity_compatible` + `mult_bounds` + `mult_specificity`. |
| Type inference (bottom-up) | P2 | 🔲 Deferred | Pass 2.5 in pipeline doc. Infer expression types. |
| Constraint evaluation | P3 | 🔲 Deferred | Class constraints need expression evaluation at validation time. |

---

## Pipeline Architecture

| Item | Priority | Status | Notes |
|---|---|---|---|
| Pass 2a/2b split | P0 | ✅ Done | Signatures resolved in 2a, bodies in 2b. |
| Parallel Pass 2 | P3 | 🔲 Deferred | After 2a/2b, bodies can be parallelized per-element. |
| Incremental compilation | P3 | 🔲 Deferred | Re-resolve only changed chunks. Chunk IDs enable this without rewriting. |

---

## Developer Experience / Observability

| Item | Priority | Status | Notes |
|---|---|---|---|
| Compilation tracing (`tracing` crate) | P1 | 🔲 Planned | Add `tracing` instrumentation to pipeline passes, function dispatch, expression lowering, and type narrowing. Enable via `RUST_LOG=legend_pure_parser_pure=debug`. Shows pass timing, dispatch decisions, candidate narrowing, and resolution fallback paths. |
| Dispatch decision log | P1 | 🔲 Planned | Log each `resolve_function_call`: function name, arg count, candidates found, type-narrowed set, final pick or error. Critical for debugging false ambiguity / false elimination. |
| Pass timing | P2 | 🔲 Planned | `tracing::info_span!` on each pipeline pass (1, 1.5, 2a, 2b, 2.5, 3) with element count and duration. |
| Error source chain | P2 | 🔲 Planned | For cascading errors (arg lowering fails → function unresolved), link parent error to child cause. |

---

## Error Reporting

| Item | Priority | Status | Notes |
|---|---|---|---|
| Error count baseline | — | 243 | 161 ambiguous, 75 unresolved, 6 parse, 1 duplicate |
| Target after metaclass inference | — | ~100 | M3 metaclass types (Type, Class, GenericType) resolve elementToPath/dynamicNew |
| Target after expression fixes | — | ~7 | Only parse failures + duplicates remain |

---

## How To Use This Document

1. **Pick an item** matching your skill and the current priority
2. **Check dependencies** — items marked "Blocked" need their prerequisites done first
3. **Update status** when you start/finish work
4. **Add new items** as you discover gaps during development
