# Legend CLI — Examples

This directory contains example Pure files and Protocol JSON to demonstrate the
`legend` CLI. Use these to explore what the toolchain can do today.

## Quick Setup

```bash
# Build the CLI (from workspace root)
cargo build -p legend-cli

# Or install it
cargo install --path crates/cli
```

## What's Inside

| File | What it shows |
|------|---------------|
| [`01_trading_model.pure`](01_trading_model.pure) | Classes, Enums, Associations, Constraints — a realistic trading domain |
| [`02_profiles.pure`](02_profiles.pure) | Profiles with stereotypes & tags, annotated classes |
| [`03_functions.pure`](03_functions.pure) | Functions with lambdas, arrow functions, let bindings, collection ops |
| [`04_inheritance.pure`](04_inheritance.pure) | Class inheritance (`extends`), complex constraints, portfolio model |
| [`05_measures.pure`](05_measures.pure) | Measure types with canonical/non-canonical units and conversions |
| [`06_protocol_sample.json`](06_protocol_sample.json) | Hand-crafted Protocol JSON (the format `legend parse` produces) |
| [`01_trading_model.output.json`](01_trading_model.output.json) | Generated output from parsing `01_trading_model.pure` |
| [`errors/`](errors/) | **7 intentionally broken files** for diagnostics demos (see below) |

---

## Demo Walkthrough

### 1. Check all valid examples for syntax errors

```bash
legend check examples/*.pure
```

Expected output:
```
Checking 5 .pure file(s)...
  ✓ examples/01_trading_model.pure (7 elements)
  ✓ examples/02_profiles.pure (4 elements)
  ✓ examples/03_functions.pure (5 elements)
  ✓ examples/04_inheritance.pure (6 elements)
  ✓ examples/05_measures.pure (3 elements)

Result: all 5 file(s) are valid ✓
```

### 2. Diagnostics — see how errors are reported

```bash
# Compact mode (default) — one-line-per-file summary
legend check examples/errors/
```

Expected output:
```
Checking 7 .pure file(s)...
  ✗ examples/errors/01_missing_brace.pure — Expected identifier, found end of file at ...:9:1
  ✗ examples/errors/02_missing_semicolons.pure — Expected ';', found identifier at ...:7:3
  ✗ examples/errors/03_bad_multiplicity.pure — Expected multiplicity, found identifier at ...:6:16
  ...

Result: 0 passed, 7 failed
```

Add `--show-source` for rich, rustc-style diagnostics with source snippets:

```bash
legend check --show-source examples/errors/
```

```
Checking 7 .pure file(s)...
  ✗ examples/errors/02_missing_semicolons.pure — Expected ';', found identifier at ...:7:3
      |
     6 |   name: String[1]
     7 |   age: Integer[1]
       |   ^^^ Expected ';', found identifier
     8 |   active: Boolean[1]
      |
  ✗ examples/errors/04_unknown_keyword.pure — Unexpected token Clazz at ...:4:1
      |
     3 | // ERROR: Completely unrecognized element keyword
     4 | Clazz model::errors::Typo
       | ^^^^^ Unexpected token Clazz
     5 | {
      |
  ...
```

Or mix valid + invalid files together:

```bash
legend check --show-source examples/
```

```
Checking 12 .pure file(s)...
  ✓ examples/01_trading_model.pure (7 elements)
  ✓ examples/02_profiles.pure (4 elements)
  ...
  ✗ examples/errors/02_missing_semicolons.pure — Expected ';', found identifier at ...:7:3
      |
     6 |   name: String[1]
     7 |   age: Integer[1]
       |   ^^^ Expected ';', found identifier
     8 |   active: Boolean[1]
      |
  ...

Result: 5 passed, 7 failed
```

### 3. Parse a single file to Protocol JSON

```bash
# Pretty-printed JSON on stdout
legend parse examples/01_trading_model.pure

# Compact JSON
legend parse examples/01_trading_model.pure --compact

# Write to a file
legend parse examples/01_trading_model.pure --output output.json
```

### 4. Parse an entire directory

```bash
# All .pure files merged into one PureModelContextData
legend parse --dir examples/
```

This recursively discovers all `.pure` files, parses each one, and merges
the elements into a single Protocol JSON output — exactly what the Java
Legend Engine expects.

### 5. Initialize a new project

```bash
legend init my-new-model
cd my-new-model
legend check src/main/pure/
legend parse src/main/pure/model.pure
```

This scaffolds a ready-to-go project with a starter class, enum, and function.

### 6. See what's coming

```bash
# These commands show what the CLI will do once the backends are ready:
legend compile examples/
legend test
legend package
legend publish --dry-run

# Each prints a friendly message about what's in development.
```

### 7. Explore help

```bash
legend --help              # All commands
legend parse --help        # Parse-specific options
legend version             # Version + capability matrix
```

### 8. Enable shell completions

```bash
# Zsh (macOS)
legend completions zsh > ~/.zfunc/_legend
source ~/.zshrc

# Bash
legend completions bash >> ~/.bashrc
source ~/.bashrc

# Fish
legend completions fish > ~/.config/fish/completions/legend.fish
```

Now `legend <TAB>` completes commands, `legend check --<TAB>` completes flags.

---

## Example: Trading Model

Here's `01_trading_model.pure` — a realistic domain model for a trading system:

```pure
Enum model::trading::AssetClass
{
  Equity,
  FixedIncome,
  FX,
  Commodity,
  Crypto
}

Class model::trading::Trade
[
  positiveQuantity: $this.quantity > 0,
  priceNonNegative: $this.price >= 0.0
]
{
  tradeId: String[1];
  quantity: Integer[1];
  price: Float[1];
  tradeDate: StrictDate[1];
  status: model::trading::OrderStatus[1];
}

Association model::trading::TradeInstrument
{
  instrument: model::trading::Instrument[1];
  trades: model::trading::Trade[*];
}
```

Running `legend parse examples/01_trading_model.pure` produces Protocol
JSON byte-compatible with the Java Legend Engine. See
[`01_trading_model.output.json`](01_trading_model.output.json) for the
full output.

---

## Tips for Demos

1. **Start with `legend version`** — shows the capability matrix (✓ ready / ◌ coming soon)
2. **Use `legend check`** on the valid examples — fast visual validation of 25 elements across 5 files
3. **Show compact diagnostics** — `legend check examples/errors/` shows one-line-per-file error summary
4. **Show rich diagnostics** — `legend check --show-source examples/errors/` shows source snippets with `^^^` carets
5. **Mix valid + invalid** — `legend check --show-source examples/` shows ✓ and ✗ with inline snippets
6. **Pipe JSON through `jq`** — `legend parse examples/01_trading_model.pure | jq '.elements | length'` → `7`
7. **Scaffold a project** — `legend init` shows the full project structure instantly
8. **Enable tab-completion** — `legend completions zsh > ~/.zfunc/_legend` for instant command/flag completion
9. **Show future commands** — `legend compile`, `legend test`, `legend package`, `legend publish` all have help text and friendly TODO messages
