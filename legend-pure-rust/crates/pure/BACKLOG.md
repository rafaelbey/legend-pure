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
| Generic params treated as `Any` | P1 | ✅ Done | Substitution wired end-to-end: `infer_generic_bindings` collects T+m bindings, `bind_type` recurses through nested generics with type-LUB on conflict, `substitute_type` and `substitute_mult` rewrite the candidate's return signature, `infer_function_call` (infer.rs) substitutes BOTH type AND multiplicity at FunctionCall return positions (the multiplicity half was missing — without it, `f<T\|m>(T[m]):T[m]` left return-mult as `Variable(m)` and downstream `is_multiplicity_compatible` accepted any declared decl permissively). Property-access path uses its own `compute_type_arg_bindings` + `substitute_class_generics` for class-level T substitution. Locked by `crates/pure/tests/integration_tests.rs::generic_subst_*` (14 cases incl. head, two-arg LUB, multiplicity threading, chained-receiver substitution). |
| Generic return-type flow through `T[m]` methods | P1 | ✅ Done | Root cause was *inherited property lookup*: `infer_type_from_valuespec`'s `PropertyAccess` branch didn't walk the supertype chain, and the M3 parser stored property types as `TypeExpr::Generic("ClassName")` never resolved to `Named`. Extended `resolve_m3_supertypes` to resolve property types too, and added a supertype walk when looking up a property. After fix, `.package->at(0)` flows through `at<T\|m>(T[m],...)` correctly. Unblocked `testPackageablesToPath`/`testEnumerationToPath`. |
| `TypeExpr::Named.multiplicity_arguments` for class-level mult-vars | P1 | 🔲 Deferred | AST `TypeReference` carries `multiplicity_arguments` (`crates/ast/src/type_ref.rs:449`) but the resolved `TypeExpr::Named` only has `type_arguments` + `value_arguments` (`crates/pure/src/types.rs:53-64`). So `Class Holder<T\|m> { items: T[m]; }` referenced as `Holder<String\|*>[1]` parses cleanly, but the `*` mult-arg is dropped during lowering. Without it, `compute_type_arg_bindings` (infer.rs:1327) only builds `T → String`, never `m → *`. Property `items: T[m]` substitutes T but leaves m as `Variable("m")`, and `is_multiplicity_compatible` swallows it permissively. Fix requires: (a) add `multiplicity_arguments: Vec<Multiplicity>` to `TypeExpr::Named`, (b) thread through every constructor (~20 sites — pipeline, fqn, m3_parser, lower, runtime), (c) extend `compute_type_arg_bindings` to also map Class.multiplicity_parameters, (d) add a `substitute_class_mults` helper next to `substitute_class_generics`. Locked by `#[ignore]`'d test `generic_subst_property_chain_with_mult_variable_decl_one_errors`. |
| Generic unification (`Z` propagation) | P2 | 🔲 Deferred | Complex; Java uses `TypeInferenceObserver` |
| Lambda parameter type inference from `Function<{T->X}>` shape | P1 | 🔲 Deferred | Today the second-pass in `infer_generic_bindings` (resolve.rs:1906-1950) infers T from the lambda body's `last_expr` only — sufficient for `map(coll, x \| $x.field)` to bind T, but lambdas that use the param's type-vars *internally* (typed conditionals, branches that consume `Function<{T[1]->Boolean[1]}>` and need `t:Integer[1]` in scope) still see the param as `Unresolved`. Java's `processParamTypesOfLambdaUsedAsAFunctionExpressionParamValue` walks the template FunctionType's parameter types, substitutes through the active TypeInferenceContext bindings, and assigns the resolved type to each lambda parameter before reprocessing the body. We do the equivalent for the *first* arg (when the receiver type binds T) but not for downstream args. |
| Multiplicity arithmetic on combinators (`concatenate`/`+`/`at`) | P1 | 🔲 Deferred | `T[1] + T[1]` should produce `T[2]`, not `T[*]`. Today `multiply_multiplicities` (infer.rs:994) handles property-chain multiplication correctly, but native operators like `concatenate(a:T[*], b:T[*]):T[*]` and `+(a:T[1], b:T[1]):T[2..2]` don't compute precise output multiplicities — they take their declared return shape verbatim. Fix: add `Multiplicity::add` (`[a..b] + [c..d] = [a+c..b+d]`) and `Multiplicity::concat` next to `multiplicity_product`, then wire them in the relevant native-return-type computations (operator overloads in `bootstrap.rs` + builtin returns in `infer_builtin_return_type`). |
| Numeric coercion (`Integer` → `Float`) | P3 | 🔲 Deferred | Java has implicit widening |
| Return type influence on dispatch | P3 | 🔲 Deferred | Expected return type narrows candidates |

---

## Bootstrap / Chunk 0

| Item | Priority | Status | Notes |
|---|---|---|---|
| Move chunk 0 to compile-time | P2 | 🔲 Deferred | Currently built at runtime via `create_bootstrap_chunk()`. Could be a `const` or `lazy_static` built from m3.pure at compile time (build.rs or proc-macro). Saves ~1ms startup per compilation. |
| M3 parser completeness | P1 | ✅ Done | `m3_parser.rs` handles classes/enums/associations/primitives. User-class constraints flow via `parse_constraints` (`parser/class.rs:54`) → `lower_constraints` (`pipeline.rs:1602`); qualified properties via `parse_class_body` → `lower_qualified_property_bodies`. M3 metamodel reflective properties (`properties`, `qualifiedProperties`, `propertiesFromAssociations`, `typeParameters`, `multiplicityParameters`, etc.) are populated directly on Class and inherited members (`constraints`, `name`, `package`) reach Class via the supertype walk through `ElementWithConstraints`. Locked by `crates/pure/tests/m3_class_metaprops.rs`. "Derived properties" listed in the previous entry was a phantom — Pure has no construct distinct from qualified properties. |
| M3 property types | P1 | 🔲 Partial | M3 class properties are registered with placeholder `Any` types. Should resolve actual types from m3.pure definitions. |

---

## Units & Measures

| Item | Priority | Status | Notes |
|---|---|---|---|
| Unit as child of Measure | P2 | ⚠️ Partial | Reflective `package.children` now skips `Element::Unit` (`crates/runtime/src/eval.rs:1278`) so the user-visible API matches Java semantics. Units are still indexed in the package internally for `Measure~UnitName` type-position resolution; making that fully canonical (units indexed only on `Measure`) is the residual debt. Lock test: `eval_package_children_excludes_units` in `crates/runtime/tests/eval_tests.rs`. Canonical navigation works today via `Measure.canonicalUnit` / `Measure.nonCanonicalUnits` (`eval.rs:1437-1451`). |
| Unit conversion functions | P2 | 🔲 Deferred | `convert(value, sourceUnit, targetUnit)` — needs conversion factor storage. |
| Canonical unit references | P1 | 🔲 Deferred | Parser emits `MeasureName~UnitName` for unit refs in expressions. This works for resolution but should follow M3's `Measure.canonicalUnit` pattern. |

---

## Expression Lowering

| Item | Priority | Status | Notes |
|---|---|---|---|
| Root package `::` references | P1 | ✅ Done | Root-anchored resolution lives in `resolve.rs:554-585` (`get_package(model.root_package)` walks). Platform compile clean (0 errors / 1338 elements) confirms `::meta::pure::...` paths resolve. |
| Lambda variable scope | P1 | ✅ Done | Lambda params extend the active scope in `infer.rs:46`; type-tracked through `ResolutionContext.variable_types`. |
| Package-as-value references | P1 | ✅ Done | Qualified package paths in expression context resolve through the same root-walk. Verified by clean platform compile. |
| `^ClassName<TypeArgs>(props)` constructor | P1 | ✅ Done | Parsed in `parser/expression.rs:437-553`; type arguments threaded through to `NewInstanceExpr` so `genericType().typeArguments` reflects bindings at runtime. |
| Variable type tracking | P0 | ✅ Done | See Function Dispatch table above (`ResolutionContext.variable_types` HashMap). |

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
| Error count baseline | — | ✅ 0 | Was 243 → 8 → 0. Platform compile clean (236 files, 1338 elements). |
| Target after metaclass inference | — | ✅ Achieved | M3 metaclass types fully resolved. |
| Target after expression fixes | — | ✅ Achieved | All expression lowering errors resolved. |

---

## How To Use This Document

1. **Pick an item** matching your skill and the current priority
2. **Check dependencies** — items marked "Blocked" need their prerequisites done first
3. **Update status** when you start/finish work
4. **Add new items** as you discover gaps during development
