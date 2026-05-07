# CLAUDE.md — `legend-pure-rust/`

This file provides guidance to Claude Code (claude.ai/code) when working inside the
Rust workspace. It supplements the root `/CLAUDE.md` with Rust-specific details.

**Key references:**
→ [`docs/PURE_LANGUAGE_SPEC.md`](docs/PURE_LANGUAGE_SPEC.md) — formal grammar, operator precedence, type system, compiler pipeline
→ [`BACKLOG.md`](BACKLOG.md) — unified backlog

## Workspace Overview

Cargo workspace (edition 2024) that reimplements the Legend Pure parser, compiler, and
interpreter in Rust. Targets the same Pure language and Legend Protocol v1 JSON as the
upstream Java stack, so it must stay behaviourally compatible with the platform `.pure`
sources under `../legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`.

## Crate Layers

```
cli, jni       ← Layer 7 (developer tools + Java FFI)
runtime        ← Layer 6 (interpreter + heap: Value, RuntimeHeap, Executor)
pure           ← Layer 5 (semantic layer: PureModel, ElementId, type resolution)
protocol       ← Layer 3 (AST ↔ Legend Protocol v1 JSON)
parser, compose ← Layer 2 (recursive-descent parser; AST → grammar text)
lexer          ← Layer 1 (tokenizer)
ast, ast-derive ← Layer 0 (data model + proc-macros for Spanned/Annotated/PackageableElement)
core-platform-pure ← embeds platform .pure files at build time (build.rs)
stress         ← criterion benchmarks (1K → 100K class models; feature-gated `heavy` tier)
```

**Dependency rule:** lower layers never depend on higher layers. The `ast` crate has no
`serde` — it's kept lean for direct compiler consumption. All protocol/JSON conversion
lives only in the `protocol` crate.

The `legend` CLI (`crates/cli`) is the primary developer tool: `legend parse`,
`legend check`, `legend init`. Install with `cargo install --path crates/cli`.

## Current State (April 2026)

| Component | Status | Tests | Key Milestone |
|-----------|--------|-------|---------------|
| Parser | ✅ Complete | ~400+ | Full grammar coverage, 0 errors |
| Compiler | ✅ Platform clean | ~50+ | 0 errors on 236 files / 1338 elements |
| Runtime | 🚧 Active | ~160+ | ~50 native functions, PCT subset passing |
| CLI | 🚧 Partial | ~20+ | 7/11 commands implemented (`+java-bindings`) |
| Java codegen | ✅ v1 | 16 | `legend java-bindings` + proxy runtime support; explicit `--classes` / `--associations` seeds |
| **Total** | | **739** | |

→ See `BACKLOG.md` at workspace root for the unified backlog.

## Build & Test Commands

```bash
cargo build --workspace
cargo test --workspace
cargo nextest run --workspace   # faster local test runner; doesn't run doctests
                                # (run `cargo test --doc` separately if needed)
cargo lint-lib          # strict: no unwrap/expect in library code
                        #   (alias → clippy --lib -D unwrap_used -D expect_used)
cargo lint              # standard clippy on all targets
cargo fmt --check
cargo audit             # alias → lint-lib + lint + test --all
./scripts/check-copyright.sh              # required on .rs/.toml/.pure/.sh

# Coverage
cargo llvm-cov --workspace --fail-under-lines 85   # CI gate
cargo llvm-cov --workspace --html --output-dir coverage/   # local HTML report

# Run a single test
cargo test -p legend-pure-pure <test_name>
cargo test -p legend-pure-runtime --test <test_file>

# Snapshot tests (protocol JSON golden files)
cargo insta review

# Verbose tracing during tests
RUST_LOG=legend_pure_parser=debug cargo test

# Benchmarks
cargo bench --workspace
cargo bench -p legend-pure-parser-stress --features heavy   # 100K-class tier
```

CI (`/.github/workflows/rust.yml`) runs format-check → `lint-lib` → `lint` →
`llvm-cov --fail-under-lines 85` → `bench`. All three lint gates must pass.

## Code Conventions (enforced by CI)

- Edition **2024**. `#![forbid(unsafe_code)]` in every crate except `jni`.
- No `unwrap()` / `expect()` in library code — `cargo lint-lib` is a hard gate.
- Public items need `///` doc comments (`#![deny(missing_docs)]` on public crates).
- `SmolStr` (not `String`) for identifiers; 24-byte inline, O(1) clone.
- `thiserror` for library error types; `miette` for CLI diagnostics.
- `tracing` (not `log`). Tracing levels:
  - `ast` — no tracing (pure data)
  - `lexer` — `trace` for tokens, `debug` for state transitions
  - `parser` — `#[instrument]` on grammar-rule methods, debug level
  - `protocol` — debug per element conversion
  - `jni` — `info` for call boundaries, `error` for failures
  - Never emit `info+` in hot paths.
- Copyright header required on `.rs`, `.toml`, `.pure`, `.sh` (Markdown/JSON exempt);
  validated by `scripts/check-copyright.sh`.
- Snapshot tests use `insta` — run `cargo insta review` when protocol JSON changes.
- Default visibility `pub(crate)`; only expose `pub` when it's part of the crate's API.

## Correctness policy: no tactical test-pass hacks

We are building **parity with Java Pure**, not "tests that pass today". Every
fix must be structurally correct, or it's not a fix — it's debt that makes
the next Java-divergence bug harder to find.

- **Fix at the source of truth.** If Java emits `X`, produce `X` where the
  data is born (build script, parser, compiler, model), not where it's
  displayed. Stack traces, error messages, `SourceInformation.source`, and
  protocol JSON should all see the same canonical value. When they diverge,
  any one of them is wrong.
- **No pattern-matching on test-specific shapes.** Never write a branch
  that reads *"if the input starts with `essential/`, prepend
  `/platform/pure/`"* — that's tuned to a test fixture and breaks the
  moment user code or a renamed directory enters the picture. If the
  convention is `/platform/pure/<rel>`, produce it uniformly from the
  build step, not conditionally at the consumer.
- **Don't transform intermediate values to satisfy an assertion.** When a
  test expects `X` and we produce `Y`, the answer is "why does the pipeline
  produce `Y` instead of `X`?" — not "how do we post-process `Y` into `X`
  at the last native before the assertion?".
- **Prefer one correct layer over many half-correct ones.** A single
  structural fix usually removes several symptomatic bugs at once. Shallow
  fixes tend to multiply.
- **If the right fix is too large for this change, defer cleanly.** Mark
  the test `#[ignore = "needs <structural fix>"]` with a one-line reason
  and file a follow-up. Never paper over with a guard that only works for
  this test's input.
- **"The test passes" ≠ "the behaviour is correct."** A green check on a
  narrowly tuned fix is worse than a red one, because the red one keeps
  pointing at the real work.

This applies doubly to Java-parity work: the platform tests exist precisely
to detect divergence from Java semantics. Gaming them erases that signal.

## Key Traits & Derive Macros

| Trait | Crate | Purpose |
|-------|-------|---------|
| `Spanned` | ast | Source-location access; every AST node with `source_info` |
| `Annotated` | ast | Stereotypes + tagged values (`<<stereo>>`, `{tag = 'val'}`) |
| `PackageableElement` | ast | Package-qualified elements; supertrait `Spanned + Annotated` |
| `ElementVisitor` / `ExpressionVisitor` | ast | Walk top-level elements / expression trees |
| `IslandPlugin` / `SectionPlugin` | parser | Parse `#>{}#`, `#s{}#` / `###Section` grammars |
| `CompilerExtension` | pure | `declare()` / `define()` / `validate()` plugin hooks |

Derives in `ast-derive` are **hierarchical** — use exactly one per struct:
- `#[derive(Spanned)]` → needs `source_info`
- `#[derive(Annotated)]` → needs `stereotypes`, `tagged_values`, `source_info`
- `#[derive(PackageableElement)]` → needs `package`, `name`, `source_info`

The higher-level derive generates the lower-level impls automatically.

## Compiler Internals (`crates/pure`)

- **Pass 2a/2b split is load-bearing.** Pass 2a resolves all function *signatures*
  before Pass 2b compiles *bodies*. Type-based dispatch requires every signature to be
  visible in `PureModel` at body-lowering time. Do not merge these passes.
- **Pass-1 shells carry every syntactic AST field.** `create_shell` (Pass 1) populates
  `Class.type_parameters`/`multiplicity_parameters` and `Profile.stereotypes`/`tags`
  directly from the AST. This is what makes resolver-eager validation sound regardless
  of topological hydration order. Do not regress to empty placeholders — it's the
  unlock for the whole validator architecture below.
- **Validators run next to the data they inspect (Java-parity).** Three seams:
  - **Resolver-eager** (`crate::resolve`): `resolve_type_ref` checks generic-class
    type-arg arity; `resolve_stereotypes` / `resolve_tagged_values` check profile-kind
    and name-existence. All sound thanks to populated Pass-1 shells.
  - **Hydration-inline** (`hydrate_element_signature`): `validate_super_types`,
    `validate_association`, `validate_duplicate_properties`,
    `validate_no_access_on_properties`, `validate_no_multiple_access_levels` —
    each fires the moment its inputs (slice of super-types / properties /
    stereotypes / …) are built, before the parent `Element` value is constructed.
  - **Cross-chunk** (`crate::validate::validate`, called from `finalize_model`):
    only validators whose inputs *genuinely* span multiple chunks —
    `validate_repo_visibility` and the access-level use-site walker (Step B).
  Adding a new element kind = one match arm in `create_shell` + one in
  `hydrate_element_signature` calling whichever piecewise validators apply. No
  batch validator to update.
- **Two resolution entry points** — don't confuse them:
  - `resolve_element_ptr` — exact mangled-name lookup. Used for type references,
    annotations, property access.
  - `resolve_function_call` — simple-name + overload dispatch. Used only for
    `FunctionApplication` and `ArrowFunction` lowering. Filters by param count, then
    narrows by type + multiplicity scoring.
- See `crates/pure/FUNCTION_DISPATCH.md` for the dispatch algorithm and
  `crates/pure/BACKLOG.md` for open work items with P0–P3 priorities.

## Runtime Internals (`crates/runtime`)

Four-layer storage model:
1. Immutable `Arc<PureModel>` — shared across threads; class/function defs + type hierarchy.
2. Per-executor `RuntimeHeap` — metamodel-only arena, `HashMap<ElementId,
   ObjectHandle>`. User objects are `Value::Object(ObjectHandle)` clones
   (`ObjectHandle = Rc<RefCell<HeapEntry>>`), tracked by the variable
   context, return chain, captures, and memoization cache — freed by RAII
   when the last reference drops. Identity via `Rc::ptr_eq`; mutation via
   `borrow_mut`. `getAll(UserClass)` is a reachability walk from the
   variable context (paid only on call).
3. Persistent collections via `im-rc` (HAMT/RRB) — O(log N) fold+put, no GC.
4. Memoization is **purity-gated** — transitive `SideEffectFunction` analysis at compile
   time, then a bool check at runtime.

Deep-dive docs live under `docs/runtime/` (hybrid compilation, memoization,
`mutateAdd` mechanics, convergence gaps vs. the Java interpreter).

## AST ↔ Protocol Design

The AST and Protocol JSON are intentionally **different shapes**. AST keeps
`Arithmetic { op, left, right }`; protocol normalizes to
`{"_type": "func", "function": "plus", "parameters": [...]}`. The `protocol` crate is
the only place this translation lives. The AST also **preserves type parameters**
(unlike the upstream Java parser which rejects them) and does **not desugar**
expressions — `Expression` stays isomorphic to the AST to support future Pure emission.

## Adding a Grammar Feature

1. `crates/ast/` — add types or variants for the new syntax.
2. `tests/` — failing tests first (TDD against `TestGrammarParser`/`TestGrammarRoundtrip`).
3. `crates/lexer/` — new tokens if needed.
4. `crates/parser/` — parse into the AST.
5. `crates/protocol/` — AST ↔ Protocol JSON conversion.
6. `cargo insta review` to approve new golden files.

## Where to Look First

- `ARCHITECTURE.md` — crate graph, derive macros, full decision log.
- `crates/runtime/ARCHITECTURE.md` — interpreter internals + benchmark baselines.
- `docs/runtime/` — runtime design deep-dives.
- `crates/pure/FUNCTION_DISPATCH.md` + `BACKLOG.md` — active compiler work.
- `IMPLEMENTATION_PLAN.md` — historical design rationale and test catalog
  (all phases complete; kept for reference only).
