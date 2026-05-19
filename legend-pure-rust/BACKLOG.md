# Legend Pure Rust — Backlog

Unified backlog for the Rust workspace. Each component's backlog is detailed
in its crate directory; this file provides the high-level view.

**Priority legend:** P0 (blocking) → P1 (important) → P2 (architectural) → P3 (nice-to-have)

---

## Milestone Status

| Component | Status | Platform Errors | Test Count | Key Milestone |
|-----------|--------|----------------|------------|---------------|
| Parser | ✅ Complete | 0 | ~400+ | Full grammar coverage |
| Compiler | ✅ Platform clean | **0** | 117 lib + ~180 integration | 244 files / ~1660 elements (M3 + Store + Mapping + Diagram + Graph + TDS + **Relational** DSL metamodels) |
| Runtime | ✅ Surveyor 246/0/0; PCT 465/465 | N/A | 357 lib + 71 eval + 246 surveyor | PCT broad-canary at 100% with 9 manifest exclusions |
| CLI | ✅ Shipped | N/A | ~20+ | All 16 subcommands implemented; `legend test --pct` defaults to bundled `pct_grammar_rust_native.json` exclusions |
| Java codegen | ✅ v1 | N/A | 16 | `legend java-bindings` + proxy runtime; hand-written `Any` as universal proxy supertype |
| **Total** | | **0** | **~1500+** | |

---

## Compiler (`crates/pure`)

> Detailed tracking: [`crates/pure/BACKLOG.md`](crates/pure/BACKLOG.md)
> Dispatch design: [`crates/pure/FUNCTION_DISPATCH.md`](crates/pure/FUNCTION_DISPATCH.md)

### Done ✅
- Param count filtering, Pass 2a/2b split, type-compatible matching
- Multiplicity narrowing, subtype matching, variable type tracking
- Generic type + multiplicity substitution
- M3 supertype resolution, metatype inference
- Platform compile: **0 errors**

### Open Work

| Item | Priority | Notes |
|------|----------|-------|
| Full generic unification (`Z` propagation) | P2 | ✅ Lambda-body Z-propagation shipped 2026-05-18 | Per-expression drift histogram baseline 5931 → **4531** after the 2026-05-18 fix (ceiling lowered 6500 → 4800 with ~270 headroom; `crates/core-platform-pure/tests/inference_drift_histogram.rs`). Two-part fix: (a) `infer_generic_bindings` pass-2 substitutes `ty_auth` bindings into the param type before checking for a FunctionType slot, so a PCT-style `eval<T,V|m,n>(func:Function<{T[n]->V[m]}>, param:T[n])` whose `T` was bound from arg-1's structural slot to `Function<{->Z[y]}>` exposes the inner FunctionType to the lambda-body binding logic. (b) `inference::lambda::bind_from_lambda_body` uses `infer_typeexpr_from_valuespec` (preserves `type_arguments`) instead of `infer_type_from_valuespec` (returns bare ElementId), so `Z` binds to `List<Variant>` not bare `List`. Locked by `crates/pure/tests/z_propagation_probes.rs::zp_lambda_body_drives_outer_pct_generic_through_class_typearg` (concrete RED case provided by user). Earlier structural-compat fix (May 2026) covered the `is_type_compatible_structural` half; the binding-pipeline half landed 2026-05-18. Future work: overload-elimination retry loop and the dormant `TypeInferenceContext` skeleton — still unwired, still gated on a future RED test. |
| Numeric coercion (`Integer` → `Float`) | P3 | Java has implicit widening |
| Return type influence on dispatch | P3 | Expected return type narrows candidates |
| Move bootstrap chunk 0 to compile-time | P2 | Saves ~1ms startup |
| Unit as child of Measure (canonical M3) | P2 | User-visible `package.children` now excludes Units (Java parity); residual debt is making units indexed only on the Measure internally. |
| Type inference (bottom-up) | P2 | Pass 2.5 in pipeline |
| Parallel Pass 2 | P3 | Bodies can be parallelized per-element |
| Incremental compilation — Phase 2 (element-scoped, Salsa-or-equivalent) | P2 | Phase 1 (chunk-scoped LSP recompile) shipped in `pipeline::compile_chunks_incremental` (T-20260513-01). A body-only edit on function `f` today still reruns Pass 2b for every element in `f`'s chunk + transitive dependent chunks. Phase 2 pushes invalidation down to the `ElementId` level so only `f` and its element-level dependents rerun Pass 2b lowering; Pass-1 shells, Pass-2a signatures, and validators on unrelated elements stay cache-hit. **Design doc precedes implementation** per T-20260513-01 plan: evaluate Salsa-vs-handrolled, query granularity (per-`ElementId`? per pass × `ElementId`?), interaction with the existing `Arc<PureModel>` snapshot model, behaviour under DAP sessions. Also covers the cold-build path: `cargo build`'s DSL `.purem` rebuilds drive 5–15 min link times today (`LEGEND_PURE_SKIP_DSL_SNAPSHOTS=1` escape hatch is a tell that the build graph isn't fine-grained enough). |
| `meta::relational::metamodel::schema` widens `$schemas` to `Any` | P2 | Investigation during the Bug 2 fix (2026-05-02) showed that even on first compile, the receiver of `$schemas.tables` at `functions.pure:232:40` is typed as `Any[*]` rather than `Schema[*]`. The error stays silent on the live compile path because `is_metatype_carrier(ANY_ID)` returns `true` and `infer_property_access` returns `UnknownTarget`, but the type widening itself is a real inference gap — probably in how `concatenate<T>` binds T or how the lambda parameter `d` flows through `$d->cast(@Database)->schema(...)` inside the surrounding `map` call. Worth investigating once the residual dispatch ambiguity (above) is closed; not blocking. |

### Recently Closed
- Stress bench `compile/hub_spoke_1k` panic — Closed 2026-05-17. The hub_spoke generator at `crates/stress/src/generate/hub_spoke.rs:322,387` emitted `function test::greet{f}(name: String[1]): String[1] { 'hello ' + $name }`. `+` is the platform-defined `plus` function, which the compile-only benches (`crates/stress/benches/pipeline.rs:343`) couldn't dispatch without loading the platform. Fix: change generated function bodies to `{ $name }` — same param-use + signature compile load, no platform-only operator dependency. `cargo bench -p legend-pure-parser-stress --bench pipeline -- compile/hub_spoke_1k` now completes cleanly (`time: [9.46 ms 9.55 ms 9.91 ms]` median). Same fix applied to both the in-place generator (`generate`) and the per-file generator (`generate_files`); the in-place case has the explanatory comment, the per-file case cross-references it.
- Lambda parameter type inference — Closed 2026-05-16 after primary-source verification against Java. The two-phase fix from `b4ff09432a5` covers the literal-arrow / direct-application form (`{x,y|$x+$y}->eval(1,2)`); the let-bound case (`let f = {x,y|$x+$y}; $f->eval(1,2)`) now fails eagerly with `CannotInferLambdaParameterTypes`. The prior open-entry claimed per-call-site specialisation as a Java-parity gap, but the primary source contradicts that: `TypeInference.java:114-117` (`processParamTypesOfLambdaUsedAsAFunctionExpressionParamValue`) throws `"Can't infer the parameters' types for the lambda. Please specify it in the signature."` whenever the template type isn't concrete, and an empirical test (`TestBacklogLambdaRepro` against the JDK-11 Maven build at HEAD) confirms Java rejects the exact `let f = {x,y|$x+$y}; $f->eval(1,2)` example with that error at line 3 col 13. The current Rust diagnostic IS Java parity — both engines reject this case identically, just with a friendlier message on the Rust side. Per-call-site specialisation would be a divergence from Java semantics, not a parity item; the Rust port's policy forbids that direction. Plan doc at `~/.claude/plans/lambda-parameter-type-inference-encapsulated-pebble.md` covers the already-shipped eager-failure work and stays accurate; the speculative "Open" follow-up paragraph it contains is now obsolete.
- Repo descriptors + manifest (Pure-graph composition) — Closed 2026-05-15 (audit). All five sub-tasks ship: (1) descriptor JSON parsed into `RepoMeta { name, pattern, dependencies }` by `crates/build/src/lib.rs`; (2) top-level manifest via `[[package.metadata.legend-pure.repos]]` in each consuming crate's `Cargo.toml` — `crates/core-platform-pure/Cargo.toml` enumerates 9 repos (platform + 7 DSLs + relational); (3) topological sort via Kahn's algorithm in `crates/core-platform-pure/src/topo.rs::topo_sort_repos`, called from `repo::load`; (4) pattern membership validated at compile-time by `validate_repo_pattern_membership` in `crates/pure/src/validate.rs` — `snapshot-builder::populate_repo_patterns` registers each repo's descriptor pattern with the model so `.purem` builds fail at `cargo build` on FQN-vs-pattern mismatch; (5) loader supports four shapes (`purem-embedded`, `purem-artifact`, `embedded`, `Filesystem`) with the trait open for future remote artifact-coordinate kinds. Test coverage: `crates/pure/tests/repo_pattern_smoke.rs` (197 lines) locks the pattern-violation diagnostic surface. The audit-flagged "hand-listed directories in build.rs" line was a documentation lie — the build crate already delegated to descriptor-driven embedding.
- Repo-boundary visibility enforcement — dsl-mapping (Phase 2) — `validate_repo_visibility` in `crates/dsl-mapping/src/compiler.rs` walks every cross-element reference (`MappingInclude.included` via the registry, `MappingInclude.store_substitutions[].source/target` + `ClassMapping.class` + `PureClassMappingBody.src_class` + `OperationClassMappingBody.operation` + `EnumValueMapping.source_values[].EnumRef.enumeration` via the model) and emits `NotVisible` for refs whose target home repo is not in the use-site repo's declared dependencies. Mirrors the dsl-relational Phase D template (`crates/dsl-relational/src/compiler.rs::validate_repo_visibility`). Locked by `crates/dsl-mapping/tests/visibility_smoke.rs` (6 cases: include cross-repo / declared-dep / class-mapping target / `~src` / empty-map no-op). Phase 1 + Phase 2 + relational Phase D now together cover Java's `TestVisibility` parity for the cross-repo enforcement path. The use-site `SourceInfo` granularity tightening (Java's column 31/32/17 assertions) and `TestVisibility.testEnumMapping`/`testMappingIncludes` direct-port are follow-up polish items that don't block the core enforcement.
- 4 `elementToPath` ambiguities in `relationalRuntime.pure` (Bug 1) — Class-body resolution didn't bind `$this` into `ctx.variable_types`, so dispatch on `$this.prop->toOne()->elementToPath()` chains saw the inner `->toOne()` arg as `Any[*]` (no scope to resolve `$this` →`PostProcessor`/`PostProcessors`). With `arg_type = Any` and `is_subtype(Any, Function<Any>) = false`, every overload failed Phase 1 → fallback to original 3 candidates → ambiguity. Fixed in `crates/pure/src/pipeline.rs` (the per-element body-lowering scope at lines 1183-1198) by seeding `variable_types` with `$this` for `Class`, `Association`, and `PrimitiveType` bodies, with proper parametric type for generic Classes (`Class<T,V>` → `Named { id, type_arguments: [Generic("T"), Generic("V")] }`). Investigation confirmed (via primary source on `m3.pure` lines 2068-2378 and a no-overlay check in `M3CoreInstanceGenerator.java`) that the lattice itself is correct — `ConcreteFunctionDefinition` reaches `PackageableElement` via `PackageableFunction`, so once dispatch sees the right `arg_type`, Phase 1 filters `Type` and the Phase 4 `type_distance` / Phase 5 declaration-order tiebreaker pick `Function<Any>` cleanly. Platform compile: **0 errors**.
- Purem reload broke `is_metatype_carrier(Any)` — m3.pure re-declares `Class meta::pure::metamodel::type::Any` (and `Nil`); our `m3_parser` was allocating a fresh slot for it, so after `register_m3_packages` the package had two children named `Any` (the bootstrap `ANY_ID` alias plus the m3-parsed slot 34). `build_fqn_index` picked the longer path for `ANY_ID`, but on `merge_slice` the FQN re-resolved to whichever was registered first (the m3-parsed slot), so post-load `Any` ≠ `bootstrap::ANY_ID` and `is_metatype_carrier` returned false — the previously-silenced `UnknownProperty: 'tables' on Any` error in `functions.pure:232` started firing. Fixed in `crates/pure/src/m3_parser.rs::parse_top_level_instance` by skipping the `Class Any` / `Class Nil` body in `meta::pure::metamodel::type` (mirrors the existing primitive-type skip), so the bootstrap slots stay canonical and FQN round-trips. Re-enabled `crates/core-platform-pure/tests/purem_load_e2e.rs::purem_repo_loads_into_fresh_model`.
- Root package `::` references — `crates/pure/src/resolve.rs:554-585` walks from `model.root_package`; platform compile clean.
- Lambda variable scope in expressions — `crates/pure/src/infer.rs:46`; tracked through `ResolutionContext.variable_types`.
- Package-as-value references — same root-walk path; clean platform compile (0 errors / 1338 elements) confirms.
- `^ClassName<TypeArgs>(props)` constructor — parsed at `crates/parser/src/parser/expression.rs:437-553`.
- M3 parser completeness (constraints, QPs) — user-class constraints/QPs lower via `pipeline.rs:1602`/`1526`; M3 metamodel reflective props populated directly + via supertype walk. Locked by `crates/pure/tests/m3_class_metaprops.rs`.
- Compilation tracing — `#[tracing::instrument]` on every pipeline pass plus the existing `resolve_function_call` dispatch log. Locked by `crates/pure/tests/tracing_smoke.rs`.
- Store DSL metamodel embedded — `legend-pure-dsl-store/.../platform_dsl_store/grammar/{store,runtime}.pure` now flow through `crates/core-platform-pure/build.rs` alongside the M3 platform sources, exposing `meta::pure::store::*` and `meta::core::runtime::*` classes in the compiled model. Hand-embedded via the new `PureRepo { root, prefix }` helper; the descriptor-driven loader (separate backlog item) is the proper long-term path. Locked by `store_dsl_metamodel_resolves_in_platform` in `crates/runtime/tests/eval_tests.rs`.

---

## Runtime (`crates/runtime`)

> Detailed tracking: [`crates/runtime/BACKLOG.md`](crates/runtime/BACKLOG.md)
> Architecture: [`crates/runtime/ARCHITECTURE.md`](crates/runtime/ARCHITECTURE.md)

### Done ✅
- Four-layer storage model (PureModel → Heap → VariableContext → Extensions)
- Dual heap (Dynamic + Typed), undo-log VariableContext
- Lazy call stack error handling
- ~50 native functions (arithmetic, string, collection, comparison, lang)
- Persistent collections (`im-rc`), `rust_decimal` for Decimal, `jiff` for dates
- **RAII-driven heap**: `SlotMap<ObjectId, …>` replaced with
  `ObjectHandle = Rc<RefCell<HeapEntry>>`. Identity via `Rc::ptr_eq`,
  freed automatically when the last clone drops. `RuntimeHeap` retains
  only metamodel bootstrap rows. `getAll(UserClass)` is a reachability
  walk from the variable context. JNI cross-FFI uses a `JniHandleTable`
  side-table to mediate `i64 ↔ ObjectHandle`. (Plan:
  `~/.claude/plans/the-fact-that-the-peaceful-goblet.md`. Bench:
  `crates/runtime/benches/heap_growth.rs` — ~62ns alloc+drop, no
  retention.)
- **Platform-only binary trim** (commit `47f8e5925f3`): only `platform`
  is embedded (as a `.purem` blob via `include_bytes!`); every other
  repo (precise primitives, all DSLs, store-relational) ships as a
  build-emitted `.purem` artifact next to the binary, loaded via
  classpath auto-discovery. `load_platform()` deserialises the blob
  rather than re-parsing 250 `.pure` files, knocking ~70 µs off
  `evaluator_setup_only` (−18 %) and ~15 % off function-call dispatch
  vs the prior all-source path. Filesystem override of any embedded
  repo via classpath `kind = "filesystem"` is the local-dev path.

### Performance Baselines

Captured 2026-05-02 from `cargo bench --bench runtime_eval -- --quick` on
M-series macOS, release. Numbers are running medians from criterion;
±1 % run-to-run variance is normal. Track regressions >20% from these.

These supersede the 2026-04-26 baselines, which became stale across a
month of compiler/runtime work (visibility validator, lambda-param
inference rebinding, the dsl-relational metamodel, parser unblocks for 7
grammar gaps, plus the platform-only binary trim that picked up a
+15-18 % win on function-call hot paths). See
`docs/runtime/perf_session_2026-05-01.md` for the
intermediate session that landed the recursion / classifier-resolve
wins.

| Benchmark | Time | Notes |
|---|---|---|
| `eval_function_call/trivial_return_literal` | ~49 µs | Smallest possible call — `function(): Integer[1] { 42 }`. Floor for every per-call cost |
| `eval_function_call/call_with_one_int_arg` | ~52 µs | `add_one(x:Integer[1]):Integer[1] { $x + 1 }` |
| `eval_function_call/evaluator_setup_only` | **~330 µs** | `Evaluator::new(&model, &registry)` — heap + bootstrap_metamodel walk over 1300+ M3 elements. Dominates every other measurement when calls reuse a model; **the hot fix target** if call-throughput becomes a goal. Phase-3b's purem-load path knocks ~70 µs off vs the prior all-source compile. |
| `eval_property_access/read_property_inline_construct` | ~60 µs | `^Person(...)..firstName` |
| `eval_property_access/read_via_let_binding` | ~62 µs | `let p = ^Person(...); $p.age` |
| `eval_lambda/map_double_5` | ~57 µs | `[1..5]->map(x \| $x * 2)` |
| `eval_lambda/map_concat_typed_lambda_param` | ~54 µs | `['a','b','c']->map(s \| $s + 'X')->joinStrings(',')` — locks lambda-param narrowing fix from `b4ff09432a5` |
| `eval_lambda/filter_then_map` | ~62 µs | 10-element filter+map chain |
| `eval_relation/addColumns_2cols_to_1col_source` | ~65 µs | The full `@(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne()->addColumns(~[ab,z])` chain — locks the surveyor-211/0/0 path |
| `eval_recursion/fib_15` | ~2.2 ms | 1973 recursive calls to `fib`; sensitive to function-call dispatch cost. The May-1 `Rc<[T]>` body-storage win cut this from ~4.1 ms |
| `eval_recursion/sum_to_50` | ~122 µs | Tail-style accumulation; same dispatch path |
| `eval_member_wrapper/class_properties_repeat_20` | ~68 µs | 20 repeated `Person.properties->size()` reads — locks the May-1 cache-collapsed-Value win on `member_wrapper_cache` |

Run with `--quick` (lower variance bound, ~2-3s total) for change-time
checks; drop the flag for full statistical-strength baselines (~5min).

### Open Work

| Item | Priority | Notes |
|------|----------|-------|
| One file per native function | P2 | Split domain bundles into individual files |
| Compiled function support | P2 | AOT-compiled Pure → Rust for hot paths |
| Full PCT native coverage | P1 | Complete remaining stdlib functions |
| Non-recursive structural equality | P2 | `equality.rs::values_equal` / `objects_equal` recurse on every collection element and every `<<equality.Key>>` property. Pathological inputs (deep `List<List<…>>`, long `Pair<U, Pair<…>>` chains, equality-keyed self-references) will blow the Rust stack — default 8 MB on the main thread but as little as 512 KB on spawned threads (LSP, test runners, DAP). Path: explicit work-stack (`Vec<(Value, Value)>`) with cycle guard (`HashSet<(ObjectId, ObjectId)>`) plus precomputed equality-key cache on Class. Same audit needed for `toRepresentation`, `toString`, `deep_copy_object` in Copy, and `reactivate_value`. Stopgap: depth-limit guard exists today; full iterative rewrite is the long-term fix. |
| Step 5 — class()/genericType() return ObjectId via BiMap | P2 | `TypeOf` and `GenericTypeOf`'s `rawType` slot return `Value::Element` rather than `Value::Object` projected via the BiMap. Required for parametric-type chains (`^MyClass<String>()->class()->typeArguments`) to thread bindings end-to-end. Tried 2026-04-26: regresses surveyor 211/0/0 → 188/15/8 because `Match`'s `value_matches_type`, `ElementToPath`'s value-variant dispatch, and `class.pure`'s `cast(@Class<T>)` chain all read `Value::Element` strictly. Lock-step audit of every metamodel-consuming native required before re-attempting — defer until a concrete test surfaces the chain. (BiMap is now `RuntimeHeap::element_to_object: HashMap<ElementId, ObjectHandle>`.) |
| Promote PT-key memoization-cache holders to `Weak` | P3 | The memoization cache currently holds strong `Rc`s on cached object results, so cached entries keep their objects alive until cleared. Matches Java semantics, but in long-running REPL/test-runner sessions a cache flush is the only way to reclaim. Consider a `Weak`-based variant for a "soft" cache mode. |
| Reactivate inner-call dispatch + InstanceValue unwrap | P2 | `[xs]->map(p\|$p.lastName)->deactivate()->reactivate()` errors with `"reactivate: simple-name dispatch for SimpleFunctionExpression without 'func' is not implemented"` (`meta.rs:2435-2448`). Path forward: in `reactivate_value` for InstanceValue, unwrap `.values` but distinguish lambdas by parameter arity — zero-arg are "deactivated expressions to invoke", parametrised are "first-class function values to pass through". Plus: `parametersValues` needs an additional InstanceValue-unwrap layer because deactivated collections wrap as `InstanceValue { values: [p1, …] }`. Not currently exercised by any surveyor test — defer until a concrete test forces it. |
| Cleaner SourceInformation name-span representation | P2 | Currently we carry `ElementNode { source_info, name_source_info, … }` in `crates/pure/src/model.rs` — two parallel `SourceInfo`s per element. Matches Java's `SourceInformation(startLine, startColumn, line, column, endLine, endColumn)` but feels like the wrong layer. Recommended: collapse into the AST's `SourceInfo` to match Java's 7-field struct directly. **Measurement (2026-05-17, locked in `model::tests::element_node_size_for_source_info_audit`):** `sizeof(SmolStr) = 24`, `sizeof(SourceInfo) = 40` (SmolStr + 4×u32, no padding), `sizeof(ElementNode) = 112` (SmolStr + 2×SourceInfo + PackageId + 4-byte tail padding). Dropping `name_source_info` would save 40 bytes/element (35.7% of ElementNode); widening SourceInfo to Java's 7-field shape (source + 6×u32 = 48 bytes aligned) would save 32 bytes/element (28.6%). At platform scale (1,660 elements) that's 53–66 KB; at the 100K stress benchmark it's 3.2–4.0 MB; at 1M synthetic it's 32–40 MB. The trade-off the wider audit must weigh: `SourceInfo` is used at every expression node, error site, and AST traversal — widening it 40→48 bytes adds ~20% per-use everywhere, which may offset the ElementNode savings depending on the expression/error-to-element ratio. Concrete next step: write a one-shot allocation counter that breaks down `SourceInfo` instance counts by use site (ElementNode vs Expression vs CompilationError vs …) on the platform compile, then pick: (a) drop `name_source_info` only (`Option<NameSpan>` next to current SourceInfo — saves 16 bytes when None), or (b) full Java-7-field collapse (widens SourceInfo, drops name_source_info), or (c) leave as-is if Expression-node count dominates. |
| H2 backend follow-ups | P3 | The H2 backend ships in `crates/store-relational-runtime` (auto-spawned Java sub-process in PG-wire mode + sync `postgres` client, `RelationalBackend` trait dispatching DuckDB / H2 per native, `[extension.relational.h2]` classpath section + env-var overrides). Two known follow-ups: **(a) Canonical Pure `<<test.Test>>` H2 functions** — the platform root surveyor (`eval_surveyor_root_strict_pass`) demands every shipped test passes; H2 tests in `main/resources` would fail without `LEGEND_PURE_H2_JAR`. The earlier "skip when external dep unavailable" framing (adding a `requiresExtension` tag to `meta::pure::profiles::test` + runtime SKIP dispatch) was reconsidered 2026-05-18 — touches the Java-shared `profile.pure`, widens the `STATUS_SKIP` policy in `testing.rs:284-293` (today reserved for the exclusion-map path so SKIP can't mask real runtime gaps), and conflates discovery-time (Java's `PureTestBuilder`) with dispatch-time (Rust runtime) semantics. Preferred reframing: H2 tests don't go in the platform root surveyor at all — they live in a `store-relational` test surveyor invoked only when `[extension.relational.h2]` is configured on the active classpath, so the surveyor never discovers tests it can't run. No `profile.pure` change required under this reframing. **(b) NUMERIC cell mapping** — DATE and TIMESTAMP are now recognized in `cell_to_value_pg_text` (text-mode jiff parsing produces `Value::Date`). NUMERIC text overlaps with `Float` syntax (`123.45` could be either) — disambiguating without binary-protocol type OIDs is unsafe, so NUMERIC currently falls through to `Value::Float`. Tighter mapping needs either (i) a prepared-statement column-type peek so we can route cells through extended-protocol binary decoding with `postgres`' decimal feature, or (ii) a column-type-hint side channel from the SQL builder. |

### Recently Closed
- **Distributed-slice self-registration (`linkme`)** — Closed 2026-05-17 (audit). The full v2 extension architecture (`linkme` distributed slices for `RuntimeExtension`, `CompilerExtension`, `IdeExtension`, `SectionParser`, `IslandParser`, `DSLPopulator`) shipped across Phases 1–6 of the recipe doc. `NativeRegistry::discovered()` collects every linked `#[distributed_slice(RUNTIME_EXTENSIONS)]` contribution; in-tree DSLs (`diagram`, `mapping`, `relational`, `graph`, `store`, `tds`, `path`) and `store-relational` all self-register. See `docs/extensions/downstream-recipe.md` §6 / §10 and `examples/mydsl-extension/`.
- **Standard registry built into `Evaluator`** — Closed 2026-05-17 (audit). `Evaluator::builder()` defaults to `NativeRegistry::discovered()` + `discovered_populators()` + classpath-driven `extension_configs`. The CLI test/run/repl/snapshot subcommands and the JNI `JniContext::new` / `new_with_configs` constructors all route through it. See `crates/runtime/src/eval.rs:builder()` and `crates/jni/src/context.rs`.
- **CLI `legend test` classpath integration (was H2 follow-up `(d)`)** — Closed 2026-05-17 in commit `63d8f9cc4`. `test` / `run` / `repl` / `snapshot` all consume `--classpath` via `crate::classpath::resolve_classpath`, including `extension_configs` reaching the H2 backend through per-evaluator config (Phase 4 `set_extension_configs` flow, no more `OnceLock` global).
- **Wire Java `AutoCloseable` to `JniContext::release`** — Closed in commit `142cd216a feat(jni): wire AutoCloseable to JniContext::release`; entry audit 2026-05-17. `PureRustInstance` implements `Closeable` and registers a `Cleaner` for GC-time cleanup; `instance.close()` (or GC) → `PureRustEvaluator.free()` → `cleanable.clean()` → `nativeFreeInstance` → `JniContext::release` → `JniHandleTable::release` drops the strong `Rc`. The full Java↔Rust release chain is wired both for explicit `try-with-resources` AND for unreferenced instances via the `Cleaner` registered at line 160/275 of `PureRustEvaluator.java`. `PureRustResult` (the transient FFI marshalling wrapper) deliberately does NOT implement `Closeable` — handle lifetime is owned by `PureRustInstance`, not by the result type.
- **Native arity checks: drop or keep?** — Closed 2026-05-17 (investigation). Verdict: KEEP. `expect_args` is NOT redundant boilerplate. The compiler's Pass-2 FQN-mangling narrows by param count for `FunctionApplication`, but reflective dispatch paths (`apply_callable` at `crates/runtime/src/eval.rs:2131`, JNI `nativeEvaluate`, Pure-level `eval`/`evaluate`, surveyor's `executeTest($t->cast(@Function<…>))`) route through `dispatch_compiled_function` → `execute_native_with_values` without any arity validation. The per-native `expect_args(…)?` IS the only arity guard for those callers. Locked in by `crates/runtime/tests/eval_tests.rs::expect_args_guards_reflective_dispatch_against_wrong_arity` — drops or weakens of the check will fail the test (verifies `apply_callable(size, &[])` returns an arity error instead of out-of-bounds-indexing the empty `values[0]` slot). Future polish: centralize via an `arity() -> Option<usize>` method on `NativeFunction` and have the dispatcher validate, freeing natives from boilerplate — but that's a trait change with no current pull beyond cosmetics.
- **H2 backend follow-up (c) Linux `PR_SET_PDEATHSIG` polish** — Closed 2026-05-17 as an accepted limitation. The standard Linux fix (`Command::pre_exec(|| prctl(PR_SET_PDEATHSIG, SIGTERM))`) requires `unsafe`, and `crates/store-relational-runtime` carries `#![forbid(unsafe_code)]`. The realistic alternatives — lift the crate-wide forbid for one isolated `prctl` helper module, or pull in a `nix` / `prctl` transitive crate dep for a single syscall — are both meaningful policy changes for a corner case that only fires when the user explicitly SIGKILLs `cargo test` during local-dev iteration. Documented in `crates/store-relational-runtime/src/h2_server.rs:30-48` with the manual recovery recipe (`pkill -f h2.tools.Server`). In CI the runner cleans up orphans between runs and the next run gets a fresh port; no observed impact on the test fleet.
- **Document Measure / Unit model invariants** — Closed 2026-05-17 by adding `docs/architecture/measures.md`. Covers the Java M3 source-of-truth definition (`Measure.canonicalUnit: Unit[0..1]`, `Measure.nonCanonicalUnits: Unit[*]`), the one-to-one Rust mapping (`Option<ElementId>` ↔ `Unit[0..1]`, `Vec<ElementId>` ↔ `Unit[*]`), conversion-expression direction (this-unit → canonical), the `Measure~Unit` FQN convention, Pass-1 allocation (`allocate_unit_shells` at `crates/pure/src/pipeline.rs:1184`), Java-parity table, and residual debt (units appearing in `package.children` rather than only under their parent Measure). New `docs/architecture/` directory established for future cross-compiler-runtime metamodel references.

---

## Parser (`crates/parser`)

> Grammar convergence: [`docs/grammar-convergance/`](docs/grammar-convergance/)
> Deferred items: [`docs/deferred/`](docs/deferred/)

### Done ✅
- Full Pure grammar coverage (classes, enums, associations, profiles, functions, measures)
- Graph fetch trees, lambda expressions, precedence-climbing expression parser
- Section headers, island grammar plugins
- Relation types, unit instance expressions

### Open / Deferred

| Item | Priority | Notes |
|------|----------|-------|
| Navigation path grammar | Done ✅ | `#/Type<TArgs>/p1(args)/p2!alias#` shipped through all 4 stages (lexer + AST + parser + composer; `platform_dsl_path/path.pure` embed + Protocol JSON; `ExprKind::PathLiteral` lowering with first-step property validation; `FunctionValue::Path(PathClosure)` runtime that walks each step dynamically against the running receiver). 30 tests across `crates/{lexer,parser,compose,protocol,runtime}/{src,tests}`. Open follow-ups: (a) full `Path<U,V|m>` parametric inference (chain return-type + multiplicity product) — Pass 2.5 currently leaves type-args empty; (b) `CastPathElement` syntax (M2 has the class but Java grammar never surfaced it); (c) variance prefix on type-params (`<-U>`, `<+U>`) is parsed-and-discarded — capture in AST if/when the type system enforces variance. Original design at `docs/deferred/navigation_path_grammar.md`. Milestoning rewrites — Phase A landed 2026-05-19; see Compiler section. |
| Milestoning — Phase A (declare-side) | Done ✅ | Classes carrying `<<temporal.businesstemporal>>` / `<<temporal.processingtemporal>>` / `<<temporal.bitemporal>>` now synthesize the date properties (`businessDate` / `processingDate`), the `milestoning: {Business,Processing,BiTemporal}DateMilestoning[0..1]` slot, and — for each declared property whose target is milestoned — the edge-point `pAllVersions: T[*]` + qualified-property signatures (`p(td: Date[1])`, `pAllVersionsInRange(start, end)`, optionally `p()`, optionally `p(pd, bd)` for bitemporal targets). Original properties move to `Class.original_milestoned_properties` / `Association.original_milestoned_properties`. Four declare-side validators wired into `hydrate_element_signature` (at-most-one temporal stereotype / reserved property names / hierarchy consistency) plus an in-synthesis edge-point name collision check (A4.4). Synthesis pass runs between Pass 2a and Pass 2b in `crate::pipeline` — uses resolved property `type_expr`s to detect milestoned targets. Code: `crates/pure/src/milestoning/{mod,synthesis,validate}.rs`, four new `CompilationErrorKind` variants. Tests: `crates/pure/tests/milestoning_{class_synthesis,property_rewrite,validators}.rs` (15 tests). Round-trip lock: the `original_milestoned_properties` slot serialises via the existing postcard `.purem` path (Class + Association schema fingerprints updated, slice/merge walker visits the new slot). **Deferred:** protocol JSON emission of `original_milestoned_properties` — `crates/protocol/src/v1/convert.rs` is an AST→Protocol path only, so wire-format parity for synthesized properties needs a future Pure→Protocol path; the protocol-side field is reserved but unpopulated, matching today. Java parity: `MilestoningClassProcessor` + `MilestoningPropertyProcessor` + `MilestoningClassValidator`. Plan file: `~/.claude/plans/we-need-now-to-happy-raven.md`. |
| Milestoning — Phase B-1 (runtime natives) | Done ✅ | `getAll(Class, Date)`, `getAll(Class, Date, Date)`, `getAllVersionsInRange(Class, Date, Date)` natives ship in `crates/runtime/src/native/lang.rs` and resolve at the platform-mangled FQNs (`getAll_Class_1__Date_1__T_MANY_`, etc.). PCT coverage audit moved 2→1 Missing (only `getAllVersions(Class)` remains, lands with the AllVersions edge-point runtime in a future batch) and 2→0 SignatureMismatch; ceilings tightened to 3 / 2. 7 smoke tests in `crates/runtime/tests/milestoning_natives_smoke.rs`. Commit `fbaa4c0cf`. |
| Milestoning — Phase B-2 (QP runtime dispatch) | Done ✅ | Phase A's synthesised qualified-property signatures (`address(td)`, `addressAllVersionsInRange(s, e)`, no-arg variants, bitemporal `address(pd, bd)`) now execute at runtime. Approach: runtime smart dispatch — the evaluator recognises generated milestoning QPs by the `<<milestoning.generatedmilestoningproperty>>` stereotype and routes to `crate::milestoning::try_eval_synthesized_qp`, which reads `$this.<orig>AllVersions`, filters by the appropriate date property, and narrows to the declared multiplicity. No QP-body construction at compile time — keeps the AST honest (synthesised bodies remain empty in the IR) and avoids needing to resolve `filter`/`eq`/`toOne` ElementIds at synthesis time. New module `crates/runtime/src/milestoning.rs`; hooked into `eval_qualified_property` in `crates/runtime/src/eval.rs`. 4 smoke tests in `crates/runtime/tests/milestoning_qp_smoke.rs`. |
| Milestoning — Phase B-3 (date propagation) | P1 | Open follow-up. Java's `MilestoningDatesPropagationFunctions` parity, rewriting `$x.address` (where `address` was a user-declared milestoned-target property, since moved aside) → `$x.address($contextDate)` based on enclosing lambda / function date scope. Today the user must write `$x.address($date)` explicitly or use `$x.addressAllVersions` — Phase B-3 makes the implicit-date form work, which is the ergonomic-property-access surface most existing milestoning code expects. Plus three expression-level validators (`%latest` only-in-milestoning, missing-date-arg, `%latest`-not-in-AllVersionsInRange) that depend on the propagation pass to be meaningful. |
| Milestoning — Phase C (grammar shortcuts) | P3 | `.all(%latest)` / `.allVersions()` / `.allVersionsInRange()` syntactic sugar — GAP #12 in `docs/grammar-convergance/engine_vs_rust_analysis.md`. The function-call path (`MyClass->getAll($date)`) works once Phase B lands; this is parity polish, not a blocker. |
| `.purem` Tier-2 lazy mmap (FlatBuffers) | P2 | v0 ships eager-load Postcard via `legend snapshot`. Upgrade Tier-2 to per-body lazy mmap (FlatBuffers/planus or rkyv) once profiling justifies it. Wire format is private to `write_repo`/`read_repo` so the upgrade is contained. Original design notes in `docs/deferred/lazy_loading_binary_format.md`. |
| `.purem` DSL extension registry — Diagram pilot | Done ✅ | Original framing was a side-car `PuremExtension` trait keyed off `extension_arenas`. Reframed during scoping: DSL instances are now first-class graph citizens via a new `Element::DSLInstance { dsl_name, classifier_fqn, data }` variant. The chunk machinery carries them through `.purem` slice/merge alongside every other element — no separate trait, no side-car blob, no `extension_arenas` write path. Diagram pilot (`crates/dsl-diagram/src/compiler.rs`) dual-writes during `declare`: extension RefCell (existing API) + graph element. New `DiagramExtension::diagrams_from_model(&PureModel)` decodes graph entries; round-trip locked by `crates/dsl-diagram/tests/purem_roundtrip.rs`. The `extension_arenas` field on `PureModel` stays untouched (still declared, still unpopulated — its bug was theoretical). Follow-ups: (a) migrate Mapping + Relational extensions onto `Element::DSLInstance` using the same pattern; (b) once consumers (validate pass, codegen, REPL) read from the graph instead of the RefCell, delete the RefCell side. |
| `.purem` Tier-2 lazy mmap | P2 | Separate from the registry work above — see "Tier-2 lazy mmap (FlatBuffers)" entry. |
| Test-element split (history) | Done ✅ | Build always emits `<repo>.purem` (production) and `<repo>.tests.purem` (test slice) per repo. Production binaries embed only the production blob; tests blob is artifact-shape, loaded via classpath auto-discovery for surveyor / PCT / fixture tests. Conservative reachability formulation: `Candidates = T (stereotype-tagged) ∪ {<<access.private>> in tests::* package}; Roots = AllElements \ Candidates; Reachable = forward-walk(Roots); prod = Reachable; test = (AllElements \ chunk0) \ Reachable`. The bare `tests::*` package heuristic was rejected because helpers like `assertEquals` and other public functions can be referenced from external code we have no visibility over. Embedded `platform.purem` shrunk from 3.6 MB to 603 KB (83 % reduction). Slice/merge fixed to translate `ElementId::Package` references through FQN sentinels (previously serialized as stale arena indices). See [`crates/pure/src/purem/filter.rs`](crates/pure/src/purem/filter.rs) and [`crates/snapshot-builder/src/lib.rs`](crates/snapshot-builder/src/lib.rs). Future: tagging platform fixtures with `<<access.private>>` would unlock further safe stripping without compromising correctness. |
| Semantic validations (SV-001..003) | P1 | Compiler-layer responsibility, tracked in `docs/SEMANTIC_VALIDATIONS.md`. **Triage 2026-05-18:** SV-003 closed May 2026 (eager `CannotInferLambdaParameterTypes`). SV-001 (function-test name mismatch) deferred until `Testable` (which `Function` extends) is revisited — function tests are parsed into `FunctionDef.tests` but never lowered into the compiled `Function` model and never executed, so the validator would only catch typos in dead-weight syntax. SV-002 (`->subType` not at root of graph fetch) deferred until the graph-fetch *semantics* are ported from legend-engine to legend-pure — today only the dsl-graph parser/AST/protocol live here, and the structural rule about where `subType` is meaningful belongs with the semantic layer that legend-engine owns. Both items reactivate once their prerequisites land. |

---

## CLI (`crates/cli`)

### Implemented ✅
All 16 subcommands wired in `crates/cli/src/main.rs`:
`parse`, `emit`, `compile`, `check`, `lsp`, `dap`, `mcp`, `test`,
`run`, `coverage`, `init`, `java-bindings`, `completions`, `repl`,
`snapshot`, `version`.

`legend test --pct` runs the platform PCT suite. By default it loads
`crates/runtime/resources/pct_grammar_rust_native.json` and applies the
9 documented exclusions (i16-year overflow + i64-Integer overflow
tests). `--no-default-exclusions` disables that; `--manifest <path>`
substitutes another manifest.

### Open Work

_None._ All historical "Open Work" items shipped. See "Recently
Closed" below for what landed in the 2026-05 sprint.

### Recently Closed
- **`legend run <fqn>` workspace-blind** — Closed 2026-05-17 in commits `b498423b8` and `63d8f9cc4`. `legend run`, `legend test`, `legend repl`, and `legend snapshot` all consume `--classpath <toml>` end-to-end: extension configs flow through `crate::classpath::resolve_classpath` into `Evaluator::builder().extension_configs(...)`, and the resolved repos drive `repo::load(resolved.repos, …)` when `--classpath` is explicit. `--live` / `--watch` / `--platform-dir` become no-ops with a stderr warning. Locked in by `test_test_command_with_explicit_classpath_loads_repos` (`crates/cli/tests/cli_integration.rs`).
- **`--classpath` flag wiring (Parser → Open / Deferred)** — Same closure as above; the entry was duplicated across the Parser and CLI sections of the backlog. Both reference the same end-to-end wiring through every CLI subcommand.

`plan` / `package` / `publish` were removed from the CLI surface on
2026-05-16 (commit `e71a1ea0a`). Their stubs returned
`CliError::NotImplemented` while advertising functionality that didn't
exist; design context for when the prerequisites land lives in
`docs/deferred/cli_{plan,package,publish}.md`.

---

## Java Bindings (`crates/java-codegen`)

### Implemented ✅ (v1)
`legend java-bindings` emits a single static-method facade plus one
interface per reachable user `Class` (and enum per `Enumeration`). The
hand-written runtime support lives in
`legend-pure-runtime/legend-pure-runtime-rust-evaluator/src/main/java/
org/finos/legend/pure/rust/proxy/` (`PureRegistered`,
`PureProxyFactory`, `PureInvocationHandler`, `PureLambda` placeholder).

Reachability is seeded from `--functions` parameter and return types,
plus optional `--classes <FQN>` / `--associations <FQN>` flags that
extend the seed set with explicit Pure elements (an association seed
unfolds into both participating classes). Explicit class seeds bypass
the `meta::pure::*` platform-class filter; transitive walks from them
still apply the filter.

For Maven-style "bootstrap an evaluator" workflows there is also
`--bindings-file <PATH>` — one FQN per line, kind auto-detected by the
helper `legend_pure_java_codegen::dispatch_bindings_by_kind` (shared
between the CLI and the annotation processor described below).

The `legend-pure-runtime-rust-evaluator` Maven module drives this
codegen at compile time through a sibling
`legend-pure-runtime-rust-evaluator-bindings-ap` annotation-processor
module: a `@PureBindings` marker on
`org.finos.legend.pure.rust.bootstrap.M3Bootstrap` triggers
`PureBindingsProcessor` to load `libpure_rust_jni.{dylib,so,dll}`,
call `nativeGenerateBindings` (`crates/jni/src/codegen.rs`), and emit
the produced Java sources via `Filer` into
`target/generated-sources/annotations/`. Same Rust codegen library,
no `cargo` subprocess fork, no `<build-helper-maven-plugin>` source
root wiring — javac picks up the AP-emitted sources automatically.
Pass `-Dmaven.compiler.proc=none` to skip generation on a host
without a built cdylib.

Generic-typed property/QP returns on generated interfaces (e.g.
`Enumeration<E>.values: E[*]`) render as `Object` / `Iterable<Object>`
under a `GenericPolicy::AsObject` mode; static-facade function
signatures still hard-fail on generics (`GenericPolicy::Reject`).

`[0..1]` / `[*]` properties get `default` bodies returning
`Optional.empty()` / `Collections.emptyList()`, so user `implements`
classes only override the fields they care about. Proxies always go
native, bypassing the default body.

`PureProxyFactory.create(userImpl, iface, eval)` materialises a
hand-written interface implementation as a real heap object via a new
`nativeNew` JNI native (`crates/jni/src/lib.rs` →
`JniContext::new_object` → `RuntimeHeap::alloc_dynamic` +
`mutate_set`). Recursive: nested user impls become heap children,
`Optional` and `Iterable` are unpacked, existing proxies pass through
without re-materialisation. Cycle / sharing detection uses an
`IdentityHashMap` keyed by user object identity.

The hand-written `org.finos.legend.pure.rust.proxy.Any` interface
mirrors Pure's `meta::pure::metamodel::type::Any` as the universal
proxy supertype: every generated interface extends it (transitively),
the codegen substitutes references to the Pure FQN at emission time
and never emits an `Any.java`, and `pickInterface` falls back to
`Any.class` whenever the runtime classifier isn't registered for any
more-specific generated interface — so `wrap(...)` always returns a
typed proxy with a `$rustInstance()` drop-down to dynamic dispatch
via `PureRustInstance.getProperty(...)`.

Known limitation (advisor follow-up): `pickGeneratedInterface` only
walks declared interfaces of the user class (not the inherited
chain), so a user class whose *parent* implements the generated
interface won't resolve. Adequate for the `class MyPerson implements
Person` case, needs a fix for deeper hierarchies.

**Diamond-inheritance break (2026-05-07; multiplicity/type narrowing
added 2026-05-15).** Pure's M3 lets a subclass *redeclare* an
inherited property as a no-op shadow (`Type.name` shadows
`ModelElement.name`, etc.). When two such ancestors-in-closure each
emit a `default` method for the same property name, Java rejects the
joining interface ("inherits unrelated defaults"). The codegen now
(a) suppresses redeclarations on the child whose direct
supertype-in-closure already declares the same property name,
(b) emits an explicit `@Override default` on any joining class whose
supertype graph carries two distinct declarations of the same simple
property, and (c) when the colliding declarations disagree, takes the
**intersection** of all multiplicities (`max(lower)..min(upper)`) and
the **most-specific common type** so the emitted method satisfies
every upstream contract. The cross-hierarchy property override
validator (`crates/pure/src/validate.rs::validate_property_overrides`)
rejects mismatched-mult/type diamonds before they reach codegen, but
the narrowing logic stays sound when codegen runs on a partial
(unvalidated) model. See
`crates/java-codegen/src/interfaces.rs::diamond_overrides`,
`intersect_multiplicities`, and `most_specific_type`, plus
`tests/diamond_overrides.rs`.

Verification: 16 codegen unit + integration tests (`-p
legend-pure-java-codegen`) + 4 new `dispatch_smoke` tests for the
relocated `dispatch_bindings_by_kind` helper, including a `javac
--release 11` round-trip on the generated set + the runtime support
classes (`tests/javac_compiles.rs`). 5 JNI unit tests (`-p
legend-pure-parser-jni`) exercise the FFI-free inner of
`nativeGenerateBindings`. Live `mvn compile` of
`legend-pure-runtime-rust-evaluator` regenerates 50 Java sources from
the curated M3 manifest via the annotation processor and compiles
them clean alongside the hand-written runtime classes; the byte-set
is identical to what the CLI emits.

### Open Work (v2)

| Item | Priority | Notes |
|------|----------|-------|
| Function-typed parameters (`map`, `filter`, `fold`, …) | P1 | v1 errors at codegen. v2 should add a `PureLambda` functional interface (placeholder file already exists) and a JNI callback path that lets a Pure-side `Function<{T[1]→V[1]}>` invoke a Java lambda. The cliff for usability — most platform collection operators take a callable. |
| Generic type-arg propagation on user classes | P1 | `Person.friends: List<Person>` currently renders as `Object friends()` (the platform `List<T>` is filtered as a `meta::pure::*` class). v2 should let collection-shaped platform types (`List`, `Pair`, `Map`) project to typed Java collections by special-casing them or by walking their type-arguments. |
| Streaming `[*]` returns | P3 | v1 materialises lists eagerly into `java.util.List`. v2 could expose `Iterable` lazily or a `Stream`. |
| Round-trip JNI integration test | P2 | The plan called for a JUnit test that loads `libpure_rust_jni`, calls a generated wrapper, fetches a heap object, traverses via the Proxy, and asserts subtype-aware dispatch. Substituted in v1 by `tests/javac_compiles.rs` which proves the generated code is syntactically valid against `--release 11` together with the runtime support — but the live-evaluator round-trip remains to be wired (Maven module + cdylib loading). |
| AP processor unit test (Java side) | P2 | The Rust-side codegen, the JNI shim, and the byte-parity diff between CLI and AP outputs are covered. A dedicated Java-side AP test (e.g. via `com.google.testing.compile` or `javax.tools.ToolProvider.getSystemJavaCompiler`) that exercises `PureBindingsProcessor` end-to-end against a fixture annotated with `@PureBindings` is the missing piece — would have caught the cdylib-arch and `<provided>` dep wiring issues earlier. |
| Multi-platform cdylib bundling | P2 | Today `PureBindingsProcessor` requires the developer to build a host-arch cdylib at `-Apure.cdylib.path`. Next phase: GitHub Actions matrix builds `libpure_rust_jni.{dylib,so,dll}` for `{macOS-arm64, macOS-x64, Linux-x64, Linux-arm64, Windows-x64}`, the `legend-pure-runtime-rust-evaluator-bindings-ap` JAR ships them under `META-INF/native/<os>-<arch>/`, and a `NativeLibraryLoader` extracts the right binary on the fly. Removes the Rust-toolchain prereq for downstream Java consumers. |
| Cdylib loaded via Filer (incremental tracking) | P2 | `PureBindingsGenerator.ensureLoaded()` currently `System.load`s the cdylib at the absolute path supplied by `-Apure.cdylib.path`. That bypasses javac's incremental-build dependency tracking — Develocity / Maven won't notice when the cdylib changes and won't re-run the AP. Future: read the cdylib through `processingEnv.getFiler().getResource(StandardLocation.CLASS_OUTPUT, "", path)` (or `CLASS_PATH`) so it's recorded as a compile input. Probably needs the cdylib bundled inside the AP JAR (see "Multi-platform cdylib bundling" above) so a Filer-resource path resolves cleanly. |
| Rust-classpath descriptor read by the AP | P3 | Forward-looking: the AP currently only knows about Pure FQNs declared in `.jpure` manifests. A future enhancement could let the AP also consume a Rust-side classpath descriptor (e.g. `META-INF/legend-pure-bindings/<artifact>.classpath.json`) that lists which Pure repos / modules each upstream module contributes, so the codegen knows where each FQN was bound without the user re-declaring it via `@import:`. Pairs naturally with the auto-discovered sidecar (`META-INF/legend-pure-bindings/<artifact>.bindings.json`) idea below. |
| Multiplicity narrowing across diamond declarations | P3 | The current `diamond_overrides` policy picks the first ancestor's declaration. If two ancestors disagree on multiplicity (e.g. `[0..1]` vs `[1]`), the override picks one and the other branch's signature is not honoured. Rare in M3; revisit if a real case shows up. |
| Relation-typed parameters/returns | P3 | Currently rejected at codegen. |
| Mapping / Diagram / Path / TDS DSL elements | P3 | Out of scope for v1. |
| Two-way binding (mutating Pure objects from Java) | P3 | Read-only in v1. |

---

## How To Use This Document

1. **Pick an item** matching your skill and the current priority
2. **Check the detailed backlog** in the per-crate file for full context
3. **Update status** when you start/finish work
4. **Add new items** as you discover gaps during development
