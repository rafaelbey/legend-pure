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
| CLI | 🚧 Partial | N/A | ~20+ | 8/11 commands; `legend test --pct` defaults to bundled `pct_grammar_rust_native.json` exclusions |
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
| Full generic unification (`Z` propagation) | P2 | 🚧 Drift baselined; structural compat shipped (Item 3) | Per-expression drift histogram baseline 5931 / ceiling 6500 (`crates/core-platform-pure/tests/inference_drift_histogram.rs`). Higher-order eval-arg-mismatch closed via `is_type_compatible_structural` — type-arguments-aware compat that recurses through nested `Named.type_arguments` and `FunctionType` shapes; the diagnosis turned out to be local to the compat check (not the binding pipeline), avoiding the prior-spikes graveyard. `function_type_higher_order_wrong_inner_type_errors` un-ignored and green. The dormant `TypeInferenceContext` skeleton stays unwired — driven by no concrete RED test today. Future work: `pushTypeInferenceContextAhead` lambda-deferral and overload-elimination retry loop, both gated on a future failing test. |
| Lambda parameter type inference | P2 | Two-phase fix from `b4ff09432a5` covers the literal-arrow / direct-application form: when a lambda flows into a `Function<{T[m]→…}>` parameter and the caller supplies known-type arguments, lambda-param types are rebound before body lowering (`testEvalTwo` passes). Open: per-call-site specialisation when the same body is reused with multiple concrete types (`let f = {x,y\|$x+$y}; $f->eval(1,2); $f->eval('a','b')`). The let-bound case now fails eagerly with `CannotInferLambdaParameterTypes` instead of cascading "Ambiguous function call" — see [plan](../../.claude/plans/lambda-parameter-type-inference-encapsulated-pebble.md). To enable per-call-site specialisation, lift the type-hole (`TypeExpr::Unresolved`) at the call site by re-lowering the lambda body with caller-supplied expectations. |
| Numeric coercion (`Integer` → `Float`) | P3 | Java has implicit widening |
| Return type influence on dispatch | P3 | Expected return type narrows candidates |
| Move bootstrap chunk 0 to compile-time | P2 | Saves ~1ms startup |
| Unit as child of Measure (canonical M3) | P2 | User-visible `package.children` now excludes Units (Java parity); residual debt is making units indexed only on the Measure internally. |
| Type inference (bottom-up) | P2 | Pass 2.5 in pipeline |
| Parallel Pass 2 | P3 | Bodies can be parallelized per-element |
| Incremental compilation | P3 | Re-resolve only changed chunks |
| Stress bench `compile/hub_spoke_1k` panics | P2 | `crates/stress/benches/pipeline.rs:343` unwraps a `Result` but the synthesized hub_spoke source emits `plus()` calls that are unresolved without the platform. Either load the platform before benching `compile/*`, or change the hub_spoke generator to avoid platform-only operators. |
| `meta::relational::metamodel::schema` widens `$schemas` to `Any` | P2 | Investigation during the Bug 2 fix (2026-05-02) showed that even on first compile, the receiver of `$schemas.tables` at `functions.pure:232:40` is typed as `Any[*]` rather than `Schema[*]`. The error stays silent on the live compile path because `is_metatype_carrier(ANY_ID)` returns `true` and `infer_property_access` returns `UnknownTarget`, but the type widening itself is a real inference gap — probably in how `concatenate<T>` binds T or how the lambda parameter `d` flows through `$d->cast(@Database)->schema(...)` inside the surrounding `map` call. Worth investigating once the residual dispatch ambiguity (above) is closed; not blocking. |
| Repo descriptors + manifest (Pure-graph composition) | P1 | Java Pure ships a JSON descriptor next to every repo's `src/main/resources/`: `platform.json`, `platform_dsl_store.definition.json`, `platform_dsl_diagram.definition.json`, `platform_dsl_mapping.definition.json`, `platform_store_relational.definition.json`. Shape: `{ name, pattern (regex over FQN), dependencies: [name…] }`. Sources live relative to the descriptor. Today `crates/core-platform-pure/build.rs` walks two hand-listed directories (`platform`, `platform_dsl_store`); replace with a manifest-driven loader: (1) parse one descriptor JSON into `RepoDescriptor { name, pattern: Regex, dependencies, source_root }`; (2) top-level manifest enumerates repos to compose; (3) topo-sort by dependencies; (4) validate every `.pure` file's declared FQN against its repo's `pattern` regex (build-time error on mismatch — this is the layering primitive); (5) keep the loader trait open for future `.purem` (binary) and remote artifact-coordinate (Maven-style) source kinds. Goal: compose the Pure graph through descriptor files instead of hardcoded paths. The current hand-list is an interim stopgap. |

### Recently Closed
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
| Distributed-slice self-registration (`linkme`) | P2 | Auto-register natives |
| Standard registry built into `Evaluator` | P2 | Default `Evaluator::new` without explicit registry |
| Compiled function support | P2 | AOT-compiled Pure → Rust for hot paths |
| Full PCT native coverage | P1 | Complete remaining stdlib functions |
| Native arity checks: drop or keep? | P2 | Every native calls `expect_args("name", &values, N)`. The compiler's Pass 2 dispatcher already narrows overloads by param count during FQN mangling, so the arity is structurally correct by the time a call reaches the registry. Investigate: (a) can any call path reach a native with the wrong arity (reflective calls via `eval` / `evaluate` / `apply_callable`)? (b) if not, replace `expect_args` with `debug_assert_eq!` or delete it to cut ~50 lines of boilerplate. If yes, keep it but move to a shared wrapper so the message format stays consistent. |
| Non-recursive structural equality | P2 | `equality.rs::values_equal` / `objects_equal` recurse on every collection element and every `<<equality.Key>>` property. Pathological inputs (deep `List<List<…>>`, long `Pair<U, Pair<…>>` chains, equality-keyed self-references) will blow the Rust stack — default 8 MB on the main thread but as little as 512 KB on spawned threads (LSP, test runners, DAP). Path: explicit work-stack (`Vec<(Value, Value)>`) with cycle guard (`HashSet<(ObjectId, ObjectId)>`) plus precomputed equality-key cache on Class. Same audit needed for `toRepresentation`, `toString`, `deep_copy_object` in Copy, and `reactivate_value`. Stopgap: depth-limit guard exists today; full iterative rewrite is the long-term fix. |
| Step 5 — class()/genericType() return ObjectId via BiMap | P2 | `TypeOf` and `GenericTypeOf`'s `rawType` slot return `Value::Element` rather than `Value::Object` projected via the BiMap. Required for parametric-type chains (`^MyClass<String>()->class()->typeArguments`) to thread bindings end-to-end. Tried 2026-04-26: regresses surveyor 211/0/0 → 188/15/8 because `Match`'s `value_matches_type`, `ElementToPath`'s value-variant dispatch, and `class.pure`'s `cast(@Class<T>)` chain all read `Value::Element` strictly. Lock-step audit of every metamodel-consuming native required before re-attempting — defer until a concrete test surfaces the chain. (BiMap is now `RuntimeHeap::element_to_object: HashMap<ElementId, ObjectHandle>`.) |
| Wire Java `AutoCloseable` to `JniContext::release` | P2 | `JniHandleTable` exposes `release(i64)` to drop the strong `Rc` for a JNI-handed-out object. Today Java never calls it, so entries leak for the `JniContext`'s lifetime — same monotonic behaviour as the pre-Rc heap. Have Java's `PureRustResult` (or whatever wraps the `i64`) implement `AutoCloseable` so try-with-resources cleans up. Path: `legend-pure-runtime/legend-pure-runtime-rust-evaluator/src/main/java/org/finos/legend/pure/rust/PureRustEvaluator.java` (and the result wrapper). |
| Promote PT-key memoization-cache holders to `Weak` | P3 | The memoization cache currently holds strong `Rc`s on cached object results, so cached entries keep their objects alive until cleared. Matches Java semantics, but in long-running REPL/test-runner sessions a cache flush is the only way to reclaim. Consider a `Weak`-based variant for a "soft" cache mode. |
| Reactivate inner-call dispatch + InstanceValue unwrap | P2 | `[xs]->map(p\|$p.lastName)->deactivate()->reactivate()` errors with `"reactivate: simple-name dispatch for SimpleFunctionExpression without 'func' is not implemented"` (`meta.rs:2435-2448`). Path forward: in `reactivate_value` for InstanceValue, unwrap `.values` but distinguish lambdas by parameter arity — zero-arg are "deactivated expressions to invoke", parametrised are "first-class function values to pass through". Plus: `parametersValues` needs an additional InstanceValue-unwrap layer because deactivated collections wrap as `InstanceValue { values: [p1, …] }`. Not currently exercised by any surveyor test — defer until a concrete test forces it. |
| Cleaner SourceInformation name-span representation | P2 | Currently we carry `ElementNode { source_info, name_source_info, … }` in `crates/pure/src/model.rs` — two parallel `SourceInfo`s per element. Matches Java's `SourceInformation(startLine, startColumn, line, column, endLine, endColumn)` but feels like the wrong layer. Recommended: collapse into the AST's `SourceInfo` to match Java's 7-field struct directly. Gate on AST memory-footprint measurement first. |
| Snapshot-builder section parsers (`###Relational`, `###Mapping`, …) | P2 | `crates/snapshot-builder/src/lib.rs:482` calls `parse_with_islands` only; section parsers aren't wired. Means a `.pure` file inside an embedded repo can't contain `###Relational Database … ###Pure function …`. Today the only ###-section sources in the platform repos are M3 metamodel definitions, but as soon as a repo wants to ship a `<<test.Test>>` function that depends on a Database/Mapping/Diagram declaration (e.g. testing `loadValuesToDbTable($outer, mydb.schemas->at(0).tables->at(0), $db)`) the snapshot fails to compile with `Unexpected token Database`. Path: extend `Embedder` / `snapshot-builder` with a per-repo `section_parsers: Vec<Box<dyn SectionParser>>` hook; the platform repo descriptors declare which DSL sections their sources may use. Unlocks DSL-driven loadCsv/loadValues coverage on platform-shipped Pure tests. |
| `testTempTableDroppedInFinally` semantic | P3 | Java native asserts the runtime auto-drops a temp table at the enclosing function-call exit when `relyOnFinallyForCleanup=false`. The Rust port lacks a function-call teardown hook — only Evaluator-drop teardown, which is too coarse. Current behaviour: callers must `dropTempTable` explicitly. End-result is similar to Java (drop happens, just later — at evaluator scope, not function-call scope) because connection lifecycle bounds it. Per current decision the contract is dropped; revisit if a use-case surfaces real divergence. |
| Document Measure / Unit model invariants | P3 | Cross-reference Java Pure's `M3Properties.canonicalUnit` to clarify the Measure → Unit relationship: canonical unit (marked with `*`, exactly one per Measure), non-canonical units (each carrying a conversion lambda back to canonical). `Measure.canonicalUnit: Unit[0..1]`, `Measure.nonCanonicalUnits: Unit[*]` reflection properties. Compiled node lives in `crates/pure/src/nodes/measure.rs`. Add `docs/architecture/measures.md` so future runtime work doesn't reverse-engineer this from test diffs. |

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
| Navigation path grammar | Done ✅ | `#/Type<TArgs>/p1(args)/p2!alias#` shipped through all 4 stages (lexer + AST + parser + composer; `platform_dsl_path/path.pure` embed + Protocol JSON; `ExprKind::PathLiteral` lowering with first-step property validation; `FunctionValue::Path(PathClosure)` runtime that walks each step dynamically against the running receiver). 30 tests across `crates/{lexer,parser,compose,protocol,runtime}/{src,tests}`. Open follow-ups: (a) full `Path<U,V|m>` parametric inference (chain return-type + multiplicity product) — Pass 2.5 currently leaves type-args empty; (b) `CastPathElement` syntax (M2 has the class but Java grammar never surfaced it); (c) milestoning rewrites; (d) variance prefix on type-params (`<-U>`, `<+U>`) is parsed-and-discarded — capture in AST if/when the type system enforces variance. Original design at `docs/deferred/navigation_path_grammar.md`. |
| `.purem` Tier-2 lazy mmap (FlatBuffers) | P2 | v0 ships eager-load Postcard via `legend snapshot`. Upgrade Tier-2 to per-body lazy mmap (FlatBuffers/planus or rkyv) once profiling justifies it. Wire format is private to `write_repo`/`read_repo` so the upgrade is contained. Original design notes in `docs/deferred/lazy_loading_binary_format.md`. |
| `--classpath` flag wiring | P2 | `legend_cli::classpath::load_classpath` already builds `Vec<Repo>` from a TOML; thread it through `check`, `compile`, `test`, `repl`, `run`, `snapshot` so the CLI accepts `--classpath <PATH>` end-to-end. |
| `.purem` DSL extension registry — Diagram pilot | Done ✅ | Original framing was a side-car `PuremExtension` trait keyed off `extension_arenas`. Reframed during scoping: DSL instances are now first-class graph citizens via a new `Element::DSLInstance { dsl_name, classifier_fqn, data }` variant. The chunk machinery carries them through `.purem` slice/merge alongside every other element — no separate trait, no side-car blob, no `extension_arenas` write path. Diagram pilot (`crates/dsl-diagram/src/compiler.rs`) dual-writes during `declare`: extension RefCell (existing API) + graph element. New `DiagramExtension::diagrams_from_model(&PureModel)` decodes graph entries; round-trip locked by `crates/dsl-diagram/tests/purem_roundtrip.rs`. The `extension_arenas` field on `PureModel` stays untouched (still declared, still unpopulated — its bug was theoretical). Follow-ups: (a) migrate Mapping + Relational extensions onto `Element::DSLInstance` using the same pattern; (b) once consumers (validate pass, codegen, REPL) read from the graph instead of the RefCell, delete the RefCell side. |
| `.purem` Tier-2 lazy mmap | P2 | Separate from the registry work above — see "Tier-2 lazy mmap (FlatBuffers)" entry. |
| Test-element split (history) | Done ✅ | Build always emits `<repo>.purem` (production) and `<repo>.tests.purem` (test slice) per repo. Production binaries embed only the production blob; tests blob is artifact-shape, loaded via classpath auto-discovery for surveyor / PCT / fixture tests. Conservative reachability formulation: `Candidates = T (stereotype-tagged) ∪ {<<access.private>> in tests::* package}; Roots = AllElements \ Candidates; Reachable = forward-walk(Roots); prod = Reachable; test = (AllElements \ chunk0) \ Reachable`. The bare `tests::*` package heuristic was rejected because helpers like `assertEquals` and other public functions can be referenced from external code we have no visibility over. Embedded `platform.purem` shrunk from 3.6 MB to 603 KB (83 % reduction). Slice/merge fixed to translate `ElementId::Package` references through FQN sentinels (previously serialized as stale arena indices). See [`crates/pure/src/purem/filter.rs`](crates/pure/src/purem/filter.rs) and [`crates/snapshot-builder/src/lib.rs`](crates/snapshot-builder/src/lib.rs). Future: tagging platform fixtures with `<<access.private>>` would unlock further safe stripping without compromising correctness. |
| Semantic validations (SV-001..003) | P1 | Compiler-layer responsibility, tracked in `docs/SEMANTIC_VALIDATIONS.md` |

---

## CLI (`crates/cli`)

### Implemented ✅
`parse`, `check`, `init`, `version`, `completions`, `emit`, `test`,
`java-bindings`

`legend test --pct` runs the platform PCT suite. By default it loads
`crates/runtime/resources/pct_grammar_rust_native.json` and applies the
9 documented exclusions (i16-year overflow + i64-Integer overflow
tests). `--no-default-exclusions` disables that; `--manifest <path>`
substitutes another manifest.

### Open Work

| Item | Priority | Notes |
|------|----------|-------|
| `legend run <fqn>` is workspace-blind | P2 | The subcommand evaluates `fqn` against the embedded platform only — there's no `--classpath` plumbing, so any user-defined FQN dies with `Function not found`. The IntelliJ ▶ gutter sidesteps this entirely by routing through the LSP's `workspace/executeCommand`, which evaluates against the LSP's compiled `PureModel` (open buffers + classpath cascade). Terminal users still need the CLI parity: thread `legend_cli::classpath::load_classpath` into `crates/cli/src/commands/run.rs` so `legend run --classpath <toml> <fqn>` resolves the same set of files the editor sees. Shares fix with the `--classpath` flag-wiring entry under "Parser → Open / Deferred". |

### Stubs 🚧

| Command | Priority | Blocked On |
|---------|----------|------------|
| `compile` | P1 | Compiler crate stabilization |
| `plan` | P2 | Execution plan generation |
| `package` | P2 | Compilation + artifact packaging |
| `publish` | P3 | Package + registry upload |

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

**Diamond-inheritance break (2026-05-07).** Pure's M3 lets a subclass
*redeclare* an inherited property as a no-op shadow (`Type.name`
shadows `ModelElement.name`, etc.). When two such ancestors-in-closure
each emit a `default` method for the same property name, Java
rejects the joining interface ("inherits unrelated defaults"). The
codegen now (a) suppresses redeclarations on the child whose direct
supertype-in-closure already declares the same property name, and
(b) emits an explicit `@Override default` on any joining class whose
supertype graph carries two distinct declarations of the same simple
property. Multiplicity narrowing across distinct ancestors is rare
in M3 and not handled in v1 — would need a richer override-emission
policy. See `crates/java-codegen/src/interfaces.rs::diamond_overrides`.

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
