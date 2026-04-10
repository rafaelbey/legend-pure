# Pure Semantic Layer: Design Decisions

This document records the architectural choices for the Pure Semantic Layer. It serves as a reference for contributors, future maintainers, and AI agents.

## Architecture

```
Source → Parser → AST → Pure (this crate) → Execution / Validation
                         ↑
                   Arena/Index graph
```

The Pure crate consumes `ast::SourceFile` and produces a `PureModel` — a fully resolved, navigable semantic graph equivalent to Java's `PureModel`.

## Design Decisions

### 1. Arena/Index Pattern (No References)

The entire graph uses flat `Vec` arenas indexed by lightweight `Copy` IDs. **No `&` references between nodes.**

```rust
pub struct ElementId { chunk_id: u16, local_idx: u32 }
```

**Why not references?** Rust's borrow checker makes reference-based graphs extremely painful — you need `Rc<RefCell<>>`, `unsafe`, or arena lifetimes. The index pattern gives O(1) lookup with zero borrow checker friction, and the IDs are trivially serializable.

**Why segmented IDs?** The `chunk_id` enables zero-rewrite model merging. When a new chunk is added, all existing `ElementId`s remain valid — no reindexing.

### 2. Unidirectional Canonical Data + Derived Indexes

The Java `PureModel` stores **bidirectional pointers** everywhere. A Class holds both its supertypes AND is poked by subclasses (`_specializationsAdd`), Associations inject properties into Classes (`_propertiesFromAssociationsAdd`), etc. This creates a mutation nightmare.

**Our rule: Nodes store only canonical, unidirectional data from the AST.** All reverse lookups are computed once after the model freezes.

| Java Bidirectional Mutation | Pure Solution |
|---|---|
| `superTypeClass._specializationsAdd(g)` | `specialization_index` (derived) |
| `source._propertiesFromAssociationsAdd(prop)` | `association_property_index` (derived) |
| `_class._qualifiedPropertiesFromAssociationsAdd(q)` | `association_qualified_property_index` (derived) |
| `Generalization._specific(_class)` back-pointer | **Eliminated** — `super_types` is `Vec<ElementId>` |
| `Milestoning.generateMilestoningProperties()` | Synthetic properties in derived index pass |

The derived indexes are:
- Built in a single O(N) scan via `rebuild_derived_indexes()`
- `#[serde(skip)]` — never serialized
- Rebuilt after deserialization and after chunk merging
- Read-only after construction — safe for parallel access

### 3. No Generalization Node

The Java model has a first-class `Generalization` object with both `_general` (upward) and `_specific` (downward) pointers. We eliminate this indirection entirely.

A Class's `super_types: Vec<ElementId>` directly references the parent Class IDs. "What classes extend me?" is answered by `specialization_index`, not by querying `Generalization` objects.

**Why?** The `Generalization` node adds complexity (two-pointer object, separate collections) with zero semantic value beyond what `super_types` and the inverted index provide.

### 4. Hard vs Soft Dependencies

Cyclic data models are expected: `Person { company: Company }` ↔ `Company { employees: Person }`. To support this without breaking the topological sort:

| Dependency Type | Example | In Topological DAG? |
|---|---|---|
| **Hard** | `Class A extends B` | ✅ Sorted — A needs B fully hydrated |
| **Soft** | `prop: OtherClass[*]` | ❌ — only needs `ElementId` shell |

- `ClassDef.super_types` → **Hard** (cycles = compilation error)
- Property types, parameter types, return types → **Soft** (cycles are fine)
- Stereotype/tag profile references → **Soft**

This mirrors the Java compiler's `associationPrerequisiteElementsPass()` that explicitly declares Association→Class dependencies.

### 5. Element vs Type — Two Overlapping Concepts

`Element` answers: **"What named things does the compiler manage?"** (Java's `PackageableElement`). Every `Element` has a name, a package, an `ElementId`, and a lifecycle (declared → defined → validated).

`Type` answers: **"What can appear in a type position?"** (Java's `Type` hierarchy + `GenericType` wrapper). When you write `prop: X[1]`, `X` must be a Type.

These concepts **overlap but don't align**:

| Concept | Is an Element? | Is a Type? | Example |
|---|---|---|---|
| `Class` | ✅ | ✅ | `Person`, `List` |
| `Enumeration` | ✅ | ✅ | `Color`, `DayOfWeek` |
| `PrimitiveType` | ✅ (Chunk 0) | ✅ | `String`, `Integer`, `Boolean` |
| `Measure` | ✅ | ✅ | `Mass`, `Length` |
| `Unit` | ✅ (promoted) | ✅ | `Kilogram`, `Meter` |
| `Any` | ✅ (Chunk 0) | ✅ | Top of type lattice |
| `Nil` | ✅ (Chunk 0) | ✅ | Bottom of type lattice |
| `Function` | ✅ | ❌ | `doSomething()` — not a type |
| `Profile` | ✅ | ❌ | `doc` — not a type |
| `Association` | ✅ | ❌ | `PersonCompany` — not a type |
| `FunctionType` | ❌ (structural) | ✅ | `{String[1] -> Bool[1]}` |
| `RelationType` | ❌ (structural) | ✅ | `(a: Integer, b: String)` |

The `Element` enum represents packageable elements:

```rust
pub enum Element {
    Class(Class),
    Enumeration(Enumeration),
    Function(Function),
    Profile(Profile),
    Association(Association),
    Measure(Measure),
    PrimitiveType(PrimitiveType),  // String, Integer, Any, Nil, etc.
    Unit(Unit),                     // Kilogram, Meter — promoted from Measure
}
```

**Java's abstract `DataType`** (parent of `PrimitiveType` and `Enumeration`) is not needed — Rust enums replace class hierarchies. To check "is this a data type?", use `matches!(element, Element::PrimitiveType(_) | Element::Enumeration(_))`.

Types that are NOT elements (`FunctionType`, `RelationType`) live in `TypeExpr` (see §6).

Each `ModelChunk` has two parallel arenas: `Arena<ElementNode>` for common metadata and `Arena<Element>` for the typed payload. `ElementNode` holds the common fields (name, source, package) — the equivalent of Java's `PackageableElement` interface.

### 6. TypeExpr — The Rust Equivalent of Java's GenericType

In Java, you almost never reference a `Type` directly — you wrap it in a `GenericType` that carries type arguments (`<String, Integer>`), multiplicity arguments, and type parameter bindings.

Our `TypeExpr` plays the same role:

```rust
/// The Rust equivalent of Java's GenericType.
/// Every property type, parameter type, and return type is a TypeExpr.
pub enum TypeExpr {
    /// A resolved named type, optionally with type/value arguments.
    /// Covers: String, Person, List<String>, Map<K,V>, Varchar(255)
    Named {
        element: ElementId,
        type_arguments: Vec<TypeExpr>,
        value_arguments: Vec<ConstValue>,
    },
    /// Anonymous function signature: {String[1] -> Boolean[1]}
    FunctionType {
        parameters: Vec<(TypeExpr, Multiplicity)>,
        return_type: Box<TypeExpr>,
        return_multiplicity: Multiplicity,
    },
    /// Structural relation type (anonymous column bag)
    Relation(RelationId),
    /// Unresolved type variable: T, U
    Generic(SmolStr),
    /// Algebraic union: T + V (relation column merging)
    AlgebraUnion(Box<TypeExpr>, Box<TypeExpr>),
}
```

**Lowering from AST**: `ast::TypeReference` maps directly to `TypeExpr`. The path resolves to an `ElementId`, `type_arguments` lower recursively, and `type_variable_values` become `value_arguments`.

**Validation**: If `TypeExpr::Named { element }` points to a `Profile`, `Association`, or `Function` → compilation error ("not a type").

### 7. Extension via AnyMap

Third-party plugins (Relational, Services, DataSpaces) register their own typed arenas:

```rust
pub extension_arenas: HashMap<TypeId, Box<dyn Any>>,
```

The `Extension(TypeId)` variant in the extension arena routes lookups to the correct plugin data. This avoids modifying core types when adding a new element kind.

### 8. Global Packages + Chunked Elements

Packages span across files and modules. We use a **Global Package Tree** (single `Arena<Package>`) combined with **Local Element Chunks** (`Vec<ModelChunk>`).

**Why global packages?** A package `meta::pure` might contain elements from Chunk 0 (bootstrap), Chunk 1 (user code), and Chunk 2 (plugin code). Packages are the namespace — they must be unified.

**Why chunked elements?** Model merging is O(1): push a chunk, link its elements into existing packages, rebuild derived indexes. No element copying, no ID rewriting.

### 9. SmolStr for All Names

The AST uses `SmolStr` (aliased as `Identifier`) for all identifiers. The Pure layer follows suit. Most Pure identifiers are under 24 characters, making `SmolStr`'s inline representation a perfect fit — O(1) clone, no heap allocation, and zero-copy interop with the AST.

### 10. SourceInfo on All Pure Nodes

Pure nodes carry `SourceInfo` on every element, property, expression, and constraint — just like the AST. This is **non-negotiable** for two reasons:

1. **Compilation diagnostics**: Error messages must point users to the exact source location of the problem (e.g., "duplicate property 'name' at Person.pure:12:5").
2. **Runtime execution errors**: When executing Pure code, runtime failures (null dereference, constraint violation, etc.) must point back to the user's source. The `PureModel` is the live runtime model — the AST may not be retained.

```rust
pub struct ElementNode {
    pub name: SmolStr,
    pub source_info: SourceInfo,  // Always present
    pub parent_package: PackageId,
    pub kind: ElementKind,
}
```

**Trade-off**: `SourceInfo` is 20 bytes per node. For a model with 100K nodes, that's ~2MB — negligible compared to the rest of the model. The alternative (a side-table `HashMap<ElementId, SourceInfo>`) adds indirection and cache misses with zero meaningful savings.

### 11. Pure → AST Reconstruction (Future Work)

The Pure model is **not a lossless representation** of the AST. Certain syntactic details are lost:

| Information | Preserved? | Reconstruction Strategy |
|---|---|---|
| Source locations | ✅ On every node | Direct |
| Element names & types | ✅ Fully resolved | Direct — resolve `ElementId` to package path |
| Expression structure | ✅ If not desugared | Keep `Expression` isomorphic to `ast::Expression` |
| Source order | ✅ Via `SourceInfo` | Sort by line/column |
| Import statements | ❌ Resolved away | **Infer** — analyze referenced packages, generate optimal import list |
| Section boundaries | ❌ Flattened | **Infer** — group elements by `SourceInfo` file origin |

**Design constraints for forward compatibility:**
- **Don't desugar expressions.** Keep `Expression` structurally parallel to `ast::Expression` with names resolved to `ElementId`s. If `a + b` becomes `plus(a, b)`, idiomatic source emission is impossible.
- **One `ModelChunk` per source file.** This naturally preserves file origin, enabling section boundary reconstruction.

**Import inference** (future utility): Walk all `ElementId` references in a file's elements, collect their packages, subtract the element's own package, and emit the minimal import set. This isn't identical to user input but produces an optimal, deduplicated version.

### 12. Freeze-Then-Query Lifecycle

The `PureModel` has a clear two-phase lifecycle:

1. **Mutable phase**: `PureModelBuilder` populates arenas during compiler passes.
2. **Frozen phase**: After `rebuild_derived_indexes()`, the model becomes read-only. All query methods (`all_properties()`, `specializations()`, etc.) operate on the frozen model. Parallel validation is safe because the model is immutable.

## Query API Summary

| Question | Method | Data Source |
|---|---|---|
| "What are A's supertypes?" | `super_types(id)` | `class.super_types` (direct) |
| "What extends A?" | `specializations(id)` | Derived index |
| "A's declared properties?" | `declared_properties(id)` | `class.properties` (direct) |
| "A's association properties?" | `association_properties(id)` | Derived index |
| "ALL of A's properties?" | `all_properties(id)` | Composed: declared + inherited + association |
