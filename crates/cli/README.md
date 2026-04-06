# Legend CLI

The primary developer tool for the Legend ecosystem. Parse, compile, test, and publish Pure models from the command line.

## Install

```bash
# From the workspace root
cargo install --path crates/cli
```

## Quick Start

```bash
# Initialize a new Legend project
legend init my-model
cd my-model

# Parse a Pure file to Protocol JSON
legend parse src/main/pure/model.pure

# Check all Pure files for syntax errors
legend check src/main/pure/

# See all available commands
legend --help
```

## Commands

| Command | Status | Description |
|---------|--------|-------------|
| `legend parse` | ✅ | Parse Pure grammar into Protocol JSON |
| `legend check` | ✅ | Validate Pure files (parse-only) |
| `legend init`  | ✅ | Scaffold a new Legend project |
| `legend version` | ✅ | Print version information |
| `legend completions` | ✅ | Generate shell completion scripts |
| `legend emit` | 🚧 | Convert Protocol JSON back to Pure grammar |
| `legend compile` | 🚧 | Compile Pure files with semantic analysis |
| `legend test` | 🚧 | Run Pure tests on testable elements |
| `legend plan` | 🚧 | Generate execution plans |
| `legend package` | 🚧 | Parse, compile, and generate distributable artifacts |
| `legend publish` | 🚧 | Package and publish artifacts to the depot repository |

## Shell Completions

Enable tab-completion for commands, flags, and arguments:

```bash
# Zsh (macOS / Linux)
legend completions zsh > ~/.zfunc/_legend

# Bash
legend completions bash >> ~/.bashrc

# Fish
legend completions fish > ~/.config/fish/completions/legend.fish

# PowerShell
legend completions powershell >> $PROFILE
```

After sourcing the script, `legend <TAB>` completes commands and `legend check --<TAB>` completes flags.

## Diagnostics

Use `--show-source` on `check` and `parse` to see rich, source-level error output:

```bash
legend check --show-source src/main/pure/
```

```
  ✗ model.pure — Expected ';', found identifier at model.pure:7:3
      |
     6 |   name: String[1]
     7 |   age: Integer[1]
       |   ^^^ Expected ';', found identifier
     8 |   active: Boolean[1]
      |
```

## Architecture

The CLI is a thin orchestration layer — it contains zero parsing or compilation
logic. All heavy lifting is done by the existing crate stack:

```
legend (CLI)
  ├── legend-pure-parser-parser     → Parsing .pure files
  ├── legend-pure-parser-protocol   → AST ↔ Protocol JSON
  ├── legend-pure-parser-ast        → AST data model
  └── legend-pure-parser-pure       → Semantic layer (future)
```

## Verbosity

Control logging output with `-v` flags:

```bash
legend parse model.pure           # warn level (default)
legend -v parse model.pure        # info level
legend -vv parse model.pure       # debug level
legend -vvv parse model.pure      # trace level
```

Or use the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug legend parse model.pure
```

