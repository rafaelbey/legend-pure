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
├── type_ref.rs     # TypeReference → "Path<Args>(VarVals)"
├── expression.rs   # All 17 expression variants + operator precedence
├── element.rs      # All 6 element types (Class, Enum, Function, Profile, Association, Measure)
└── section.rs      # SourceFile → full grammar text (entry point)
```

## Design Decisions

### Canonical Formatting (Current)

The composer currently produces **deterministic, canonically formatted** output with
2-space indentation, matching the Java `PureGrammarComposer`. It does not attempt to
preserve original source formatting.

### Source-Info-Aware Formatting (Planned)

The AST carries `SourceInfo` on every node, which contains the original line/column
offsets from parsing. The composer should use this to reproduce the user's original
formatting (indentation, spacing, line breaks) when source info is available.

When source info is **absent** — as happens with protocol→AST conversion, where nodes
are constructed programmatically — the composer falls back to canonical 2-space
indentation.

This enables two modes:
- **Parse → compose**: Faithful reproduction of the user's original `.pure` files
- **Protocol → compose**: Clean canonical formatting for machine-generated AST

Key design questions still open:
- At what granularity do we use source info? (element-level, property-level, expression-level)
- Does `IndentWriter` need a mode switch, or do we need a separate formatting strategy?
- How do we handle partially-present source info (e.g., element has source info but its child expressions don't)?

### Operator Precedence

The composer implements a 6-level precedence model to emit minimal-but-correct
parentheses:

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

### Untested Areas

The following are implemented but lack roundtrip test coverage:

- Complex constraints (`~externalId`, `~enforcementLevel`, `~message`)
- Function tests (data bindings, assertions)
- Aggregation kinds (`(shared)`, `(composite)`) on properties
- Date/time literals (`%2024-01-01`, `%2024-01-01T12:00:00`)
- Decimal literals (`1.5D`)
- Type variable values (`VARCHAR(200)`)
- Arithmetic precedence edge cases (`1 - (2 - 3)`)

## Testing

```bash
# Run all tests (57 total: 5 unit + 52 roundtrip)
cargo test -p legend-pure-parser-compose

# Run with clippy
cargo clippy -p legend-pure-parser-compose --tests -- -D warnings
```

Roundtrip tests verify both:
1. `compose(parse(source)) == source` — correctness
2. `compose(parse(compose(parse(source)))) == compose(parse(source))` — idempotency
