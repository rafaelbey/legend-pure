# `mydsl-extension` — worked example of a downstream Legend Pure extension

This directory is a self-contained Cargo project sitting **outside**
the main `legend-pure-rust/` workspace. It's the runnable template
that the [downstream-recipe doc](../../docs/extensions/downstream-recipe.md)
points at — copy this directory into your own repo, replace the stub
trait impls with your DSL's real logic, and ship a binary.

## What it demonstrates

All six self-registering extension surfaces, end-to-end, in ~200
lines of library code:

| Surface | Trait | Distributed slice |
|---|---|---|
| Native functions | `RuntimeExtension` | `RUNTIME_EXTENSIONS` |
| Compiler hooks | `CompilerExtension` | `COMPILER_EXTENSIONS` |
| Section grammar (`###MyDsl`) | `SectionParser` | `SECTION_PARSERS` |
| Island grammar (`#mytag{ … }#`) | `IslandParser` | `ISLAND_PARSERS` |
| Heap hydration | `DSLPopulator` | `DSL_POPULATORS` |
| IDE reference contribution | `IdeExtension` | `IDE_EXTENSIONS` |

Each contribution is registered via a `#[distributed_slice]` static
in `src/lib.rs`. The companion `mydsl-legend` binary at `src/main.rs`
is a wrapper that link-forces this crate (so the linker doesn't drop
the statics) and calls `legend_cli::main_entry()`.

## Layout

```
mydsl-extension/
├── Cargo.toml              # empty [workspace] + path deps + bin target
├── src/
│   ├── lib.rs              # the six trait impls + distributed_slice statics
│   └── main.rs             # force-link `use`s + legend_cli::main_entry()
├── tests/
│   └── smoke.rs            # asserts each contribution reaches its discovered slice
└── README.md
```

Everything is a unit struct with a stub body. The stubs compile and
self-register; replace each one with real logic incrementally.

## Why outside the parent workspace

The `[workspace]` table in `Cargo.toml` is intentionally empty — it
anchors this directory as the root of its own workspace so cargo
doesn't include it as a member of the parent `legend-pure-rust/`
workspace. Critical: an in-workspace example could reach `pub(crate)`
items through workspace unification, masking visibility bugs a real
external consumer would hit. Path-deps to the in-tree crates mirror
what a published-crate consumer would write as version constraints.

## Build & test

```bash
# From the legend-pure-rust/ workspace root (or anywhere with the
# example's Cargo.toml in the dir tree):
cd examples/mydsl-extension
cargo build                  # build lib + mydsl-legend binary
cargo test                   # 6 smoke tests asserting each surface is discovered
cargo run --bin mydsl-legend -- version   # exercise the wrapped CLI
```

## Adapting for your DSL

1. **Rename the crate** in `Cargo.toml`.
2. **Replace the stubs** in `src/lib.rs` one surface at a time:
   - `MyDslRuntimeExtension::register_natives` — your native FQN-keyed
     dispatch table.
   - `MyDslSectionParser::parse_body` — your `###Foo` body grammar.
     See `crates/dsl-relational/src/parser.rs` for the canonical
     in-tree pattern.
   - `MyDslIslandParser::parse` — your `#foo{ … }#` body grammar.
   - `MyDslCompilerExtension::{declare,define_signatures,define_bodies,validate}` —
     your DSL's compile hooks. The canonical patterns are
     `crates/dsl-mapping/src/compiler.rs` and
     `crates/dsl-relational/src/compiler.rs`.
   - `MyDslPopulator::populate` — heap hydration of your
     `Element::DSLInstance` payloads. See
     `crates/dsl-relational-runtime/src/lib.rs`.
   - `MyDslIdeExtension::walk_references` — reference-site emission
     for goto-def / find-usages. See
     `crates/dsl-mapping/src/compiler.rs` (`MappingIdeExtension`).
3. **Update the binary's force-link `use`s** in `src/main.rs` if you
   rename the extension types or add more crates.
4. **Distribute**: publish your crate, then `cargo install --bin mydsl-legend`
   ships your custom CLI to end users.

## Link forcing (critical)

`linkme` distributed slices live in dedicated linker sections. The
linker is allowed to drop crate object files entirely when nothing
in the final binary references them — which would silently empty
your distributed-slice contributions.

Every consumer binary that wants discovered behaviour MUST `use`
something from each extension crate it depends on. `src/main.rs`
shows the pattern: one `use … as _;` line per crate, with
`#[allow(unused_imports)]` to silence the dead-import warning. See
the recipe doc's [Link forcing](../../docs/extensions/downstream-recipe.md#link-forcing-important)
section for the full explanation.
