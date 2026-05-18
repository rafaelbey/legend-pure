# Pure Compiler — Backlog & Deferred Work

Tracking document for known gaps, deferred tasks, and future improvements
in the Pure compiler crate. Each item includes rationale for deferral and
dependencies, so future contributors know what's safe to pick up.

## Priority Legend

- **P0** — Blocking correctness; next up
- **P1** — Important for completeness; near-term
- **P2** — Architectural improvement; medium-term
- **P3** — Nice-to-have; long-term

## Encapsulation layout

The lowering and inference layers own their own files:

- `lower/mod.rs` is ~245 lines orchestrating 12 sibling submodules
  (one per AST kind: literal, collection, operator, member_access,
  type_ref, lambda, let_expr, copy_slice, new_instance, relation,
  unit_navpath, function_app). Adding a new AST kind = one match arm
  in `lower_expression` + one file under `lower/<kind>.rs`.
- `inference/` consolidates the type-inference seam: `context.rs`
  (`GenericBindings` with the `ty_auth`/`ty` split, `RegisterMode`,
  unresolved-generic walkers), `lambda.rs` (both the lower-side
  `compute_lambda_param_expectations` and the resolve-side
  `bind_from_lambda_body`), and `processor.rs` (placeholder for the
  future FunctionCallProcessor phase split).

The divergent-from-Java inference behaviours documented in
`parity_semantics.md` are **always on** (no toggle). The earlier
`with_strict_mode` thread-local + `LEGEND_PURE_STRICT_INFERENCE` env
var toggle was removed once the platform reached zero errors under
strict — see commit `d32dafdde89`. The `tic_*_strict_mode_*` test
names in `inference_context.rs` are kept for traceability; they now
run under default semantics.

`crates/pure/tests/negative_tests.rs` pins what should and shouldn't
compile across reference errors, structural model errors, lambda
inference, and the auth/constraint pin pairs.

**Two-branch dispatch**: `infer_generic_bindings` classifies args by
pass-1 convergence and switches between `RegisterMode::Authoritative`
(any arg unconverged → `potentiallyUpdate…`-style first-wins) and
`RegisterMode::Constraint` (all converged → `update…`-style LUB-merge).
`bind_type_with_mode` provides the leaf-level switch; the structural
recursion is shared. Three pins —
`tic_two_branch_all_converge_lubs_to_any`,
`tic_two_branch_lambda_unconverged_uses_authoritative`,
`tic_two_branch_fold_accumulator_preserved_under_authoritative` —
lock the dispatch behaviour. The fold-style chain that defeated both
prior spikes (`c17a06`, reverted `3f1a64`) is green.

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
| Generic unification (`Z` propagation) | P2 | 🚧 Drift baselined; structural compat shipped (May 2026) | Per-expression drift histogram (`crates/core-platform-pure/tests/inference_drift_histogram.rs`) measures total surviving `Generic`/`Variable` markers across all platform expressions and asserts under a hard ceiling. Baseline 5931 / ceiling 6500. The dormant `TypeInferenceContext` skeleton (`crates/pure/src/inference/context.rs`) stays unwired — module doc-comment captures the rationale. **Higher-order eval-arg-mismatch closed** via `is_type_compatible_structural` (`crates/pure/src/resolve.rs`) — type-arguments-aware compat that recurses through nested `Named.type_arguments` and `FunctionType` shapes. The audit's diagnosis (auth/constraint propagation depth) was **wrong**: the actual root cause was the compat check ignoring `type_arguments`, not the binding pipeline failing to track depth. The fix is local to one function, sidesteps the prior-spikes graveyard entirely. `function_type_higher_order_wrong_inner_type_errors` un-ignored and green. |
| `eval` arg validation (always-on Java divergence) | P1 | ✅ Done | Java silently widens `eval(func:Function<{Integer->X}>, 'wrong')` via `findBestCommonGenericType` covariant LUB at `register():467-480` (Java acknowledges the gap at `:474-478`). Rust port substitutes `param.type_expr` through `ty_auth` bindings (Generic(T) sourced from a structural FunctionType slot) before the per-arg type-compat check, catching the wrong-arg case while letting `compare(1, 'a')` keep covariant-LUB-to-common-supertype. Plus Java-parity unresolved-generic diagnostics at every call site (Java gates these on outermost only). All always-on; toggle removed in commit `d32dafdde89`. See `parity_semantics.md` for the full divergence ledger. Locked by `tic_eval_wrong_arg_strict_mode_errors` plus the `_currently_silent` lenient-pin counterparts (`tic_pick_t_t_with_unrelated_args_lubs_silently`, `tic_unbound_multiplicity_at_nested_call_currently_silent`, `tic_lambda_param_with_unbound_t_currently_silent`) — Java parities the Rust port also keeps. |
| Lambda parameter type inference from `Function<{T->X}>` shape | P1 | ✅ Done | Wired end-to-end via `inference::lambda::compute_lambda_param_expectations`: binds T (and `m`, after the multiplicity_arguments work) from sibling non-lambda args via `infer_generic_bindings`, extracts the lambda slot's `Function<{ft_params, …}>` from the candidate's parameter type, substitutes both ty + mult through `ft_params`, and passes the resulting concrete `(TypeExpr, Multiplicity)` tuples to `lower_lambda_with_expected_types`. Lambda params then dispatch correctly inside the body — `[1,2,3]->filter(x \| $x->plus(1))` types `x` as `Integer[1]` and the inner `plus` overload is unambiguous. The resolve-side counterpart `bind_from_lambda_body` cohabits in the same module so the lambda-parameter algorithm has one home. Locked by `crates/pure/tests/integration_tests.rs::lambda_*` (5 cases) plus `tic_collection_of_lambdas_match`, `tic_fold_lambda_body_v_source`, `tic_higher_order_pct_runner`. The "Cannot infer lambda parameter types" eager diagnostic fires when the lambda's expected type lands on `Unresolved`; `Generic(T)` expectations stay silent (matches Java parity — `tic_lambda_param_with_unbound_t_currently_silent` pins this). |
| Numeric coercion (`Integer` → `Float`) | — | ❌ Won't do | Pure has **no** implicit numeric widening. `Integer`/`Float`/`Decimal` are siblings under `Number`. `compare(1, 2.2)` works via Generic LUB to `Number`, not via Integer-widens-to-Float. `takesFloat(1)` correctly errors — explicit `toFloat` required. The platform errors that earlier framing attributed to "missing numeric coercion" are all auth/constraint issues (see the Type System table entry below) — special-casing numerics there masks them rather than fixing them. |
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
| Constraint evaluation (runtime: Class inheritance walk) | — | ✅ Shipped 2026-05-18 | `evaluate_class_constraints` (`crates/runtime/src/native/lang.rs`) walks the supertype chain via `class.super_types` (visited-set guard against multi-inheritance diamonds), evaluates each level's constraints with `$this` bound to the same instance, raises `ConstraintViolation` with the constraint-owning class's name. Java parity: `_Class.allConstraints()` walks generalization order; subclass instances are validated against every ancestor's constraint. Parent-level `type_variable_parameters` aren't rebound during the walk (`Class.super_types: Vec<TypeExpr>` carries no value-args today); parametric-inheritance value-arg threading is a future follow-up. Locked by `testNewWithInheritedConstraintPasses` / `testNewWithInheritedConstraintFails` in `platform/pure/grammar/functions/lang/creation/new.pure`, surveyor 398/0/0. |
| Constraint evaluation (compile-time validation) | P3 | 🔲 Deferred | The constraint body must evaluate to `Boolean[1]` and the optional message must evaluate to `String[1]`. Today the validator can't fire because `pass_infer` doesn't include constraint bodies in its target list — `Class.constraints[*].function.type_info` is never populated. The full fix is a three-part change: (a) extend `pass_infer` to infer Class + PrimitiveType constraint bodies with `$this` bound to the owning type (mirrors the existing QP-body pattern), (b) add a cross-chunk validator that reads inferred `type_info` and checks Boolean/String compat with `is_type_compatible_structural`, (c) new `CompilationErrorKind::ConstraintBodyTypeMismatch` variant + LSP `diagnostics.rs` arm. Deferred — today's Pure programmers reliably write Boolean constraint bodies; bad shapes manifest as runtime errors with the constraint's source location, which is workable until a real user-bug surfaces. |
| Clean up `infer_builtin_return_type` | P2 | 🔲 Deferred | The `match name { … }` table in `infer.rs` is a tactical hack — handles `equal`/`and`/`plus`/`copy` etc. as hardcoded names. Each entry should live next to the function it represents (operators dispatched via existing native-table; `copy` needs special handling because `^$var(…)` lowers without a function-id and is the only construct producing a `FunctionCall { function_name: "copy", function: None }`). The user identified a "better place" for the copy case — track it down and migrate. |
| `^$var(…)` copy producing `function: None` | P3 | 🚧 Workaround | `lower_copy` produces `FunctionCall { function: None, function_name: "copy" }` because copy isn't a real Pure function — it's structural. Both `lower::let_expr::infer_let_type` and `infer::infer_builtin_return_type` had to add `"copy"` arms to recover the source's type. The cleaner fix: produce a different `ExprKind` for copy (e.g., `ExprKind::Copy { source, overrides }`) so type inference doesn't have to special-case a string. Companion to the `infer_builtin_return_type` cleanup above. |
| Variance (contravariant `<<-T>>`) | P1 | ✅ Done | `Class.type_parameter_variances: Vec<Variance>` (position-aligned with `type_parameters`, `#[serde(default)]`). Two surface forms collapse onto the same flag: m3 metamodel `^TypeParameter{contravariant: true}` (captured by `m3_parser.rs::parse_type_parameter_instance`) and class-level `<-T>` / `<+T>` prefix syntax (parser accepts; AST → Class wiring uses default Invariant for synthetics). `subtype_view` applies a Nil-→-Any lift for contravariant slots when substituting a class's type-args into super-types — the single hot spot that makes contravariance flow through Property's reflective-dispatch chain without touching every consumer. Cleared all 14 `dynamicNew.pure` errors plus eval.pure / canReactivateDynamically variance cases. Tests: `crates/pure/tests/variance_tests.rs` (5 cases incl. negative + platform-load wire pin). Docs: `docs/PURE_LANGUAGE_SPEC.md` §4.5. |
| Auth/constraint binding distinction | P1 | ✅ Done | Track `Generic(T)` bindings sourced from a structural FunctionType slot (Pure-invariant) separately from those sourced from a top-level Generic-typed arg (Java-style covariant LUB). Per-arg type check substitutes via `ty_auth` only; constraint-only T's stay `Generic("T")` and `is_type_compatible`'s wildcard arm accepts. `eval(intFunc, 'wrong')` catches; `compare(1, 'a')` / `compare(1, 2.2)` don't false-positive (matches Java's LUB-to-common-supertype semantics for top-level Generic params). Commit `eaf4ee4cd44`. |
| Let-bound lambda binding chain | P1 | ✅ Done | Three coordinated fixes: (1) Collection LUB preserves type/mult arguments for same-element pairs in `lower::let_expr::infer_let_type`, (2) `type_lub` handles FunctionType-vs-FunctionType (param Nil fallback, return covariant LUB), (3) bare-FunctionType-vs-Named-Function bridge in `bind_type_with_mode` so let-bound lambdas (whose `var_types[name]` is bare FT after `process_let_function_call` copies `arg_types[1]`) flow into Pass-1 binding. Commit `f10589b06e6`. Locked by `tic_let_bound_collection_of_lambdas_match`. |
| Function-ref Generic-alias chain resolution | P1 | ✅ Done | Function-ref like `reverse_T_m__T_m_->eval([1,2,3])` lifts to `FunctionType{Generic("T")[Variable("m")]→Generic("T")[Variable("m")]}` carrying the callee's generic names verbatim. Eval's structural binding records T_eval → Generic("T"), V_eval → Generic("T"); arg 1 binds T_eval = Integer; substitute_type follows Generic→Generic alias chains so V_eval also resolves to Integer. Plus `type_lub` skips Generic-as-LUB-side (treat as identity element so a function-ref's placeholder doesn't widen the caller's binding to Any). Commit `48d4081c577`. Locked by `tic_function_ref_eval_two_args_binds_through_lift`. |
| Chain inference for toOne/first/sort/cast/class | P2 | 🚧 Mostly fixed | Four orthogonal fixes (commits `f21f3cd0cb2`, `0663fe02b8e`, `1ecb3d63be8`, `4d742738932`) closed 10+ residuals: bare-FunctionType property-access bridge for inline lambdas (`{|1}->evaluateAndDeactivate().expressionSequence`), function-ref-scope `unresolved_type_params` no longer recurses into FunctionType (Generics inside a function-ref's lifted slot belong to that ref's scope, not the caller's), multiplicity alias-chain follow + cross-key promotion (mirrors the type-side `Generic→Generic` work — `eval`'s `m_eval` now resolves through the function-ref's lifted `Variable("m_callee")` when its sibling `n` concretises), `infer_multiplicity_from_valuespec` honours `type_info` first (cleared `^Class<T>(...)` new-instance mult lookup), and the bind-bridge does Constraint-not-Authoritative for lambda args (cleared fold's `(coll, lambda, init)` arg-3 type widening). Platform compiles clean at zero errors. Deep multi-pass cases (long Pair/List chains, deep metamodel chains, deep enum-value chains) need Java's `TypeInferenceObserver`-driven multi-pass — tracked under "Generic unification (Z propagation)". |

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
