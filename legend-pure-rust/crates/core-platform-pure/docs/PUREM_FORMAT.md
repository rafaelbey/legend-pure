# `.purem` — Pre-Compiled Pure Snapshot Format

> **Status: v0 implemented (May 2026).** Per-repo slices, hand-rolled
> magic-header + Postcard-encoded payload, topo-sorted load, CLI
> snapshot subcommand, classpath TOML — all shipping. FlatBuffers + lazy
> Tier-2 mmap deferred to v1 (see "Wire format choice" below). Today's
> CLI: `legend snapshot --output platform.purem` produces a
> byte-deterministic 3 MB blob; `Repo::Purem` + topo-sorted `repo::load`
> merge it back into a fresh `PureModel` with no parse + compile cost.
>
> This document records the design as shipped; sections describing
> deferred decisions retain their original "open question" framing for
> historical context.

## Why

`load_platform()` today walks ~250 `.pure` files through the parser and
the five-pass compiler on every process startup. The parser/compiler
work is deterministic given fixed sources, so a one-time
parse-and-compile, serialized to disk, would let downstream processes
skip directly to a usable `PureModel`.

In `Repo::Embedded` form the sources are even bundled into the binary,
but the parse-and-compile cost is paid every run. A `Repo::Purem`
variant would carry the post-compilation `PureModel` directly, replacing
`parse → compile` with `deserialize`.

Two consumer scenarios drive this:

1. **CLI startup** — `legend test`, `legend check`, `legend repl`. ~1 s
   parse-and-compile on a warm cache; long enough that REPL feels
   sluggish on cold start.
2. **JNI / FFI initialization** — `Java_*_nativeInitContext` does the
   same parse-and-compile every JVM run. JNI cannot use `--live`, so
   speeding up the embedded path benefits every Java consumer.

## Java parallel

Java already solved a structurally identical problem with
[`BinaryRepositorySerializer`][java-bin]:

- `idCounter` (u32)
- `anonymousIdCounter` (u32)
- `fileNames[]` (length-prefixed strings)
- `topLevelIds[]` (u32s)
- `nodeBytes[]` per node, with a flag-driven optional "real keys" tail

That format is consumed inside the Java runtime (`PureRepositoryJar`)
and produced by the Maven `legend-pure-maven-generation-platform-java`
plugin. The artifact lands at `target/classes/pure-platform.par`.

A few questions stand out before any Rust implementation:

- **Interop goal.** Should Rust read Java's `.par`? It would let the
  same `target/` artifact serve both stacks, but couples Rust's
  `PureModel` to Java's CoreInstance/M4 representation. Rust's
  `PureModel` is a *post-compilation* object (resolved types, dispatch
  tables); Java's serializer captures the unresolved instance graph and
  re-resolves on load. These are different layers — interop probably
  isn't worth the constraint.
- **Independent format.** A Rust-native `.purem` written for the
  post-compilation `PureModel` is simpler to design *and* faster to
  load (no resolution step). The cost is no shared artifact across
  stacks. **Recommended starting point.**

[java-bin]: ../../../../legend-pure-core/legend-pure-m4/src/main/java/org/finos/legend/pure/m4/serialization/binary/BinaryRepositorySerializer.java

## What gets serialized

A serialized `.purem` must reconstruct everything the runtime depends
on — anything not in the file must be re-derivable from what is.

**Carried in the blob:**

- `PureModel.chunks` — the per-source-file `ModelChunk` list, including:
  - `Arena<ElementNode>` (M3 instance graph)
  - `Arena<Element>` (resolved declarations)
  - Per-chunk index maps (FQN → ElementId)
- Type hierarchy: classifier links, generalization edges
- Function dispatch tables: overload sets keyed by simple name
- Auto-import packages applied during compilation
- Source-information records (`SourceInfo`) for diagnostics & coverage

**Re-derivable on load (do not serialize):**

- Computed caches like memoization tables — populated lazily at runtime
- Type-erasure caches the runtime warms up on first use

**Open question:** the M3 chunk (chunk 0, populated by `m3_parser` from
the embedded `m3.pure`) is itself derivable. We can either serialize it
like every other chunk, or always re-derive it at load time. The former
costs a few KB; the latter saves disk but couples the format to the M3
parser. **Recommended: serialize like everything else** — keeps load
time deterministic.

## ID stability across `Arena<ElementNode>` round-trips

`ElementId` is an arena index. After deserialize-into-fresh-arena the
indices will not generally match the originals. References inside the
graph (e.g. property `type: ElementId` pointing at a Class) must
survive the round-trip.

**Approach: canonical ordering + remap table.**

1. **At serialize time**, write each chunk's nodes in a canonical order
   — sort by FQN before writing. The serialized index of each node is
   thus a stable function of its name.
2. **At deserialize time**, push nodes into a fresh arena in serialized
   order. Build `serialized_id → new_id: HashMap<u32, ElementId>` as a
   side effect of the push.
3. **Cross-references** (every field of type `ElementId`) get rewritten
   via the map after all nodes are pushed.

This is the same pattern Java uses (`idCounter` + `topLevelIds[]` form
implicit indices). The cost is one extra pass over every cross-reference
edge during deserialize. For the platform's ~1300 elements that's
negligible.

**Open question:** anonymous nodes. Java tracks
`anonymousIdCounter` separately. We need to decide whether anonymous
nodes (synthetic types created during compilation, e.g.
`FunctionType<...>`) get serialized at all, or are reproduced from a
canonical form. **Recommended: serialize them too** — they're real
ElementIds, referred to by other parts of the model, and trying to
recompute them invites divergence.

## `im-rc` structural sharing

`PureModel` uses `im-rc` (HAMT/RRB) collections for things like
property maps and association tables. These share structure across
clones, which is great for the in-memory representation but bad for
serialization: a naive walk produces an inflated wire format that loses
the sharing on rebuild.

**Two strategies:**

- **Canonicalize then rebuild.** Walk each `im-rc` collection, write a
  flat `Vec<(K, V)>`, deserialize back into a fresh `im-rc` collection.
  Loses memory-side sharing across collections but is simple and
  correct. **Recommended for v1.**
- **Wire-level dedup.** Write each subtree once, indirect references on
  the wire. Saves disk but adds significant code. Only worth it if
  profiling shows wire size matters.

Either way, in-memory sharing is rebuilt by the deserializer; the
on-disk format does not need to model it.

## Versioning

```text
[u8;5]   magic = b"PUREM"
[u8]     reserved (always 0 in v1)
u32      format_version    // bumps on incompatible layout
u32      schema_hash       // hash of the PureModel struct layout
u64      content_length    // payload bytes after this header
```

- **Magic + reserved** lets us detect the format unambiguously.
- **`format_version`** bumps any time the on-disk layout changes
  incompatibly.
- **`schema_hash`** is a build-time fingerprint of the Rust types being
  serialized — protects against subtle drift (e.g. a new field on
  `ElementNode` that didn't bump `format_version`).
- **`content_length`** lets readers skip / mmap precisely.

**Refusal policy in v1:** `format_version` mismatch → hard error.
`schema_hash` mismatch → hard error. No automatic migration.

## Determinism

Re-serializing the same `PureModel` must produce byte-identical output.
Otherwise CI caching is unsound and `git diff` is misleading.

**Requirements:**

- Sorted iteration over every map; no `HashMap` in serialized output.
  Use `BTreeMap` or sort keys before writing.
- No timestamps, build paths, or environment data in the blob. (The
  `PureModel` itself doesn't carry any; verify this hasn't drifted.)
- Stable `ElementId` ordering as above.

**CI test:** `serialize(model) == serialize(deserialize(serialize(model)))`
asserted byte-identical, twice.

## Verification

Two test gates land alongside the implementation:

1. **Round-trip property test.**
   `compile-from-source → serialize → deserialize → compare to original`.
   Compares element-name sets, error counts, and per-element
   resolved-type fingerprints. (Same shape as the existing
   `repo_parity` test.)
2. **Platform PCT against both load paths.** Run the existing PCT suite
   first against `Repo::Embedded` (parse + compile from sources) and
   then against `Repo::Purem` (deserialize). Identical pass/fail
   counts. Any divergence flags a serialization gap.

## `Repo::Purem` integration

The `Repo` enum already has a placeholder slot for a third variant.
Sketch:

```rust
pub enum Repo {
    Embedded { /* ... */ },
    Filesystem { /* ... */ },
    Purem {
        prefix: String,
        meta: Option<RepoMeta>,
        blob: Vec<u8>,            // or Bytes / mmap'd region
    },
}
```

Crucially, `Repo::Purem` does *not* expose `.pure` sources. Its
`sources()` returns an empty iterator. Callers must distinguish
"this repo's content is `.pure` text" from "this repo's content is a
pre-compiled `PureModel`." That changes the shape of `repo::load`:

```rust
pub fn load(repos: &[Repo], auto_imports: &[SmolStr])
    -> Result<PureModel, PartialPureModel>
{
    // Conceptual:
    //  1. Deserialize every Purem repo into a partial PureModel.
    //  2. Parse-and-compile every Embedded/Filesystem repo's sources.
    //  3. Merge the resulting PureModels.
    // (3) is the load-bearing new step — see "Merge semantics" below.
}
```

### Merge semantics (the hard part)

Combining two `PureModel`s requires resolving:

- **Cross-model `ElementId` references.** Each model has its own
  `Arena<ElementNode>`; merging means relocating ids again. Probably
  cheaper to merge during deserialize (place every Purem chunk into a
  shared arena before compile-from-source kicks in).
- **Auto-imports.** Each repo may bring different auto-import lists.
  The current API takes a single `auto_imports` argument; need to
  decide whether to honor per-repo lists or require callers to provide
  the union.
- **Overload tables.** Function dispatch is keyed by simple-name across
  the whole model. Two repos defining the same simple name (one in
  Purem, one in source) need the same conflict-resolution as today.

**Open question:** is `Repo::Purem` constrained to be a *whole-platform*
snapshot (no merging), or can it be a per-repo snapshot mixed with
source repos? The latter is more flexible but multiplies the design
surface. **Recommended for v1: whole-platform Purem only.** A single
`Repo::Purem` variant in the list short-circuits compile-from-source
entirely; mixing isn't supported until the merge story is clear.

## Decisions (as shipped — v0)

1. **Wire format: hand-rolled magic header + Postcard-encoded payload.**
   FlatBuffers / planus deferred to v1; the wire format is private to
   `write_repo` / `read_repo` so swapping is contained. Postcard's
   non-self-describing encoding is deterministic by construction —
   zero field reordering, fixed varint discipline. Determinism gate
   asserts `write_repo(slice) == write_repo(slice)` and the byte-stable
   round-trip; both pass on the real platform (3 MB blob).
   - **Rust-decimal note:** `rust_decimal` defaults to a lenient
     `deserialize_any`-driven serde impl that postcard rejects. Use
     `rust_decimal = { features = ["serde-str"] }` instead.
2. **Java compatibility.** No — operates on the post-compilation
   `PureModel`, not the Java unresolved CoreInstance graph.
3. **Per-repo not whole-platform.** `slice_by_repo(model, chunk_range)`
   carves any chunk-id range; `merge_slice(running_model, slice)`
   resolves cross-range refs via FQN strings against the running
   model. Per-repo is the composition primitive: `repo::load`
   topo-sorts every repo by its declared dependencies and processes
   each one in dep order.
4. **Anonymous nodes.** Serialized (whatever appears in the chunk
   range, including synthetic types).
5. **M3 chunk.** Always re-derived at load time via
   `init_bootstrap_model()` — no benefit to serializing the bootstrap
   chunk since it's deterministic given the embedded `m3.pure`.
6. **Format-version policy on mismatch.** Strict refusal — bad magic /
   wrong `format_version` / wrong `schema_hash` are all hard errors at
   read time. Migration tooling deferred.
7. **`im-rc` rebuild strategy.** Canonicalize-then-rebuild via Postcard's
   default `Vec<T>` encoding for `Rc<[T]>` (with serde's `rc` feature).
   Wire-level dedup deferred.
8. **Producer surfaces.** Library:
   `legend_pure_parser_pure::purem::{write_repo, read_repo}`. CLI:
   `legend snapshot --output …`. Both ship in v0.
9. **Classpath TOML.** `legend-pure-classpath.toml` with `[[repo]]`
   entries (`kind = "purem"` / `"filesystem"`); parsed by
   `legend_cli::classpath::load_classpath`. Full per-command
   `--classpath` wiring deferred to a follow-up.

## Crate layout (as shipped)

- `crates/pure/src/purem/header.rs` — magic + version + schema_hash +
  payload_len.
- `crates/pure/src/purem/{slice,walk,fqn_path}.rs` — in-memory slice +
  ElementId visitor + canonical FQN computation.
- `crates/pure/src/purem/{writer,reader}.rs` — bytes ↔ slice.
- `crates/core-platform-pure/src/topo.rs` — topo_sort_repos + cycle
  detection.
- `crates/core-platform-pure/src/repo.rs` — `Repo::Purem` variant +
  `repo::load` rework (topo-sort + per-repo + finalize).
- `crates/cli/src/commands/snapshot.rs` — `legend snapshot`.
- `crates/cli/src/classpath.rs` — TOML parser + `Repo` builder.

## Producer

Whatever format wins, the producer story is small:

```bash
legend snapshot --output platform.purem
```

Internally calls `load_platform()` then `Repo::serialize_purem(&model)`.
Tests would freeze the serialized output as a checked-in artifact
(probably under `tests/fixtures/`) and assert byte-equality on every
build — that's the determinism gate.

## What this doc explicitly does NOT decide

- Specific byte layouts, struct field orderings, or wire schemas.
- Whether to pre-compute and cache *anything* beyond `PureModel`
  (e.g. dispatch caches, type-erasure tables).
- How `--live` interacts with `Purem` (probably: `--live` always wins
  over a `Purem` repo of the same name).
- JNI surface for selecting source vs. snapshot loading.

These decisions land in a follow-up design pass once the open questions
above are resolved.
