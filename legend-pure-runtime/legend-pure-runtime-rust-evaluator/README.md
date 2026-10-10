# legend-pure-runtime-rust-evaluator

The Java side of the JNI boundary to the Rust Pure evaluator in
`legend-pure-rust/` (Step 2 of `legend-pure-rust/docs/rust-evaluator-mvp.md`).

## Surface

The only value currency across the boundary is `org.finos.legend.pure.m4.coreinstance.CoreInstance`,
in both directions.

| Class | Role |
|---|---|
| `PureRustEvaluator` | A `FunctionExecution` over the Rust evaluator: `init(runtime, message)` binds it to a `PureRuntime`; `start(function, arguments)` takes graph nodes (arguments wrapped as `InstanceValue`s, as the interpreted engine expects) and returns an `InstanceValue`; `create(classifierPath, names, values)` allocates a Rust object; `liveHandleCount()` reports Rust-side handles still alive; `close()` frees everything. |
| `RustNodeBridge` | The Java half of the node bridge, called from Rust: describes a node in one call (kind, classifier path, primitive payload, keys), reads property values on demand, and materialises Rust values through the `ModelRepository` factories and `_Package.getByUserPath`. |
| `RustCoreInstance` | A Rust heap object seen from Java as a read-only `CoreInstance`; its classifier is the Java graph node for the Rust classifier path. One proxy per Rust handle; `close()` releases it. |
| `PureRustEvaluationException` | Typed failure: `getKind()` is one of `EXECUTION_ERROR`, `ASSERTION_FAILED`, `CONSTRAINT_VIOLATION`, `FUNCTION_NOT_FOUND`, `UNSUPPORTED`; `getMessage()` starts with the kind's display name (`FunctionNotFound: …`). An `ASSERTION_FAILED` carries a `PureAssertFailException` as its cause. |

Crossing in: primitives (Boolean, Integer, Float, Decimal, String, every Date kind, StrictTime) become
Rust values; a packageable element whose path the Rust model knows becomes that element; enum values
of known enumerations become enum values; every other node becomes a lazily read Java-backed heap
object. Crossing out: primitives through the repository factories, elements and enum values by path,
Java-backed nodes as their original object, other Rust objects as `RustCoreInstance`. Functions, maps
and unit instances raise `UNSUPPORTED` until a test needs them.

## Build and test

```bash
cd legend-pure-rust && cargo build --workspace          # produces target/debug/libpure_rust_jni.{dylib,so,dll}
cd .. && JAVA_HOME=$(/usr/libexec/java_home -v 17) \
  mvn test -pl legend-pure-runtime/legend-pure-runtime-rust-evaluator -Dcheckstyle.skip=true
```

Surefire passes the library directory to the tests as the system property
`legend.pure.rust.libdir`; without it `PureRustEvaluator` falls back to
`System.loadLibrary("pure_rust_jni")`. When the library is absent the module's
tests are skipped (profile `rust-cdylib-missing-*`), so `mvn install` stays green on
hosts and CI without a Rust toolchain. Override with `-DskipTests=false` to force a run.

The bindings annotation processor module (`…-bindings-ap`) is out of the reactor:
its generator is the archived Rust `java-codegen` crate. It returns with typed proxies.
