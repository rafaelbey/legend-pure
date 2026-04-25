# Legend Pure Rust — Backlog

Unified backlog for the Rust workspace. Each component's backlog is detailed
in its crate directory; this file provides the high-level view.

**Priority legend:** P0 (blocking) → P1 (important) → P2 (architectural) → P3 (nice-to-have)

---

## Milestone Status

| Component | Status | Platform Errors | Test Count | Key Milestone |
|-----------|--------|----------------|------------|---------------|
| Parser | ✅ Complete | 0 | ~400+ | Full grammar coverage |
| Compiler | ✅ Platform clean | 0 | ~50+ | 236 files / 1338 elements |
| Runtime | 🚧 Active | N/A | ~160+ | ~50 natives, PCT subset |
| CLI | 🚧 Partial | N/A | ~20+ | 6/11 commands |
| **Total** | | **0** | **739** | |

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

### Open Work

| Item | Priority | Notes |
|------|----------|-------|
| One file per native function | P2 | Split domain bundles into individual files |
| Distributed-slice self-registration (`linkme`) | P2 | Auto-register natives |
| Standard registry built into `Evaluator` | P2 | Default `Evaluator::new` without explicit registry |
| Compiled function support | P2 | AOT-compiled Pure → Rust for hot paths |
| Full PCT native coverage | P1 | Complete remaining stdlib functions |
| `+=` augmented assignment + Function-element materialisation | P1 | The parser already eats the `+` token in `^$src(prop += val)` / `^Class(prop += val)` but discards the flag; the runtime always uses `mutate_set` so `+=` silently behaves like `=`. Three platform tests fail today: `testFlatCopyObjectToManyAdd`, `testFlatCopyPrimitiveToManyAdd`, `testProperties` (the platform-level `properties()` function uses `+=` to concatenate `propertiesFromAssociations` and `generalizations`). The structural fix is straightforward — add `augmented: bool` to `ast::KeyValuePair`, change the compiler's `lower_copy` / `lower_new_instance` to emit `(key, value, augmented)` triples instead of flat pairs, and have `apply_key_value_pairs` (rename to `apply_key_value_triples`) branch on the flag (`mutate_add` vs `mutate_set`). This was prototyped in a session, gated cleanly, and flipped the two `testFlatCopy*ToManyAdd` tests + made `properties()` correctly accumulate. **Blocking discovery**: with `+=` working, the platform `properties()` function returns the *full* list including reflective properties like `applications`, `expressionSequence`, etc. `testFunctionDefinitionCopy` then fails: `^$f1()` (Function Element → heap Object copy) produces an Object with no properties carried over, so `$f2.applications` is empty while `$f1.applications` errors with `"Property 'applications' not supported on model element references"`. Net PASS gain is 0 because two tests flip PASS→non-PASS at the same time. **Path forward**: land `+=` *together with* a Function-Element materialisation step in `Copy` — when the source is `Value::Element(fn_id)` and the target heap classifier is `ConcreteFunctionDefinition`, populate every M3 reflective property from the Function compiled node (`function_name`, `expressionSequence`, `parameters`, `returnType`, `package`, etc.) so the heap Object mirrors the Element. With both pieces, the four PASS deltas stack: 2 from `+=` + 2 from no longer regressing FunctionDefinitionCopy. Until that's wired, `+=` is structurally correct but PASS-neutral. |
| Native arity checks: drop or keep? | P2 | Every native currently calls `expect_args("name", &values, N)`. The compiler's Pass 2 dispatcher already narrows overloads by param count during FQN mangling, so by the time a call reaches the registry the arity is structurally correct. Investigate: (a) can any call path reach a native with the wrong arity (e.g. reflective calls via `eval` / `evaluate` / `apply_callable`)? (b) if not, replace `expect_args` with `debug_assert_eq!` or delete it to cut ~50 lines of boilerplate. If yes, keep it but move to a shared wrapper so the message format stays consistent. |
| Non-recursive structural equality | P2 | `crates/runtime/src/native/equality.rs::values_equal` / `objects_equal` recurse on every collection element and every `<<equality.Key>>` property. Pathological inputs (deeply nested `List<List<…>>`, a long `Pair<U, Pair<…>>` chain, or a user-authored class with an equality-keyed self-reference) will blow the Rust stack — default 8 MB on the main thread but as little as 512 KB on spawned threads (LSP, test runners, DAP). Brainstorm: **(a) explicit work-stack**. Replace recursion with a `Vec<(Value, Value)>` (or `Vec<(&Value, &Value)>` if lifetimes allow — both sides come from `force_all` so borrows are stable). Loop: pop a pair; if primitives, compare and short-circuit-false on mismatch; if collections, length check then push each `(xs[i], ys[i])` pair; if objects, resolve classifier + equality-key props once and push each `(prop_a[i], prop_b[i])` pair. Depth becomes heap-bounded. **(b) cycle guard**. Self-referential classes (`Class Node { <<equality.Key>> parent : Node[0..1]; }`) would infinite-loop without tracking visited `(ObjectId, ObjectId)` pairs in a `HashSet` — Java Pure's `Any.equal` does this implicitly via `==` short-circuit at the JVM level; we need it explicit. The set also prunes redundant work when the same sub-graph appears twice. **(c) precomputed equality-key cache on Class**. Today every `objects_equal` call redoes `equality_key_properties(model, class_id)` — a `HashMap` lookup + filter over `class.properties`. Lift this into a `HashMap<ElementId, Vec<SmolStr>>` computed once on model load (or lazy-populated on first use in `Evaluator`). Matters both for allocation-free hot paths (map lookups, assertEquals in tight loops) and for the iterative rewrite, where the cache becomes the predicate driving whether to expand an object pair into prop pairs. **(d) depth limit + graceful error**. Simpler stopgap: count recursion depth, return `PureRuntimeError::StackOverflow` at e.g. 1000 levels. Preserves the current shape but converts a segfault-class bug into a Pure-level `throw`. Worth landing first if the iterative rewrite takes longer than expected. Do the same audit for `toRepresentation`, `toString`, `deep_copy_object` in Copy, and `reactivate_value` — all share the same structural-recursion risk profile. |
| Cleaner SourceInformation name-span representation | P2 | Currently we carry `ElementNode { source_info, name_source_info, … }` in `crates/pure/src/model.rs` — two parallel `SourceInfo`s per element (6 constructor sites touched: `pipeline.rs` x2, `bootstrap.rs`, `m3_parser.rs`, `model.rs` tests x2). It works and matches Java Pure's `SourceInformation(startLine, startColumn, line, column, endLine, endColumn)`, but feels like the wrong layer — we're hanging language-specific "name identifier position" state off a generic span struct. Brainstorm cleaner shapes: **(a) collapse into SourceInfo**. Make `crates/ast/src/source_info.rs` match Java's 7-field struct directly: `{ source, start_line, start_column, name_line, name_column, end_line, end_column }` with `name_line`/`name_column` defaulting to `start_line`/`start_column` via the `new()` constructor. Every Spanned node picks up the name-span capability for free and there's no second field on `ElementNode`. Downside: most uses of `SourceInfo` don't care about a name span, so the extra 8 bytes bloat every AST node. **(b) per-variant field on concrete Element kinds**. `Class { name_source_info, … }`, `Function { name_source_info, … }`, `Profile { name_source_info, … }` — put it on the payload struct, not the shared `ElementNode` header. Drops the field for `Element::Unit`/`Element::Package` which don't have a user-declared name token. Slightly more ceremony at read sites (runtime has to match on the Element enum), but the data model matches reality: only named elements have a name span. **(c) recover from ast::Element on demand**. `sourceInformation()` is a rare native — invoked by reflection, not in hot paths. Instead of caching the name span, let the runtime accept a `(ChunkId, SectionIdx, ElementIdx)` triple on `ElementNode` and re-read `ast::Element.name.source_info()` when the native is called. Trades cold-path work for a smaller compiled model. Only works if we retain the AST past Pass 2 (we currently do, for compilation-error diagnostics). **(d) keep the field, rename for intent**. `ElementNode.name_source_info` reads like "position of an unrelated thing"; call it `identifier_span` or inline the rename in a follow-up. Low-effort option if (a)–(c) don't pan out. Recommendation: lean toward (a) — matches Java's canonical struct, benefits Properties / QualifiedProperties / Constraints without further plumbing, and the 8-byte cost per SourceInfo is negligible next to the `SmolStr` source id. Gate on measuring AST memory footprint first. Entry also calls out the parser-side fixes landed alongside: `parse_qualified_name` returning last-segment span (`parser/mod.rs`), and Class/Function/Association/Enum/Profile/Measure `source_info` now merging with the closing brace span. |
| Document Measure / Unit model invariants | P3 | Cross-reference Java Pure's `M3Properties.canonicalUnit` (`legend-pure-core/legend-pure-m3-core/src/main/java/org/finos/legend/pure/m3/navigation/M3Properties.java:32`) to clarify the Measure → Unit relationship and make the canonical / non-canonical split explicit. Specifically document: (1) *canonical unit* — the unit marked with `*` in the Measure body (exactly one per Measure; the base unit all conversions express against; e.g. `Pes` in `RomanLength { *Pes: x -> $x; … }`). (2) *non-canonical units* — every other unit in the Measure, each carrying a conversion lambda mapping its magnitude back to the canonical unit (e.g. `Cubitum: x -> $x * 1.5` means 1 Cubitum = 1.5 Pes). (3) `Measure.canonicalUnit: Unit[0..1]` and `Measure.nonCanonicalUnits: Unit[*]` reflection properties, where `Unit.name` is the LOCAL unit segment (`"Pes"`), not the composite `"RomanLength~Pes"` stored on `ElementNode.name`. (4) Where the invariants live in code: compiled node is `Measure { canonical_unit: Option<ElementId>, non_canonical_units: Vec<ElementId> }` at `crates/pure/src/nodes/measure.rs`; Unit shell allocation is at `crates/pure/src/pipeline.rs:456`. Add a short `docs/architecture/measures.md` or a top-of-file block comment on `measure.rs` so future runtime / codegen work doesn't have to reverse-engineer this from test diffs. |

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
`parse`, `check`, `init`, `version`, `completions`, `emit`

### Stubs 🚧

| Command | Priority | Blocked On |
|---------|----------|------------|
| `compile` | P1 | Compiler crate stabilization |
| `test` | P1 | Runtime PCT coverage |
| `plan` | P2 | Execution plan generation |
| `package` | P2 | Compilation + artifact packaging |
| `publish` | P3 | Package + registry upload |

---

## How To Use This Document

1. **Pick an item** matching your skill and the current priority
2. **Check the detailed backlog** in the per-crate file for full context
3. **Update status** when you start/finish work
4. **Add new items** as you discover gaps during development
