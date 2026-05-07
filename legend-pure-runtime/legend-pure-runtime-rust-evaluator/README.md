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

Bindings are produced by the `legend java-bindings` CLI from the Rust
workspace. The Maven module wires it into `generate-sources`:

```xml
<plugin>
  <groupId>org.codehaus.mojo</groupId>
  <artifactId>exec-maven-plugin</artifactId>
  …
  <arguments>
    <argument>run</argument>
    <argument>--quiet</argument>
    <argument>-p</argument>
    <argument>legend-cli</argument>
    <argument>--</argument>
    <argument>java-bindings</argument>
    <argument>--output</argument>
    <argument>${project.build.directory}/generated-sources/java-bindings</argument>
    <argument>--java-package</argument>
    <argument>org.finos.legend.pure.rust.generated</argument>
    <argument>--bindings-file</argument>
    <argument>${project.basedir}/src/main/pure-bindings/m3-bindings.txt</argument>
  </arguments>
</plugin>
```

The manifest at `src/main/pure-bindings/m3-bindings.txt` is one Pure FQN
per line; the CLI auto-detects each entry's element kind (Function /
Class / Association) and dispatches accordingly. Blank lines and
`#`-prefixed comments are ignored. The default manifest covers the M3
metamodel surface (`Class`, `GenericType`, `Function`, `Property`,
`Multiplicity`, `Profile`, `Stereotype`, …) plus a few reflection
helpers (`type()`, `genericType()`, `elementToPath()`,
`pathToElement()`).

**Generation is idempotent.** The CLI reads the existing file at each
target path and only writes when the byte content differs, so reruns
on an unchanged manifest leave file mtimes intact. That keeps Maven's
incremental compiler and Develocity's remote caches warm — the
exec-maven-plugin step still runs (cargo's incremental check is fast),
but javac sees zero "modified" sources and skips recompile.

To add more Pure surface to the bindings, append FQNs to the manifest
and re-run `mvn compile`. To skip generation entirely (e.g. on a host
without a Rust toolchain), pass `-Dlegend.skipBindings=true`.

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

1. Invokes `cargo run -p legend-cli -- java-bindings …` to regenerate
   Java sources from the manifest.
2. Adds `target/generated-sources/java-bindings` to the compile path.
3. Runs `javac --release 11` over the hand-written runtime classes plus
   the generated set.

The shared library that `PureRustEvaluator` loads via
`System.loadLibrary("pure_rust_jni")` must be on `java.library.path`
when you actually *call* the evaluator at runtime — it's not needed at
compile time. Build it with `cargo build -p legend-pure-parser-jni` and
either install onto `~/Library/Java/Extensions/` (macOS) or pass
`-Djava.library.path=…/legend-pure-rust/target/debug/`.

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

- `legend-pure-rust/crates/java-codegen/` — the codegen library and
  CLI implementation.
- `legend-pure-rust/crates/jni/` — the Rust side of this bridge.
- `legend-pure-rust/BACKLOG.md` — open follow-ups (function-typed
  parameters, generic type-arg propagation, streaming `[*]` returns,
  full live-evaluator JUnit round-trip).
