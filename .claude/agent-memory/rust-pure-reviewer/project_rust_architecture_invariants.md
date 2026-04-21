---
name: Review watch-points — invariants easy to silently break
description: The specific places in the Rust workspace where a well-meaning refactor can silently violate a load-bearing invariant
type: project
---

# Review watch-points — where reviews earn their keep

These are the concrete "if this slips, it will be hard to notice and expensive to undo" sites. Each one is a place where a PR can look innocuous but breaks a load-bearing invariant.

## Pass ordering in `crates/pure/src/pipeline.rs`
- **Pass 2a must fully complete before Pass 2b starts.** Merging them reverts dispatch to shell/placeholder types.
- **Topo-sort uses hard deps ONLY** (class supertypes). If someone "helpfully" adds property types or param types to `extract_hard_dependencies`, they break cyclic data models (Person↔Company) and throw false cycle errors.
- **Pass 2.5 scans only the last chunk** (`chunk_idx = model.chunks.len() - 1`). Not bootstrap. If user code ever gets split across chunks, this assumption needs reconsidering.
- **`pass_define_signatures` skips `Element::Unit`** — there's a SAFETY INVARIANT comment about AST coordinates pointing to the parent Measure. If the skip is removed, units get re-hydrated as duplicate Measures.

## Parallel arenas inside `ModelChunk`
- `ModelChunk.nodes` and `ModelChunk.elements` are index-parallel. `alloc_element()` has a `debug_assert_eq!`. Any code that allocates into one but not the other, or that allocates them out of order, desyncs the model. Only `alloc_element()` should be used.

## Name mangling
- **Function `ElementNode.name` = mangled FQN** (`plus_Integer_1__Integer_1_`). `Function.function_name` = simple name (`plus`).
- `resolve_by_path` / `resolve_in_package` / `resolve_by_fqn` match `ElementNode.name` (mangled).
- `resolve_functions_by_name_in_package` / `resolve_function_by_path` match `Function.function_name` (simple) or mangled-prefix.
- A change that stores simple names in `ElementNode.name` will silently break FQN resolution (the 75 "UnresolvedElement" errors would explode).

## Two resolution entry points
- `resolve_element_ptr` vs `resolve_function_call` — keep them separate. A PR that "unifies" them will re-introduce the pre-split bug where functions resolved by element-ptr path skipped overload dispatch.

## AST/Protocol split
- **AST has no `serde`.** If a PR adds `#[derive(Serialize, Deserialize)]` anywhere in `crates/ast/`, flag it. Protocol translation belongs in the `protocol` crate.
- **AST does not desugar** `Arithmetic`, `Comparison`, `Logical`, `if`, `let`, `new`. Any PR that pre-desugars in the AST breaks AST ≠ Protocol isomorphism and blocks future Pure→AST emission.

## Runtime thread-safety
- `PureModel` is `Send + Sync` (contains no `Rc`).
- `Evaluator`/`RuntimeHeap`/`VariableContext` are **not `Send`** (contain `Rc` via `im_rc`).
- Any PR adding `Arc` inside the runtime data types, or any `Send` bound on an `Evaluator` method, needs to acknowledge: this switches the whole crate away from `im_rc` to `im` (the `Arc`-backed variant), which loses the O(1) clone benchmarks.

## Lazy call stack
- `Evaluator` has no call-stack field. Errors propagate as `Result<Value, PureException>`, with `map_err` appending `StackFrame`s on unwind.
- If someone adds a `call_stack: Vec<StackFrame>` field to `Evaluator`, they've silently regressed the zero-overhead happy path.

## Native dispatch order (`eval_function_call`)
- **Order: exact-FQN native → prefix-based simple-name native → user function by ElementId → error.**
- The prefix fallback (`find_by_prefix`) is load-bearing because many operators are lowered with simple names before full FQN resolution is available. Removing it breaks arithmetic/comparison operators.

## `find_by_prefix` ordering
- Whichever native is registered first under a given simple-name prefix wins. If `plus` has 8 overloads, the order `NativeRegistry` is built in determines which one is picked when the compiler gave up on full FQN. Any refactor that sorts/randomizes registration order can change runtime behavior without any source change.

## Clone patterns in `eval.rs`
- `call_user_function` clones `func.parameters`, `func.body`, and `node.source_info` before pushing scope — known cost, tolerated because of borrow-checker pressure.
- `Pass 2.5` clones each `ValueSpec` body before inference and writes back.
- These are known-suboptimal but intentional. Flag when reviewing perf-focused work, but don't "fix" them casually — changing them requires rethinking borrow-checker interactions.

## `#![forbid(unsafe_code)]`
- Set in every crate except `jni` (FFI) and (possibly) `stress/src/alloc.rs` (tracking allocator via `GlobalAlloc`). A PR adding `unsafe` anywhere else needs explicit safety-invariant docs and justification.

## `unwrap()` / `expect()`
- Banned in library code via `cargo lint-lib`. A PR that uses them needs to either move them under `#[cfg(test)]` or convert to `?` / `ok_or` / `unwrap_or_else`. CI will catch it but the reviewer should too.

## Dispatch error baseline
- **243 errors currently** on the platform `.pure` corpus (per `crates/pure/BACKLOG.md`). Any PR touching `resolve.rs` / `lower.rs` / `pipeline.rs` should be scored against this baseline. Regressions here are the most visible semantic metric.

## Generics-as-`Any` escape hatch
- Treating generic type params as `Any` during dispatch is a known compromise that hides several errors behind false matches. A PR that tightens this behavior without real generic unification will increase the error count even though it looks "more correct." The team's target is "fewer errors AND more correct matches" — a trade is not acceptable.

## Why this memory exists

**Why:** These are the high-signal review targets. If the reviewer spends time here, they catch the expensive bugs; if they spend time on surface-level Rust idioms elsewhere, they leave these to rot.

## How to apply

- On any PR that touches `pipeline.rs`, `resolve.rs`, `lower.rs`, `model.rs`, or `eval.rs`, walk the relevant watch-point before approving.
- On any PR that looks like a refactor ("cleanup", "simplification", "unification"), be especially suspicious — refactors are the main vector for silently breaking these invariants.
- For each watch-point violation, surface not just the violation but the reason it matters (borrow the "Why" from this file).
