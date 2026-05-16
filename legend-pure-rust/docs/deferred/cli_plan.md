# `legend plan` — Execution Plan Generation

> **Status: Deferred.** Real dependencies (Pure semantic layer, connection
> management, plan generator) aren't ready yet. The stub command was
> removed from the CLI surface on 2026-05-16 — leaving a
> `NotImplemented`-returning command in the help output was misleading
> users about what works today. This doc captures the design context so
> the work can be picked up when the prerequisites land.

## What the command will do

Compile a Pure function or query, generate an execution plan, and
output it as JSON. Execution plans describe how the Legend Engine will
execute a query against one or more data stores — the routing,
push-down, and post-processing graph that bridges the abstract Pure
expression and concrete connection APIs.

This is the Rust port of `legend execute --plan` from the Java engine.

## Expected usage

```bash
legend plan "my::package::myFunction()"
legend plan --element "my::package::myFunction" --runtime my-runtime.json
legend plan --format text "my::package::myFunction"   # human-readable variant
```

## Argument shape (from the prior stub)

| Flag | Type | Purpose |
|---|---|---|
| `<element>` | positional `String` | Fully-qualified function/query to plan |
| `--runtime <FILE>` | `Option<PathBuf>` | Runtime configuration with connection details |
| `--format <fmt>` | `String` default `json` | `json` or `text` |

## Why it's blocked

1. **Execution plan model** — the Java engine's `ExecutionPlan` and its
   node hierarchy (`FunctionParametersValidationNode`, `SequenceExecutionNode`,
   `RelationalRootQueryTempTableExecutionNode`, …) aren't ported. The
   plan-generator runs over the compiled `PureModel` plus a `Runtime`
   binding stores to connections; both sides need definitions in the
   Rust workspace first.
2. **Connection management** — runtime JSON parsing, credential
   handling, store-specific drivers. The `legend-pure-rust` workspace
   has zero connection code today.
3. **Plan-generation pass** — the Java pipeline does this in
   `legend-engine`, not in `legend-pure`. Porting it is a substantial
   downstream initiative; the CLI surface needs to land *after* the
   underlying engine, not before.

## When to revisit

- After `legend run` grows beyond parameterless functions and starts
  handling real query input (it currently routes through the
  interpreter only, no plan).
- After connection / runtime configuration lands in the workspace
  (currently nothing — `--classpath` only handles Pure repos, not data
  stores).
- After at least one execution-plan node type is ported (probably
  starting with `FunctionParametersValidationNode` + a leaf relational
  node) so there's a concrete plan shape to emit.
