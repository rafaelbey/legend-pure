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
| [`01_trading_model.pure`](01_trading_model.pure) | Classes, Enums, Associations, Profiles, Constraints — a realistic trading domain |
| [`02_profiles.pure`](02_profiles.pure) | Profiles with stereotypes & tags, annotated classes |
| [`03_functions.pure`](03_functions.pure) | Functions with lambdas, let bindings, collection ops |
| [`04_inheritance.pure`](04_inheritance.pure) | Class inheritance (`extends`), complex constraints, portfolio model |
| [`05_measures.pure`](05_measures.pure) | Measure types with canonical/non-canonical units and conversions |
| [`06_protocol_sample.json`](06_protocol_sample.json) | Hand-crafted Protocol JSON (the format `legend parse` produces) |
| [`01_trading_model.output.json`](01_trading_model.output.json) | Generated output from parsing `01_trading_model.pure` |
| [`errors/`](errors/) | **8 intentionally broken files** for diagnostics demos (see below) |

---

## Demo Walkthrough

### 1. Compile all valid examples

```bash
legend compile examples/*.pure
```

Expected output:
```
Compiling 5 .pure file(s)...
  ✓ examples/01_trading_model.pure (8 elements)
  ✓ examples/02_profiles.pure (4 elements)
  ✓ examples/03_functions.pure (5 elements)
  ✓ examples/04_inheritance.pure (6 elements)
  ✓ examples/05_measures.pure (3 elements)

Running semantic analysis...

Success: compiled 26 element(s) from 5 file(s) in 0.00s
         10 class(es), 3 enum(s), 5 function(s), 2 association(s), 3 profile(s), 3 measure(s)
```

This parses all files, resolves types across files, checks for
cyclic inheritance, and builds the full `PureModel`.

### 2. Compilation error diagnostics

```bash
legend compile --show-source examples/errors/08_compilation_errors.pure
```

This file has **4 categories** of semantic error:

```
Compiling 1 .pure file(s)...
  ✓ examples/errors/08_compilation_errors.pure (6 elements)

Running semantic analysis...

  ✗ Duplicate element: 'model::errors::Duplicate' at .../08_compilation_errors.pure:19:1
     --> .../08_compilation_errors.pure:19:1
       |
     18 | }
     19 | Class model::errors::Duplicate
        | ^^^^^ Duplicate element: 'model::errors::Duplicate'
     20 | {
       |
  ✗ Cyclic inheritance detected involving 'CycleB' at .../08_compilation_errors.pure:29:1
  ✗ Cannot resolve type 'model::nonexistent::Address' at .../08_compilation_errors.pure:11:12
  ✗ Cannot resolve element 'nonexistent::profileName' at .../08_compilation_errors.pure:35:9

error: 5 compilation error(s)
```

All error locations are clickable in IDE terminals (VS Code, IntelliJ, iTerm2).

### 3. Check all valid examples for syntax errors

```bash
legend check examples/*.pure
```

Expected output:
```
Checking 5 .pure file(s)...
  ✓ examples/01_trading_model.pure (8 elements)
  ✓ examples/02_profiles.pure (4 elements)
  ✓ examples/03_functions.pure (5 elements)
  ✓ examples/04_inheritance.pure (6 elements)
  ✓ examples/05_measures.pure (3 elements)

Result: all 5 file(s) are valid ✓
```

### 4. Parse error diagnostics

```bash
# Compact mode (default) — one-line-per-file summary
legend check examples/errors/

# Rich mode — source snippets with carets
legend check --show-source examples/errors/
```

```
  ✗ examples/errors/02_missing_semicolons.pure — Expected ';', found identifier at ...:7:3
     --> /full/path/to/02_missing_semicolons.pure:7:3
       |
      6 |   name: String[1]
      7 |   age: Integer[1]
        |   ^^^ Expected ';', found identifier
      8 |   active: Boolean[1]
       |
```

### 5. Parse a single file to Protocol JSON

```bash
# Pretty-printed JSON on stdout
legend parse examples/01_trading_model.pure

# Compact JSON
legend parse examples/01_trading_model.pure --compact

# Write to a file
legend parse examples/01_trading_model.pure --output output.json
```

### 6. Parse an entire directory

```bash
# All .pure files merged into one PureModelContextData
legend parse --dir examples/
```

This recursively discovers all `.pure` files, parses each one, and merges
the elements into a single Protocol JSON output — exactly what the Java
Legend Engine expects.

### 7. Initialize a new project

```bash
legend init my-new-model
cd my-new-model
legend check src/main/pure/
legend compile src/main/pure/
legend parse src/main/pure/model.pure
```

This scaffolds a ready-to-go project with a starter class, enum, and function.

### 8. See version and capabilities

```bash
legend version
```

```
legend 0.1.0 (rust/legend-cli aarch64)

  ✓ Pure grammar parser
  ✓ Protocol JSON ↔ Grammar conversion
  ✓ Compile Pure models
  ◌ Package artifacts    (coming soon)
  ◌ Publish to depot     (coming soon)
  ◌ Run Pure tests       (coming soon)
  ◌ SDLC integration     (coming soon)
```

### 9. Explore help

```bash
legend --help              # All commands
legend compile --help      # Compile-specific options
legend parse --help        # Parse-specific options
legend version             # Version + capability matrix
```

### 10. Enable shell completions

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

Now `legend <TAB>` completes commands, `legend compile --<TAB>` completes flags.

---

## Example: Trading Model

Here's `01_trading_model.pure` — a realistic domain model for a trading system:

```pure
Profile model::trading::doc
{
  stereotypes: [doc, internal];
  tags: [description, author];
}

Enum model::trading::AssetClass
{
  Equity,
  FixedIncome,
  FX,
  Commodity,
  Crypto
}

Class <<model::trading::doc.doc>> model::trading::Trade
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

Running `legend compile examples/01_trading_model.pure` verifies the
full semantic model. Running `legend parse examples/01_trading_model.pure`
produces Protocol JSON byte-compatible with the Java Legend Engine.
See [`01_trading_model.output.json`](01_trading_model.output.json) for
the full parse output.

---

## Error Examples

The `errors/` directory contains intentionally broken files organized
by error type:

| File | Error Type |
|------|-----------|
| `01_missing_brace.pure` | `parse` — Missing closing brace |
| `02_missing_semicolons.pure` | `parse` — Missing semicolons between properties |
| `03_bad_multiplicity.pure` | `parse` — Invalid multiplicity syntax |
| `04_unknown_keyword.pure` | `parse` — Unrecognized element keyword |
| `05_missing_return_type.pure` | `parse` — Function without return type |
| `06_enum_errors.pure` | `parse` — Malformed enum syntax |
| `07_mixed_valid_invalid.pure` | `parse` — Mix of valid and broken elements |
| **`08_compilation_errors.pure`** | **`compile`** — **Semantic errors** (unresolved types, duplicates, cycles, bad annotations) |

Use `legend check --show-source` for parse errors and
`legend compile --show-source` for compilation errors.

---

## Tips for Demos

1. **Start with `legend version`** — shows the capability matrix (✓ ready / ◌ coming soon)
2. **Compile all examples** — `legend compile examples/*.pure` produces a clean 26-element model in <1ms
3. **Show compilation diagnostics** — `legend compile --show-source examples/errors/08_compilation_errors.pure` shows all 4 error categories
4. **Show parse diagnostics** — `legend check --show-source examples/errors/` shows source snippets with `^^^` carets
5. **Mix valid + invalid** — `legend check --show-source examples/` shows ✓ and ✗ together
6. **Pipe JSON through `jq`** — `legend parse examples/01_trading_model.pure | jq '.elements | length'` → `8`
7. **Multi-file compilation** — `legend compile examples/01_trading_model.pure examples/03_functions.pure` resolves cross-file references
8. **Scaffold a project** — `legend init` shows the full project structure instantly
9. **Enable tab-completion** — `legend completions zsh > ~/.zfunc/_legend` for instant command/flag completion
