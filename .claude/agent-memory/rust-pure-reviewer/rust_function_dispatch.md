---
name: Rust function dispatch
description: How resolve_function_call narrows overloads, what the scoring ranges are, where false ambiguity comes from
type: project
---

## Entry points (see `crates/pure/src/resolve.rs`)

- **`resolve_element_ptr(ptr, ..)`** — exact mangled-name lookup. Used for type refs, annotations (stereotypes, tagged values), property access, `PackageableElementRef`. Does NOT dispatch.
- **`resolve_function_call(ptr, arg_count, lowered_args, ..)`** — simple-name + overload dispatch. Used ONLY for `FunctionApplication` and `ArrowFunction` lowering in `crates/pure/src/lower.rs`.

Confusing these two entry points is the most common dispatch bug — keep them mentally separate.

## "Lower first" pattern

Arguments are lowered to `ValueSpec` **before** dispatch, so the resolver can structurally infer arg types + multiplicities. Parameter-count filtering uses the *AST* arg count (`e.arguments.len()`), not the lowered count — an argument that failed to lower still counts toward the intended arity.

## Scoring (see `narrow_candidates_by_type`)

Two phases:

1. **Filter** — eliminate candidates whose params are incompatible with inferred args. Unknown arg type OR mult = treated as "compatible" (can't eliminate). If filtering empties the set, fall back to original candidates so the ambiguity error surfaces.
2. **Rank** — score by per-param specificity:
   - Exact type = +3, subtype = +1, Any/Generic = 0 (uncapped sum)
   - Exact mult = +4, else specificity-of-param: `[1]=4, [0..1]=3, [1..*]=2, [*]=1, bounded-range=3`
   - Highest total wins; all ties at best score are returned — caller reports ambiguity if more than one.

## Known compromises (BACKLOG.md top of the list)

- **Generic type parameters (`T`, `V`) are treated as `Any`** — no unification. This causes *false matches*; real Java uses `TypeInferenceObserver` + backtracking. 243 remaining errors (161 AmbiguousImport, 75 UnresolvedElement, 6 ParseFailure, 1 DuplicateElement) as of last tracked baseline.
- **Lambda parameter types not inferred from the surrounding `Function<{...}>` signature** — deferred.
- **Numeric coercion (`Integer → Float`) is not modeled** — deferred.
- **Return-type-driven narrowing** (expected type influences dispatch) — deferred.

## Heuristic in `infer_type_from_valuespec`

For unresolved function calls (e.g., `toOne`, `cast`), the inferred type falls back to the type of the **first argument** — many M3 functions are type-preserving. This is an explicit heuristic, not a bug; it turns several otherwise-ambiguous chains into unique matches. Be careful if removing it.

## Why: reviewer needs this

- Any PR touching the overload scoring table MUST be scored against the 243-error baseline — regressions here are the most load-bearing semantic metric.
- False matches from Generic-as-Any are the known escape hatch keeping error count from ballooning. A PR that tightens this without adding real unification will increase errors even though it looks "more correct."

## How to apply

- When reviewing dispatch changes, ask: "Does this maintain or improve the 243-error baseline? How is it measured?"
- Push for dispatch-decision logging (BACKLOG P1 item) on any PR that rebalances scores — otherwise root-cause analysis of regressions is guessing.
