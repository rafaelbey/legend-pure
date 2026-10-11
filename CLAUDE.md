# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Shape

Two implementations of Legend Pure live side-by-side at the root:

- **Java/Maven stack** (upstream FINOS) — `legend-pure-core`, `legend-pure-dsl`,
  `legend-pure-maven`, `legend-pure-runtime`, `legend-pure-store`. Entry point: root `pom.xml`.
- **Rust evaluator** — `legend-pure-rust/` (Cargo workspace, three crates:
  `crates/metamodel`, `crates/evaluator` and `crates/jni`, the cdylib the Java
  module loads). **The plan for everything Rust is
  `legend-pure-rust/docs/rust-evaluator-mvp.md`**; `legend-pure-rust/CLAUDE.md` is the
  working guide. Read the plan's §0 before touching that directory.

The earlier Rust workspace (parser, compiler, LSP, DSL crates, CLI, binary snapshot
format) is frozen at tag `archive/pre-evaluator-extract-2026-10` and branch
`legend-pure-rust-archive`. Nothing from it is current; pieces are fished back one at a
time when the plan asks for them.

`.pure` source in `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`
is the canonical platform definition. The Rust metamodel crate embeds exactly one file
from it, `platform/pure/grammar/m3.pure`, as its bootstrap.

## Current State (October 2026)

### Rust Workspace — Evaluator MVP, Steps 0–2 done
- Step 0 (extract the evaluator and its metamodel into a clean workspace), Step 1
  (the evaluator proven over the bootstrap with hand-built nodes; 26 native
  signatures fished) and Step 2 (the JNI boundary with `CoreInstance` as the only
  currency, and the platform PCT corpus running red through the `Rust_JNI` adapter)
  are complete.
- Java lambdas are lowered through the bridge (plan, Step 2 "Next", done) and the
  natives backlog is fished from the archive (natives round 1, 2026-10-11): PCT state
  `Rust_JNI essential: 303 / 327`, `Rust_JNI grammar: 126 / 136`. The module's manifests
  rank what is left by kind: `FunctionNotFound:` (nothing registered),
  `NotImplemented:` (registered under its key, body not fished), `Unsupported:`
  (lowering or value-shape gap), then bare Pure error texts. Next: natives round 2 on
  that residue, then Step 3 (PELT); Steps 3–7 are in the plan.
- Build and test: `cargo build --workspace`, `cargo test --workspace` inside
  `legend-pure-rust/`; then `mvn -pl legend-pure-runtime/legend-pure-runtime-rust-evaluator
  clean verify` for the boundary tests and the PCT suites. CI: `.github/workflows/rust.yml`.

### Java Stack — Maintenance Mode
- Stable. All tests pass. Used by FINOS `legend-engine` as a compiled dependency.
- No active feature work in this repository.
- `legend-pure-runtime-rust-evaluator` is in the reactor (its tests skip when the Rust
  cdylib is absent); `…-bindings-ap` stays out. The platform declares the native
  `evalRust` and the `Rust_JNI` PCT adapter; the interpreted engine implements `evalRust`
  through the rust-evaluator module's extension, the compiled engine through its
  `EvalRustExecutor` hook, which the same module provides. Both engines run the corpus:
  interpreted 303 / 327 and 126 / 136, compiled 300 / 327 and 125 / 136.
- Root installs need `mvn clean install` (the PAR plugin trips on a stale descriptor
  otherwise); JDK 11 or 17.

## Java Stack — Build & Test

Requires **JDK 11 or 17** (the Maven enforcer rejects all others) and **Maven 3.6+**.

```bash
mvn -T 4 install -DskipTests                    # fast first build (15–30 min)
mvn -T 4 install                                # full build + tests
mvn test -pl legend-pure-core/legend-pure-m3-core
mvn install -pl legend-pure-store/legend-pure-store-relational -am   # module + deps
mvn install -Dcheckstyle.skip=true
```

`mvn install -DskipTests` must run at least once so the
`legend-pure-maven-generation-platform-java` plugin emits the generated `CoreInstance`
Java accessors before IDE-driven test runs will compile.

## High-Level Architecture

### M4 → M3 → M2 → M1 metamodel stack

This is the mental model for the entire codebase — every module maps onto a layer:

- **M4** (`legend-pure-m4`) — meta-metamodel; defines what a node is (`CoreInstance`).
- **M3** (`legend-pure-m3-core`) — Pure language metamodel: Class, Function, Association, etc.
- **M2** — DSL metamodels written in Pure: Mapping, Store, Diagram, TDS, Path, Graph.
- **M1** — user `.pure` source (the platform library under `m3-core/.../resources/platform/`
  is M1 source that *bootstraps* M2/M3).

### Java compiler pipeline

`src → ANTLR4 parse → first-pass symbol registration → PostProcessor (type linking,
milestoning rewrite, association resolution, matcher loop) → Validator → serialize to
.par/binary elements → Java codegen (compiled mode) OR tree-walk (interpreted mode)`.

PCT (`<<PCT.test>>` functions in the platform `.pure` sources) runs every tagged test
through **both** Java engines on every build and fails if results diverge; the `Rust_JNI`
adapter is a third run of the same corpus. This is the integration-test contract — don't
bypass it.

## Java Code Conventions

- JUnit 4 (ADR-001), Eclipse Collections for immutable collections (ADR-002), no mocking
  (ADR-003) — integration tests hit real engines. See `docs/decisions/`.
- Checkstyle is enforced during `verify`; root config is `checkstyle.xml`.
- Public API in `legend-pure-m4`, `legend-pure-m3-core`, and runtime modules is consumed
  as a compiled dependency by `legend-engine`. Changing those signatures is a breaking
  change for the entire FINOS Legend stack — treat them like a published library API.
- The Maven plugin suite (`legend-pure-maven-*`) is also a published contract: goal names,
  parameters, and default phases are consumed by `legend-engine` and downstream projects.

## Where to Look First

- `docs/README.md` — full docs index (architecture, compiler pipeline, grammar references).
- `docs/architecture/compiler-pipeline.md` — parse → post-process → validate → serialize → codegen.
- `docs/architecture/overview.md` — position in the Legend ecosystem, stack diagrams.
- `docs/reference/` — Pure language, Legend grammar, Mapping, Relational, Maven plugin refs.
- `legend-pure-rust/docs/rust-evaluator-mvp.md` — the Rust plan; `legend-pure-rust/CLAUDE.md` — the Rust working guide.
