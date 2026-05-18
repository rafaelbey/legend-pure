# Measures and Units

Reference for the Measure / Unit metamodel as implemented in
`legend-pure-rust`. Cross-references Java Pure's M3 definition so
future runtime work (unit-aware arithmetic, conversion, classifier
checks) doesn't have to reverse-engineer the invariants from test
diffs.

## What is a Measure?

A **Measure** is a system of related units of the same physical /
abstract quantity — mass, time, currency, etc. It bundles:

- **At most one canonical unit** (the reference form, marked `*` in
  source). All other units are defined relative to this one.
- **Zero or more non-canonical units**, each carrying a conversion
  expression that converts a value in that unit back to the canonical
  unit.

A Measure itself is not a runtime value — it's a *type*. Runtime values
live in specific units (`5 Mass~Kilogram`), and the conversion lambdas
provide the bridge between units of the same Measure.

## Java M3 definition (source of truth)

From `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/grammar/m3.pure:783-867`:

```pure
Class Measure extends DataType, PackageableElement
{
    canonicalUnit     : Unit[0..1]
    nonCanonicalUnits : Unit[*]
}
```

Two `M3Properties` only:

| Property            | Multiplicity | Notes                                                        |
| ------------------- | ------------ | ------------------------------------------------------------ |
| `canonicalUnit`     | `Unit[0..1]` | The reference unit. Optional — a Measure with zero units is legal but rarely useful. |
| `nonCanonicalUnits` | `Unit[*]`    | Each has a conversion lambda back to `canonicalUnit`.        |

The `Unit` class itself extends `Measure` (the unit is-a measure in
the M3 lattice) and carries the conversion expression.

## Rust mapping

The mapping is one-to-one with Java's shape, preserving the
multiplicity exactly:

- **`crates/pure/src/nodes/measure.rs`**
  ```rust
  pub struct Measure {
      pub canonical_unit: Option<ElementId>,        // Unit[0..1]
      pub non_canonical_units: Vec<ElementId>,      // Unit[*]
  }
  ```

- **`crates/pure/src/nodes/unit.rs`**
  ```rust
  pub struct Unit {
      pub measure: ElementId,                       // back-pointer to parent Measure
      pub conversion_expression: Option<Expression>,// None for the canonical unit
  }
  ```

`Option<ElementId>` ↔ `Unit[0..1]`; `Vec<ElementId>` ↔ `Unit[*]`. Units
are *standalone elements* (each gets its own `ElementId`) so they can
appear in type positions — `prop: Mass~Kilogram[1]` references the
`Mass~Kilogram` unit element directly, not the parent Measure.

The Measure→Unit references are `ElementId`s rather than embedded
nodes; this means a Measure and its Units are independently resolvable
from the model's element table without an embedded-node walk. The
Unit→Measure back-pointer is also an `ElementId`, so the relationship
is bidirectional and constant-time in both directions.

## Conversion expression direction

`Unit::conversion_expression` is a lambda whose **input is a value in
this unit** and whose **output is the equivalent value in the
canonical unit**. The canonical unit's `conversion_expression` is
`None` — it IS the canonical form, no conversion needed.

Example (hypothetical `Mass` measure with `Kilogram` canonical and
`Pound` non-canonical):

```pure
Measure Mass {
    *Kilogram;                      // canonical, no conversion body
     Pound: lb -> $lb * 0.453592;   // given lb, produce kg
}
```

The lambda parameter (`lb`) names a value in the non-canonical unit;
the body evaluates to the equivalent canonical-unit value. Same
direction as Java Pure's runtime: when comparing two values in
different units of the same measure, both are converted *to* the
canonical unit and then compared.

## Grammar

From `docs/PURE_LANGUAGE_SPEC.md` §3.7:

```ebnf
MeasureDef       = 'Measure' Annotations? QualifiedName
                   '{' CanonicalUnit? NonCanonicalUnit* '}'
CanonicalUnit    = '*' Identifier (':' ConversionDef)?  ';'
NonCanonicalUnit = Identifier ':' ConversionDef ';'
ConversionDef    = Identifier '->' Expression
```

The leading `*` marks the canonical unit. The grammar allows the
canonical unit to omit its conversion (it IS the canonical form),
which matches the `Option<Expression>` field in the compiled `Unit`.

Non-canonical units MUST declare a conversion (`ConversionDef` is
required for them per the grammar) — even though `Unit::conversion_expression`
is `Option<Expression>` in the compiled model, the parser ensures
non-canonical units always supply one.

## Pass-1 allocation

`crates/pure/src/pipeline.rs:1184 allocate_unit_shells` runs during
Pass 1 (declaration). For each `MeasureDef`:

1. The Measure itself gets an `ElementId` and an `ElementNode` in its
   declaring chunk.
2. **Each canonical / non-canonical unit also gets its own
   `ElementId`**, allocated as a sibling element in the same chunk and
   registered in the same package as the parent Measure.
3. The Unit's FQN is `{measure_fqn}~{unit_name}` (e.g.
   `my::pkg::Mass~Kilogram`).
4. The Unit shell starts with `conversion_expression: None`; Pass 2
   hydrates the actual expression from the `UnitDef.conversion_body`.

The pipeline returns a `UnitMapping { canonical, non_canonical }`
keyed by the Measure's `ElementId`; this is used in Pass 2 to populate
the `Measure { canonical_unit, non_canonical_units }` fields with the
allocated unit IDs.

```rust
struct UnitMapping {
    canonical:     Option<ElementId>,    // mirrors Measure.canonicalUnit
    non_canonical: Vec<ElementId>,       // mirrors Measure.nonCanonicalUnits
}
```

## FQN convention: `Measure~Unit`

The `~` separator distinguishes unit names from regular package /
element navigation (`::`). This matches Java Pure's grammar and lets
type references like `prop: Mass~Kilogram[1]` parse unambiguously:

- `Mass` resolves as a Measure element (or any other identifier that
  happens to be in scope).
- `Mass~Kilogram` resolves as the Unit element nested inside that
  Measure.

Both Measure and Unit are `PackageableElement`s, so navigation from
the package root works for either: `Root.children[my].children[pkg].children[Mass]`
gets the Measure; `Root.children[my].children[pkg].children[Mass~Kilogram]`
gets the Unit.

## Java parity notes

| Concept                       | Java Pure                                        | Rust Pure                                         |
| ----------------------------- | ------------------------------------------------ | ------------------------------------------------- |
| Measure has canonical unit    | `Measure.canonicalUnit: Unit[0..1]`              | `Measure::canonical_unit: Option<ElementId>`      |
| Measure has non-canonical     | `Measure.nonCanonicalUnits: Unit[*]`             | `Measure::non_canonical_units: Vec<ElementId>`    |
| Unit has back-pointer         | `Unit.measure: Measure[1]` (via Generalization)  | `Unit::measure: ElementId`                        |
| Conversion direction          | this-unit → canonical                            | `Unit::conversion_expression: Option<Expression>` |
| Unit is a `PackageableElement` and `DataType` | yes — Unit extends Measure extends DataType | yes — Unit is its own element kind in the model lattice; package navigation works |
| FQN separator                 | `Measure~Unit`                                   | `Measure~Unit`                                    |
| Canonical conversion          | absent / NoOp                                    | `Option::None`                                    |

## Residual debt

> From BACKLOG: *"User-visible `package.children` now excludes Units
> (Java parity); residual debt is making units indexed only on the
> Measure internally."*

Current state: Units ARE registered in the parent package's element
list during Pass 1 (`model.register_element(package_id, unit_id)` at
pipeline.rs:1230). This means `package.children` includes them
unless filtered at the consumer. Java Pure's
`Package.children` reflection property hides Units (they appear only
under `Measure.canonicalUnit` / `Measure.nonCanonicalUnits`), so the
Rust path needs a filter at the navigation site to match — or, more
structurally, units should live exclusively under their parent Measure
in the model's children index. The latter is the future cleanup; the
former (filter at the reflection site) is the today shipping behavior.

## Where to look first

| File                                            | Responsibility                                          |
| ----------------------------------------------- | ------------------------------------------------------- |
| `crates/parser/src/parser/measure.rs`           | Grammar → `MeasureDef` / `UnitDef` AST                  |
| `crates/pure/src/nodes/measure.rs`              | Compiled `Measure { canonical_unit, non_canonical_units }` |
| `crates/pure/src/nodes/unit.rs`                 | Compiled `Unit { measure, conversion_expression }`      |
| `crates/pure/src/pipeline.rs:1184`              | `allocate_unit_shells` — Pass 1 Unit element allocation |
| `crates/pure/src/pipeline.rs:712`               | `UnitMapping` — per-Measure unit ID tracking            |
| `legend-pure-core/.../platform/pure/grammar/m3.pure:783` | Java M3 source of truth for Measure properties |
