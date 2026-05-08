# legend-pure-runtime-rust-evaluator

A Java JNI bridge over the Rust Pure runtime, plus a typed-binding
toolchain that turns curated Pure functions and `Class`es into idiomatic
Java methods and interfaces.

## What you get

| Surface                         | Where                                           | Purpose |
| ------------------------------- | ----------------------------------------------- | ------- |
| `PureRustEvaluator`             | `org.finos.legend.pure.rust`                    | Loads the platform model in a Rust context, exposes `evaluate`, `getClassifier`, and `create` (materialise a fresh heap object). `Closeable` — wires `Cleaner` so native resources free even if you forget. |
| `PureRustInstance`              | `org.finos.legend.pure.rust`                    | Opaque handle to a heap object; supports `getProperty(name, args...)` and `getClassifier()`. |
| `PureRustResult`                | `org.finos.legend.pure.rust`                    | Internal marshalling shape. Not user-facing. |
| `PureRustException` / `…EvaluationException` | `org.finos.legend.pure.rust`       | Native errors propagate as these. |
| Generated facade `PureFunctions` (or your custom name) | configurable Java root package | One static method per Pure function listed in the manifest. Names are `<pure_pkg>_<mangled FQN>`, e.g. `meta_pure_functions_math_plus_Integer_MANY__Integer_1_(Iterable<Long> values, PureRustEvaluator eval)`. |
| Generated interfaces            | `<root>.<pure-package>.<ClassName>`             | One Java interface per reachable user `Class` (and `Enumeration`). Properties become methods — `[1]` abstract, `[0..1]` defaults to `Optional.empty()`, `[*]` defaults to `Collections.emptyList()`. Interfaces extend their Pure supertypes' generated interfaces, register their classifier on first init, and inherit two sentinel accessors (`$instancePointer()`, `$evaluator()`) from `PureRegistered`. |
| `PureProxyFactory`              | `org.finos.legend.pure.rust.proxy`              | `wrap`/`unwrap` between native handles and proxies, plus `create(userImpl, iface, eval)` to materialise a hand-written `implements` of a generated interface as a real heap object. |
| `PureInvocationHandler`         | `org.finos.legend.pure.rust.proxy`              | `InvocationHandler` backing every proxy. Routes property reads through `PureRustInstance.getProperty(...)` and re-wraps results using the declared `Method.getGenericReturnType()` so element types of `Iterable<X>` survive erasure. |
| `Any`                           | `org.finos.legend.pure.rust.proxy`              | Universal supertype every generated interface extends (transitively). Mirrors Pure's `meta::pure::metamodel::type::Any`. Adds `$rustInstance()` for falling back to dynamic dispatch via `PureRustInstance.getProperty(...)`. Used as the proxy fallback when the runtime classifier isn't registered to a more-specific generated interface — so `wrap(...)` always returns a typed proxy, never a bare `PureRustInstance`. |
| `PureRegistered`                | `org.finos.legend.pure.rust.proxy`              | Marker supertype of `Any`; declares the `$instancePointer()` / `$evaluator()` sentinels every proxy answers. |
| `PureLambda` (placeholder)      | `org.finos.legend.pure.rust.proxy`              | Anchors v2 function-typed-parameter support. |

## Quickstart

```java
PureRustEvaluator eval = new PureRustEvaluator();

// 1. Call a generated wrapper. Pure FQN + types are resolved at codegen,
//    so the call site is a regular Java method invocation.
Long sum = PureFunctions.meta_pure_functions_math_plus_Integer_MANY__Integer_1_(
    java.util.List.of(1L, 2L, 3L), eval);

// 2. Receive a Pure object via evaluate(...) and traverse it through the
//    typed proxy. The classifier registry picks the most-specific
//    generated interface so subtypes work via instanceof.
Person bob = eval.evaluate("user_test::makePerson_String_1__Person_1_", "Bob");
String first = bob.firstName();
for (Address a : bob.addresses()) { System.out.println(a.street()); }

// 2b. If the runtime classifier isn't registered for a specific
//     interface (or you only have an Object reference), you still get a
//     typed Any proxy — drop down to dynamic dispatch via $rustInstance().
Any opaque = eval.evaluate("some::function::returning::Any_Any_1_", x);
String classifier = opaque.$rustInstance().getClassifier();
Object firstName = opaque.$rustInstance().getProperty("firstName");

// 3. Build a fresh heap object from a hand-written implementation of a
//    generated interface. Only override the fields you care about — the
//    rest fall through to the [0..1]/[*] defaults.
Person alice = PureProxyFactory.create(new Person() {
    public String firstName() { return "Alice"; }
}, Person.class, eval);

eval.close();
```

## Generated bindings — what gets emitted

Bindings are produced by the `PureBindingsProcessor` annotation
processor in the sibling
[`legend-pure-runtime-rust-evaluator-bindings-ap`](../legend-pure-runtime-rust-evaluator-bindings-ap/)
module. The processor is wired into this module's `mvn compile` via
the standard `<annotationProcessorPaths>` mechanism:

```xml
<plugin>
  <artifactId>maven-compiler-plugin</artifactId>
  <configuration>
    <annotationProcessorPaths>
      <path>
        <groupId>${project.groupId}</groupId>
        <artifactId>legend-pure-runtime-rust-evaluator-bindings-ap</artifactId>
        <version>${project.version}</version>
      </path>
    </annotationProcessorPaths>
    <compilerArgs>
      <arg>-Apure.bindings.basedir=${project.basedir}</arg>
      <arg>-Apure.cdylib.path=${legend.cdylib.path}</arg>
    </compilerArgs>
  </configuration>
</plugin>
```

The marker class
[`org.finos.legend.pure.rust.bootstrap.M3Bootstrap`](src/main/java/org/finos/legend/pure/rust/bootstrap/M3Bootstrap.java)
carries the `@PureBindings` annotation that points at the manifest:

```java
@PureBindings(
    bindingsFile = "src/main/pure-bindings/m3-bindings.txt",
    javaPackage  = "org.finos.legend.pure.rust.generated"
)
public final class M3Bootstrap {}
```

At compile time the AP loads `libpure_rust_jni.{dylib,so,dll}` (path
resolved from `-Apure.cdylib.path`), calls into the Rust
`legend-pure-java-codegen` crate to walk the model and produce Java
sources, and emits each one through `Filer` (which puts them under
`target/generated-sources/annotations/` and feeds them into the same
javac round). No `cargo` subprocess fork, no extra `<build-helper>`
plugin to wire the generated source root.

The manifest at `src/main/pure-bindings/m3-bindings.txt` is one Pure
FQN per line; the AP auto-detects each entry's element kind (Function /
Class / Association) and dispatches accordingly. Blank lines and
`#`-prefixed comments are ignored. The default manifest covers the M3
metamodel surface (`Class`, `GenericType`, `Function`, `Property`,
`Multiplicity`, `Profile`, `Stereotype`, …) plus a few reflection
helpers (`type()`, `genericType()`, `elementToPath()`,
`pathToElement()`).

To add more Pure surface to the bindings, append FQNs to the manifest
and re-run `mvn compile`. To skip generation entirely (e.g. on a host
without a built cdylib), pass `-Dmaven.compiler.proc=none`.

### Why the cdylib at compile time?

The annotation processor reuses the **same Rust codegen library** the
`legend java-bindings` CLI calls into, by loading
`libpure_rust_jni.{dylib,so,dll}` and invoking a JNI shim. That keeps
the codegen logic single-sourced in the Rust crate (where its 16-test
suite covers the emission rules) instead of forcing a parallel Java
port.

For local development that means the cdylib must already be built
*before* `mvn compile`:

```bash
cargo build -p legend-pure-parser-jni
```

The pom defaults `legend.cdylib.path` to
`${project.basedir}/../../legend-pure-rust/target/debug/libpure_rust_jni<EXT>`
where `<EXT>` is filled in by an OS-detection profile (`.dylib` on
macOS, `.so` on Linux, `.dll` on Windows). Override with
`-Dlegend.cdylib.path=…` if you've built the cdylib elsewhere or
cross-compiled it for a different target arch.

> **Note.** The current build expects the developer host to ship its
> own cdylib. A future phase will bundle pre-built `.dylib` / `.so` /
> `.dll` for the common arch matrix into the AP JAR (under
> `META-INF/native/<os>-<arch>/`) and extract them automatically via a
> `NativeLibraryLoader` so downstream Java consumers no longer need a
> Rust toolchain.

### Type mapping summary

| Pure                         | Java                                  |
| ---------------------------- | ------------------------------------- |
| `Boolean`                    | `Boolean`                             |
| `Integer`                    | `Long`                                |
| `Float`                      | `Double`                              |
| `Decimal`                    | `java.math.BigDecimal`                |
| `Number`                     | `Number`                              |
| `String`                     | `String`                              |
| `StrictDate`, `LatestDate`   | `java.time.LocalDate`                 |
| `DateTime`                   | `java.time.OffsetDateTime` (UTC)      |
| `StrictTime`                 | `java.time.OffsetTime` (UTC)          |
| `Date` (abstract supertype)  | `java.time.temporal.Temporal`         |
| `Any`, `Nil`                 | `Object`                              |
| user `Class`                 | generated interface                   |
| user `Enumeration`           | generated `enum`                      |
| `Function<{…}>` / Relation   | rejected by codegen (deferred to v2)  |

### Multiplicity rules

| Pure         | Java                       | Default body for hand-written impls |
| ------------ | -------------------------- | ----------------------------------- |
| `[1]`        | `T`                        | abstract — user must supply         |
| `[0..1]`     | `java.util.Optional<T>`    | `Optional.empty()`                  |
| `[*]`, `[1..*]`, `[m..n]` | `Iterable<T>`  | `Collections.emptyList()`           |

Generated proxies always route through `PureInvocationHandler`, so the
default bodies are never invoked for proxies — they exist solely to make
hand-written `implements` of an interface easier to write.

## Materialising user implementations

`PureProxyFactory.create(userImpl, iface, eval)` walks every 0-arg
property method on `iface`, invokes it on `userImpl`, recursively
materialises any nested `PureRegistered` values that aren't already
proxies, and calls `nativeNew` on the JNI side to allocate a fresh
dynamic heap object with those properties. The returned proxy reads
back through the heap, so any subsequent `getProperty(...)` calls go
through the runtime — including from generated qualified properties.

Cycle / sharing detection is handled by an `IdentityHashMap` keyed by
user-object identity, so a self-referencing or shared graph
(`bob.spouse = alice; alice.spouse = bob`) materialises into a single
heap pair instead of looping.

Qualified properties (methods with parameters) are skipped — their
semantics is computed, not stored. Override them in the proxy chain by
supplying a different evaluator, or by re-materialising via a derived
interface that doesn't declare them.

## Build prerequisites

| Tool       | Required version | Notes                                              |
| ---------- | ---------------- | -------------------------------------------------- |
| JDK        | 11 or 17         | Repo enforces via toolchains.                      |
| Maven      | 3.6+             |                                                    |
| Cargo / rustc | edition 2024  | The Rust workspace at `legend-pure-rust/` builds the JNI shared library `libpure_rust_jni.{dylib,so,dll}` and the `legend` CLI. |

`mvn compile` from this module:

1. Triggers `PureBindingsProcessor` (annotation-processor-paths)
   inside javac. The processor `System.load`s the cdylib via
   `-Apure.cdylib.path`, calls into the Rust codegen, and emits each
   produced source through `Filer.createSourceFile`.
2. javac picks up the Filer-emitted sources from
   `target/generated-sources/annotations/` (auto-registered) and
   compiles them in the same round as the hand-written runtime
   classes under `--release 11`.

The shared library that `PureRustEvaluator` loads via
`System.loadLibrary("pure_rust_jni")` must be on `java.library.path`
when you actually *call* the evaluator at runtime. The AP also needs
the cdylib at *compile* time (see above). The same artifact services
both: build it once with `cargo build -p legend-pure-parser-jni` then
either install onto `~/Library/Java/Extensions/` (macOS) and let
`-Apure.cdylib.path` resolve to the workspace's `target/debug/`
default, or pass `-Djava.library.path=…/legend-pure-rust/target/debug/`
explicitly when you run the evaluator at runtime.

## Tests

JUnit 5 lives under `src/test/java/`. Run with the platform model on the
classpath via the parent Maven build, or directly:

```bash
mvn -pl legend-pure-runtime/legend-pure-runtime-rust-evaluator test \
    -Djava.library.path=…/legend-pure-rust/target/debug
```

The Rust-side codegen suite (`-p legend-pure-java-codegen`) carries a
`javac --release 11` round-trip that compiles the generated set
together with the runtime support classes; that test runs as part of
`cargo test`.

## See also

- [`legend-pure-runtime-rust-evaluator-bindings-ap/`](../legend-pure-runtime-rust-evaluator-bindings-ap/)
  — the annotation processor that drives codegen.
- `legend-pure-rust/crates/java-codegen/` — the codegen library
  (single source of truth, called by both the AP and the
  `legend java-bindings` developer CLI).
- `legend-pure-rust/crates/jni/` — the Rust side of this bridge,
  including the `nativeGenerateBindings` JNI export.
- `legend-pure-rust/BACKLOG.md` — open follow-ups (function-typed
  parameters, generic type-arg propagation, streaming `[*]` returns,
  full live-evaluator JUnit round-trip, multi-platform cdylib
  bundling).
