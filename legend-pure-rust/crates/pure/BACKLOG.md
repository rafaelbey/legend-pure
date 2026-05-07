# Pure Compiler — Backlog & Deferred Work

Tracking document for known gaps, deferred tasks, and future improvements
in the Pure compiler crate. Each item includes rationale for deferral and
dependencies, so future contributors know what's safe to pick up.

## Priority Legend

- **P0** — Blocking correctness; next up
- **P1** — Important for completeness; near-term
- **P2** — Architectural improvement; medium-term
- **P3** — Nice-to-have; long-term

## Recent encapsulation work

The lowering and inference layers now own their own files:

- `lower/mod.rs` shrunk from 2174 → ~245 lines via 12 sibling submodules
  (one per AST kind: literal, collection, operator, member_access,
  type_ref, lambda, let_expr, copy_slice, new_instance, relation,
  unit_navpath, function_app). Adding a new AST kind = one match arm
  in `lower_expression` + one file under `lower/<kind>.rs`.
- `inference/` consolidates the type-inference seam: `context.rs`
  (GenericBindings, RegisterMode, the dormant TypeInferenceContext
  skeleton, and unresolved-generic walkers), `lambda.rs` (both the
  lower-side `compute_lambda_param_expectations` and the resolve-side
  `bind_from_lambda_body`), `processor.rs` (placeholder for the future
  FunctionCallProcessor phase split), and `strict_mode.rs` (the
  thread-local + env-var toggle wiring strict-mode arg validation).
- `crate::strict_mode::with_strict_mode(true, || compile(...))` opts
  into the deliberate divergence-over-Java behaviour that catches
  wrong-arg-types and surface unresolved generics at every call site.
  Default stays Java-parity-lenient.

Plan: `~/.claude/plans/do-we-have-enought-quiet-swing.md`. Steps 1
through 5 plus 3a, 3b, 3c (carrier wiring), 3d (phase split + carrier
through `infer_generic_bindings`), 3e.1, 3e.2, 3f, 3g all complete.
The negative-test phase `crates/pure/tests/negative_tests.rs`
explicitly pins what should-and-shouldn't compile across reference
errors, structural model errors, lambda inference, and the
strict-mode lenient/strict pin pairs.

The remaining open item is the **authoritative-vs-constraint
two-branch dispatch** (Java's
`FunctionExpressionProcessor:567-594` `merge=false`/`merge=true`
distinction). Currently every binding uses Constraint mode (the
LUB-merging shape platform corpus depends on); the two prior spikes
(`c17a06`, reverted `3f1a64`) tried changing this and regressed
fold-style chains. The min-viable strict mode in 3g works without
it via the per-arg excluding-self trick — so this isn't blocking any
user-visible feature, and the safer path is to defer the structural
change until a feature genuinely requires it.

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
| `eval` strict arg validation (Java divergence) | P1 | ✅ Done (min viable) | **Reframed**: this is a *strict-mode divergence over Java*, not a parity bug. Reading primary source (`FunctionExpressionProcessor.java:121-265`, `TypeInferenceContext.java:325-495`, `register():467-480`) shows Java itself silently widens for `eval(func:Function<{Integer->X}>, 'wrong')` — the existing-concrete + incoming-concrete branch calls `findBestCommonGenericType(...)` which LUBs covariantly, so `(Integer, String) → Any`. No `TestFunctionTypeInference` test asserts an error for this shape; it's a known-broken corner Java acknowledges in the comment at `register():474-478`. Two prior auth-vs-constraint spikes (commits `c17a06`, reverted `3f1a64`) regressed platform fold-style chains because they inverted Java's order. **Landed**: Steps 1-3a-c-e-f-g (`crates/pure/src/inference/`): TDD scaffold (10 → 13 `tic_*` tests), encapsulation seam (12 sibling files in `lower/`, 4 inference helpers consolidated in `inference/lambda.rs`), `make_concrete` substitution entry point, `TypeInferenceContext` + `RegisterMode` skeleton (dormant), the strict-mode toggle (`crate::strict_mode::with_strict_mode` thread-local + `LEGEND_PURE_STRICT_INFERENCE` env var fallback) wired through `validate_call_arguments` (per-arg `bind_type` excluding-self trick: replaces `arguments[i]` with an Unresolved-typed placeholder so LUB doesn't widen T back to Any), and Java-parity unresolved-generic diagnostics emitted under strict at every call site. **Open**: Step 3d-cont (full `TypeInferenceContext::register(merge: bool)` two-branch dispatch wiring through `infer_generic_bindings`) — high-risk; the prior spikes' graveyard. The min-viable strict mode in 3g works without 3d-cont via the excluding-self trick, so 3d-cont is no longer a blocker for catching wrong-arg-types. Locked by `tic_eval_wrong_arg_strict_mode_errors`, `tic_unbound_t_at_nested_call_strict_mode_errors`, `tic_unbound_multiplicity_at_nested_call_strict_mode_errors` (strict-mode pins) plus their lenient-mode counterparts. |
| Lambda parameter type inference from `Function<{T->X}>` shape | P1 | ✅ Done | Wired end-to-end via `inference::lambda::compute_lambda_param_expectations` (relocated from `lower/mod.rs` in Step 3e.1): binds T (and `m`, after the multiplicity_arguments work) from sibling non-lambda args via `infer_generic_bindings`, extracts the lambda slot's `Function<{ft_params, …}>` from the candidate's parameter type, substitutes both ty + mult through `ft_params`, and passes the resulting concrete `(TypeExpr, Multiplicity)` tuples to `lower_lambda_with_expected_types`. Lambda params then dispatch correctly inside the body — `[1,2,3]->filter(x \| $x->plus(1))` types `x` as `Integer[1]` and the inner `plus` overload is unambiguous. The resolve-side counterpart `bind_from_lambda_body` (Step 3e.2) cohabits in the same module so the lambda-parameter algorithm has one home. Locked by `crates/pure/tests/integration_tests.rs::lambda_*` (5 cases) plus `tic_collection_of_lambdas_match`, `tic_fold_lambda_body_v_source`, `tic_higher_order_pct_runner` in `crates/pure/tests/inference_context.rs`. The "Cannot infer lambda parameter types" eager diagnostic fires when the lambda's expected type lands on `Unresolved`; `Generic(T)` expectations stay silent (matches Java parity — `tic_lambda_param_with_unbound_t_currently_silent` pins this). Extending strict mode to also reject `Generic(T)`-not-in-scope at the lambda site is a follow-up task — Step 3f wired the unresolved-generic checks at the *call's return* site, not yet at the lambda-param site. |
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
