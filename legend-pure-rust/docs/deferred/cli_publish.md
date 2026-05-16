# `legend publish` — Depot Publication

> **Status: Deferred.** Depot client + packaging are both not ready.
> The stub command was removed from the CLI surface on 2026-05-16 to
> stop advertising functionality that doesn't exist. This doc captures
> the design context for when the prerequisites land.

## What the command will do

Run `legend package` first to produce the artifact bundle, then push
the result to a depot repository (the Legend equivalent of a Maven
repository — shared registry for Pure models across projects and
teams). Equivalent of `cargo publish`.

## Expected usage

```bash
legend publish                         # Package + publish to default depot
legend publish --depot https://depot.example.com
legend publish --dry-run               # Build artifacts without publishing
```

## Argument shape (from the prior stub)

| Flag | Type | Purpose |
|---|---|---|
| `[paths]...` | `Vec<PathBuf>` default `.` | Input `.pure` files / dirs |
| `--depot <URL>` | `Option<String>` | Override the default depot URL |
| `--dry-run` | `bool` | Package only; skip the upload |
| `--format <fmt>` | `String` default `json` | Artifact format |

## Why it's blocked

1. **`legend package` doesn't exist yet** — `publish` is `package` +
   upload. Without the bundle generator there's nothing to push.
   See [`cli_package.md`](cli_package.md).
2. **No depot client** — the Java engine uses Goldman's `depot-client`
   library to authenticate against and publish to depot servers. The
   Rust workspace has no HTTP/auth/depot code; this is a substantial
   port on its own.
3. **Credential handling** — depot pushes need API tokens or service
   accounts. The CLI has no secret-management story today (no
   `legend login`, no keychain integration, no env-var convention
   for credentials). Building that surface area carefully is its own
   project.

## When to revisit

- After `legend package` ships and has at least one round of real-use
  feedback so its output shape is stable enough to upload.
- After a depot client lands — likely as a separate crate
  (`legend-pure-depot-client`?) that this command would wrap.
- After CLI credential / auth conventions are set (probably as part of
  any other command that needs server-side auth, not driven by
  `publish` in isolation).
