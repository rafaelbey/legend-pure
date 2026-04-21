---
name: Pure language shape
description: Core semantic contract of the Pure language that constrains both Java and Rust implementations
type: project
---

Pure is a strongly-typed, pure-functional, expression-oriented DSL. Everything is an expression; there is no `return`. Key facts a reviewer must keep in head:

- **Multiplicity is part of every type.** Signatures look like `String[1]`, `Person[0..1]`, `T[*]`. The type checker must treat `(type, multiplicity)` as the unit of compatibility — never check type alone.
- **`Any` is top; `Nil` is bottom.** `Nil` shows up in library lambda parameter types (`Function<{Nil[n]->T[m]}>`) and as the type of empty `[]`.
- **`^Class(prop=val, ...)` constructs instances.** Desugared in the Rust compiler to `FunctionCall("new", ...)`.
- **`if`, `let`, `match` are functions, not keywords.** `if` takes zero-arg lambdas for its branches (`valid: Function<{->T[m]}>[1]`) for lazy evaluation.
- **Functions are overloaded by mangled signature.** The canonical mangled name incorporates parameter types and multiplicities: `plus_Integer_MANY__Integer_1_`.
- **Stereotypes and tagged values (`<<stereo>>`, `{tag='val'}`) are first-class annotations.** Milestoning (`<<temporal.businesstemporal>>`), `<<PCT.function>>`, `<<access.*>>`, etc. are all implemented as stereotype-driven rewrites or tests.
- **The M4→M3→M2→M1 metamodel stack is the spine of the repo.** M4 = "what is a node" (`CoreInstance` in Java); M3 = Pure metamodel (Class, Function…); M2 = DSL metamodels written in Pure itself; M1 = user `.pure` source. Platform sources under `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/` are M1 that *bootstrap* M2/M3.

## Why: reviewer needs this

- Multiplicity mistakes are the most common type-system bugs in new implementations — if a Rust change touches type compatibility, check multiplicity is considered alongside type.
- `if`/`let`/`match`-as-functions means lowering them to `FunctionCall` is *correct*, not an abstraction leak — accept this pattern in `crates/pure/src/lower.rs`.
- The shared platform `.pure` files are the integration-test contract between Java and Rust; a Rust change that breaks platform parsing is much more serious than breaking Rust-local tests.

## How to apply

- When reviewing type resolution or dispatch code, verify `Multiplicity` is threaded through alongside `TypeExpr`.
- Treat the `^` instance syntax and `<<stereo>>` annotations as load-bearing, not syntactic sugar.
- Check that any change touching the semantic layer has a corresponding platform-source-compatible test, not just a synthetic one.
