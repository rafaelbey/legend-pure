# Error Examples

These files contain **intentional syntax errors** to demonstrate the CLI's
diagnostic output. Each file targets a specific error class.

## Running

```bash
# Compact mode (default) — one-line-per-file summary
legend check examples/errors/

# Rich mode — source snippets with line numbers and carets
legend check --show-source examples/errors/

# Check a single error file
legend check --show-source examples/errors/01_missing_brace.pure
```

## Error Catalog

| File | Error | Diagnostic |
|------|-------|------------|
| `01_missing_brace.pure` | Unclosed class body | `Expected identifier, found end of file at ...:9:1` |
| `02_missing_semicolons.pure` | Missing `;` after properties | `Expected ';', found identifier at ...:7:3` |
| `03_bad_multiplicity.pure` | Invalid multiplicity `[abc]` | `Expected multiplicity, found identifier at ...:6:16` |
| `04_unknown_keyword.pure` | `Clazz` instead of `Class` | `Unexpected token Clazz at ...:4:1` |
| `05_missing_return_type.pure` | Function missing `: ReturnType` | `Expected ':', found '{' at ...:5:1` |
| `06_enum_errors.pure` | Enum values with `= 1` syntax | `Expected identifier, found '=' at ...:6:7` |
| `07_mixed_valid_invalid.pure` | Good class + broken class | `Expected identifier, found '[' at ...:12:9` |

## What to notice

1. **Clickable file links** — errors use absolute `path:line:col` format, auto-linked in IDE terminals (VS Code, IntelliJ, iTerm2)
2. **Location precision** — every error pinpoints the exact line and column
3. **Expected vs found** — messages say what was expected and what was actually encountered
4. **Source snippets** — `--show-source` renders the error line with context, `^^^` carets, and a clickable `-->` header
5. **Per-file reporting** — when checking a directory, valid files show ✓ and broken files show ✗
6. **Summary** — final line reports total passed vs failed
7. **Exit code** — exits 0 on success, 1 if any file fails (CI-friendly)

## Mix valid + error files

The most compelling demo is checking the entire `examples/` directory with snippets:

```bash
legend check --show-source examples/
```

This produces a mixed report:
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
