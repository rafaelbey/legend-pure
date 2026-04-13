# Lazy-Loading Arena via Compiled Binary Format

## Revised Framing

The compiler emits a binary format (`.purem`). Loading = mmap that binary. No parser, no AST, no re-lowering. The laziness lives in the binary format itself — mmap the file, read the header table (element signatures, package tree), and only page in function bodies when actually executed.

---

## Decisions Made

| Question | Decision | Rationale |
|----------|----------|-----------|
| Format versioning | Reject incompatible | Focus on current compiler version; no migration |
| Body granularity | Per-body | Minimal memory; each function body = separate subtable |
| Derived indexes | Recompute on load | Simpler, no cross-chunk stale data risk |
| Serialization library | **FlatBuffers** | Schema evolution must remain an option for future; field-ID-based schema handles it for free |
| Cross-chunk references | **Option (b) — FQN strings** | Robust, decoupled, one-time resolution cost at load |

---

## The Two-Tier Binary Layout

A compiled Pure module (`.purem`) contains two tiers of data:

```
┌──────────────────────────────────────────────────┐
│ File Header                                      │
│   magic, compiler_version, element_count         │
├──────────────────────────────────────────────────┤
│ Tier 1: Element Directory (always read)          │
│   ├── Package Tree (flat)                        │
│   ├── Element Headers[]                          │
│   │     name, fqn, package_id, element_kind      │
│   │     For Class:  super_types, property_sigs   │
│   │     For Function: params, return_type, mult  │
│   │     For Enum: values                         │
│   │     For Profile: stereotypes, tags           │
│   │     ...                                      │
│   └── Body Index Table                           │
│         element_idx → body_offset + body_length  │
├──────────────────────────────────────────────────┤
│ Tier 2: Body Data (paged on demand via mmap)     │
│   ├── Body[0]: self-contained per-element body   │
│   ├── Body[1]: ...                               │
│   ├── Body[N]: ...                               │
│   └── (each body is a separate FlatBuffer table) │
└──────────────────────────────────────────────────┘
```

**Tier 1** — small, read eagerly. Contains everything name resolution, type checking, and import resolution need.

**Tier 2** — potentially large, never read unless execution touches a specific body. Each body is a **separate FlatBuffer table** at a distinct offset. FlatBuffers' vtable-based access means touching one body's accessor never reads adjacent body data — the OS only pages in the mmap regions that are actually accessed.

---

## Why FlatBuffers

The deciding factor is **schema evolution**. While not needed today (we reject incompatible versions), the `.purem` format will outlive any single compiler version. FlatBuffers' field-ID-based schema makes adding new fields, deprecating old ones, and maintaining backward compatibility free — no migration code, no version-specific deserialization branches.

| Criterion | FlatBuffers | rkyv |
|-----------|-------------|------|
| **Schema evolution** | ✅ Field IDs — add/deprecate fields freely | ⚠️ Layout-dependent — any struct change = incompatible |
| **Zero-copy read** | ✅ Offset-based, no deserialization | ✅ Pointer cast |
| **Lazy field access** | ✅ Native — vtable skips untouched fields | ✅ Native |
| **mmap friendly** | ✅ Designed for it | ✅ Designed for it |
| **Cross-language** | ✅ C++, Java, Go, Python, Rust | ❌ Rust-only |
| **Build complexity** | ⚠️ Requires `flatc` codegen | ✅ Derive macros only |
| **Validation** | ✅ Built-in verifier | ✅ `bytecheck` |

> [!NOTE]
> FlatBuffers' vtable+offset access is slightly slower than rkyv's direct pointer cast in microbenchmarks, but the difference is negligible for our access patterns (read-once headers, occasional body loads). Schema evolution is worth far more than nanoseconds per field access.

### How FlatBuffers Works For Us

The `.fbs` schema defines the binary layout. `flatc --rust` generates zero-copy accessor types:

```fbs
// pure_model.fbs
namespace legend.pure.binary;

table ElementHeader {
  name: string;
  fqn: string;
  kind: ElementKind;
  parameters: [ParameterSig];
  return_type: TypeExprSig;
  return_multiplicity: Multiplicity;
  body_offset: uint32;   // offset into Tier 2
  body_length: uint32;
}
```

```rust
// Generated: zero-copy read from mmap buffer
let buf: &[u8] = /* mmap'd .purem file */;
let header = root_as_element_header(buf).unwrap();
println!("{}", header.name());  // no allocation — reads directly from buffer
// header.parameters() is lazy — only accessed fields are read
```

Each per-body subtable is a standalone FlatBuffer at a distinct offset:

```fbs
table FunctionBody {
  expressions: [Expression];
}
```

```rust
// Read body on demand from Tier 2
let body_buf = &mmap[body_offset..body_offset + body_length];
let body = root_as_function_body(body_buf).unwrap();
// Only this page is faulted in by the OS
```

---

## Cross-Chunk Reference Encoding — Tradeoff Analysis

When `user.purem` references a type from `stdlib.purem` (e.g., a function parameter typed as `String` or a supertype `Animal` from the stdlib), that reference must be encoded in the binary. Three options:

### Option (a): Embed `chunk_id` at Write Time

**How it works**: When serializing `user.purem`, the compiler knows that `String` is `ElementId { chunk_id: 0, local_idx: 2 }`. It writes the literal `chunk_id=0, local_idx=2` into the binary.

```
TypeExpr::Named { element: { chunk_id: 0, local_idx: 2 } }
```

| Aspect | Assessment |
|--------|------------|
| **Load speed** | ⚡ Fastest — IDs are already resolved, direct arena lookup |
| **Correctness** | ⚠️ **Fragile** — requires that `stdlib.purem` is loaded at exactly `chunk_id=0`. If loading order changes, or stdlib is split into multiple chunks, all references break |
| **Rebuild story** | ❌ If stdlib is recompiled (element order changes), all user `.purem` files become invalid — stale chunk_id/local_idx references point to wrong elements |
| **Multi-dependency** | ❌ If user code depends on `libA.purem` and `libB.purem`, the compiler must know their future chunk IDs at compile time — coupling |

**Verdict**: Fast but extremely brittle. Only works if you guarantee a fixed, stable loading order and never recompile dependencies independently.

---

### Option (b): FQN Strings Resolved at Load Time

**How it works**: Cross-chunk references are stored as fully qualified name strings (e.g., `"String"`, `"my::pkg::Animal"`). When loading `user.purem`, the loader resolves each FQN against the already-loaded model to get an `ElementId`.

```
TypeExpr::Named { element_ref: "my::pkg::Animal" }
                                ↓ at load time
TypeExpr::Named { element: { chunk_id: 1, local_idx: 7 } }  // resolved
```

| Aspect | Assessment |
|--------|------------|
| **Load speed** | 🐢 Slower — O(N × R) where N = elements, R = avg references per element. Each FQN needs a package-tree walk. But this is **amortizable**: cache resolved FQNs during load |
| **Correctness** | ✅ **Robust** — FQNs are stable identifiers. If stdlib is recompiled with different internal ordering, FQNs still resolve correctly |
| **Rebuild story** | ✅ Dependencies can be recompiled independently. As long as FQNs are preserved, user `.purem` files remain valid |
| **Multi-dependency** | ✅ No coupling to loading order or chunk IDs |
| **File size** | ⚠️ Slightly larger — strings take more space than u32+u16 IDs. Mitigated by FlatBuffers' shared-string deduplication |

**Verdict**: Robust, simple, decoupled. The resolution cost is one-time at load and cached. This is how JVM class files work (symbolic references resolved by the classloader).

---

### Option (c): Relocation Table

**How it works**: The binary uses local placeholder IDs for all cross-chunk references. A "relocation table" at the end of the file maps each placeholder to a FQN. The loader reads the relocation table, resolves FQNs to real `ElementId`s, and patches the in-memory structures.

```
Relocation Table:
  placeholder_id=1001 → FQN="String"
  placeholder_id=1002 → FQN="my::pkg::Animal"

Body data uses placeholder_id=1001 wherever it means String
```

| Aspect | Assessment |
|--------|------------|
| **Load speed** | ⚡ Fast read + one-time O(R) relocation pass |
| **Correctness** | ✅ Same robustness as (b) — FQNs in the reloc table |
| **Rebuild story** | ✅ Same independence as (b) |
| **Complexity** | ⚠️ More moving parts — you maintain a relocation table, placeholder ID space, and a patching pass |
| **Zero-copy** | ⚠️ FlatBuffers data is read-only (mmap). Can't patch in place — would need a side `HashMap<PlaceholderId, ElementId>` adding indirection to every lookup |

**Verdict**: Relocation tables are the right tool for ELF-style pointer patching. For FlatBuffers on mmap, they add complexity (side table for every reference lookup) without meaningful benefit over Option (b), which also resolves once and caches.

---

### Recommendation: Option (b) — FQN Strings

For our use case, **(b) wins**:

1. **Correctness first**: Dependencies can be recompiled independently. No coupling to load order.
2. **Simplicity**: No relocation machinery. The loader just does what `resolve.rs` already does — walk the package tree.
3. **One-time cost**: Resolution happens once at load time, results cached in the `Arena<Element>`. After that, all access is by `ElementId` (fast).
4. **Natural fit**: Within a `.purem` file, *intra-chunk* references use `local_idx` (fast, stable). Only *cross-chunk* references use FQN strings.

The hybrid encoding:
```
// Intra-chunk reference (same .purem file)
ElementRef::Local(local_idx: u32)

// Cross-chunk reference (different .purem file)
ElementRef::External(fqn: String)
```

This means Tier 1 headers and Tier 2 bodies use `ElementRef` instead of raw `ElementId`. During loading, the loader resolves all `External` refs to `ElementId`s, producing a fully-resolved in-memory `PureModel` with zero ongoing string lookup costs.

---

## Integration with Current Architecture

### What Changes

| Component | Today | With `.purem` |
|-----------|-------|---|
| `Arena<Element>` | Fully hydrated `Element` enums | `Arena<LazyElement>` — header in RAM, body = offset into mmap |
| `ModelChunk` | Built by compiler pipeline | Built by compiler pipeline **OR** loaded from `.purem` |
| `pipeline::compile()` | Returns `PureModel` | Returns `PureModel` — then optionally serialized to `.purem` |
| `get_element()` | Returns `&Element` | Returns view of header (always available) |
| `get_body()` | N/A | New — reads body from mmap buffer via FlatBuffers, caches in `OnceLock` |
| CLI | `legend compile` | `legend compile --emit-binary` + `legend run foo.purem` |

### What Stays the Same

- **Package tree**: same `Arena<Package>`, same resolution logic
- **ElementId**: same `{ chunk_id, local_idx }` — chunk = loaded `.purem`
- **Bootstrap**: Chunk 0 stays in memory (13 elements, not worth serializing)
- **Derived indexes**: recomputed on load after all chunks are assembled
- **Type resolution**: works on Tier 1 headers — no change
- **Validation**: works on headers — body access only for expression-level checks

### The `LazyElement` Type

```rust
pub enum LazyElement {
    /// Fully loaded in memory (compiled from source, or body already accessed).
    Loaded(Element),
    /// Header loaded, body deferred to the backing buffer.
    Deferred {
        header: ElementHeader,
        body_ref: BodyRef,
    },
}

pub struct BodyRef {
    /// Byte offset into the mmap buffer (points to a FlatBuffer subtable).
    offset: u32,
    /// Byte length of the body subtable.
    length: u32,
    /// Cached deserialized body (populated on first access).
    cache: OnceLock<ElementBody>,
}
```

---

## Standard Library Story

1. **Build**: Compile `.pure` stdlib sources → `stdlib.purem`
2. **Ship**: Embed `stdlib.purem` in the `legend` binary via `include_bytes!` or install alongside
3. **Load**: mmap → read Tier 1 headers → resolve package tree (instant)
4. **Execute**: function bodies page in from disk only when called

---

## CLI Surface

```bash
# Compile to binary
legend compile src/ --output lib.purem

# Run from binary
legend run lib.purem --function "my::pkg::main"

# Load stdlib + user code
legend run stdlib.purem user.purem --function "my::app::run"

# Inspect a binary
legend inspect lib.purem
```

---

## All Decisions Finalized

All open questions have been resolved:

| Decision | Choice |
|----------|--------|
| Serialization | FlatBuffers (schema evolution) |
| Body granularity | Per-body subtables |
| Derived indexes | Recompute on load |
| Cross-chunk refs | FQN strings, resolved at load time |
| Format versioning | Reject incompatible |

This design is ready for implementation planning when the time comes.
