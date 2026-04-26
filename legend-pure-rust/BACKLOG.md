# Legend Pure Rust — Backlog

Unified backlog for the Rust workspace. Each component's backlog is detailed
in its crate directory; this file provides the high-level view.

**Priority legend:** P0 (blocking) → P1 (important) → P2 (architectural) → P3 (nice-to-have)

---

## Milestone Status

| Component | Status | Platform Errors | Test Count | Key Milestone |
|-----------|--------|----------------|------------|---------------|
| Parser | ✅ Complete | 0 | ~400+ | Full grammar coverage |
| Compiler | ✅ Platform clean | **0** | ~50+ | 236 files / 1338 elements; `load_platform()` compiles with **zero errors** |
| Runtime | ✅ Surveyor 246/0/0; PCT 465/465 | N/A | 357+ lib + 71 eval + 246 surveyor | PCT broad-canary at 100% with 9 manifest exclusions |
| CLI | 🚧 Partial | N/A | ~20+ | 8/11 commands; `legend test --pct` defaults to bundled `pct_grammar_rust_native.json` exclusions |
| **Total** | | **0** | **739+** | |

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
| Full generic unification (`Z` propagation) | P2 | Java uses `TypeInferenceObserver` |
| Lambda parameter type inference | P2 | Infer from expected `Function<{…}>` type. Also required for operator dispatch inside generic lambda bodies — e.g. `{x, y | $x + $y}->eval('1', '2')` binds `Z = String` at the call site; without the inference the body lowers `+` to the numeric `plus` FQN and string arguments reach the wrong native (`testEvalTwo` regression). Fix: when a lambda flows into a `Function<{T[m]->…}>` parameter and the caller supplies known-type arguments, rebind lambda-param types before lowering the body; per-call-site specialisation if the same body is reused for multiple types. |
| Numeric coercion (`Integer` → `Float`) | P3 | Java has implicit widening |
| Return type influence on dispatch | P3 | Expected return type narrows candidates |
| Move bootstrap chunk 0 to compile-time | P2 | Saves ~1ms startup |
| M3 parser completeness (constraints, QPs) | P1 | Missing from `m3_parser.rs` |
| Unit as child of Measure (canonical M3) | P1 | Currently promoted to package-level |
| Root package `::` references | P1 | 16 expression lowering errors |
| Lambda variable scope in expressions | P1 | 6+ errors |
| Package-as-value references | P1 | 5+ errors |
| Compilation tracing | P1 | `tracing` instrumentation for pipeline |
| Type inference (bottom-up) | P2 | Pass 2.5 in pipeline |
| Parallel Pass 2 | P3 | Bodies can be parallelized per-element |
| Incremental compilation | P3 | Re-resolve only changed chunks |

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

### Performance Baselines

Captured 2026-04-26 from `cargo bench --bench runtime_eval -- --quick` on
M-series macOS, debug+release. Track regressions >20% from these:

| Benchmark | Time | Notes |
|---|---|---|
| `eval_function_call/trivial_return_literal` | ~17 µs | Smallest possible call — `function(): Integer[1] { 42 }` |
| `eval_function_call/call_with_one_int_arg` | ~18 µs | `add_one(x:Integer[1]):Integer[1] { $x + 1 }` |
| `eval_function_call/evaluator_setup_only` | **~285 µs** | `Evaluator::new(&model, &registry)` — heap + bootstrap_metamodel walk over 1300+ M3 elements. Dominates every other measurement when calls reuse a model; **the hot fix target** if call-throughput becomes a goal |
| `eval_property_access/read_property_inline_construct` | ~20 µs | `^Person(...)..firstName` |
| `eval_property_access/read_via_let_binding` | ~21 µs | `let p = ^Person(...); $p.age` |
| `eval_lambda/map_double_5` | ~20 µs | `[1..5]->map(x \| $x * 2)` |
| `eval_lambda/map_concat_typed_lambda_param` | ~20 µs | `['a','b','c']->map(s \| $s + 'X')->joinStrings(',')` — locks lambda-param narrowing fix from b4ff09432a5 |
| `eval_lambda/filter_then_map` | ~30 µs | 10-element filter+map chain |
| `eval_relation/addColumns_2cols_to_1col_source` | ~28 µs | The full `@(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne()->addColumns(~[ab,z])` chain — locks the surveyor-211/0/0 path |

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
| Step 5 — class()/genericType() return ObjectId via BiMap | P2 | `TypeOf` and `GenericTypeOf`'s `rawType` slot return `Value::Element` rather than `Value::Object` projected via the BiMap. Required for parametric-type chains (`^MyClass<String>()->class()->typeArguments`) to thread bindings end-to-end. Tried 2026-04-26: regresses surveyor 211/0/0 → 188/15/8 because `Match`'s `value_matches_type`, `ElementToPath`'s value-variant dispatch, and `class.pure`'s `cast(@Class<T>)` chain all read `Value::Element` strictly. Lock-step audit of every metamodel-consuming native required before re-attempting — defer until a concrete test surfaces the chain. |
| Reactivate inner-call dispatch + InstanceValue unwrap | P2 | `[xs]->map(p\|$p.lastName)->deactivate()->reactivate()` errors with `"reactivate: simple-name dispatch for SimpleFunctionExpression without 'func' is not implemented"` (`meta.rs:2435-2448`). Path forward: in `reactivate_value` for InstanceValue, unwrap `.values` but distinguish lambdas by parameter arity — zero-arg are "deactivated expressions to invoke", parametrised are "first-class function values to pass through". Plus: `parametersValues` needs an additional InstanceValue-unwrap layer because deactivated collections wrap as `InstanceValue { values: [p1, …] }`. Not currently exercised by any surveyor test — defer until a concrete test forces it. |
| Cleaner SourceInformation name-span representation | P2 | Currently we carry `ElementNode { source_info, name_source_info, … }` in `crates/pure/src/model.rs` — two parallel `SourceInfo`s per element. Matches Java's `SourceInformation(startLine, startColumn, line, column, endLine, endColumn)` but feels like the wrong layer. Recommended: collapse into the AST's `SourceInfo` to match Java's 7-field struct directly. Gate on AST memory-footprint measurement first. |
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
| Navigation path grammar | P2 | See `docs/deferred/navigation_path_grammar.md` |
| Lazy loading binary format | P2 | See `docs/deferred/lazy_loading_binary_format.md` |
| Semantic validations (SV-001..003) | P1 | Compiler-layer responsibility, tracked in `docs/SEMANTIC_VALIDATIONS.md` |

---

## CLI (`crates/cli`)

### Implemented ✅
`parse`, `check`, `init`, `version`, `completions`, `emit`, `test`

`legend test --pct` runs the platform PCT suite. By default it loads
`crates/runtime/resources/pct_grammar_rust_native.json` and applies the
9 documented exclusions (i16-year overflow + i64-Integer overflow
tests). `--no-default-exclusions` disables that; `--manifest <path>`
substitutes another manifest.

### Stubs 🚧

| Command | Priority | Blocked On |
|---------|----------|------------|
| `compile` | P1 | Compiler crate stabilization |
| `plan` | P2 | Execution plan generation |
| `package` | P2 | Compilation + artifact packaging |
| `publish` | P3 | Package + registry upload |

---

## How To Use This Document

1. **Pick an item** matching your skill and the current priority
2. **Check the detailed backlog** in the per-crate file for full context
3. **Update status** when you start/finish work
4. **Add new items** as you discover gaps during development
