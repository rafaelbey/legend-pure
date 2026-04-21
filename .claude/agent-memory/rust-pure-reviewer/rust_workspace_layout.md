---
name: Rust workspace layout
description: Crate layering, dependency rules, and where each responsibility lives in legend-pure-rust
type: project
---

## Layered Cargo workspace at `/Users/cocobey73/Projects/legend-pure/legend-pure-rust/`

```
ast, ast-derive        Layer 0 — data model + proc-macros (Spanned/Annotated/PackageableElement)
lexer                  Layer 1 — tokenizer
parser, compose        Layer 2 — recursive-descent parser; AST → grammar text
protocol               Layer 3 — AST ↔ Legend Protocol v1 JSON
core-platform-pure     build.rs that embeds platform .pure files
pure                   Layer 5 — PureModel, ElementId, semantic layer
runtime                Layer 6 — interpreter + heap
cli, jni               Layer 7 — dev CLI and Java FFI
stress                 criterion benchmarks (1K/10K/100K; heavy feature)
```

- **Dependency rule**: lower layers never depend on higher. The `ast` crate has no `serde` — protocol/JSON translation is *exclusively* in `protocol`.
- **`#![forbid(unsafe_code)]`** is set in every crate except `jni` (FFI requirement) and the `stress/src/alloc.rs` tracking allocator (`GlobalAlloc` requires `unsafe impl`). That's it — two known unsafe sites, both deliberate.
- **`#![deny(missing_docs)]`** on public crates; CI enforces `cargo lint-lib` which forbids `unwrap()` / `expect()` in library code (tests/benches exempt).
- Edition **2024**. MSRV implied by edition.

## Why: reviewer needs this

- If a reviewed change introduces a reverse dependency (e.g., ast pulling in serde, or pure depending on runtime), it's a layering violation.
- If a change introduces an `unsafe` block outside jni/stress, that's a red flag needing explicit justification — the team has made a conscious `forbid(unsafe_code)` stance.
- If a PR introduces `.unwrap()` or `.expect()` in lib code and the team expects it to merge, CI (`cargo lint-lib`) will block it.

## How to apply

- Flag layering violations immediately.
- Ask for safety-invariant docs on any new `unsafe`.
- Suggest `?` + custom error variants or `unwrap_or_else` / `ok_or` in place of `unwrap()`/`expect()`.
