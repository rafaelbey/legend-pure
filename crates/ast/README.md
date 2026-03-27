# legend-pure-parser-ast

Core AST data model for the Pure grammar parser. **Zero serialization dependencies** — designed for direct consumption by both the protocol crate and a future Rust compiler.

## Key Types

- `Element` — Top-level enum: `Class`, `Enum`, `Function`, `Profile`, `Association`, `Measure`
- `Expression` — Value specifications: literals, variables, function calls, lambdas, operators
- `TypeReference` — Type with optional generics and type variable values
- `SourceInfo` — Line/column tracking for every AST node
- `Multiplicity` — `[1]`, `[*]`, `[0..1]`, `[1..*]`

## Trait Hierarchy

- **`Spanned`** — Access `source_info()` on any node
- **`Annotated: Spanned`** — `stereotypes()` + `tagged_values()`
- **`PackageableElement: Spanned + Annotated`** — `package()` + `name()` for top-level elements

## Derive Macros

Use one derive per struct — higher-level derives bring lower-level traits automatically:

```rust
#[derive(crate::PackageableElement)]  // → Spanned + Annotated + PackageableElement
pub struct ClassDef { ... }

#[derive(crate::Annotated)]           // → Spanned + Annotated
pub struct EnumValue { ... }

#[derive(crate::Spanned)]             // → Spanned only
pub struct Constraint { ... }
```
