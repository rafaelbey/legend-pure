# legend-pure-parser-pure

Semantic Layer for the Pure compiler. Consumes `ast::SourceFile` and produces a fully resolved `PureModel` — an Arena/Index-based graph equivalent to Java's `PureModel`.

## Key Types

- `PureModel` — The frozen semantic graph: packages, elements, derived indexes, query API
- `ModelChunk` — A unit of compilation with typed arenas (classes, enums, functions, etc.)
- `ElementId` — Segmented `(chunk_id, local_idx)` index for zero-rewrite merging
- `ElementNode` — Universal graph node (name, package, kind discriminant)
- `TypeExpr` — Resolved type expressions: `Named`, `Relation`, `Generic`, `Parameterized`

## Element Nodes

| Node | Mirrors |
|------|---------|
| `Class` | `ast::ClassDef` — properties, qualified properties, constraints, super_types |
| `Enumeration` | `ast::EnumDef` — values with annotations |
| `Function` | `ast::FunctionDef` — parameters, return type, body |
| `Profile` | `ast::ProfileDef` — stereotype and tag names |
| `Association` | `ast::AssociationDef` — two properties linking classes |
| `Measure` | `ast::MeasureDef` — canonical + non-canonical units |

## Compiler Pipeline

1. **Declaration Pass** — Assign `ElementId`s, allocate shells
2. **Topological Sort** — Hard dependencies only (supertypes)
3. **Definition Pass** — Hydrate shells in topological order
4. **Freeze & Index** — `rebuild_derived_indexes()` computes inverted indexes
5. **Validation Pass** — Read-only, parallelizable via `rayon`

## Design

See [DESIGN.md](DESIGN.md) for detailed architectural decisions including:
- Arena/Index pattern (no `&` references)
- Unidirectional data + derived indexes (eliminating 5 Java bidirectional pointer patterns)
- Hard vs Soft dependency classification for cyclic data models
- Global packages + chunked elements for O(1) merging
