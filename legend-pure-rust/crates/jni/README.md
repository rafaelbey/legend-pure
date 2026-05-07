# legend-pure-parser-jni

JNI bridge exposing the Rust Pure evaluator to Java. The only crate in
the workspace that uses `unsafe` (required for the FFI surface);
forbid-unsafe lints are explicitly waived here.

Builds a `cdylib` named `pure_rust_jni`, which the Java side loads via
`System.loadLibrary("pure_rust_jni")` (see
`legend-pure-runtime/legend-pure-runtime-rust-evaluator/`).

## Native entry points

Six FFI symbols, all under
`Java_org_finos_legend_pure_rust_PureRustEvaluator_*`:

| Symbol                | Returns           | Purpose                                                                 |
| --------------------- | ----------------- | ----------------------------------------------------------------------- |
| `nativeInitContext`   | `jlong`           | Loads the platform model (embedded `.purem` + filesystem repos), wraps it in a `JniContext`, returns the heap-allocated pointer Java holds opaque. |
| `nativeFreeContext`   | `void`            | Drops the boxed `JniContext` and every strong handle it owned.          |
| `nativeEvaluate`      | `PureRustResult`  | Resolves a Pure FQN (mangled or unmangled) to an `ElementId`, applies the callable to the `args[]`, returns the marshalled result. |
| `nativeGetProperty`   | `PureRustResult`  | Reads a property off a heap object by name (e.g. `firstName`); arg list supports qualified properties. |
| `nativeGetClassifier` | `String`          | Returns the runtime classifier FQN of a heap object, e.g. `"meta::pure::metamodel::type::Class"`. |
| `nativeNew`           | `jlong`           | Allocates a fresh dynamic heap object: takes a classifier FQN plus parallel `propertyNames[]` / `propertyValues[]` arrays, calls `RuntimeHeap::alloc_dynamic` + `mutate_set`, registers the handle, returns the encoded `i64` pointer. Used by `PureProxyFactory.create(userImpl, iface, eval)`. |
| `nativeFreeInstance`  | `void`            | Releases the strong handle for a single instance pointer. Wired to the Java `Cleaner` so handles drop when their `PureRustInstance` is GC'd. |

## Internals

| File             | Responsibility                                                                        |
| ---------------- | ------------------------------------------------------------------------------------- |
| `lib.rs`         | The seven `Java_*` exports, `catch_unwind` panic-→-`PureRustException` translation, JNI argument marshalling. |
| `context.rs`     | `JniContext` (model + native registry + evaluator) and `JniHandleTable` — a `slotmap`-backed registry mapping the `i64` Java holds back to the strong `Rc<RefCell<HeapEntry>>` clones that keep heap objects alive. |
| `conversion.rs`  | `PureRustResult` ↔ `Value` marshalling. Handles primitives (`Boolean`, `Integer`, `Float`, `String`), `INSTANCE_POINTER` (registers / lookups via `JniHandleTable`), and `ARRAY` (recursive). |

## Lifetime / leak model

`Rc<RefCell<HeapEntry>>` has no integer form and no stable address that
would survive cloning, so the table mediates the boundary: every
Pure→Java emission registers the handle and returns the integer; every
Java→Pure callback resolves the integer back to the retained `Rc`
clone, keeping the underlying object alive until `nativeFreeInstance`
drops the strong reference. Java's `Cleaner` drives the free.

## Building

```bash
cargo build -p legend-pure-parser-jni                 # debug cdylib
cargo build -p legend-pure-parser-jni --release       # release cdylib
```

The Java side expects the resulting library on `java.library.path`:

- macOS: `target/{debug,release}/libpure_rust_jni.dylib`
- Linux: `target/{debug,release}/libpure_rust_jni.so`
- Windows: `target/{debug,release}/pure_rust_jni.dll`

## See also

- `legend-pure-rust/crates/java-codegen/` — the typed Java wrapper
  generator that targets this JNI surface.
- `legend-pure-runtime/legend-pure-runtime-rust-evaluator/README.md` —
  user-facing Java bridge module that consumes this cdylib.
