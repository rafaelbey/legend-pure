---
name: Repository shape
description: Top-level layout of the two Legend Pure implementations and where the shared parity contract lives
type: project
---

**Root:** `/Users/cocobey73/Projects/legend-pure` — git repo; main branch `master`, active branch `legend-pure-rust`.

**Two parallel implementations at the root.**

Java/Maven stack (reference, upstream FINOS):
- `legend-pure-core/` — M4 (`legend-pure-m4/`) and M3 (`legend-pure-m3-core/`) + DSLs
- `legend-pure-dsl/` — Mapping, Store, Diagram, TDS, Path, Graph DSLs
- `legend-pure-runtime/` — `legend-pure-runtime-java-engine-compiled` + `-interpreted`
- `legend-pure-store/` — Relational store
- `legend-pure-maven/` — Maven plugins (`build-pure-jar`, `compile-pure`, `build-pure-compiled-jar`)
- Root `pom.xml`; requires JDK 11 or 17

Rust rewrite (target):
- `legend-pure-rust/` — Cargo workspace (edition 2024)
- Crates: `ast`, `ast-derive`, `lexer`, `parser`, `compose`, `protocol`, `pure`, `runtime`, `jni`, `cli`, `core-platform-pure` (embeds platform .pure via build.rs), `stress`

**Shared parity contract:** `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/` — ~237 `.pure` files. Both Java and Rust stacks consume these; behavioural equivalence against this source set is the acceptance criterion.

Guide files:
- Root overview: `/Users/cocobey73/Projects/legend-pure/CLAUDE.md`
- Rust overview: `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/CLAUDE.md`
- Docs index: `/Users/cocobey73/Projects/legend-pure/docs/README.md`
- Rust architecture: `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/ARCHITECTURE.md`
- Java compiler: `/Users/cocobey73/Projects/legend-pure/docs/architecture/compiler-pipeline.md`
