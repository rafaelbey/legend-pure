# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Shape

Two parallel implementations of Legend Pure live side-by-side at the root:

- **Java/Maven stack** (upstream FINOS) — `legend-pure-core`, `legend-pure-dsl`,
  `legend-pure-maven`, `legend-pure-runtime`, `legend-pure-store`. Entry point: root `pom.xml`.
- **Rust rewrite** — `legend-pure-rust/` (Cargo workspace). The active fork branch
  `legend-pure-rust` reimplements parsing, compilation, and a tree-walking interpreter in
  Rust. **When working inside that directory, see `legend-pure-rust/CLAUDE.md` for the
  Rust-specific guide.**

Most current development targets the Rust workspace. The Java modules still compile and run
tests — `.pure` source in `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`
is the canonical platform definition consumed by **both** stacks.

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

PCT (`@PCT` annotation) runs every annotated function through **both** engines on every
build and fails if results diverge. This is the integration-test contract — don't bypass it.

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
- `legend-pure-rust/CLAUDE.md` — dedicated guide for the Rust workspace.
