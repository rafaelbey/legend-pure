---
name: Rust rewrite — current phase, what's implemented, what's stubbed
description: Where the Rust compiler+runtime actually is vs. its aspirational docs — calibrates which kind of review is appropriate
type: project
---

# Rust rewrite — actual state as of 2026-04-17

Active branch `legend-pure-rust`. Per `legend-pure-rust/IMPLEMENTATION_PLAN.md`, all phases of the original plan are marked complete. The `convergence_analysis.md` doc is **stale** — it claims expression lowering is missing, but `crates/pure/src/lower.rs` is ~1000 lines and fully wired.

## What actually works

- **Lexer + recursive-descent parser** covering all Pure grammar (classes, functions, enums, associations, measures/units, profiles, lambdas, collections, new/copy, slicing, islands, `###Section` headers).
- **AST crate** with hierarchical derive macros (`Spanned` → `Annotated` → `PackageableElement`). **AST has no serde** — protocol/JSON conversion lives only in `protocol`.
- **Compiler pipeline** with Pass 1 declare → Pass 1.5 topo-sort (Kahn's, hard deps = supertypes only) → Pass 2a signatures → Pass 2b bodies → Pass 2.5 type inference → freeze → Pass 3 validate. Pass 2a/2b split is **load-bearing** for type-based dispatch.
- **Function dispatch** with simple-name lookup + param-count filter + type+multiplicity scoring. Variable-type tracking for params/lets/lambdas. Narrowest-match wins.
- **Runtime interpreter** (`crates/runtime`) — `Evaluator` walks `ValueSpec` tree; `RuntimeHeap` = `SlotMap<ObjectId, HeapEntry>` with Dynamic/Typed split; `VariableContext` uses flat-HashMap + undo-log (O(1) get); `PureDate`/`StrictTime` via `jiff`; `Decimal` via `rust_decimal`; `im_rc::Vector`/`HashMap` for persistent collections; lazy call-stack on error (zero overhead happy path).
- **CLI** `legend parse|check|init` (`cargo install --path crates/cli`).
- **41 native functions** implemented (`arithmetic`, `boolean`, `collection`, `comparison`, `lang`, `string`, `testing`).

## What's still in progress / gaps

- **243 compile errors** on the platform `.pure` corpus (per `crates/pure/BACKLOG.md`, down from 538). Dominated by: AmbiguousImport (161), UnresolvedElement (75), ParseFailure (6), DuplicateElement (1). Top ambiguous hotspots: `elementToPath`, `dynamicNew`, `map`, `assertEquals`, `abs`, `plus`.
- **Generic type parameters treated as `Any`** during dispatch — P1 compromise; causes false matches. No Java-style `TypeInferenceContext`/`TypeInferenceObserver` yet. No generic unification (`Z` propagation across params), no lambda param type inference from expected `Function<{...}>` type, no numeric widening `Integer → Float`, no return-type-directed dispatch.
- **M3 parser (`m3_parser.rs`, 1396 lines)** partially populates classes/enums/associations. Missing: constraints, qualified properties, derived properties, real property types (registered as placeholder `Any`).
- **Units-as-child-of-Measure**: currently promoted to package-level `Element::Unit` with `Measure~Unit` naming. Non-canonical vs M3.
- **Runtime**: `NativeRegistry` is ~25% of Java's 173 natives. `QualifiedPropertyAccess` returns an error (`"not yet supported"`). `Lambda` captures are stubbed as empty `HashMap` on deferred args. Memoization described in docs but not yet implemented. No compiled-code AOT path (only `HeapEntry::Dynamic` exercised so far).
- **Bootstrap chunk built at runtime** each compile (~1ms) — P2 item to move to compile-time via `build.rs`/proc-macro.

## Why this memory exists

**Why:** The Rust docs are aspirational and several (e.g. `convergence_analysis.md`) have bit-rotted. Reviewing against the docs alone will produce wrong calls.

## How to apply

- Expect to find gaps where Java has richer behavior — check `crates/pure/BACKLOG.md` before flagging something as missing (it may already be tracked at P0/P1/P2/P3).
- When reviewing dispatch-related changes, remember generics-as-`Any` is a known compromise; ambiguity error counts in `FUNCTION_DISPATCH.md` are the scorecard.
- When reviewing runtime changes, remember 132 natives are still missing and the compiled/AOT path doesn't exist yet — the `HeapEntry::Typed` branch is dead code for now.
- Treat `legend-pure-rust/docs/runtime/*.md` as design intent, not current state. Cross-check against the source before repeating claims from those docs.
