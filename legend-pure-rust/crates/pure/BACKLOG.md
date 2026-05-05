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
| `TypeExpr::Named.multiplicity_arguments` for class-level mult-vars | P1 | ✅ Done | Added `multiplicity_arguments: Vec<Multiplicity>` to `TypeExpr::Named` (`crates/pure/src/types.rs:57-75`) and `multiplicity_parameters: Vec<SmolStr>` to `Class` (`crates/pure/src/nodes/class.rs:42-49`). Threaded through every constructor (~25 sites across pipeline, fqn, m3_parser, lower, runtime, validate, compose). `resolve_type_ref` lowers `MultiplicityArgument::Identifier(name)` → `Multiplicity::Variable(name)` and `Concrete(m)` → `lower_multiplicity(m)`. `compute_type_arg_bindings` now returns the full `GenericBindings` (ty + mult) and `lookup_member_in_class` calls `substitute_mult` on property/QP/association-injected multiplicities (with `bindings.mult` threaded through supertype recursion). Locked by `crates/pure/tests/integration_tests.rs::generic_subst_property_chain_with_mult_variable_decl_one_errors` (previously `#[ignore]`'d, now green). |
| Generic unification (`Z` propagation) | P2 | 🔲 Deferred | Complex; Java uses `TypeInferenceObserver` |
| Authoritative vs constraint bindings (`eval` arg validation) | P1 | 🔲 Deferred | When a generic var like `T` binds from BOTH a structural `Function<{T->V}>` slot (authoritative) AND a sibling `param:T` slot (constraint), our `bind_type` LUB-merges unconditionally. For `eval(func:Function<{Integer[1]->X}>, 'string')`, T binds Integer from func, then String from param, LUB widens to Any, and `is_type_compatible(String, Any)` trivially passes — the wrong-arg-type slips through silently. Java Pure's `TypeInferenceContext` distinguishes "this slot supplies T" from "this slot consumes T" via processing order; constraint slots check against the bound T rather than widening it. **Two attempts spiked + reverted**: (a) substitute-before-check at the per-arg validation site (commit c17a06) — knocked out platform `fold([]->cast(@Integer), λ, []->cast(@Any))` because the simple "Generic = constraint, else authoritative" classification widened V improperly; (b) auth-set tracking in `GenericBindings` with a structural `bind_type_inner` — same regression: in fold-style chains, V's authoritative source is the lambda body's inferred return, but the existing constraint-bind-from-accumulator gets reclassified by the lambda-body second-pass and the per-arg substitution then sees a too-narrow V. Real fix needs to handle the lambda-body-as-V-source case explicitly. Locked by `#[ignore]`'d tests `function_type_one_arg_wrong_type_errors` and `function_type_higher_order_wrong_inner_type_errors` in `crates/pure/tests/integration_tests.rs`. The 3 sibling tests for the canonical PCT shapes (`function_type_zero_arg_returning_z_y_typechecks`, `function_type_one_arg_typechecks`, `function_type_higher_order_pct_shape_typechecks`) are green. |
| Lambda parameter type inference from `Function<{T->X}>` shape | P1 | ✅ Done | Wired end-to-end via `lower::compute_lambda_param_expectations` (lower.rs:945-1021): binds T (and now `m`, after the multiplicity_arguments work) from sibling non-lambda args via `infer_generic_bindings`, extracts the lambda slot's `Function<{ft_params, …}>` from the candidate's parameter type, substitutes both ty + mult through `ft_params`, and passes the resulting concrete `(TypeExpr, Multiplicity)` tuples to `lower_lambda_with_expected_types`. Lambda params then dispatch correctly inside the body — `[1,2,3]->filter(x \| $x->plus(1))` types `x` as `Integer[1]` and the inner `plus` overload is unambiguous. Locked by `crates/pure/tests/integration_tests.rs::lambda_*` (5 cases: filter signature, chained generic-class receiver, T→T return, dispatch-on-overload, multi-param `{x,y\|…}`). Open: a follow-up "diagnose unresolvable lambda params at non-parametric call sites" item — when `Generic("T")` flows into a lambda from a callee but the enclosing fn has no `<T>` parameter and no concrete arg can bind it, we silently leave x as `Unresolved` rather than emit `CannotInferLambdaParameterTypes`. Fixing requires reachability/flow analysis to avoid false positives on PCT-style chains where the outer `eval` does the binding. |
| Numeric coercion (`Integer` → `Float`) | P3 | 🔲 Deferred | Java has implicit widening |
| Return type influence on dispatch | P3 | 🔲 Deferred | Expected return type narrows candidates |

---

## Bootstrap / Chunk 0

| Item | Priority | Status | Notes |
|---|---|---|---|
| Move chunk 0 to compile-time | P2 | 🔲 Deferred | Currently built at runtime via `create_bootstrap_chunk()`. Could be a `const` or `lazy_static` built from m3.pure at compile time (build.rs or proc-macro). Saves ~1ms startup per compilation. |
| M3 parser completeness | P1 | ✅ Done | `m3_parser.rs` handles classes/enums/associations/primitives. User-class constraints flow via `parse_constraints` (`parser/class.rs:54`) → `lower_constraints` (`pipeline.rs:1602`); qualified properties via `parse_class_body` → `lower_qualified_property_bodies`. M3 metamodel reflective properties (`properties`, `qualifiedProperties`, `propertiesFromAssociations`, `typeParameters`, `multiplicityParameters`, etc.) are populated directly on Class and inherited members (`constraints`, `name`, `package`) reach Class via the supertype walk through `ElementWithConstraints`. Locked by `crates/pure/tests/m3_class_metaprops.rs`. "Derived properties" listed in the previous entry was a phantom — Pure has no construct distinct from qualified properties. |
| M3 property types | P1 | ✅ Done | The previous "registered with placeholder `Any`" framing was outdated — `m3_parser` already extracted `rawType` as `Generic(name)` and `resolve_m3_supertypes` rewrote to `Named { class }`. The real residual gap was that **type_arguments and multiplicity_arguments were dropped** during the parse: a property declared `Property<U, V>[*]` survived as bare `Named { Property, type_arguments: [], multiplicity_arguments: [] }`, dropping anchoring info for reflective chains. Fix: `m3_parser::parse_generic_type_full` now also captures `typeArguments` and `multiplicityArguments` from the inline `^GenericType{...}` form (recursing into nested type-args), produces a sentinel `Named { ANY_ID, type_arguments, multiplicity_arguments, value_arguments: [String(rawType)] }` when args are present, and `pipeline::resolve_m3_supertypes` recognises the sentinel and rewrites `element` to the resolved class id while preserving the args. Inline `^FunctionType{...}` rawType references (9 sites in m3.pure) fall back to `Any` for now — same precision as before; tracked separately if needed. Locked by `crates/pure/tests/integration_tests.rs::m3_property_parametric_types_preserved` (asserts zero stripped-parametric properties; surfaced 27 fixes from the prior diagnostic). |

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
| Type inference (bottom-up) | P2 | ✅ Done | `pass_infer` runs as Pass 2.5 in `pipeline::finalize_model` — bottom-up inference over every function body, populating `expr.type_info` on every node. The recent generic-substitution + lambda second-pass fixes (e5e27c592) make this layer's output precise enough that `inference_precision_sweep.rs` keeps platform `body→Any` divergences under 2. |
| Constraint evaluation | P3 | 🔲 Deferred | Class constraints need expression evaluation at validation time. |

---

## Pipeline Architecture

| Item | Priority | Status | Notes |
|---|---|---|---|
| Pass 2a/2b split | P0 | ✅ Done | Signatures resolved in 2a, bodies in 2b. |
| Eager validators (Java-parity placement) | P0 | ✅ Done | Validators run next to the data they inspect across three seams: resolver-eager (`resolve_type_ref` does type-arg arity; `resolve_stereotypes`/`resolve_tagged_values` do profile-kind + name-existence), hydration-inline (`validate_super_types`, `validate_association`, `validate_duplicate_properties`, `validate_no_access_on_properties`, `validate_no_multiple_access_levels` called from `hydrate_element_signature`), and cross-chunk (`validate(model)` keeps only `validate_repo_visibility` + access-level Step B). Pass-1 `create_shell` populates `Class.type_parameters`/`multiplicity_parameters` + `Profile.stereotypes`/`tags` from the AST so resolver-eager checks are sound regardless of topo order. Adding a new element kind = one match arm in each. Commit `86fe8b9ea96`. |
| Parallel Pass 2 | P3 | 🔲 Deferred | After 2a/2b, bodies can be parallelized per-element. |
| Incremental compilation | P3 | 🔲 Deferred | Re-resolve only changed chunks. Chunk IDs enable this without rewriting. |

---

## Developer Experience / Observability

| Item | Priority | Status | Notes |
|---|---|---|---|
| Compilation tracing (`tracing` crate) | P1 | ✅ Done | `#[tracing::instrument]` on every pipeline pass (declare, topo_sort, define_signatures, define_bodies, define_class_bodies, infer, finalize_model), `resolve_function_call` (resolve.rs:811), `narrow_candidates_by_type` (resolve.rs:2349), and lower-side `lower_expression_body`. Enable via `RUST_LOG=legend_pure_parser_pure=debug`. Locked by `crates/pure/tests/tracing_smoke.rs`. |
| Dispatch decision log | P1 | ✅ Done | `resolve_function_call` (resolve.rs:811-820) emits `#[tracing::instrument]` with `name=`, `package=`, `arg_count`, `n_imports` fields and inline `tracing::debug!` events for each phase (root-package lookup, import-scope lookup, single-vs-multi candidates). |
| Pass timing | P2 | ✅ Done | Pipeline passes use `#[tracing::instrument(level = "info", name = "...", skip_all, fields(...))]` so `RUST_LOG=info` produces per-pass spans with timing. |
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
