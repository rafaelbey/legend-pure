# `mydsl-jni-extension` — worked downstream JNI cdylib

Standalone Cargo project sitting **outside** the parent
`legend-pure-rust/` workspace, just like
[`../mydsl-extension/`](../mydsl-extension/). Demonstrates the
cdylib-on-rlib pattern downstream consumers use to ship their own
JNI-loadable dynamic library containing the in-tree JNI surface
**plus** their custom extension crate's distributed-slice
registrations.

Produces `libmydsl_pure_jni.{dylib,so,dll}` — Java consumers load it
via `System.loadLibrary("mydsl_pure_jni")` instead of the stock
`pure_rust_jni`. Same `Java_org_finos_legend_pure_rust_…` symbol
table, with the downstream's extensions live.

## What it demonstrates

1. **JNI crate consumed as an rlib.** `legend-pure-parser-jni`'s
   `[lib] crate-type = ["rlib", "cdylib"]` lets downstream cdylibs
   add it as a dependency without rebuilding the entire JNI surface.
2. **Forwarder pattern for `#[no_mangle]` re-export.** Rustc's
   default cdylib build DCEs unreachable rlib code, so plain
   `pub use upstream::*;` strips all `Java_*` symbols from the final
   dylib. The fix is one `#[no_mangle] pub extern "system" fn`
   forwarder per upstream entry point in the downstream
   `src/lib.rs`. Each forwarder is a one-line delegating call;
   their `#[no_mangle]` carries the symbol through to the cdylib's
   export table.
3. **Force-linked extension crate.** The downstream extension
   crate's distributed-slice statics reach the cdylib's link graph
   via `use mydsl::… as _;` lines — same pattern as the stock
   `legend` binary and the `mydsl-legend` example.

## Verify the symbols ship

```bash
cd legend-pure-rust/examples/mydsl-jni-extension
cargo build --release

# 9 expected: 8 PureRustEvaluator entries (including the classpath-byte-array
# `nativeInitContextWithClasspath` for Java consumers loading the TOML from
# classpath resources) + 1 PureBindingsGenerator.
nm -gU target/release/libmydsl_pure_jni.dylib | grep ' _Java_' | wc -l

# Should match the stock cdylib for the same target:
nm -gU ../../target/release/libpure_rust_jni.dylib | grep ' _Java_' | wc -l
```

Linux: substitute `nm` with `nm -D` and the dylib extension with
`.so`. Windows: use `dumpbin /EXPORTS Mydsl_pure_jni.dll`.

## Layout

```
mydsl-jni-extension/
├── Cargo.toml      # empty [workspace] + cdylib + path deps to jni + mydsl + jni v0.21
├── src/
│   └── lib.rs      # 6 force-link `use`s + 7 #[no_mangle] forwarders
├── tests/
│   └── symbols.rs  # asserts the built .dylib exports all Java_* symbols
└── README.md
```

## Adapting for your DSL

1. **Rename the crate** in `Cargo.toml` (`mydsl-pure-jni-extension`
   → `yourcrate-pure-jni-extension`).
2. **Rename the cdylib's library name** so Java's
   `System.loadLibrary` call resolves it.
3. **Replace the extension force-link `use`s** with imports from
   your own extension crate.
4. **Leave the forwarders alone** unless the upstream JNI surface
   adds / changes a `Java_*` entry — when it does, `cargo build`
   fails with a signature mismatch and you update the forwarder.
5. **Distribute** the built `lib*.dylib` / `.so` / `.dll` in your
   release artifacts alongside your Java code.

## When the upstream changes its JNI surface

The forwarder signatures here mirror the upstream `Java_*` fn
signatures one-for-one. If upstream adds a parameter or changes a
return type, your downstream `cargo build` fails with a clear
`E0061` signature-mismatch error pointing at the affected
forwarder. Update the forwarder to match; cargo + rustc handle
binding the symbol correctly thereafter.

This means: changes to the upstream JNI surface are visible at
build-time, not at JNI-load-time. The recipe trades a bit of
copy-pasta on the downstream side for compile-time safety vs.
silent runtime failures.

## Windows note

Windows cdylibs cannot do prefix-based linker symbol exports, and
the forwarder `#[no_mangle]` pattern works without additional
flags on `x86_64-pc-windows-msvc`. No special build.rs needed —
the `pub extern "system" fn` forwarders Just Work.

## Why outside the parent workspace

Same reasoning as
[`../mydsl-extension/README.md`](../mydsl-extension/README.md): an
in-workspace cdylib could reach `pub(crate)` items through
workspace unification, masking visibility / linker bugs a real
external consumer would hit. Path-deps to the in-tree crates
mirror what a published-crate consumer would write as version
constraints.
