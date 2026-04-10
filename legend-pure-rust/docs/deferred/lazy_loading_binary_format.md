# Lazy-Loading Arena via Compiled Binary Format (`.purem`)

> **Status: Deferred** — This is a future improvement. Current focus is on the
> MVP of loading and running core Pure code.

## Overview

The compiler emits a binary format (`.purem`). Loading = mmap that binary. No
parser, no AST, no re-lowering. The laziness lives in the binary format itself —
mmap the file, read the header table (element signatures, package tree), and
only page in function bodies when actually executed.

This is the `.class` file model (JVM), the `.pyc` model (Python), the `.rmeta`
model (Rust), taken to its logical conclusion for Pure.

---

## Design Decisions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Format versioning | Reject incompatible | Focus on current compiler version; no migration |
| Body granularity | Per-body | Minimal memory; each function body = separate subtable |
| Derived indexes | Recompute on load | Simpler, no cross-chunk stale data risk |
| Serialization library | **FlatBuffers** | Schema evolution must remain an option for future; field-ID-based schema handles it for free |
| Cross-chunk references | **FQN strings** | Robust, decoupled, one-time resolution cost at load |

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

**Tier 1** — small, read eagerly. Contains everything name resolution, type
checking, and import resolution need.

**Tier 2** — potentially large, never read unless execution touches a specific
body. Each body is a **separate FlatBuffer table** at a distinct offset.
FlatBuffers' vtable-based access means touching one body's accessor never reads
adjacent body data — the OS only pages in the mmap regions that are actually
accessed.

---

## Why FlatBuffers

The deciding factor is **schema evolution**. While not needed today, the `.purem`
format will outlive any single compiler version. FlatBuffers' field-ID-based
schema makes adding new fields, deprecating old ones, and maintaining backward
compatibility free — no migration code, no version-specific deserialization
branches.

| Criterion | FlatBuffers | rkyv |
|-----------|-------------|------|
| **Schema evolution** | ✅ Field IDs — add/deprecate fields freely | ⚠️ Layout-dependent — any struct change = incompatible |
| **Zero-copy read** | ✅ Offset-based, no deserialization | ✅ Pointer cast |
| **Lazy field access** | ✅ Native — vtable skips untouched fields | ✅ Native |
| **mmap friendly** | ✅ Designed for it | ✅ Designed for it |
| **Cross-language** | ✅ C++, Java, Go, Python, Rust | ❌ Rust-only |
| **Build complexity** | ⚠️ Requires `flatc` codegen | ✅ Derive macros only |
| **Validation** | ✅ Built-in verifier | ✅ `bytecheck` |

### FlatBuffers Schema Sketch

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

table FunctionBody {
  expressions: [Expression];
}
```

---

## Cross-Chunk Reference Encoding

When `user.purem` references a type from `stdlib.purem`, the reference is
encoded as an **FQN string** (fully qualified name). At load time, the loader
resolves each FQN against the already-loaded model to get an `ElementId`.

### Why FQN Strings (Option b)

Three options were evaluated:

| Option | Speed | Correctness | Complexity |
|--------|-------|-------------|------------|
| **(a) Embedded chunk_id** | ⚡ Fastest | ❌ Fragile — breaks on recompile or load-order change | Low |
| **(b) FQN strings** | 🐢 One-time resolution cost | ✅ Robust — FQNs are stable identifiers | Low |
| **(c) Relocation table** | ⚡ Fast + one-time reloc pass | ✅ Robust | ⚠️ Higher — side table, patching |

**Option (b) wins**: robust, simple, decoupled. Resolution happens once at load
time, results are cached. After that, all access is by `ElementId` (fast).

### Hybrid Encoding

```
// Intra-chunk reference (same .purem file)
ElementRef::Local(local_idx: u32)

// Cross-chunk reference (different .purem file)
ElementRef::External(fqn: String)
```

---

## Compilation Boundary: What Goes Into the `.purem`

When compiling user code that depends on stdlib, the compiler knows exactly what
to output because **chunks define the boundary**:

```
   stdlib.purem ──────┐
   (loaded, Tier 1)   │
                      ├──▶ Compiler ──▶ user.purem
   user/*.pure  ──────┘                  (ONLY user elements)
   (parsed + compiled)
```

| Source | Chunk | In output `.purem`? |
|--------|-------|---------------------|
| Bootstrap (primitives, Any, Nil) | Chunk 0 | ❌ Always built-in |
| `stdlib.purem` | Chunk 1 (loaded) | ❌ Read-only dependency |
| `libA.purem` | Chunk 2 (loaded) | ❌ Read-only dependency |
| User `.pure` sources | Chunk 3 (compiled) | ✅ **This is the output** |

The emitter uses `chunk_id` as the ownership tag:

```rust
fn encode_element_ref(id: ElementId, my_chunk_id: u16, model: &PureModel) -> ElementRef {
    if id.chunk_id == my_chunk_id {
        ElementRef::Local(id.local_idx)     // same chunk → fast local index
    } else {
        let fqn = model.fully_qualified_name(id);
        ElementRef::External(fqn)           // cross-chunk → FQN string
    }
}
```

This is analogous to:

| System | Dependencies | Output |
|--------|-------------|--------|
| **javac** | Reads `.class` from classpath | Emits `.class` for your `.java` |
| **rustc** | Reads `.rmeta` from deps | Emits `.rlib` for your crate |
| **legend** | Reads `.purem` Tier 1 headers | Emits `.purem` for your `.pure` |

---

## Integration with Current Architecture

### What Changes

| Component | Today | With `.purem` |
|-----------|-------|---------------|
| `Arena<Element>` | Fully hydrated | `Arena<LazyElement>` — header in RAM, body = offset |
| `ModelChunk` | Built by pipeline | Built by pipeline **OR** loaded from `.purem` |
| `get_element()` | Returns `&Element` | Returns view of header (always available) |
| `get_body()` | N/A | Reads body from mmap, caches in `OnceLock` |

### What Stays the Same

- **Package tree**: same `Arena<Package>`, same resolution logic
- **ElementId**: same `{ chunk_id, local_idx }` — chunk = loaded `.purem`
- **Bootstrap**: Chunk 0 stays in memory (13 elements)
- **Derived indexes**: recomputed on load after all chunks assembled
- **Type resolution**: works on Tier 1 headers — no change

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
