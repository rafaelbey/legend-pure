# `legend package` — Artifact Generation

> **Status: Deferred.** The artifact generators aren't ported yet. The
> stub command was removed from the CLI surface on 2026-05-16 to stop
> advertising functionality that doesn't exist. This doc captures the
> design context for when the prerequisites land.

## What the command will do

The full build pipeline as a single invocation: parse `.pure` files,
compile them through the semantic layer, and emit distributable
artifacts (serialized `PureModelContextData` bundles, execution plans,
generated Java code, etc.). Equivalent to `cargo build --release` for
Pure projects.

## Expected usage

```bash
legend package                         # Package the current project
legend package --output dist/          # Package to a specific directory
legend package --format json           # Output format (json, bincode)
```

## Argument shape (from the prior stub)

| Flag | Type | Purpose |
|---|---|---|
| `[paths]...` | `Vec<PathBuf>` default `.` | `.pure` files or dir to package |
| `-o`, `--output <DIR>` | `PathBuf` default `target/package` | Output directory |
| `--format <fmt>` | `String` default `json` | Artifact format (`json` or `bincode`) |

## Relationship to commands that DO ship today

`legend snapshot` already compiles repos into `.purem` binary
snapshots — that's effectively the bincode-leaning corner of
`package`. `legend java-bindings` already emits Java interface/proxy
files — that's another. `legend emit` writes pure-syntax round-trip
output. `legend package` would be the umbrella that ties these
together with a project-level manifest and ships them as a unit.

## Why it's blocked

1. **`PureModelContextData` JSON emitter** — the canonical Legend
   protocol bundle. The `protocol` crate has v1 read/write for individual
   elements, but the bundling-multiple-elements-into-one-document story
   isn't a single API yet.
2. **Project-level manifest** — `Cargo.toml`-style "what to include in
   this artifact" doesn't exist. Today repos are described by Java
   `*.definition.json` files for embedding; the *output* side needs a
   matching concept.
3. **No urgency** — every concrete sub-output (`.purem`, Java bindings,
   pure-emit) has its own command. The package-everything-into-one-dir
   convenience can wait until one user actually asks for it.

## When to revisit

- When a downstream consumer needs a single bundle (Maven artifact,
  Docker image build step, CI publish gate).
- When `PureModelContextData` write paths converge so the bundling
  step has a stable input shape.
- After `legend publish` is closer to ready, since `package` is the
  natural prerequisite.
