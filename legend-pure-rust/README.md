# Legend Pure Parser

A Rust-based Pure grammar parser replacing the existing Java/ANTLR4 parser. The architecture separates **parsing (→ AST)** from **output generation (→ Protocol JSON)** via JNI, with an extensible plugin system for island and section grammars.

## Architecture

```
┌──────────────────────────────────────────────┐
│  Layer 7: CLI / JNI (developer tools + FFI)  │
├──────────────────────────────────────────────┤
│  Layer 6: Runtime (interpreter + heap)        │  ← Value, RuntimeHeap, Executor
├──────────────────────────────────────────────┤
│  Layer 5: Pure (Semantic Layer)                │  ← AST → Resolved Graph
├──────────────────────────────────────────────┤
│  Layer 3: Protocol                             │  ← AST ↔ Protocol JSON
├──────────────────────────────────────────────┤
│  Layer 2: Parser + Compose                     │  ← Tokens → AST → Text
├──────────────────────────────────────────────┤
│  Layer 1: Lexer (tokenizer)                    │  ← Source → Tokens
├──────────────────────────────────────────────┤
│  Layer 0: AST (data model)                     │  ← Shared types
└──────────────────────────────────────────────┘
```

## Quick Start

```bash
# Build
cargo build --workspace

# Test
cargo test --workspace

# Lint (strict: no unwrap/expect in library code)
cargo lint-lib

# Lint (standard clippy, all targets)
cargo lint
cargo fmt --check

# Copyright check (all .rs and .toml files must have headers)
./scripts/check-copyright.sh

# Code coverage (requires cargo-llvm-cov)
cargo llvm-cov --workspace --html --output-dir coverage/
cargo llvm-cov --workspace --fail-under-lines 90
```

## Legend CLI

The `legend` CLI is the primary developer tool — install and use it directly:

```bash
# Install
cargo install --path crates/cli

# Parse a Pure file to Protocol JSON
legend parse model.pure

# Check all Pure files for syntax errors
legend check src/main/pure/

# Initialize a new project
legend init my-model

# See all commands
legend --help
```

## Crate Map

| Crate | Purpose | Key Types | Dependencies |
|-------|---------|-----------|--------------|
| `legend-pure-parser-ast` | AST data model | `Element`, `Expression`, `TypeReference`, `SourceInfo` | `smol_str` |
| `legend-pure-parser-lexer` | Tokenizer | `Token`, `TokenKind`, `Lexer` | ast, `smol_str`, `tracing` |
| `legend-pure-parser-parser` | Recursive descent parser | `Parser`, `PluginRegistry`, `ParseResult` | ast, lexer, `tracing` |
| `legend-pure-parser-protocol` | AST ↔ Protocol v1 JSON | `convert_source_file()`, protocol model | ast, parser, `serde`, `serde_json` |
| `legend-pure-parser-compose` | AST → Pure grammar text | `compose()`, roundtrip support | ast, protocol, parser |
| `legend-pure-parser-pure` | Semantic Layer | `PureModel`, `ElementId`, `Class`, `TypeExpr` | ast, parser, `serde`, `bincode` |
| `legend-pure-runtime` | Interpreter + heap | `Value`, `RuntimeHeap`, `VariableContext` | pure, ast, `im-rc`, `slotmap`, `rust_decimal`, `jiff` |
| `legend-pure-parser-jni` | JNI bridge to Java | `Java_*` FFI functions | ast, lexer, parser, protocol, `jni` |
| `legend-cli` | Developer CLI | `legend` binary | ast, lexer, parser, protocol, compose, pure, `clap` |

## Development Guide

See [ARCHITECTURE.md](ARCHITECTURE.md) for design details and [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.

### Adding a New Element Type

1. Add the type to `crates/ast/src/element.rs`
2. Add a variant to the `Element` enum
3. Derive traits: `#[derive(crate::PackageableElement)]` (brings `Spanned` + `Annotated` + `PackageableElement`)
4. Add manual `Spanned`, `Annotated`, `PackageableElement` dispatch to the `Element` enum impls
5. Add parsing in `crates/parser/`
6. Add protocol conversion in `crates/protocol/`
7. Add tests

### Adding a New Plugin

See the plugin walkthrough in [ARCHITECTURE.md](ARCHITECTURE.md).

## Testing

Tests follow TDD — test cases are derived from the existing Java grammar tests (`TestGrammarParser`, `TestGrammarRoundtrip`). See [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for the full test catalog.

## Tracing

The parser uses the `tracing` crate for structured diagnostics. Enable verbose output:

```bash
RUST_LOG=legend_pure_parser=debug cargo test
```
