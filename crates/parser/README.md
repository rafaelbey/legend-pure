# legend-pure-parser-parser

Recursive descent parser for the Pure grammar. Consumes tokens, produces AST. Supports plugin dispatch for island and section grammars.

## Key Types

- `Parser` — Main parser struct
- `PluginRegistry` — Holds island, section, and sub-parser plugins
- `ParseResult` — Success result containing `Vec<Element>`
- `ParseError` — Error with message and source span
