 # Downstream extensions: adding your own DSL and native functions

This guide walks an **external** Rust crate or repo through everything it
takes to ship a Legend Pure extension — a new top-level DSL section
(`###MyDsl`), embedded island grammars (`#mytag{ … }#`), compile-time
hooks, and native functions implemented in Rust — on top of the
`legend-pure-rust` workspace.

Audience: anyone outside this repo who depends on `legend-pure-*` as
Cargo dependencies. The in-tree DSLs under `crates/dsl-*` are the
worked references; this guide shows how to follow the same patterns
from a sibling crate.

> **Status:** the library SPI and discovery layer are stable. Each
> trait — `RuntimeExtension`, `CompilerExtension`, `SectionParser`,
> `IslandParser`, `DSLPopulator`, `IdeExtension` — has a `linkme`
> distributed slice; your extension annotates a `static` next to its
> trait impl and any binary that pulls your crate in as a Cargo
> dependency picks it up automatically. Stateful `CompilerExtension`s
> still need explicit wiring (see §5 for the carve-out).

---

## 0. Quick start

The 30-second version. Suppose you're adding a single native function
called `greet`:

```toml
# Cargo.toml
[dependencies]
legend-pure-runtime = "<version>"
linkme              = "0.3"
```

```rust
// src/lib.rs
use legend_pure_runtime::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry,
    RUNTIME_EXTENSIONS, RuntimeExtension, expect_args, force_all,
};
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::value::Value;
use legend_pure_parser_pure::types::ValueSpec;
use linkme::distributed_slice;

#[derive(Debug)]
struct Greet;

impl NativeFunction for Greet {
    fn execute(
        &self, args: &[ValueSpec], ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("mydsl::greet", args, 1)?;
        let v = force_all(args, ctx)?;
        let name = v[0].as_string()?;
        Ok(Evaluated::new(Value::String(format!("hello, {name}").into())))
    }
    fn signature(&self) -> &'static str { "greet(String[1]): String[1]" }
}

pub struct GreetExtension;
impl RuntimeExtension for GreetExtension {
    fn name(&self) -> &'static str { "greet-extension" }
    fn register_natives(&self, r: &mut NativeRegistry) {
        r.register("greet_String_1__String_1_", Greet);
    }
}

#[distributed_slice(RUNTIME_EXTENSIONS)]
static GREET_EXT: &(dyn RuntimeExtension + Sync) = &GreetExtension;
```

That's it. Any consumer binary that depends on this crate and includes
**one forcing import** (see [Link forcing](#link-forcing-important)
below) picks up `greet` via `NativeRegistry::discovered()` or
`Evaluator::builder().build(&model)` — no per-binary slice juggling.

---

## 1. The five extension surfaces

Pick the surfaces you need; they're independent and you can implement
one without touching the others.

| Surface | Trait | Crate path | When you need it |
|---|---|---|---|
| Native functions | `RuntimeExtension` | `crates/runtime/src/native.rs` | Pure code calls a function whose body is Rust |
| Compiler hooks | `CompilerExtension` | `crates/pure/src/extension.rs` | Your DSL contributes elements / validates references / participates in type-checking |
| Section grammar | `SectionParser` | `crates/parser/src/section_parser.rs` | You introduce `###MyDsl` blocks |
| Island grammar | `IslandParser` | `crates/parser/src/island.rs` | You embed `#tag{ … }#` micro-syntaxes inside Pure expressions |
| Heap hydration | `DSLPopulator` | `crates/runtime/src/dsl.rs` | Your DSL stores data in `Element::DSLInstance` and you want it reflected on the runtime heap |

A native-only extension (no new syntax, just new functions) needs only
`RuntimeExtension`. A typical full DSL implements
`SectionParser` + `CompilerExtension` + `DSLPopulator`, and ships a
sibling `RuntimeExtension` if it adds natives.

There is a sixth surface — `IdeExtension` (`crates/pure/src/refs.rs`)
— for contributing IDE references (go-to-definition, find-usages).
It's a separate, self-discoverable trait. In-tree DSLs still expose
their `walk_references` via `CompilerExtension::walk_references`
today; migration onto `IdeExtension` follows the state-into-model
lift described in §5.

In-tree templates this guide cites verbatim:

- `crates/store-relational-runtime/src/extension.rs` — `RuntimeExtension`
  with `#[distributed_slice(RUNTIME_EXTENSIONS)]`
- `crates/dsl-relational/src/parser.rs` — `SectionParser`
- `crates/dsl-relational/src/compiler.rs` — `CompilerExtension`
- `crates/dsl-mapping-runtime/src/lib.rs` and
  `crates/dsl-relational-runtime/src/lib.rs` — `DSLPopulator` with
  `#[distributed_slice(DSL_POPULATORS)]`
- `crates/cli/src/main.rs` — the canonical force-link recipe
- `crates/cli/src/commands/test.rs` — the runtime wiring exemplar
  (`NativeRegistry::discovered()` + `discovered_populators()` —
  no per-extension slice in the command body)

---

## 2. Recommended project layout

```
mydsl-pure-extension/
├── Cargo.toml
├── src/
│   ├── lib.rs            # re-exports the trait impls
│   ├── runtime.rs        # impl RuntimeExtension
│   ├── compiler.rs       # impl CompilerExtension (+ snapshot type)
│   ├── parser.rs         # impl SectionParser, IslandParser
│   ├── populator.rs      # impl DSLPopulator
│   └── natives/          # one zero-sized struct per native
├── resources/
│   └── mydsl/
│       └── mydsl.pure    # metamodel (classes your DSL projects into)
├── tests/
│   └── smoke.rs
└── bin/
    └── mydsl-legend.rs   # custom CLI binary (see §9)
```

`Cargo.toml` dependency floor:

```toml
[dependencies]
legend-pure-runtime       = "<version>"   # NativeRegistry, Evaluator, DSLPopulator
legend-pure-pure          = "<version>"   # CompilerExtension, PureModel, Element
legend-pure-parser-parser = "<version>"   # ParserContext, SectionParser, IslandParser
legend-pure-parser-ast    = "<version>"   # DSLElement, Expression, SourceInfo
legend-pure-parser-pure   = "<version>"   # ValueSpec, type system
smol_str                  = "0.3"
linkme                    = "0.3"          # distributed-slice self-registration
```

Add `legend-cli` if you want to inherit its `main()` dispatcher
(recommended — see §8). Add `clap` + `miette` only if you write a
bespoke binary.

> **Today**: these crates are not yet published to crates.io. While you
> build against this repo, use `path = "../legend-pure-rust/crates/<name>"`
> dependencies. Publication is tracked separately.

---

## 3. Adding native functions via `RuntimeExtension`

Implement the trait, register one entry per **mangled** function FQN.
Mangling follows
`funcName_ParamType_Mult__ReturnType_Mult_` — match the Java
`ConcreteFunctionDefinitionNameProcessor` format so the compiler and
runtime agree on the key.

```rust
use legend_pure_runtime::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry,
    RuntimeExtension, expect_args, force_all,
};
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::value::Value;
use legend_pure_parser_pure::types::ValueSpec;

#[derive(Debug)]
struct GreetNative;

impl NativeFunction for GreetNative {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("mydsl::greet", args, 1)?;
        let forced = force_all(args, ctx)?;
        let name = forced[0].as_string()?;
        Ok(Evaluated::new(Value::String(
            format!("hello, {name}").into(),
        )))
    }

    fn signature(&self) -> &'static str {
        "greet(String[1]): String[1]"
    }
}

#[derive(Debug, Default)]
pub struct MyExtension;

impl RuntimeExtension for MyExtension {
    fn name(&self) -> &'static str {
        "mydsl-pure-extension"
    }

    fn register_natives(&self, r: &mut NativeRegistry) {
        r.register("greet_String_1__String_1_", GreetNative);
    }
}
```

The pattern mirrors
[`crates/store-relational-runtime/src/extension.rs:46-120`](../../crates/store-relational-runtime/src/extension.rs)
exactly — including the convention of one zero-sized type per native so
dispatch never probes argument shape positionally.

**Wiring** — register the static next to the impl:

```rust
use legend_pure_runtime::native::RUNTIME_EXTENSIONS;
use linkme::distributed_slice;

#[distributed_slice(RUNTIME_EXTENSIONS)]
static MY_EXT: &(dyn RuntimeExtension + Sync) = &MyExtension;
```

That's all the registration. Any consumer binary using
`NativeRegistry::discovered()` or `Evaluator::builder().build(&model)`
picks it up automatically — provided the consumer adds the link-forcing
import described in [§8.1 Link forcing](#link-forcing-important).

The explicit alternative — `NativeRegistry::with_extensions(&[&MyExtension])`
— remains for tests that compose a deliberate native set, or for
embedders that want to suppress discovered registrations.

### Eager vs. short-circuit natives

`force_all` forces every argument up front and is the right helper for
arithmetic / string / collection-style natives. Short-circuit natives
(`and`, `or`, `if`-shaped) call `ctx.evaluate(&args[i])` themselves so
unused branches stay unevaluated. Lambda-receiving natives (`map` /
`fold` shape) evaluate the lambda spec to a `Value::Function` and
invoke it per-element via `ctx.call_function(&fn_val, &[arg])`. See
`crates/runtime/src/native.rs:262-282` for the activator model.

### Per-evaluator state

If your native needs state that lives as long as the evaluator does
(connection handles, caches), use `ExtensionStateStore`. Inside a
native:

```rust
let state = ctx
    .extensions()
    .get_or_init::<MyState, _>(MyState::new)?;
state.do_something();
```

This is the pattern `store-relational-runtime` uses to keep one DuckDB
connection per evaluator.

### Per-evaluator configuration

For declarative configuration (jar paths, connection strings,
credentials, feature flags) sourced from
`legend-pure-classpath.toml`'s `[extension.<name>]` tables, use
[`EvalContextTrait::config_for`](https://docs.rs/legend-pure-runtime/0.1/legend_pure_runtime/native/trait.EvalContextTrait.html#method.config_for):

```rust
let sub = ctx.config_for("mydsl");        // Option<&HashMap<String, toml::Value>>
let port = sub
    .and_then(|m| m.get("port"))
    .and_then(toml::Value::as_integer)
    .unwrap_or(default_port);
```

The CLI threads the resolved
`legend_cli::classpath::ResolvedClasspath::extension_configs` through
to `Evaluator::builder().extension_configs(...)` for you — your
extension just reads its sub-table. Per-evaluator (not process-wide),
so two evaluators can carry independent tenant configs / credentials
without leaking into each other.

### Sync requirement

The `RUNTIME_EXTENSIONS` slice stores `&'static (dyn RuntimeExtension + Sync)`
because the slice is a `static` and `Sync` is required for `static`
items shared across threads. Stateless unit structs
(`pub struct MyExtension;`) are `Sync` automatically. Stateful
extensions that need interior mutability must use thread-safe
primitives (`Mutex`, `RwLock`, atomics) or move per-evaluator state
into [`ExtensionStateStore`](#per-evaluator-state) as the section
above shows.

---

## 4. Adding a section grammar (`###MyDsl`)

Implement `SectionParser`. The trait requires `Send + Sync` because the
CLI parallelises file parsing via Rayon — keep your parser stateless
(zero-sized struct is ideal).

```rust
use legend_pure_parser_parser::{ParserContext, SectionParser, error::ParseError};
use legend_pure_parser_ast::dsl::DSLElement;

pub struct MyDslSectionParser;

impl SectionParser for MyDslSectionParser {
    fn kind(&self) -> &str { "MyDsl" }

    fn parse_body(
        &self,
        ctx: &mut ParserContext<'_>,
        errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>> {
        let mut out = Vec::new();
        // Use ctx.cursor() for raw token access, or higher-level
        // helpers like ctx.parse_qualified_name() / ctx.parse_expression().
        // Drive the parse until you hit a section boundary or EOF;
        // push ParseError onto `errors` and recover rather than abort.
        out
    }
}
```

Define your `DSLElement` payloads as plain structs that implement
`legend_pure_parser_ast::dsl::DSLElement`. The shape that the
`CompilerExtension::declare` hook receives is whatever you push here.

Template: `crates/dsl-relational/src/parser.rs:73` (`RelationalSectionParser`)
and `:1293` (`RelationalClassMappingBodyParser`, a nested section parser
for the body of `~class` blocks inside `###Mapping`).

### Island grammars (`#tag{ … }#`)

Same shape, different trait. Use when you want a micro-DSL embedded
inside a Pure expression (graph fetch trees, path expressions, embedded
SQL):

```rust
use legend_pure_parser_parser::{ParserContext, IslandParser, error::ParseError};
use legend_pure_parser_ast::island::IslandContent;

pub struct MyIslandParser;

impl IslandParser for MyIslandParser {
    fn tag(&self) -> &str { "mytag" }  // matches `#mytag{ … }#`

    fn parse(
        &self,
        ctx: &mut ParserContext<'_>,
    ) -> Result<Box<dyn IslandContent>, ParseError> {
        // Consume from `#mytag{` up to but NOT including `}#`.
        todo!()
    }
}
```

**Wiring** — both go through the parser entry points:

```rust
use legend_pure_parser_parser::{parse_with_islands, parse_with_sections};

let islands: Vec<Box<dyn IslandParser>> = vec![Box::new(MyIslandParser)];
let sections: Vec<Box<dyn SectionParser>> = vec![Box::new(MyDslSectionParser)];

let (file, errors) = parse_with_sections(source, "file.pure", &islands, &sections);
```

**Wiring** — register the parser statics next to the impls:

```rust
use legend_pure_parser_parser::island::{IslandParser, ISLAND_PARSERS};
use legend_pure_parser_parser::section_parser::{SectionParser, SECTION_PARSERS};
use linkme::distributed_slice;

#[distributed_slice(SECTION_PARSERS)]
static MY_SECTION: &(dyn SectionParser + Send + Sync) = &MyDslSectionParser;

#[distributed_slice(ISLAND_PARSERS)]
static MY_ISLAND: &(dyn IslandParser + Send + Sync) = &MyIslandParser;
```

Any consumer using `legend_pure_parser_parser::parse(src, name)`
picks up both. The explicit `parse_with_islands` / `parse_with_sections`
calls remain for tests that compose plugin sets by hand.

Discovery validates at startup: two parsers with the same `tag()` or
`kind()` panic with both source extensions named.

---

## 5. Adding a `CompilerExtension`

This is where your DSL becomes part of the compiled `PureModel`. The
pipeline calls four hooks per pass:

| Hook | Pass | Purpose |
|---|---|---|
| `declare` | 1 | Allocate element shells in the model (assign IDs, populate the package tree) |
| `define_signatures` | 2a | Resolve element / type references needed before bodies lower |
| `define_bodies` | 2b | Lower function bodies, constraint expressions, qualified-property bodies |
| `validate` | 3 | Read-only checks on the frozen model |
| `walk_references` | — | Contribute references to the IDE index (called by `build_reference_index`) |

All hooks default to no-op, so override only what you contribute.

```rust
use legend_pure_pure::extension::{
    CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx,
};
use legend_pure_pure::model::{Element, DSLInstance};

pub struct MyDslExtension;

impl CompilerExtension for MyDslExtension {
    fn name(&self) -> &'static str { "mydsl-pure-extension" }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        // Walk ctx.source_files, find your ###MyDsl section elements,
        // build a Snapshot payload, push Element::DSLInstance into
        // ctx.model. Errors push onto ctx.errors.
    }

    fn validate(&self, ctx: &mut ValidateCtx<'_>) {
        // Read-only checks against ctx.model. Use
        // lower_and_infer_expression() if you need to type-check user
        // expressions inside your DSL bodies.
    }
}
```

Template: `crates/dsl-relational/src/compiler.rs:116` (`RelationalExtension`
struct) and `:426` (the trait impl). The relational extension overrides
`declare` and `validate`; Mapping additionally overrides
`define_bodies` for property-mapping bodies.

### Stateless vs. stateful: the discovery carve-out

The `COMPILER_EXTENSIONS` distributed slice stores
`&'static (dyn CompilerExtension + Sync)`. A stateless unit struct
(`pub struct MyDslExtension;`) registers with one line:

```rust
use legend_pure_pure::extension::COMPILER_EXTENSIONS;
use linkme::distributed_slice;

#[distributed_slice(COMPILER_EXTENSIONS)]
static MY_DSL_EXT: &(dyn CompilerExtension + Sync) = &MyDslExtension;
```

**A `CompilerExtension` that holds `RefCell`-backed per-compile state
cannot self-register** — `RefCell` isn't `Sync`. The in-tree
`RelationalExtension` and `MappingExtension` fall in this bucket today
(they accumulate `databases` / `mappings` RefCells across `declare`
and `validate`). Those extensions stay explicit callers of
[`compile_with_extensions`](https://docs.rs/legend-pure-pure/0.1/legend_pure_pure/pipeline/fn.compile_with_extensions.html)
until their state migrates into `Element::DSLInstance` (see §6) and
the struct becomes a unit type.

If your DSL needs per-compile state, two clean shapes work:

1. **Put the state in the model.** Use `Element::DSLInstance` (§6) as
   the source of truth. `declare` writes; `validate` and downstream
   reads pull from the model. The extension struct stays stateless and
   self-registers.
2. **Stay explicit.** Construct a fresh `MyDslExtension` per compile,
   pass it through `compile_with_extensions(&chunks, &[&ext])`. You
   give up auto-discovery in exchange for the simpler shape — the
   plan accepts this trade-off for the existing in-tree DSLs.

### Ordering: `depends_on`

When extension B's `declare` reads model state extension A wrote *in
the same pass*, declare the dependency:

```rust
impl CompilerExtension for MyDslExtension {
    fn name(&self) -> &'static str { "mydsl" }
    fn depends_on(&self) -> &'static [&'static str] { &["RelationalExtension"] }
}
```

`discovered_compiler_extensions()` runs a Kahn topological sort and
panics on cycles with the cycle path in the message.

### Validating embedded user expressions

When your DSL accepts a user-written Pure expression (a filter
predicate, a derivation, a constraint), don't re-implement type
inference — use the public helper:

```rust
use legend_pure_pure::extension::lower_and_infer_expression;
use legend_pure_pure::types::{Multiplicity, TypeExpr};

let inferred = lower_and_infer_expression(
    ctx.model,
    ctx.auto_imports,
    &user_expr,
    &[("src".into(), src_class_type, Multiplicity::PureOne)],
    ctx.errors,
);
```

The helper runs the full lower-and-infer pipeline against the model and
appends any errors to your accumulator. See
`crates/pure/src/extension.rs:211-268` for full semantics including
limitations around generic type parameters.

**Wiring** — production default uses discovery, tests use explicit:

```rust
use legend_pure_pure::pipeline::{compile, compile_with_extensions};

// Production (default): discovers + topo-sorts every CompilerExtension
// registered via #[distributed_slice(COMPILER_EXTENSIONS)] in linked crates.
let model = compile(chunks, &auto_imports)?;

// Tests / stateful extensions: explicit composition.
let model = compile_with_extensions(chunks, &auto_imports, &[&MyDslExtension])?;
```

---

## 6. Persisting DSL data via `Element::DSLInstance`

`Element::DSLInstance { dsl_name, classifier_fqn, data }` is a
first-class graph citizen. It rides through `.purem` slice/merge
alongside every other element, so your DSL's compiled data survives the
binary snapshot path with no per-DSL serialisation hook.

Design the `data` payload as a postcard-serializable snapshot type and
expose `Snapshot::encode` / `Snapshot::decode` helpers. The relational
DSL's `DatabaseSnapshot` (in `crates/dsl-relational/src/compiler.rs`)
is the worked template — schemas, tables, joins, filters all live as
plain Rust data in the snapshot.

In your `declare` hook:

```rust
let snapshot = MyDslSnapshot { /* fields */ };
let data = snapshot.encode()?;
ctx.model.push_element(Element::DSLInstance(DSLInstance {
    dsl_name: "MyDsl".into(),
    classifier_fqn: "meta::mydsl::Thing".into(),
    data,
}));
```

The fields on `DSLInstance` are all `pub`
(`crates/pure/src/model.rs:160-173`), so this constructor call works
unchanged from an external crate.

The actual push-into-model is a multi-step sequence — see
`crates/dsl-diagram/src/compiler.rs:245-333` for the canonical pattern.
You allocate a `ModelChunk` slot via
`ctx.model.chunks[i].alloc_element(ElementNode { … }, Element::DSLInstance { … })`,
then register the resulting `ElementId::InstanceId { chunk_id, local_idx }`
in its parent package via `ctx.model.register_element(pkg_id, eid)`.
All the surface types (`ModelChunk`, `ElementNode`,
`ElementId::InstanceId`, `PackageId`) and field accessors are `pub`;
no wrapper helper is needed for external consumers.

---

## 7. Heap hydration via `DSLPopulator`

`Element::DSLInstance` becomes a bare metamodel heap row at evaluator
setup — classifier is set, properties are empty. To make Pure-side
navigation work (`mydb.schemas->first().tables`), implement
`DSLPopulator` and project your snapshot onto the heap:

```rust
use legend_pure_runtime::dsl::{DSLPopulator, DSLPopulationCtx};
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

pub struct MyDslPopulator;

impl DSLPopulator for MyDslPopulator {
    fn dsl_name(&self) -> &'static str { "MyDsl" }

    fn populate(&self, mut ctx: DSLPopulationCtx<'_>) {
        let snapshot = match MyDslSnapshot::decode(ctx.instance_data) {
            Ok(s) => s,
            Err(_) => return, // stay evaluable on decode failure
        };
        let _ = ctx.heap.mutate_set(
            &ctx.instance_handle,
            "name",
            &[Value::String(snapshot.name)],
        );
        // Allocate nested rows for children with ctx.heap.alloc_dynamic(),
        // populate properties with ctx.heap.mutate_set, then attach them
        // to the parent via mutate_set on ctx.instance_handle.
    }
}
```

Template: `crates/dsl-relational-runtime/src/lib.rs:88-203`
(`RelationalDatabaseDSLPopulator`).

**Wiring** — register next to the impl:

```rust
use legend_pure_runtime::dsl::DSL_POPULATORS;
use linkme::distributed_slice;

#[distributed_slice(DSL_POPULATORS)]
static MY_DSL_POP: &(dyn DSLPopulator + Sync) = &MyDslPopulator;
```

Note the runtime walks populators in **chunk-element order**, so a
populator can depend on rows allocated by an earlier-running populator
in the same evaluator. The relational stack relies on this for
`RelationalClassMappingDSLPopulator` to find Table rows that
`RelationalDatabaseDSLPopulator` allocated first. The order is driven
by the compiler's `declare` hook (which decides element-emission
order), not by the populator slice order — slice order only matters
for tie-breaking among populators handling the same `dsl_name`, which
is forbidden anyway (`discovered_populators()` panics on duplicates).

---

## 8. Wiring it together — the minimal custom binary

With every trait surface self-registering, the consumer binary is one
helper call plus the **link-forcing imports** that ensure the linker
doesn't drop your extension crates.

```rust
// src/main.rs

// 1. Force-link your extension crates. Without these `use` lines
//    the linker is free to drop the entire crate object file (no
//    reachable code → no contribution to the distributed slices).
//    This is the only per-binary wiring step that remains.
#[allow(unused_imports)]
use mydsl::GreetExtension as _;
#[allow(unused_imports)]
use mydsl::MyDslSectionParser as _;
#[allow(unused_imports)]
use mydsl::MyDslPopulator as _;

fn main() -> anyhow::Result<()> {
    // 2. Hand off to the stock CLI dispatcher; everything else flows
    //    through discovery.
    legend_cli::main()
}
```

That's it. The stock `legend` CLI's command set (`parse`, `check`,
`test`, `run`, `repl`, …) all dispatch through
`NativeRegistry::discovered()`, `pipeline::compile()` (discovered
`CompilerExtension`s), `parse()` (discovered parsers), and
`Evaluator::builder().build()` (discovered populators). Your extension
flows into all of them automatically.

### Link forcing (important)

`linkme` distributed slices live in dedicated linker sections. The
sections are populated by the *static* declarations
(`#[distributed_slice(FOO)] static MY_X: …`) — but **only if the
containing crate is actually linked into the final binary**. Rust's
linker is allowed to drop crate object files entirely when nothing in
your binary references them.

The simple rule: in your binary's `main.rs`, add one `use foo::Item as _;`
line per extension crate. Any name from the crate will do — the
import's purpose is to make the linker keep the crate, not to use the
item. The `#[allow(unused_imports)]` silences the dead-import warning.

The in-tree `crates/cli/src/main.rs` follows the same pattern for the
relational extension and the three DSL populators. Mirror it for your
own crates.

### Constructing an evaluator outside the CLI

If you're embedding the evaluator in a service or test harness rather
than shipping a CLI binary:

```rust
use legend_pure_runtime::eval::Evaluator;

// Defaults: discovered native registry + discovered DSL populators.
let evaluator = Evaluator::builder().build(&model);

// Or override individual knobs for tests:
let evaluator = Evaluator::builder()
    .registry(&my_explicit_registry)
    .populators(&[&MyTestPopulator])
    .build(&model);
```

The same force-link rule applies — your embedder binary must `use`
something from each extension crate it wants discovered.

---

## 9. Classpath / `.purem` loading

The CLI supports a `legend-pure-classpath.toml` for declaring which
repos to compile and which extensions to configure. The schema:

```toml
[[repo]]
name = "mydsl"
kind = "filesystem"           # or "purem" for prebuilt binary
path = "resources/mydsl"
auto-imports = ["meta::mydsl"]

[extension.mydsl]
some_setting = "value"        # surfaced to your extension via
                              # extension_configs (see §3 state pattern)
```

The CLI top-level `--classpath <path>` flag is wired through
`crates/cli/src/main.rs:140-153` to every subcommand. The parser /
resolver lives at `crates/cli/src/classpath.rs:`
`resolve_classpath(path, cwd) -> ResolvedClasspath { repos, extra_auto_imports, extension_configs }`.

> **In flight**: most CLI subcommands still discard the resolved
> classpath (`crates/cli/src/commands/test.rs:186`: `let _ = classpath;`)
> while the receiving call sites are migrated to the builder. The
> top-level `--classpath` flag is already plumbed; subcommand bodies
> just need to call `crate::classpath::resolve_classpath(...)` and
> thread the result into `Evaluator::builder()` / `pipeline::compile()`.
> Tracked as item 4 in the [Gaps and roadmap](#gaps-and-roadmap) at
> the end of this doc.

---

## 10. Testing

Three test surfaces, increasing in scope:

- **Unit** — each trait impl in isolation. For natives, mirror
  `store-relational-runtime/src/extension.rs:122-153` — assert that
  every mangled key you register is `Some` in the registry after
  `register_natives`.
- **Compile** — feed `.pure` source through
  `parse_with_sections` + `compile_with_extensions` and assert no
  errors. For your `declare` hook, assert that `model.elements()`
  contains the expected `Element::DSLInstance` entries.
- **End-to-end** — instantiate an `Evaluator` with your extensions
  and populators, evaluate a function declared in `.pure`, assert on
  the returned `Value`. Use `Evaluator::evaluate_fqn` against a
  test-only `<<test.Test>>`-stereotyped Pure function for runnable
  parity-style tests.

For PCT-style behavioural parity against an in-tree DSL you're
mirroring, list a small `.pure` fixture file under your `tests/` and
drive it from a Rust integration test. The in-tree pattern is at
`crates/dsl-relational/tests/*.rs`.

---

## 11. Distribution

Two options, simplest first:

- **Cargo-install a custom binary** —
  `cargo install --bin mydsl-legend --path .` and downstream users
  invoke `mydsl-legend test`, `mydsl-legend run …` exactly like
  `legend`. The binary contains your extensions, the
  `legend-pure-*` crates, and any platform `.pure` source you embed.
- **Library crate consumed by an embedding app** — publish
  `mydsl-pure-extension` as a library; embedders build their own
  binary that pulls your `MyExtension` + `MyDslExtension` and
  combines them with other extensions.

> **In flight**: there is no in-tree publishing template /
> "downstream extension cookiecutter" yet. The
> `examples/mydsl-extension/` crate (item 5 in [Gaps and
> roadmap](#gaps-and-roadmap)) will serve as the runnable template
> once it lands.

---

## Stable API reference

The types below are public and form the de-facto downstream contract.
Breaking changes to these will land as `MAJOR` version bumps; everything
else is best-effort backwards-compatible.

| Crate | Public surface |
|---|---|
| `legend-pure-runtime` | `native::{NativeFunction, NativeRegistry, RuntimeExtension, RUNTIME_EXTENSIONS, ExtensionStateStore}`, `value::Value`, `eval::{Evaluator, EvalContextTrait, Evaluated}`, `builder::EvaluatorBuilder`, `heap::{RuntimeHeap, ObjectHandle}`, `dsl::{DSLPopulator, DSLPopulationCtx, DSL_POPULATORS, discovered_populators, run_populators}`, `error::{PureException, PureRuntimeError}` |
| `legend-pure-pure` | `model::{PureModel, Element, DSLInstance}`, `extension::{CompilerExtension, COMPILER_EXTENSIONS, discovered_compiler_extensions, DeclareCtx, DefineCtx, ValidateCtx, lower_and_infer_expression}`, `pipeline::{compile, compile_with_extensions}`, `refs::{Reference, IdeExtension, IDE_EXTENSIONS, discovered_ide_extensions, build_reference_index, build_reference_index_with_ide_extensions}`, `types::{TypeExpr, Multiplicity, ResolvedType, Parameter}` |
| `legend-pure-parser-parser` | `ParserContext`, `IslandParser`, `ISLAND_PARSERS`, `discovered_island_parsers`, `SectionParser`, `SECTION_PARSERS`, `discovered_section_parsers`, `parse`, `parse_with_islands`, `parse_with_sections`, `error::ParseError` |
| `legend-pure-parser-ast` | `dsl::DSLElement`, `island::IslandContent`, `expression::Expression`, `section::SourceFile`, `SourceInfo`, derive macros |
| `legend-pure-parser-pure` | `model::PureModel`, `types::{ValueSpec, ExprKind}` |

---

## Gaps and roadmap

The discovery infrastructure and the in-tree stateless extension
self-registrations have landed; the CLI flows through
`NativeRegistry::discovered()` and `discovered_populators()`. The
known remaining items, with the smallest first:

1. **Stateful `CompilerExtension` migration**. `MappingExtension`,
   `RelationalExtension`, `DiagramExtension` hold `RefCell`-backed
   per-compile state today and can't satisfy the `Sync` bound the
   distributed slice requires. Lift the state into
   `Element::DSLInstance` (Diagram is already there for its primary
   payload — Mapping / Relational still write the registry side too)
   so the structs become unit types and can self-register. Tracked as
   "Phase 3 FULL" in
   `~/.claude/plans/how-will-be-the-hashed-sparkle.md`.
2. **`IdeExtension` migration of `walk_references`**. Blocked on item
   1; once the stateful `CompilerExtension`s self-register, their
   `walk_references` impls move into companion stateless
   `IdeExtension` impls and the legacy hook can be removed from
   `CompilerExtension`. Tracked as "Phase 2".
3. **CLI `--classpath` consumption** for repo loading. The
   `[extension.<name>]` tables flow through to the evaluator (Phase 4
   wired `legend test`'s `extension_configs`), but the classpath's
   *repos* still aren't read by most subcommands. `legend test` falls
   back to `load_platform()` (embedded) unless `--live` is used;
   `legend run`, `legend check`, etc. behave similarly. Wire
   `resolve_classpath` outputs into model loading the same way
   `extension_configs` is now wired.
4. **Worked example crate** (`Phase 6`). `examples/mydsl-extension/`
   outside the workspace, demonstrating the full discovery path.
   CI-built so the recipe never goes stale.
5. **Repo descriptors + manifest** (`Phase B3`, existing BACKLOG P1).
   Independent of the discovery work — Java-Pure-style
   `repo.definition.json` schema replacing the hand-listed paths in
   `crates/core-platform-pure/build.rs`.

Until items 1 & 2 land, external consumers can still self-register
*stateless* `CompilerExtension`s, `SectionParser`s, `IslandParser`s,
`RuntimeExtension`s, and `DSLPopulator`s. Stateful
`CompilerExtension`s use the explicit
`compile_with_extensions(&chunks, &[&ext])` path.