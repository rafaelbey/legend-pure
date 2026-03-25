# legend-pure-parser-emitter

Converts AST → Protocol v1 JSON (matching the existing Java/ANTLR4 parser output). This is the **only** crate that depends on `serde` / `serde_json`.

## Key Functions

- `emit_protocol_json(result: &ParseResult) -> Result<String, EmitError>` — Full AST to JSON
