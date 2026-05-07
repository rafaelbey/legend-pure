# legend-pure-java-codegen

Library crate that turns selected Pure functions and `Class`es into
typed Java sources targeting the JNI evaluator runtime support in
`legend-pure-runtime/legend-pure-runtime-rust-evaluator/`.

The crate is library-only — it never touches the filesystem or the
JNI. The `legend java-bindings` CLI command in
[`crates/cli/src/commands/java_bindings.rs`](../cli/src/commands/java_bindings.rs)
is the I/O driver.

## What it emits

| Output                     | Per…                          | Shape                                                                |
| -------------------------- | ----------------------------- | -------------------------------------------------------------------- |
| `<root>/PureFunctions.java` (configurable name) | one per generation | `public static <Ret> <pkg>_<mangled>(<args>, PureRustEvaluator eval)` — one method per requested function. |
| `<root>/<pure-pkg>/<Class>.java` | one per reachable user `Class` | `public interface <Class> extends <Supertypes…>` with one method per property. `[1]` abstract; `[0..1]` defaults to `Optional.empty()`; `[*]` defaults to `Collections.emptyList()`. Self-registers with `PureProxyFactory` on first init. |
| `<root>/<pure-pkg>/<Enum>.java`  | one per reachable Enumeration | `public enum <Enum>` with `fromPure(String)` lookup.                |

`eval` is always the last static-method parameter. Every generated
interface (transitively) extends
`org.finos.legend.pure.rust.proxy.Any` — the hand-written universal
supertype that mirrors Pure's `meta::pure::metamodel::type::Any`. The
codegen recognises that FQN and substitutes references to it with the
hand-written interface; it is never emitted as Java source. Generated
interfaces inherit `$instancePointer()` / `$evaluator()` (from
`PureRegistered`) plus `$rustInstance()` (from `Any`) so callers can
always drop down to dynamic property access.

## Public API

```rust
pub fn generate(
    model: &PureModel,
    fns: &[FqnInput],            // function FQNs → static facade methods
    extra_classes: &[FqnInput],  // explicit class seeds for the closure walk
    extra_associations: &[FqnInput], // both endpoints seeded
    opts: &Options,
) -> Result<Vec<JavaFile>, CodegenError>;
```

`FqnInput` is the **mangled** form (e.g.
`meta::pure::functions::math::plus_Integer_MANY__Integer_1_`) — the
exact string `PureRustEvaluator.evaluate` consumes. `JavaFile` carries
`(relative_path, contents)`; the CLI writes them under the user-chosen
output root.

`CodegenError` is `thiserror`-based and carries the offending FQN on
every variant: `UnresolvedFunction`, `NotAFunction`,
`FunctionTypedParameter`, `RelationTyped`, `GenericTyped`,
`UnresolvedClass`, `NotAClass`, `UnresolvedAssociation`,
`NotAnAssociation`.

## Reachability walk

Starting from each requested function's parameter / return types and
the explicit `--classes` / `--associations` seeds, BFS over:

- supertypes (preserves the `extends` chain in generated Java);
- declared properties + qualified-property parameters/return;
- association-injected properties (using `1 - prop_idx_pointing_to_self`
  so the *navigable* end's type is followed, not the type the model
  index keys on — this mirrors `crates/pure/src/infer.rs:1755`);
- nested type arguments (`Iterable<X>`, `Optional<X>`).

Classes whose FQN starts with `meta::pure::*` are filtered as platform
types (rendered as opaque `Object`) unless explicitly opted in via
`--classes`, in which case the closure walk emits them but transitive
walks from there still apply the filter.

## Type mapping

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
| `Date` (abstract)            | `java.time.temporal.Temporal`         |
| `Any`, `Nil`                 | `Object`                              |
| user `Class` / `Enumeration` | generated interface / enum            |
| `Function<{…}>` / Relation   | rejected at codegen (deferred to v2)  |

Multiplicity wraps the inner type:

| Pure                           | Java                       |
| ------------------------------ | -------------------------- |
| `[1]`                          | `T`                        |
| `[0..1]`                       | `java.util.Optional<T>`    |
| `[*]`, `[1..*]`, `[m..n]`      | `Iterable<T>`              |

A `GenericPolicy` enum gates `TypeExpr::Generic` handling: function
signatures hard-fail (`Reject`) because there's no way to make a
generic-typed wrapper concrete in Java; class properties / qualified
properties (`AsObject`) render as `Object` so generic classes like
`Enumeration<E>` keep working — at the cost of losing the type
parameter at the Java surface (v2 will replace this with parameterised
interfaces).

## CLI usage

```bash
# Bind a few specific functions plus everything reachable from them.
legend java-bindings \
    --output ./gen-java \
    --java-package com.example.gen \
    --functions meta::pure::functions::math::plus_Integer_MANY__Integer_1_

# Add explicit class / association seeds that aren't reachable from
# any requested function.
legend java-bindings \
    --output ./gen-java \
    --java-package com.example.gen \
    --classes user_test::Person \
    --associations user_test::PersonAddress

# Bootstrap from a curated manifest — one FQN per line, kind
# auto-detected by the CLI from the model. Used by the M3 binding
# manifest at
# legend-pure-runtime/legend-pure-runtime-rust-evaluator/src/main/pure-bindings/m3-bindings.txt
legend java-bindings \
    --output target/generated-sources/java-bindings \
    --java-package org.finos.legend.pure.rust.generated \
    --bindings-file ./m3-bindings.txt
```

## Tests

`cargo test -p legend-pure-java-codegen` (16 tests, ≈3s after the
platform model warm-up):

- `naming` unit tests — identifier escaping, keyword collision handling.
- `primitive_function` — codegen for `plus(Integer[*]):Integer[1]`
  asserts the static-method shape and that the mangled FQN is preserved
  verbatim in the `evaluate(...)` call.
- `class_closure` — synthetic `Person` / `Address` model, asserts the
  full closure walk produces both interfaces with `[0..1]` /
  `[*]` defaults, an association property, and a facade init block.
- `errors` — function-typed and generic-typed parameters error with
  typed `CodegenError` variants.
- `explicit_seeds` — `--classes` / `--associations` flags emit
  interfaces for elements no requested function references; error
  paths for `NotAClass`, `UnresolvedClass`, `UnresolvedAssociation`.
- `javac_compiles` — generates a small set, compiles it through
  `javac --release 11` together with the hand-written runtime support
  classes. Skipped when `javac` or the local Maven cache isn't set up.

## See also

- [`crates/jni/README.md`](../jni/README.md) — the Rust-side JNI bridge
  the generated code targets.
- [`legend-pure-runtime/legend-pure-runtime-rust-evaluator/README.md`](../../../legend-pure-runtime/legend-pure-runtime-rust-evaluator/README.md)
  — the Java module that consumes the generated sources.
- [`BACKLOG.md`](../../BACKLOG.md) — open follow-ups (function-typed
  parameters, generic type-arg propagation, deeper `pickGeneratedInterface`
  hierarchy walk, full JNI live-evaluator JUnit round-trip).
