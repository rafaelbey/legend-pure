# Legend Pure MCP server

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes a compiled Legend Pure workspace as a set of typed tools, so AI coding agents (Claude Code, Claude Desktop, custom runners) can interact with Pure code natively — searching symbols, surfacing diagnostics, running tests, and inspecting elements without grepping the filesystem.

Sibling to `legend-pure-lsp` (IDE protocol) and `legend-pure-dap` (debugger protocol). All three share the same compiled-workspace machinery (`crates/lsp/src/workspace.rs` + `crates/runtime/src/runner/`) so the IDE ▶ Run button and the MCP `run_function` tool produce byte-identical results.

## Install

```bash
# From the workspace root
cargo install --path crates/cli
```

This installs the `legend` binary; the MCP server is a subcommand.

## Run

```bash
# Auto-discover the classpath (walks up to find legend-pure-classpath.toml,
# falls back to the embedded platform).
legend mcp

# Or point at a specific classpath descriptor:
legend mcp --classpath ./legend-pure-classpath.toml
```

The server speaks JSON-RPC 2.0 over stdio — it's intended to be spawned by an agent runner, not run interactively.

## Wire into Claude Code

Add to your `~/.config/claude-code/mcp.json` (or wherever your MCP config lives):

```json
{
  "mcpServers": {
    "legend": {
      "command": "legend",
      "args": ["mcp", "--classpath", "/abs/path/to/legend-pure-classpath.toml"]
    }
  }
}
```

Then start a Claude Code session in any directory — the agent can now reach for the Legend tools without you typing anything special.

## Tools

The MVP exposes eleven tools across four categories.

### Read

| Tool | Inputs | Returns |
|---|---|---|
| `search_symbols` | `{ query: string, limit?: u32 }` | `[{ fqn, kind, source, line, column }]` — case-insensitive FQN substring match across the compiled workspace. Default limit 50, capped at 500. |
| `get_diagnostics` | `{ file?: string }` | `[{ source, line, column, severity, message }]` — compile errors. With `file` set, returns only that source's diagnostics. |

### Execute

| Tool | Inputs | Returns |
|---|---|---|
| `run_function` | `{ fqn: string }` | `RunResult` — calls the parameter-less Pure function and returns its rendered value, captured stdout, and (on failure) a parsed PureException + stack. |
| `run_test` | `{ fqn: string }` | `TestRunResult` — runs a `<<test::Test>>`-tagged function through the platform surveyor with the right `BeforePackage` / `AfterPackage` lifecycle. Returns pass/fail/error/skip counts plus a structured failure list. |
| `run_pct` | `{ test_fqn: string, adapter_fqn: string }` | `TestRunResult` — runs a `<<PCT.test>>`-tagged function against the chosen adapter. |
| `list_pct_adapters` | — | `[{ fqn, name }]` — every `<<PCT.adapter>>`-tagged function in the workspace. |

### Introspect

| Tool | Inputs | Returns |
|---|---|---|
| `read_element` | `{ fqn: string }` | `{ fqn, kind, source, line, column }` — resolves an FQN to its element kind and source location. |
| `list_packages` | `{ prefix?: string }` | `[fqn]` — every package FQN, sorted. With `prefix`, returns only packages whose FQN starts with it. |
| `list_tests` | `{ package_prefix?: string }` | `[{ fqn, source, line, tags }]` — every function tagged with `test::Test`, `test::AlloyOnly`, or `PCT.test`. |

### Lifecycle

| Tool | Inputs | Returns |
|---|---|---|
| `workspace_status` | — | `{ compiled_at, error_count, chunk_count, repo_count }` — `compiled_at` is the RFC 3339 timestamp of the current snapshot. Use this to decide whether to call `reload_workspace`. |
| `reload_workspace` | — | The new `workspace_status` after recompiling against the same classpath / auto-imports the server was started with. Call after editing `.pure` files so subsequent tool calls see the updated model. |

## Example agent prompts

Once wired into Claude Code, prompts like these route through the MCP tools:

- *"What tests exist under `meta::pure::functions::math`?"* → `list_tests { package_prefix: "meta::pure::functions::math" }`.
- *"Find every class with `Person` in its name."* → `search_symbols { query: "Person" }`.
- *"Where is `meta::pure::test::pct::PCT` defined?"* → `search_symbols` → `read_element`.
- *"Run test `meta::pure::tests::testPlus` and tell me if it passed."* → `run_test { fqn: "..." }`.
- *"Compile this workspace and list any errors."* → `get_diagnostics`.
- *"Which PCT adapters are available?"* → `list_pct_adapters`.

The agent gets typed JSON responses for each call — no scraping, no parsing of human-formatted output.

## Workspace lifetime

The workspace is compiled **once** at server startup. Subsequent edits to `.pure` files don't automatically refresh the in-memory model.

Two tools handle the refresh cycle:

- **`workspace_status`** — returns the snapshot's `compiled_at` timestamp plus error/chunk/repo counts. Agents can compare this against external `.pure` mtimes (or just call it after any write) to decide whether the in-memory model is stale.
- **`reload_workspace`** — recompiles using the same classpath the server was started with, swaps the in-memory snapshot, and returns the new status. Subsequent tool calls see the fresh model.

Under the hood the snapshot lives in `Arc<Mutex<Arc<WorkspaceSnapshot>>>`: read tools clone the inner `Arc` under the lock and drop it immediately so reads are lock-free, while `reload_workspace` swaps the slot under the same Mutex. The Mutex is never held across an `await`.

Typical agent workflow for an edit-then-test loop:

1. Edit `.pure` files (write tool or external editor).
2. `reload_workspace` — confirms the new `compiled_at` and any new errors.
3. `run_test { fqn: "..." }` — runs against the fresh model.

## Smoke test by hand

If something isn't working through Claude Code, you can drive the server directly:

```bash
cat > /tmp/mcp-test.jsonl <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_symbols","arguments":{"query":"Person","limit":3}}}
EOF
legend mcp < /tmp/mcp-test.jsonl
```

You should see three JSON-RPC responses on stdout: the server info, the tool list, and the search results.

### Convenience shell helper

Save this in your shell profile so you can drive any single tool call from one line:

```bash
mcp_call() {
  local name="$1"
  local args="$2"
  # Plain `[ -z … ]` rather than `${2:-{}}` — the latter's nested `{}` confuses
  # bash's parameter expansion parser and concatenates the default onto your input.
  [ -z "$args" ] && args='{}'
  (
    echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
    echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    echo "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$name\",\"arguments\":$args}}"
  ) | legend mcp 2>/dev/null | python3 -c '
import sys, json
# Read all stdout at once and split — line-buffered iteration over sys.stdin
# can miss the final response when the server closes the pipe mid-flush.
for line in sys.stdin.read().splitlines():
    if not line.strip(): continue
    r = json.loads(line)
    if r.get("id") == 2:
        # Tool errors (e.g. invalid FQN, missing element) come back as a
        # top-level `error` field — no `result` to unwrap.
        if "error" in r:
            print("TOOL ERROR:", json.dumps(r["error"], indent=2))
        else:
            text = r["result"]["content"][0]["text"]
            try: print(json.dumps(json.loads(text), indent=2))
            except Exception: print(text)
        break
'
}
```

Then:

```bash
mcp_call workspace_status
mcp_call search_symbols '{"query":"Person","limit":3}'
mcp_call list_packages   '{"prefix":"meta::pure::functions::math"}'
mcp_call list_pct_adapters
mcp_call read_element    '{"fqn":"meta::pure::functions::collection::tests::fold::FO_Person"}'
mcp_call reload_workspace
```

### Verifying the reload swap persists

Send `workspace_status → reload_workspace → workspace_status` with explicit delays so the recompile lands before the next read. The two `workspace_status` reads should bracket the reload's timestamp:

```bash
(
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
  echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  sleep 1
  echo '{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"workspace_status","arguments":{}}}'
  sleep 2
  echo '{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"reload_workspace","arguments":{}}}'
  sleep 5
  echo '{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"workspace_status","arguments":{}}}'
  sleep 1
) | legend mcp 2>/dev/null | python3 -c '
import sys, json
labels = {10: "before reload  ", 20: "reload returned", 30: "after reload   "}
results = {}
for line in sys.stdin.read().splitlines():
    if not line.strip(): continue
    r = json.loads(line)
    if r.get("id") in labels:
        text = r["result"]["content"][0]["text"]
        results[r["id"]] = json.loads(text)["compiled_at"]
for k in sorted(results):
    print("  id=" + str(k) + " " + labels[k] + " -> compiled_at=" + results[k])
ok = results.get(20) and results[20] == results.get(30) and results.get(10) != results.get(20)
print("\nVERDICT:", "PASS - swap persisted" if ok else "FAIL")
'
```

Expected output:

```
  id=10 before reload   -> compiled_at=2026-…A
  id=20 reload returned -> compiled_at=2026-…B
  id=30 after reload    -> compiled_at=2026-…B

VERDICT: PASS - swap persisted
```

Where `A ≠ B` (recompile produced a fresh timestamp) and `id=20 == id=30` (subsequent reads see the swapped snapshot).

## Architecture

```
agent (Claude Code) ──stdio──► legend mcp (this crate)
                                  │
                                  │  WorkspaceSnapshot (compiled
                                  │  once at startup) — Arc<PureModel>
                                  │  + per-source diagnostics
                                  ▼
                          rmcp ServerHandler
                                  │
        ┌─────────────────────────┼─────────────────────────┐
   tools/read.rs            tools/execute.rs          tools/introspect.rs
   - search_symbols         - run_function           - read_element
   - get_diagnostics        - run_test               - list_packages
                            - run_pct                - list_tests
                            - list_pct_adapters
        │                         │                         │
        └──────► PureModel ◄──────┴─────► runtime::runner ◄─┘
                                          (shared w/ LSP)
```

The four `Execute` tools route through `legend_pure_runtime::runner::*`, the same module that backs the LSP's `workspace/executeCommand` (i.e. the IntelliJ ▶ Run gutter button). LSP and MCP are peer consumers of the runtime — neither depends on the other.

## Roadmap

Deferred from the MVP and tracked for future iterations:

- **`find_definition` / `hover_info`** keyed by file + line + column. Agents rarely have precise positions today; `read_element` covers the common by-FQN case.
- **MCP `Resource` surface** (`pure://element/{fqn}`) — cleaner UX for "give me the source of this element", but the tool form is sufficient for MVP.
- **Mutating tools** — `format`, `apply_edit`, `rename_symbol`. Hooks into the LSP-side surface once `workspace/rename` lands.
- **Streaming `progress` notifications** for long PCT runs.
- **Debug-via-MCP** — agent-driven breakpoint + step, layered on the existing DAP server.

## References

- MCP spec: <https://modelcontextprotocol.io/specification>
- Rust SDK: <https://crates.io/crates/rmcp>
- Existing LSP impl: `crates/lsp/src/{server,handlers,workspace}.rs`
- Existing DAP impl: `crates/dap/src/{server,session,hooks}.rs`
- Shared runner: `crates/runtime/src/runner/`
