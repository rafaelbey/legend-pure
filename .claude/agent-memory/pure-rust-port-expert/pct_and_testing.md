---
name: PCT and testing strategy
description: How PCT works as the Java-side parity contract, and the test corpus the Rust port should converge against
type: reference
---

**PCT = Platform Compatibility Testing.** The mechanism that keeps Java compiled and interpreted engines behaviourally identical. It's also the obvious acceptance contract for the Rust port.

**How PCT is wired:**
- Stdlib functions are annotated `<<PCT.function>>` in their `.pure` declaration.
- Tests use `<<PCT.test>>` and take an adapter parameter `f` so the test body is executed via a pluggable runner.
- Two Java suites — `Test_Compiled_*_PCT` and `Test_Interpreted_*_PCT` — route the same tests through different engines via `PureTestBuilderCompiled` / `PureTestBuilderInterpreted`. See `docs/architecture/compiler-pipeline.md` §2.⑥.
- Report scopes: `PlatformCodeRepositoryProvider.essentialFunctions` and `.grammarFunctions`.

**Exclusion mechanisms (use the right one):**
- `{test.excludePlatform = 'Java compiled'}` on the Pure test — for intentional platform differences.
- Java-side `expectedFailures` list with `one("fqn", "reason")` or `pack("package", "reason")` — for known bugs/gaps with a tracking issue.

**Java test corpus to mine:**
- `legend-pure-rust/test_catalog.md` — scan result: 1492 `@Test` methods across 232 classes, 873 containing inline Pure payloads.
- Abstract test bases in `legend-pure-core/legend-pure-m3-core/src/test/java/.../function/base/` define per-feature test suites (deleted copies for Assert/At/DayOfMonth/Hour/Minute/Second visible in current `git status`, meaning the team is reorganizing them — likely being ported to Rust or relocated).
- `legend-pure-runtime-java-engine-interpreted/src/test/` and `legend-pure-runtime-java-engine-compiled/src/test/` host the PCT runners themselves.

**Rust-side testing state:**
- `crates/pure/tests/integration_tests.rs` (1515 lines) — compiler integration tests (parse → compile → assert model shape).
- `crates/pure/tests/resolve_operators.rs` — operator dispatch resolution.
- `crates/runtime/tests/eval_tests.rs` (545 lines) — end-to-end evaluation: compiles platform + user code, runs a function by mangled FQN, asserts `Value`. Covers literals → arithmetic/comparison/boolean → let → if → collections → map/filter/fold → user functions → recursion → variable shadowing.
- Helper pattern: `eval_pure(source, fqn)` caches pre-parsed platform AST in a `OnceLock<PlatformFixture>`, then compiles user source + platform fixture and invokes by `test::<fqn>`. Replicate this helper for any new integration.
- `crates/stress/` — criterion benchmarks at three tiers (1K / 10K / 100K classes; `heavy` feature-gate for the big tier).

**Snapshot tests:** use `insta` for protocol JSON (`cargo insta review`).

**Parity harness recommendation** (not yet built): a cross-runtime runner that compiles the same `.pure` sources through Java and Rust, runs any function annotated `<<PCT.function>>` through both, and diffs the `Value`s. The natural landing zone is `crates/jni/` once the execution bridge exists — that crate today is a 22-line stub.

**Coverage gate:** `cargo llvm-cov --workspace --fail-under-lines 85` in CI, with `lint-lib` (no unwrap/expect in libs) and `lint` (clippy) gates.
