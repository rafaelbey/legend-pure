# Runtime Crate — Backlog

Items are tagged **P0** (blocking) → **P3** (nice-to-have).

---

## Native-function organisation

### [P2] One file per native
Each native function lives in its own `.rs` file alongside its unit tests.
Current layout bundles all natives for a domain (e.g., every string native) into
a single file. Splitting gives a clear 1:1 mapping and keeps each file small.

Proposed layout:
```
native/
  string/
    mod.rs        ← register() / distributed-slice iteration
    plus.rs       ← StringPlus + tests
    length.rs
    format.rs
    …
  comparison/
    mod.rs
    eq.rs
    equal.rs
    less_than.rs
    …
  lang/
    mod.rs
    let_function.rs
    if.rs
    eval.rs
    …
```

### [P2] Distributed-slice self-registration (`linkme`)
Replace per-module `register()` calls with `#[linkme::distributed_slice]` so each
native file self-registers. Adding a new native only requires creating its file.

```rust
// native/mod.rs
#[linkme::distributed_slice]
pub static NATIVES: [(&'static str, &'static dyn NativeFunction)] = [..];

// native/string/format.rs
#[linkme::distributed_slice(crate::native::NATIVES)]
static _FORMAT: (&str, &dyn NativeFunction) =
    ("format_String_1__Any_MANY__String_1_", &Format);
```

`linkme` 0.3+ is compatible with `#![forbid(unsafe_code)]`.
`NativeRegistry::build()` just iterates `NATIVES`.

### [P2] Standard registry built into `Evaluator`
`Evaluator::new` currently requires the caller to build and pass a `NativeRegistry`.
The standard registry (all built-in natives) should be the default; callers that need
to extend or override it can supply their own.

```rust
// today
let registry = NativeRegistry::build();
let mut eval = Evaluator::new(&model, &registry);

// target
let mut eval = Evaluator::new(&model);                        // uses built-in registry
let mut eval = Evaluator::with_registry(&model, registry);   // custom / extended
```

Depends on the distributed-slice item above, since `Evaluator::new` needs a
complete list of natives without manually enumerating them.
