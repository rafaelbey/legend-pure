# Runtime Perf Session — 2026-05-01

Single-session pass over the runtime crate driven by the
`rust-pure-reviewer` agent's deep-assessment plan
(`.claude/plans/rust-pure-reviewer-agent-do-deep-sleepy-wigderson.md`).
Captures what landed, what was deferred, the bench numbers, and the
pattern that emerged for what does and doesn't move real workloads on
the current platform.

## Headline

| Bench | Pre-session | Post-session | Δ |
|---|---|---|---|
| `eval_recursion/fib_15` | 4.10 ms | **2.15 ms** | **−47.5 %** |
| `eval_recursion/sum_to_50` | 169.6 µs | **122.5 µs** | **−27.8 %** |
| `eval_relation/addColumns_2cols_to_1col_source` | ~66 µs | ~59 µs | **−10.6 %** |
| `eval_property_access/read_via_let_binding` | ~58 µs | ~53 µs | **−8.6 %** |
| `eval_lambda/map_concat_typed_lambda_param` | ~56 µs | ~52 µs | **−7.1 %** |
| `eval_lambda/filter_then_map` | ~60 µs | ~56 µs | **−6.7 %** |
| `eval_property_access/read_property_inline_construct` | ~55 µs | ~53 µs | **−3.6 %** |
| `eval_function_call/evaluator_setup_only` | ~362 µs | ~358 µs | −1.1 % |
| `eval_function_call/trivial_return_literal` | ~50.7 µs | ~51 µs | flat |

Numbers are running medians from criterion across runs; ±1 % run-to-run
variance is normal and the table rounds it out. The big fib_15 win
dominates everything else because the recursive interpreter case
re-executes the inner body 1973 times per iter, so anything saved per
call multiplies.

## What committed (4 commits)

### A1 — `Rc<[T]>` function/QP/lambda bodies (`58a2d51`)

The single biggest interpreter win on file. Every user-function call
was deep-cloning `Function.body: Vec<ValueSpec>` before pushing the
call frame — the clone walked the entire compiled AST, allocated a
fresh `Vec`, copied each `ValueSpec`'s `Box<ExprKind>`, recursed.
Switching the storage to `Rc<[T]>` collapses the clone to a single
refcount bump.

Schema changes (`#[derive(Clone, PartialEq)]` preserved):

- `Function.parameters: Vec<Parameter>` → `Rc<[Parameter]>`
- `Function.body: Vec<Expression>` → `Rc<[Expression]>`
- `QualifiedProperty.parameters` / `body` → same
- `runtime::value::LambdaClosure.parameters` / `body` → same
- `eval::FoundQp` (QP-with-generalisation snapshot) → same

Construction sites wrap their existing `Vec<T>` with `.into()`; the
`From<Vec<T>>` impl on `Rc<[T]>` allocates once and freezes. Mutation
sites — pass-2b' body patch-up, pass-2c inference writeback — extract
a working `Vec<T>` via `.to_vec()`, mutate, re-freeze with `.into()`.
The M3 chunk-0 resolve pass keeps its in-place mutation through
`Rc::get_mut`, asserting unique ownership: a future Class/QP clone
between `m3_parser`/`bootstrap` and `resolve_m3_supertypes` panics
with the actual strong count rather than silently degrading.

Measured: `fib_15 −47.5 %`, `sum_to_50 −26.5 %`. A small
+3.8 % regression on `trivial_return_literal` is real — the per-call
constant cost of the Rc bump now competes with what was a near-trivial
Vec clone of a one-element body — but it's dwarfed by the recursive
case win.

Lambda *creation* still allocates: `Rc::from(&[T])` walks-and-clones
each Parameter/ValueSpec. The closure's read path is now O(1)
(`closure.body.clone()` in the Lambda dispatch and meta natives), but
per-call lambda construction hasn't gotten faster. `ExprKind::Lambda`
in the AST/IR would also need to switch to `Rc<[T]>` — a separate
change, intentionally not in scope.

### B8 — drop redundant `obj_id.clone()` chains (`b44218b`)

Five sites in `eval.rs` called heap methods as
`heap.method(&obj_id.clone(), …)`. Each `obj_id.clone()` allocated a
temporary `ObjectHandle` (`Rc<RefCell<HeapEntry>>` refcount bump plus
a stack copy) just to immediately borrow it as `&ObjectHandle` — but
the heap methods already take `&ObjectHandle`. Pure waste.

Sites stripped:

- `back_fill_call_type_arguments` — empty-slot guard, classifier-match
  guard, `mutate_set` write-back (3 calls; fires post-hoc on every
  generic-function call's return).
- `apply_qualified_property` — `__typeVariableValues` read on the
  receiver instance.
- `apply_property_to_instance`'s `Value::Object` arm — collection
  property read off the receiver.

Three other obvious-looking sites were intentionally left alone:

- `eval.rs:1839` (`apply_object_callable(obj_id.clone(), …)`) — callee
  takes `ObjectHandle` by value and threads it through several
  internal helpers (`apply_qualified_property`, `read_wrapper_name`).
  The clone is necessary for the move; rewriting the dispatch chain
  to take `&ObjectHandle` is a bigger refactor (own commit).
- `eval.rs:2017` (`try_getter_override(obj_id.clone(), name)`) —
  same shape; callee owns the handle.
- `eval.rs:2226` (`let obj_id = obj_id.clone();` inside
  `try_to_string_qualified_property`) — the local owned binding is
  needed because the closing
  `context.set('this', Value::Object(obj_id))` moves the handle into
  a `Value`.

Measured: `read_via_let_binding −2.99 %` (statistically significant on
the path that exercises `Value::Object`-routed property access).
Recursion / native-only benches unaffected (no objects allocated, so
nothing was bumped to begin with).

### B5 — cache collapsed `Value` (not `Vec<Value>`) in member_wrapper_cache (`16d5be2`)

`member_wrapper_cache` memoises class-introspection wrappers
(`.properties`, `.qualifiedProperties`,
`.propertiesFromAssociations`, `.stereotypes`, `.taggedValues`,
multiplicity bounds, function `classifierGenericType`) so repeated
reads return the same heap ObjectIds — `assertIs(C.properties->at(0),
C.properties->at(0))` holds across calls.

The cache was storing the unwrapped `Vec<Value>`; every hit then did
`Value::from_vec(cached.clone())`, which:

1. Deep-cloned the `Vec<Value>` (one heap alloc + N value copies).
2. Re-collapsed it through `from_vec`'s `0 → Unit / 1 → scalar /
   N → Collection(Box<im_rc::Vector>)` switch — for Collection that
   meant rebuilding the HAMT from an iterator on every hit.

Storing the already-collapsed `Value` instead lets cache hits be a
plain `Value::clone`. For the common Collection case that's an
`im_rc::Vector::clone` (O(1), structurally shared) plus a `Box`
allocation for the variant payload — drops the per-hit cost from
`O(N) deep clone + HAMT rebuild` to `O(1)`. Identity stability is
unchanged: the cached `Value::Collection` shares its inner `Vector`
across clones, and the `Value::Object(id)` handles inside share
their `Rc<RefCell<HeapEntry>>` exactly as before.

Six cache key/insert pairs updated. New
`bench_member_wrapper_cache` group with 20 repeated
`Person.properties->size()` reads (1 miss + 19 hits) lights up the
win; the existing benches never read `.properties` and so wouldn't
surface it.

Measured: `class_properties_repeat_20 −2.30 %` (~100 ns saved per
cache hit × 19 hits/iter ≈ 2 µs/iter).

### B7 — inline FQN resolve via `resolve_fqn_str(&str)` (`2455580`)

The most broadly impactful commit of the session. Every classifier
dispatch (`m3_paths::resolve`, runtime QP property walk, type/lambda
natives, PCT manifest loading) was building a fresh `Vec<SmolStr>` per
call:

```rust
let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
model.resolve_by_path(&segments)
```

That's one heap-allocated `Vec` plus N `SmolStr` constructions per
resolve, even though the FQN was already in hand as a `&str`.
`m3_paths::resolve` is reached on every native that touches a
classifier — in some natives 3 to 5 times — so the cost compounds
across the entire interpreter.

Added `PureModel::resolve_fqn_str(&str) -> Option<ElementId>` that
walks the package tree segment-by-segment via `str::split("::")`
without materialising any intermediate collection. The hot inner
loop holds the previous segment in a local; on each iteration it
walks one package down and shifts the look-ahead. The leaf segment
falls out of the iterator as the element name.

Updated callers: `m3_paths::resolve`, the eval-side QP dispatch,
`try_to_string_qualified_property`, `native::meta::type_of`'s Object
arm, `native::testing` PCT-manifest resolution, and
`pct::rust_native_adapter_id`.

Measured (vs. the prior B5 commit):

```
eval_lambda/map_concat_typed_lambda_param   −5.58 %
eval_relation/addColumns_2cols_to_1col_src  −5.14 %
eval_lambda/filter_then_map                 −4.15 %
eval_property_access/read_via_let_binding   −3.55 %
eval_function_call/trivial_return_literal   −2.95 %
eval_function_call/call_with_one_int_arg    −2.97 %
eval_property_access/read_property_inline   −2.55 %
eval_lambda/map_double_5                    −1.73 %
eval_function_call/evaluator_setup_only     −1.53 %  (bootstrap_metamodel
                                                     resolves M3 type
                                                     constants per row)
eval_recursion/fib_15                       noise (no classifier reads)
```

Every workload that touches a classifier-by-string moved; only fib/sum
(pure-integer recursion, never resolve a class) sit at noise.

## What's stashed (4 stashes preserved for future use)

All four passed `cargo nextest run --workspace` — they're correct
implementations. They didn't survive the bench-data filter on the
current platform/workload. Stashed rather than thrown away because the
underlying mechanism is real; under different access patterns or
larger user models, they would win.

```
stash@{0}  phase-b7-extension-class-id-cache
stash@{1}  phase-a4-box-pure-exception
stash@{2}  phase-a3-native-registry-by-simple-index
stash@{3}  phase-a2-per-package-name-indexes
```

Restore any with `git stash apply stash@{N}`.

### A2 — per-package name indexes

Replace 8 linear `children_packages.iter().find(name)` /
`children_elements.iter().find(name)` scans in `model.rs` with O(1)
hashmap lookups built post-freeze.

**Bench result:** the synthetic name-resolve micro-bench *regressed*
+11 % to +217 %. For the platform's actual package sizes (5–50
children, cache-resident SmolStr equality), a linear `Vec` scan beats
a `HashMap` lookup — the hash + memory-deref cost dominates at small
N. Real workloads showed 0 % movement.

**Revisit when:** larger user models exist (50+ children/package
becomes common), or once `resolve_fqn_str(&str)` (B7) needs the
hashmap for further wins. Could also flip with a faster hasher
(`rustc-hash` / `FxHashMap`) — open question whether the
SipHash-vs-Fx delta is enough to flip the small-N case.

### A3 — `NativeRegistry` simple-name index

Replace `find_by_prefix`'s O(N) scan + `format!()` allocation with a
single hashmap lookup keyed by simple name.

**Bench result:** the targeted micro-bench dropped −97 %/−98 % (1000s
of ns → ~10 ns). But `find_by_prefix` is *never hit* on the entire
`eval_tests` suite — instrumented experimentally, the call counter
stayed at zero. Real workloads regressed +1–4 % from binary-layout
effects without benefiting from the optimised path.

**Revisit when:** reflection-heavy workloads exist that exercise
`Value::Function(Compiled)` invocation (the
`dispatch_compiled_function` path's fallback). The current platform
test suite resolves every native by exact mangled FQN.

### A4 — `Box` PureException

Make `PureException` a newtype around `Box<PureExceptionInner>` so
`Result<T, PureException>` becomes a single-pointer error type.
Removes ~65 redundant `#[allow(clippy::result_large_err)]` annotations
and lets the workspace deny the lint.

**Bench result:** small regressions across most benches (`fib_15` +2 %
average, variance 0.5–3.7 %). Hypothesis: LLVM was already optimising
the Result-size pattern via known-Ok inlining, and the `Box` drop/
alloc code emitted on every error site adds binary footprint that
hurts the hot loop's instruction cache.

**Revisit when:** profiling data points at error-result moves as a
bottleneck, or we hit a different blocker that justifies the
maintainability win.

### B7-extension — class_id cache on HeapEntry

Cache the resolved `ElementId` of an Object's classifier on
`RuntimeObject` via `Cell<Option<ElementId>>`, populated lazily on
first lookup. Pairs with the B7 commit that already eliminated the
per-call `Vec<SmolStr>`.

**Bench result:** ~+5 % regression. The cache amortises the segment
walk across repeated resolves on the same object — but the access
pattern is overwhelmingly one-shot. `iter_batched` in criterion
creates a fresh `Evaluator` (and thus fresh objects) every iteration,
so every classifier resolution is a cache miss. Per-resolve overhead
(`Cell::get` + `Cell::set`) without amortisation is net negative.

**Revisit when:** real-world workloads show repeated classifier
resolution on the same heap object (e.g. a chain like
`$x.qprop1->qprop2()` that QPs through the same receiver many times).

## The pattern that emerged

| Won (committed) | Stashed |
|---|---|
| Eliminate per-call deep clone where the clone is on the hot path and known | Speculative caches/indexes on cold paths |
| Eliminate per-call allocation when the alloc happens many times | Structural type changes that LLVM was already optimising |
| Refactor identical code that fired on real workloads | Hashmap-vs-linear scan tradeoffs at small N |

Three filters that consistently identified real wins:

1. **Is the path actually hit on `eval_tests` / criterion?** If not
   measurable, defer. (The `find_by_prefix` instrumentation lesson —
   the advisor's "P0 hot path" was zero-firing in practice.)
2. **Does the optimisation amortise?** A cache that's cold-missed every
   iteration is just overhead.
3. **Are the wins from removing work or from changing storage?**
   Removing work (deep clone, format!, Vec alloc) wins reliably.
   Changing storage (HashMap-vs-Vec, `Box<...>` wrappers) often
   regresses at the constant-factor level even when the asymptotic
   argument is right.

## Verification standard used

Every committed change passed:

- `cargo nextest run --workspace` — 1226/1226 tests
- `cargo test --workspace --doc`
- `cargo lint-lib`, `cargo lint`, `cargo fmt --check`
- `./scripts/check-copyright.sh`
- Criterion bench against the previous baseline, with both
  improvements and regressions reported honestly

## Open items from the original plan

Not yet attempted; lower expected value given session learnings:

- **B6** drop `Box<im_rc::Vector<Value>>` from `Value::Collection` —
  trades the Box allocation for a larger Value enum size; needs
  `size_of::<Value>` measurement and a workload that doesn't penalise
  the larger discriminant.
- **B9** share metamodel rows across evaluators — only saves the ~300 µs
  of cold-start `bootstrap_metamodel`. iter_batched amortises this
  away from any criterion bench, so the win shows up in `time legend
  check ...` runs but not in CI bench gates.
- **C-tier** idiom passes (`#[inline]` annotations, member-wrapper
  cache key as enum, etc.) — likely all noise on the hot path.

## Reproducing the numbers

```bash
# Restore stash for any phase
git stash list                         # show all stashes with descriptions
git stash apply stash@{N}              # apply (keep the stash)
git stash pop stash@{N}                # apply (remove the stash)

# Reproduce a committed bench
cargo bench --bench runtime_eval -- 'eval_recursion/fib_15$'

# Compare before/after a refactor:
git stash push -- crates/runtime/src/eval.rs
cargo bench --bench runtime_eval -- … --save-baseline pre-change
git stash pop
cargo bench --bench runtime_eval -- … --baseline pre-change
```

`cargo bench` results below ±5 % run-to-run on a quiet machine; if
benches show wider variance, check `ps aux | sort -nrk 3 | head` for
external CPU contention before drawing conclusions. Two of the
session's confusing results turned out to be Chrome at 80 % CPU and a
parallel `cargo bench` run that I had backgrounded and forgotten.

## Plan reference

`.claude/plans/rust-pure-reviewer-agent-do-deep-sleepy-wigderson.md` —
the original deep-review output that drove this session.
