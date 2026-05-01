# `legend-pure-build` — Setting Up a New Pure Repo

This crate is a **build-only** utility (`[build-dependencies]`) that
embeds a set of Pure repositories into a Rust crate at compile time.
The generated code plugs into `legend_pure_core_platform::repo::Repo`,
so consumers of your crate get the same `Repo`-based API the platform
itself uses.

This document walks through standing up a brand-new Pure repo —
"Acme Forecasting", say — in a fresh Rust crate.

---

## Pre-flight

You need three things up-front:

1. A **repo name** — globally unique within Legend, and a valid Rust
   identifier when upper-snake-cased. Typical convention:
   `<company>_<area>` (e.g. `acme_forecasting`).
2. A **package pattern** — a Java regex over `::`-qualified Pure
   package paths the repo is allowed to define. Anything outside the
   pattern is a build-time error. Example:
   `(meta::acme::forecasting)(::.*)?`.
3. A **dependency list** — names of other repos this one builds on
   top of. Most repos depend on `"platform"` at minimum.

---

## Step 1 — Lay out the source tree

The build utility infers your source root from the descriptor JSON's
`name` field. The convention is:

```
my-acme-forecasting-pure/
├── Cargo.toml
├── build.rs
├── src/
│   └── lib.rs
└── resources/
    ├── acme_forecasting.definition.json     ← descriptor
    └── acme_forecasting/                    ← source root
        ├── functions/
        │   └── forecast.pure
        ├── types/
        │   └── Forecast.pure
        └── tests/
            └── forecastTests.pure
```

Two rules drive this layout:

- **Source root** is `<dir_of_descriptor>/<name>/`. So if your
  descriptor is `resources/acme_forecasting.definition.json` and its
  `"name"` field is `"acme_forecasting"`, sources live in
  `resources/acme_forecasting/`.
- **Canonical URL** for each `.pure` file is `/<name>/<rel_path>`.
  E.g. `resources/acme_forecasting/functions/forecast.pure` becomes
  `/acme_forecasting/functions/forecast.pure` in `SourceInformation`.

The same convention is reused by Java's `ClassLoaderCodeStorage`, so
the layout is interchangeable across stacks.

## Step 2 — Write the descriptor JSON

`resources/acme_forecasting.definition.json`:

```json
{
  "name": "acme_forecasting",
  "pattern": "(meta::acme::forecasting)(::.*)?",
  "dependencies": ["platform"]
}
```

Schema (matches Java's repo descriptor verbatim):

| Field          | Type       | Meaning                                                            |
|----------------|------------|--------------------------------------------------------------------|
| `name`         | `string`   | Repo identifier. Becomes `/{name}/...` URL prefix and `REPO_<NAME>_*` const suffix. |
| `pattern`      | `string`   | Regex over package paths the repo is allowed to define. Stored on `RepoMeta` for future per-repo validation. |
| `dependencies` | `string[]` | Repo names this one depends on. Stored on `RepoMeta` for future load-order resolution. |

`pattern` and `dependencies` are read but not yet enforced — Stage 1
of the Rust port preserves them on the `RepoMeta` so future stages
can hook in package-pattern validation and topological repo ordering.

## Step 3 — Wire `Cargo.toml`

```toml
[package]
name    = "my-acme-forecasting-pure"
version = "0.1.0"
edition = "2024"

# One entry per repo this crate ships. `descriptor` is relative to
# this Cargo.toml. `shape` is "embedded" today; "filesystem" and
# "purem" are reserved for future variants.
[[package.metadata.legend-pure.repos]]
descriptor = "resources/acme_forecasting.definition.json"
shape      = "embedded"

[dependencies]
# Used at runtime to consume the generated Repo / RepoMeta types.
legend-pure-core-platform = { path = "../legend-pure-rust/crates/core-platform-pure" }

[build-dependencies]
# Used at build time to embed the .pure sources.
legend-pure-build = { path = "../legend-pure-rust/crates/build" }
```

The `descriptor` path is resolved relative to `CARGO_MANIFEST_DIR`,
so anything reachable from the crate root works (sibling
directories, `../../shared/foo.json`, etc.).

## Step 4 — Add `build.rs`

```rust
// build.rs
fn main() {
    if let Err(e) = legend_pure_build::run() {
        panic!("legend-pure-build: {e}");
    }
}
```

`legend_pure_build::run()` is shorthand for
`Embedder::new().run()` — fine for the common case. Drop down to the
builder API when you need to override defaults:

```rust
fn main() {
    legend_pure_build::Embedder::new()
        // Match the convention used by the rest of your crate.
        .output("custom_output.rs")
        // Skip a synthetic file you don't want embedded.
        .skip_canonical("/acme_forecasting/internal/_bootstrap.pure")
        // Don't emit the default aggregator — you'll hand-write one.
        .emit_default_aggregator(false)
        .run()
        .expect("legend-pure-build failed");
}
```

## Step 5 — Include the generated file

Pick a module to host the generated constants and aggregator:

```rust
// src/lib.rs
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/legend_pure_repos.rs"));
}

pub use embedded::default_embedded_repos as my_repos;
```

The generated file emits, per repo declared in your `Cargo.toml`:

- `pub const REPO_<NAME>_FILES: &[::legend_pure_core_platform::sources::PureSourceFile]`
  — mixed `.pure` + `.json` files.
- `pub const REPO_<NAME>_META: ::legend_pure_core_platform::repo::RepoMeta`
  — `name`/`pattern`/`dependencies` from the descriptor JSON.

Plus (when `emit_default_aggregator` is true, default):

- `pub fn default_embedded_repos() -> Vec<::legend_pure_core_platform::repo::Repo>`
  — every repo, in `Cargo.toml` declaration order.

Where `<NAME>` is the descriptor's `"name"` field upper-snake-cased
(e.g. `acme_forecasting` → `ACME_FORECASTING`).

## Step 6 — Compile + use

`my_repos()` returns a `Vec<Repo>`. Plug it into the standard load
pipeline alongside the platform:

```rust
use legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS;
use legend_pure_core_platform::repo::{self, Repo};
use smol_str::SmolStr;

fn load() -> legend_pure_parser_pure::model::PureModel {
    // Combine our repo with the embedded platform repos.
    let mut repos: Vec<Repo> = Repo::default_embedded();
    repos.extend(my_acme_forecasting_pure::my_repos());

    let auto_imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();

    repo::load(&repos, &auto_imports).unwrap_or_else(|p| p.model)
}
```

`repo::load` parses every repo's `.pure` files and runs the full
compiler over the combined set. The resulting `PureModel` resolves
references across repo boundaries — exactly the same behaviour as
`load_platform()` for the platform repos alone.

---

## Mixed shapes (filesystem reload)

The `Repo` enum supports per-repo shape selection. Build a hybrid
configuration when you want hot-reload on the *user's* repo while
keeping the platform embedded:

```rust
use legend_pure_core_platform::repo::Repo;
use std::path::Path;

let mut repos: Vec<Repo> = Repo::default_embedded();
repos.push(Repo::from_descriptor(Path::new(
    "/path/to/acme_forecasting.definition.json",
))?);
```

`Repo::from_descriptor` reads the descriptor JSON, walks
`<dir>/<name>/`, and stamps `/<name>/...` onto every file — same
inference rules as the build-time embedder.

For ad-hoc fixtures with no descriptor, `Repo::from_filesystem(root,
prefix)` is the escape hatch. `meta()` returns `None` in that case.

## What the build script tracks

`legend-pure-build` emits these `cargo:rerun-if-changed=` directives:

- `Cargo.toml` (so editing the repo list triggers a rebuild)
- Each descriptor JSON
- Each repo's source-root directory
- Every embedded `.pure` and `.json` file by absolute path

Touching any of them forces the next `cargo build` to re-run the
embedder.

## Common pitfalls

- **`SourceRootMissing`**: the descriptor's `"name"` field doesn't
  match the on-disk directory. Fix: rename one of them so they
  agree.
- **`include_str!` errors at compile time**: usually a stale
  `OUT_DIR` after moving sources. `cargo clean -p <your-crate>` and
  retry.
- **Non-portable embedded paths**: the build crate canonicalizes
  every `.pure` path into the generated `include_str!` call, so the
  resulting `.rlib` is **not portable** between machines. This is
  inherited from `core-platform-pure` and tracked as a follow-up.
  For now, downstream users either rebuild on their machine or use
  `Repo::from_descriptor` at runtime.
- **`grammar/m3.pure`-shaped files**: the bootstrap skip is keyed by
  the canonical URL `/platform/pure/grammar/m3.pure`. If you somehow
  ship a file with that exact URL (you won't — your repo isn't named
  `platform`) the embedder silently drops it. Add your own paths via
  `Embedder::skip_canonical` if you need similar
  "embed elsewhere" carve-outs.

## `.purem` snapshots (Stage 2)

`shape = "purem"` and an `Embedder::emit_purem(true)` flag are
**not yet implemented** — they're recognised by the parser today
and silently produce no output. The format design lives at
`legend-pure-rust/crates/core-platform-pure/docs/PUREM_FORMAT.md`.
Once the format is settled, the build utility will be the
producer side: declare a repo as `shape = "embedded"` to embed
sources today, switch it to `"purem"` later to ship a pre-compiled
snapshot instead. The runtime `Repo` enum already reserves the
variant slot.

## API reference

| Item                                  | Purpose                                                                |
|---------------------------------------|------------------------------------------------------------------------|
| `legend_pure_build::run()`            | Default-config one-shot. Use this from `build.rs` unless you need to override.            |
| `Embedder::new()`                     | Builder constructor with sane defaults.                                |
| `.cargo_toml(path)`                   | Override the source `Cargo.toml`. Default: `$CARGO_MANIFEST_DIR/Cargo.toml`. |
| `.output(path)`                       | Override the generated file path. Default: `$OUT_DIR/legend_pure_repos.rs`. |
| `.crate_path(p)`                      | Module-path prefix used in emitted code. Default: `"::legend_pure_core_platform"`. Use `"crate"` only when invoked from `legend-pure-core-platform` itself. |
| `.emit_default_aggregator(bool)`      | Toggle `default_embedded_repos()` emission. Default `true`.            |
| `.skip_canonical(url)`                | Add a canonical URL to the always-skip list. Default skip list is `[M3_BOOTSTRAP_CANONICAL]`. |
| `.run()`                              | Generate the output file. Returns `Result<(), BuildError>`.            |
| `M3_BOOTSTRAP_CANONICAL`              | The bootstrap URL `/platform/pure/grammar/m3.pure` — always skipped because `m3.pure` is parsed by `m3_parser`, not the regular pipeline. |
| `DEFAULT_OUTPUT_NAME`                 | The default generated-file name `legend_pure_repos.rs`.                |
| `BuildError`                          | Error type — see the variants for what each failure mode looks like.   |
