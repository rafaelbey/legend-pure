# legend-pure-parser-ast

Core AST data model for the Pure grammar parser. **Zero serialization dependencies** — designed for direct consumption by both the emitter and a future Rust compiler.

## Key Types

- `Element` — Top-level enum: `Class`, `Enum`, `Function`, `Profile`, `Association`, `Measure`
- `Expression` — Value specifications: literals, variables, function calls, lambdas, operators
- `TypeReference` — Type with optional generics and type variable values
- `SourceInfo` — Line/column tracking for every AST node
- `Multiplicity` — `[1]`, `[*]`, `[0..1]`, `[1..*]`

## Traits

- `Spanned` — Access `source_info()` on any node
- `Packageable` — `package()` + `name()` for top-level elements
- `Annotated` — `stereotypes()` + `tagged_values()`
