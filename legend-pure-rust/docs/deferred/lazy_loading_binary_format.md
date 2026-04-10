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
| Format versioning | **Backward compatible** | FlatBuffers field-IDs give us this for free; golden-file tests enforce it |
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
  name: string;                       // id: 0
  fqn: string;                        // id: 1
  kind: ElementKind;                  // id: 2
  parameters: [ParameterSig];         // id: 3
  return_type: TypeExprSig;           // id: 4
  return_multiplicity: Multiplicity;  // id: 5
  body_offset: uint32;                // id: 6
  body_length: uint32;                // id: 7
  // Extension element support
  classifier_fqn: string;             // id: 8  (for packageable instances)
  instance_properties: [PropertyValue]; // id: 9
  // Future fields added here — old readers skip them
}

table FunctionBody {
  expressions: [Expression];
}
```

---

## Backward Compatibility Strategy

The `.purem` format is backward compatible **from v1**. We leverage FlatBuffers'
native schema evolution: new fields are appended (old readers skip them), removed
fields return defaults (new readers handle `None`). No migration code ever.

### How It Works

- Every field has a permanent numeric ID (assigned once, never reused)
- The vtable in each FlatBuffer object records which fields are present
- Old reader + new data → unknown field IDs are ignored
- New reader + old data → missing fields return `None` / default

### Golden-File Version Tests

We maintain a corpus of **versioned `.purem` golden files** in the test suite:

```
tests/golden/
  v1_basic_class.purem        ← emitted by compiler v1
  v1_function_with_body.purem
  v2_with_source_file.purem   ← emitted after adding source_file field
  v3_with_instances.purem     ← emitted after adding instance support
```

Every CI run verifies that the **current loader can read all historical golden
files**:

```rust
#[test]
fn load_v1_basic_class() {
    let bytes = include_bytes!("golden/v1_basic_class.purem");
    let model = load_purem(bytes).expect("v1 golden file must load");
    assert_eq!(model.get_node(id).name, "Person");
    // New fields return None — that's fine
    assert!(model.get_header(id).source_file().is_none());
}
```

This is the same pattern used by Protocol Buffers' conformance tests and
Android's asset bundle evolution.

### Rules for Schema Changes

| Change | Allowed? | Action |
|--------|----------|--------|
| Add new field at end | ✅ | Assign next ID, add golden file |
| Deprecate field | ✅ | Stop writing it, keep ID reserved |
| Add enum value | ✅ | Append to end |
| Remove field ID | ❌ | Never — old data becomes unreadable |
| Change field type | ❌ | Never — bytes misinterpreted |
| Reorder field IDs | ❌ | Never — IDs are permanent |

---

## Extensible Element Types: Packageable Instances

The `Element` enum today has a fixed set of variants (`Class`, `Function`,
`Enumeration`, etc.). But the Legend system is extensible — `###Mapping`,
`###Relational`, `###Service`, etc. each introduce new element types via
plugin grammars.

In the Java Legend Engine, these are **not** hardcoded element types. They are
**instances of M3 classes** added to the graph as packageable elements:

```
###Mapping             → instance of meta::pure::mapping::Mapping
###Relational          → instance of meta::relational::metamodel::Database
###Service             → instance of meta::legend::service::Service
```

### The Problem

If we use a closed `ElementKind` enum in the `.purem` format:

```fbs
enum ElementKind : byte { Class, Function, Enum, Profile, Association, Measure, Unit }
```

...then every new plugin requires a schema change. This fights the extensibility
model.

### The Solution: Packageable Instances

Instead of adding enum variants, treat extension elements as **packageable
instances** — an element that is an instance of a known class (identified by
FQN), with properties stored as key-value data.

```fbs
enum ElementKind : byte {
  Class,
  Enumeration,
  Function,
  Profile,
  Association,
  Measure,
  Unit,
  PrimitiveType,
  PackageableInstance   // ← catch-all for extensions
}

table ElementHeader {
  // ... core fields ...
  kind: ElementKind;
  // For PackageableInstance elements:
  classifier_fqn: string;              // e.g., "meta::pure::mapping::Mapping"
  instance_properties: [PropertyValue]; // key-value property data
}

table PropertyValue {
  name: string;
  value: PropertyValueData;  // union of string, int, bool, element_ref, nested
}
```

### How It Works

| Element | `kind` | `classifier_fqn` | `instance_properties` |
|---------|--------|-------------------|-----------------------|
| `Person` class | `Class` | *(unused)* | *(unused)* |
| `doSomething` function | `Function` | *(unused)* | *(unused)* |
| `myMapping` mapping | `PackageableInstance` | `"meta::pure::mapping::Mapping"` | `[{"name": "classMappings", ...}]` |
| `myDB` database | `PackageableInstance` | `"meta::relational::...::Database"` | `[{"name": "schemas", ...}]` |

Core element types (`Class`, `Function`, etc.) keep their dedicated enum values
for fast dispatch. Extension types all use `PackageableInstance` with the
`classifier_fqn` telling the loader what M3 class they instantiate.

### Why This Works

1. **No schema changes for new plugins**: A new `###FooBar` grammar just
   produces a `PackageableInstance` with `classifier_fqn = "meta::foo::Bar"`.
   The `.purem` format doesn't change.
2. **Self-describing**: The `classifier_fqn` tells any reader what the element
   is, even if the reader doesn't have the plugin loaded.
3. **Mirrors Java**: This is exactly how the Java Legend Engine works — plugins
   create instances of M3 classes and add them to the graph.
4. **Backward compatible**: Old loaders that don't know about `PackageableInstance`
   see it as an unknown enum value and can skip it gracefully.
5. **Forward compatible**: New kinds of instances can be created without any
   format changes — just new `classifier_fqn` values.

### In-Memory Representation

```rust
pub enum Element {
    // Core types (dedicated, fast dispatch)
    Class(Class),
    Function(Function),
    Enumeration(Enumeration),
    Profile(Profile),
    Association(Association),
    Measure(Measure),
    Unit(Unit),
    PrimitiveType(PrimitiveType),
    // Extension types (open, self-describing)
    PackageableInstance(PackageableInstance),
}

pub struct PackageableInstance {
    /// The M3 class this is an instance of (e.g., Mapping, Database, Service).
    pub classifier: ElementId,
    /// Property values — the instance's data.
    pub properties: Vec<InstancePropertyValue>,
    /// Properties found in the .purem but unknown to the current M3 class.
    /// Preserved for safe round-tripping.
    pub opaque_properties: Vec<OpaquePropertyValue>,
}
```

### Instance Backward Compatibility

The binary format has **two layers of compatibility** that compose independently:

| Layer | Mechanism | What It Handles |
|-------|-----------|-----------------|
| **Wire format** (FlatBuffers) | Field IDs, vtable | "Can I parse the bytes?" |
| **Semantic** (M3 class definitions) | Property defaults, lenient loading | "Do I understand the instance data?" |

FlatBuffers handles the wire format. The M3 class definition handles the semantic
layer — it is the **schema** for `PackageableInstance` data.

#### Missing Properties (Old `.purem`, New M3 Class)

When the M3 class evolves (e.g., `Mapping` gains a `testSuites` property), old
`.purem` files won't have that property. The loader resolves this using Pure's
native default value behavior:

```pure
// v2 of the Mapping class — new [1] property with default
Class Mapping {
    classMappings: ClassMapping[*];
    testSuites: MappingTestSuite[1] = ^MappingTestSuite(tests = []);
}
```

| Property Multiplicity | Missing in old `.purem` | Resolution |
|----------------------|------------------------|------------|
| `[0..*]` or `[0..1]` | Fine — optional | Defaults to empty/None |
| `[1]` **with default** | Fine — Pure default value applies | Loader fills from M3 class definition |
| `[1]` **without default** | ⚠️ True breaking change to M3 class | Validation error (class design issue, not format issue) |

This means adding a new `[1]` property is **not a breaking change** as long as
it has a default value — which is standard Pure practice.

#### Unknown Properties (New `.purem`, Old Loader)

When a new `.purem` contains properties the current loader doesn't recognize
(e.g., a property added by a newer plugin version), the loader preserves them
opaquely:

```
New .purem has: { name: "testSuites", value: [...] }
Old loader:     doesn't know "testSuites" on Mapping
Resolution:     store in opaque_properties → data round-trips safely
```

This prevents data loss when shuttling `.purem` files between different
compiler/plugin versions.

#### Hydration Flow

```rust
fn hydrate_instance(
    raw: &ArchivedPackageableInstance,
    classifier: &Class,
    model: &PureModel,
) -> PackageableInstance {
    let mut properties = Vec::new();
    let mut opaque_properties = Vec::new();

    // Step 1: Walk the M3 class properties (the semantic schema)
    for class_prop in &classifier.properties {
        if let Some(stored) = raw.find_property(&class_prop.name) {
            // Property present in .purem → use stored value
            properties.push(resolve_property_value(stored, model));
        } else if let Some(default) = &class_prop.default_value {
            // Property missing but has Pure default → apply it
            properties.push(default.clone());
        } else if class_prop.multiplicity.allows_zero() {
            // Optional → empty
            properties.push(InstancePropertyValue::empty());
        } else {
            // Required [1], no default, not in data → error
            // (This is a breaking change to the M3 class definition)
        }
    }

    // Step 2: Preserve unknown properties for round-tripping
    for stored in raw.properties() {
        if !classifier.has_property(&stored.name()) {
            opaque_properties.push(OpaquePropertyValue::from(stored));
        }
    }

    PackageableInstance {
        classifier: classifier_id,
        properties,
        opaque_properties,
    }
}
```

#### Property Value Types

The `PropertyValue` value type uses a FlatBuffers union to cover all M3 value
kinds:

```fbs
union PropertyValueData {
    StringValue,
    IntValue,
    BoolValue,
    FloatValue,
    ElementRefValue,      // cross-chunk FQN or local index
    CollectionValue,      // nested list of PropertyValueData
    InstanceValue,        // nested PackageableInstance
}

table PropertyValue {
    name: string;
    multiplicity: Multiplicity;
    value: PropertyValueData;
}
```

This mirrors M3's `InstanceValue` in Java — any property value is either a
primitive, an element reference, a collection, or a nested instance. The union is
fixed at the FlatBuffers level, but the *interpretation* is driven by the M3
class definition at load time.

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
