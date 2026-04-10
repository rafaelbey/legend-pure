# Legend Pure Parser — Compose Crate

AST → Pure grammar text composer (pretty-printer).

This crate takes an AST produced by the parser (or reconstructed from Protocol JSON)
and renders it back to syntactically valid, canonically formatted Pure grammar text.

## Usage

```rust
use legend_pure_parser_compose::compose_source_file;

// Parse → Compose roundtrip
let ast = legend_pure_parser_parser::parse(source, "file.pure").unwrap();
let text = compose_source_file(&ast);
assert_eq!(source, text);  // Roundtrip!
```

### From Protocol JSON (requires `protocol` feature)

```rust
use legend_pure_parser_compose::compose_from_json;

let grammar = compose_from_json(json_str).unwrap();
```

## Architecture

```
src/
├── lib.rs          # Public API + compose_from_protocol(), compose_from_json()
├── writer.rs       # IndentWriter — manages 2-space canonical indentation
├── identifier.rs   # Quoting logic: [a-zA-Z_][a-zA-Z0-9_]* → unquoted, else 'quoted'
├── type_ref.rs     # TypeReference, UnitReference, TypeSpec → grammar text
├── expression.rs   # All 19 expression variants + operator precedence + Group
├── element.rs      # All 6 element types (Class, Enum, Function, Profile, Association, Measure)
└── section.rs      # SourceFile → full grammar text (entry point)
```

## Type Rendering

The composer handles three type constructs via dedicated functions:

| Function | Input | Output |
|----------|-------|--------|
| `compose_type_reference` | `TypeReference` | `pkg::Name<Args>` |
| `compose_unit_reference` | `UnitReference` | `Measure~Unit` |
| `compose_type_spec` | `TypeSpec` | Dispatches to either |

Super types use `compose_type_reference` directly (always plain types).
Property types and return types use `compose_type_spec` (may be units).

## Body Composition

The `compose_body` function handles expression list rendering with a `terminate_last`
parameter:

| Context | `terminate_last` | Behavior |
|---------|-------------------|----------|
| Function body | `false` | Last expression has no `;` (implicit return) |
| Qualified property body | `true` | ALL expressions get `;` (statement terminators) |

## Design Decisions

### Source Parenthesis Preservation

`Expression::Group` preserves explicit parentheses from the original source. The
composer emits `(...)` for Group nodes, ensuring faithful roundtripping of expressions
like `(8 / 4) * 2` even when the parentheses are semantically redundant.

### Canonical Formatting (Current)

The composer produces **deterministic, canonically formatted** output with 2-space
indentation, matching the Java `PureGrammarComposer`. It does not attempt to preserve
original source formatting.

### String Escaping

The composer's `escape_pure_string` re-escapes strings for Pure single-quoted literals.
The parser's `unquote_string` handles the inverse, supporting all escape sequences:
`\'`, `\\`, `\n`, `\t`, `\r`.

### Operator Precedence

The composer implements a 6-level precedence model to emit minimal-but-correct
parentheses for expressions that are NOT wrapped in `Group`:

| Level | Operators |
|-------|-----------|
| 6 (highest) | `*`, `/` |
| 5 | `+`, `-` |
| 4 | `<`, `<=`, `>`, `>=` |
| 3 | `==`, `!=` |
| 2 | `&&` |
| 1 (lowest) | `\|\|` |

### Identifier Quoting

Matches Java's exact regex `[a-zA-Z_][a-zA-Z0-9_]*` for backward compatibility.
Identifiers that don't match are wrapped in single quotes: `'30_360'`, `'with spaces'`.

### Bitwise Operators (F#-style)

All bitwise operators use F#-style triple-char syntax to avoid ambiguity with
existing Pure syntax (`|` lambda pipe, `^` new instance, `<<`/`>>` stereotypes,
`~` columns/measures):

| Operator | Syntax |
|---|---|
| Bitwise AND | `&&&` |
| Bitwise OR | `\|\|\|` |
| Bitwise XOR | `^^^` |
| Shift Left | `<<<` |
| Shift Right | `>>>` |
| Bitwise NOT | `~~~` (reserved) |

## Known Gaps

### Section Headers

Multi-section files (`###Pure`, `###Mapping`, etc.) are not yet supported. Only
single-section Pure files are handled. See `section.rs` for the TODO.

### Graph Fetch Trees

Graph fetch trees (`#{Type{field1, field2{sub}}}#`) are represented as
`FunctionApplication` nodes in the AST. The composer will emit them as regular
function calls rather than the `#{...}#` syntax. This needs dedicated handling
once the AST has proper graph fetch types.

## Testing

```bash
# Run all tests (98 roundtrip + unit tests, 0 ignored)
cargo test -p legend-pure-parser-compose

# Run with clippy
cargo clippy -p legend-pure-parser-compose --tests -- -D warnings
```

Roundtrip tests verify both:
1. `compose(parse(source)) == source` — correctness
2. `compose(parse(compose(parse(source)))) == compose(parse(source))` — idempotency
