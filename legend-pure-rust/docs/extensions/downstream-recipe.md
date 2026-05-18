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

> **Status:** the extension story is structurally complete. Each
> trait — `RuntimeExtension`, `CompilerExtension`, `SectionParser`,
> `IslandParser`, `DSLPopulator`, `IdeExtension` — has a `linkme`
> distributed slice; your extension annotates a `static` next to its
> trait impl and any binary that pulls your crate in as a Cargo
> dependency picks it up automatically. Per-compile state lives in
> [`CompileExtensionScope`](#stateless-self-registration--per-compile-state)
> threaded through the ctx, so your extension type stays `Sync` and
> self-registers. The runnable
> [`examples/mydsl-extension/`](../../examples/mydsl-extension/)
> template proves the full path end-to-end.

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

## 1. The six extension surfaces

Pick the surfaces you need; they're independent and you can implement
one without touching the others.

| Surface | Trait | Crate path | When you need it |
|---|---|---|---|
| Native functions | `RuntimeExtension` | `crates/runtime/src/native.rs` | Pure code calls a function whose body is Rust |
| Compiler hooks | `CompilerExtension` | `crates/pure/src/extension.rs` | Your DSL contributes elements / validates references / participates in type-checking |
| Section grammar | `SectionParser` | `crates/parser/src/section_parser.rs` | You introduce `###MyDsl` blocks |
| Island grammar | `IslandParser` | `crates/parser/src/island.rs` | You embed `#tag{ … }#` micro-syntaxes inside Pure expressions |
| Heap hydration | `DSLPopulator` | `crates/runtime/src/dsl.rs` | Your DSL stores data in `Element::DSLInstance` and you want it reflected on the runtime heap |
| IDE references | `IdeExtension` | `crates/ide/src/lib.rs` | Your DSL surfaces clickable reference regions for goto-def / find-usages |

A native-only extension (no new syntax, just new functions) needs only
`RuntimeExtension`. A typical full DSL implements
`SectionParser` + `CompilerExtension` + `DSLPopulator`, and ships a
sibling `RuntimeExtension` if it adds natives. Add `IdeExtension`
when you want IDE-side reference navigation.

In-tree templates this guide cites verbatim:

- `crates/store-relational-runtime/src/extension.rs` — `RuntimeExtension`
  with `#[distributed_slice(RUNTIME_EXTENSIONS)]`
- `crates/dsl-relational/src/parser.rs` — `SectionParser`
- `crates/dsl-relational/src/compiler.rs` — `CompilerExtension`
  (declare / define_bodies / validate using `ctx.scope` for
  per-compile state) + `RelationalIdeExtension` (`IdeExtension`
  reading from `model.compile_scope` post-compile)
- `crates/dsl-mapping/src/compiler.rs` — same pattern for Mapping,
  including the canonical `MappingIdeExtension`
- `crates/dsl-mapping-runtime/src/lib.rs` and
  `crates/dsl-relational-runtime/src/lib.rs` — `DSLPopulator` with
  `#[distributed_slice(DSL_POPULATORS)]`
- `crates/cli/src/main.rs` + `crates/cli/src/lib.rs` — the canonical
  force-link recipe and the `legend_cli::main_entry()` shape
- **`examples/mydsl-extension/`** — a runnable end-to-end template
  (outside the parent workspace, just like a real downstream
  consumer would build). Six unit-struct extensions covering every
  surface, with `mydsl-legend` binary wrapping
  `legend_cli::main_entry()`. Fork this directory to start your own
  DSL crate.

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
    └── mydsl-legend.rs   # custom CLI binary (see §8)
```

`Cargo.toml` dependency floor:

```toml
[dependencies]
legend-pure-runtime       = "<version>"   # NativeRegistry, Evaluator, DSLPopulator
legend-pure-parser-pure   = "<version>"   # CompilerExtension, PureModel, Element, types
legend-pure-parser-parser = "<version>"   # ParserContext, SectionParser, IslandParser
legend-pure-parser-ast    = "<version>"   # DSLElement, Expression, SourceInfo
legend-pure-ide           = "<version>"   # IdeExtension, IDE_EXTENSIONS, Reference
smol_str                  = "0.3"
linkme                    = "0.3"          # distributed-slice self-registration
```

Add `legend-cli` if you want to inherit its `main_entry()` dispatcher
(recommended — see §8). Add `clap` + `miette` only if you write a
bespoke binary.

> **Today**: these crates are not yet published to crates.io. While you
> build against this repo, use `path = "../legend-pure-rust/crates/<name>"`
> dependencies. The runnable [`examples/mydsl-extension/`](../../examples/mydsl-extension/)
> template uses this shape verbatim — copy its `Cargo.toml` for
> reference.

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

All hooks default to no-op, so override only what you contribute.
IDE-side reference contribution lives on a separate `IdeExtension`
trait in the `legend-pure-ide` crate (see §7's companion subsection).

```rust
use legend_pure_parser_pure::extension::{
    CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx,
};
use legend_pure_parser_pure::model::{Element, DSLInstance};

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

### Stateless self-registration + per-compile state

The `COMPILER_EXTENSIONS` distributed slice stores
`&'static (dyn CompilerExtension + Sync)` — so your extension type
must be `Sync`. The cleanest shape is a unit struct:

```rust
use legend_pure_parser_pure::extension::COMPILER_EXTENSIONS;
use linkme::distributed_slice;

#[derive(Debug, Default)]
pub struct MyDslExtension;

#[distributed_slice(COMPILER_EXTENSIONS)]
static MY_DSL_EXT: &(dyn CompilerExtension + Sync) = &MyDslExtension;
```

**Per-compile state** (the data your `declare` accumulates and
`validate` consumes) doesn't live on the struct — it goes in
`ctx.scope`, the per-compile [`CompileExtensionScope`] arena
threaded through every hook:

```rust
#[derive(Default)]
struct MyDslCompileState {
    elements: HashMap<SmolStr, MyDef>,
}

impl CompilerExtension for MyDslExtension {
    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let state = ctx
            .scope
            .as_deref_mut()
            .expect("scope wired by pipeline")
            .get_or_default::<MyDslCompileState>();
        state.elements.insert(fqn, def);
    }

    fn validate(&self, ctx: &mut ValidateCtx<'_>) {
        let Some(scope) = ctx.scope else { return };
        let Some(state) = scope.get::<MyDslCompileState>() else { return };
        for (fqn, def) in &state.elements { /* validate */ }
    }
}
```

`CompileExtensionScope` is type-id-keyed and `Send + Sync`-bound,
so your state type must be plain data (no `RefCell` / `Rc`).
[`MappingExtension`](../../crates/dsl-mapping/src/compiler.rs) and
[`RelationalExtension`](../../crates/dsl-relational/src/compiler.rs)
are the canonical in-tree references — both fully migrated and
self-registering.

[`CompileExtensionScope`]: ../../crates/pure/src/extension.rs

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
use legend_pure_parser_pure::extension::lower_and_infer_expression;
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

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
use legend_pure_parser_pure::pipeline::{compile, compile_with_extensions};

// Production (default): discovers + topo-sorts every CompilerExtension
// registered via #[distributed_slice(COMPILER_EXTENSIONS)] in linked crates.
let model = compile(chunks, &auto_imports)?;

// Tests / hand-composed extension sets: explicit slice.
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

### 7.1 IDE references via `IdeExtension`

IDE-side reference contribution (clickable goto-def / find-usages
regions) is a separate trait in the [`legend-pure-ide`](../../crates/ide/)
crate. Implementations are unit structs that read your DSL's
per-compile state out of `model.compile_scope` post-compile and
push one `Reference` per source-level reference site:

```rust
use legend_pure_ide::{IDE_EXTENSIONS, IdeExtension, Reference};
use legend_pure_parser_pure::model::PureModel;
use linkme::distributed_slice;

#[derive(Debug, Default)]
pub struct MyDslIdeExtension;

impl IdeExtension for MyDslIdeExtension {
    fn name(&self) -> &'static str { "mydsl-ide" }

    fn walk_references(&self, model: &PureModel, visit: &mut dyn FnMut(Reference)) {
        let Some(state) = model.compile_scope.get::<MyDslCompileState>() else { return };
        for (fqn, def) in &state.elements {
            // visit(Reference { range, kind, target_element, target });
        }
    }
}

#[distributed_slice(IDE_EXTENSIONS)]
static MY_DSL_IDE: &dyn IdeExtension = &MyDslIdeExtension;
```

Template: [`crates/dsl-mapping/src/compiler.rs`](../../crates/dsl-mapping/src/compiler.rs)
(`MappingIdeExtension`) and the analogous
`RelationalIdeExtension` in `crates/dsl-relational/src/compiler.rs`.
`build_reference_index(&model)` walks the discovered slice
automatically.

---

## 8. Wiring it together — the minimal custom binary

With every trait surface self-registering, the consumer binary is one
helper call plus the **link-forcing imports** that ensure the linker
doesn't drop your extension crates. The shape below is the actual,
working shape from
[`examples/mydsl-extension/src/main.rs`](../../examples/mydsl-extension/src/main.rs):

```rust
// src/main.rs

// 1. Force-link your extension crate(s). Without these `use` lines
//    the linker is free to drop the entire crate object file (no
//    reachable code → no contribution to the distributed slices).
//    One `use … as _;` per extension crate is enough.
#[allow(unused_imports)]
use mydsl::prelude::MyDslRuntimeExtension as _;
#[allow(unused_imports)]
use mydsl::prelude::MyDslCompilerExtension as _;
#[allow(unused_imports)]
use mydsl::prelude::MyDslSectionParser as _;
#[allow(unused_imports)]
use mydsl::prelude::MyDslIslandParser as _;
#[allow(unused_imports)]
use mydsl::prelude::MyDslPopulator as _;
#[allow(unused_imports)]
use mydsl::prelude::MyDslIdeExtension as _;

fn main() {
    // 2. Hand off to the stock CLI dispatcher; everything else flows
    //    through discovery.
    legend_cli::main_entry();
}
```

That's it. The stock `legend` CLI's command set (`parse`, `check`,
`test`, `run`, `repl`, `snapshot`, …) all dispatch through
`NativeRegistry::discovered()`, `pipeline::compile()` (discovered
`CompilerExtension`s), `parse()` (discovered parsers), and
`Evaluator::builder().build()` (discovered populators + extension
configs from `--classpath`). Your extension flows into all of them
automatically.

> **Run the template yourself:**
> ```bash
> cd legend-pure-rust/examples/mydsl-extension
> cargo test                                # 6/6 discovery smoke tests
> cargo run --bin mydsl-legend -- version   # the wrapped CLI works
> ```

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

Three shapes, depending on how your downstream consumes Pure:

- **Cargo-install a custom CLI binary** —
  `cargo install --bin mydsl-legend --path .` and downstream users
  invoke `mydsl-legend test`, `mydsl-legend run …` exactly like
  `legend`. The binary contains your extensions, the
  `legend-pure-*` crates, and any platform `.pure` source you embed.
  The runnable template is
  [`examples/mydsl-extension/`](../../examples/mydsl-extension/) —
  fork it for your own project.
- **Library crate consumed by an embedding app** — publish your
  extension crate as a library; embedders build their own Rust
  binary that pulls your types and force-links them. The embedder's
  binary wraps `legend_cli::main_entry()` (or builds an `Evaluator`
  directly for embedded use).
- **JNI cdylib for Java consumers** — see §11.1 below.

### 11.1 JNI / Java consumers

When the downstream is a Java service (legend-engine, an SDK client,
etc.) that loads a Pure evaluator via JNI, the same discovery model
applies — the wrapping shape just shifts from a Rust binary to a
Rust **cdylib** that depends on `legend-pure-parser-jni` as an rlib
and emits its own `lib<yourname>_pure_jni.{dylib,so,dll}`. Java
consumers `System.loadLibrary("<yourname>_pure_jni")` instead of
`pure_rust_jni`; same `Java_org_finos_legend_pure_rust_…` symbol
table, plus your extensions discovered at evaluator construction.

The runnable template is
[`examples/mydsl-jni-extension/`](../../examples/mydsl-jni-extension/);
copy that directory as a starting point. The shape:

```toml
# downstream-jni/Cargo.toml
[lib]
name = "mydsl_pure_jni"
crate-type = ["cdylib"]

[dependencies]
legend-pure-parser-jni      = { version = "<version>" }   # in-tree JNI surface (rlib)
legend-pure-mydsl-extension = { version = "<version>" }   # your extension crate
jni                         = "0.21"                       # for the forwarder signatures
```

```rust
// downstream-jni/src/lib.rs

// (1) Force-link the extension crate so its distributed-slice
// statics reach this cdylib's link graph.
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslRuntimeExtension as _;
// … one `use … as _;` per extension surface …

// (2) Forwarder `#[no_mangle] pub extern "system" fn Java_*` per
// upstream entry point. Each forwarder is a one-line delegating
// call. Without these, rustc's cdylib build DCE drops the rlib's
// `Java_*` symbols entirely — `nm` would show zero. With them, the
// downstream cdylib's symbol table matches the upstream's exactly.
use jni::JNIEnv;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::jlong;

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext<
    'local,
>(env: JNIEnv<'local>, class: JClass<'local>) -> jlong {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext(env, class)
}
// … 6 more forwarders for the remaining Java_* entries; see
// `examples/mydsl-jni-extension/src/lib.rs` for all 7 …
```

#### Why forwarders, not `pub use upstream::*;`?

Rustc's default cdylib build dead-code-eliminates unreachable rlib
code. Plain `pub use legend_pure_parser_jni::*;` and even
`-Wl,-exported_symbol,_Java_*` linker-flag tricks leave the cdylib
with **zero** `Java_*` symbols — `nm` shows nothing because the
upstream's no-mangle items aren't reachable from any cdylib-owned
code at all. The forwarder pattern — one downstream-owned
`#[no_mangle] pub extern "system" fn Java_*` per entry — is the
canonical Rust-JNI cdylib-on-rlib idiom and the one shape that
robustly preserves the symbol table.

A pleasant side-effect: once *any* forwarder establishes a real
call into the upstream rlib, additional upstream `#[no_mangle]`
items in the same crate ship for free. The
`examples/mydsl-jni-extension/` template writes 7 forwarders and
ships 8 symbols — the bonus `Java_*_nativeGenerateBindings` from a
different upstream module rides along.

#### Verify

```bash
cd examples/mydsl-jni-extension
cargo build --release
nm -gU target/release/libmydsl_pure_jni.dylib | grep ' _Java_' | wc -l
# Should match the stock libpure_rust_jni.dylib for the same target.
```

The example crate's `tests/symbols.rs` automates this check —
fails loudly with the missing symbol list if a forwarder regresses.

> **Windows**: prefix-based symbol exports aren't a thing in MSVC,
> but the `#[no_mangle] pub extern "system" fn` forwarder pattern
> Just Works on `x86_64-pc-windows-msvc` — no `.def` file needed.
> Verify with `dumpbin /EXPORTS mydsl_pure_jni.dll`.

#### Classpath as a byte array (`nativeInitContextWithClasspath`)

The `Java_*_nativeInitContext` entry above loads only the embedded
platform — no classpath, no extension configs. For downstream Java
consumers that ship their own `legend-pure-classpath.toml` (typically
inside the JAR under `META-INF/`), the bridge exposes a second
init entry that takes the TOML as a `byte[]`:

```java
byte[] toml = MyApp.class
    .getResourceAsStream("META-INF/legend-pure-classpath.toml")
    .readAllBytes();
long ctx = PureRustEvaluator.nativeInitContextWithClasspath(toml);
```

No file is read from disk for the TOML itself — the bytes ARE the
TOML. On the Rust side, [`compile_classpath_bytes`] decodes the bytes
zero-copy via `std::str::from_utf8` (the `toml` crate exposes only
`from_str(&str)` since TOML is UTF-8 text), parses, merges with the
embedded platform, runs `repo::load`, and constructs a [`JniContext`]
with `evaluator.set_extension_configs(...)` so
`[extension.<domain>.<engine>]` tables from the TOML reach the
evaluator (H2 backend ports, lake credentials, … — without the old
`OnceLock` global).

Both init entries — the parameterless `nativeInitContext` and this
bytes-mode `nativeInitContextWithClasspath` — share the same registry
shape: `NativeRegistry::discovered()`. Every linked
`#[distributed_slice(RUNTIME_EXTENSIONS)]` contribution is active
regardless of which init path Java picks, so downstream cdylibs built
via the `mydsl-jni-extension` forwarder pattern can use either entry
and get their extensions. The remaining difference between the two:
bytes-mode pre-seeds the evaluator's extension configs from the TOML;
the parameterless entry starts with an empty config map.

**Self-contained TOML contract.** Bytes-mode has no on-disk source to
derive a base from, so the TOML must be self-describing — every
`[[repo]] path = "..."` must be **absolute**, OR the TOML must set
`root = "/abs/path"` at the top. Relative paths without a TOML `root`
resolve against the filesystem root (`/`) and almost always fail with
a clear `PathMissing` error. Two patterns Java consumers use to keep
the bytes self-contained:

```toml
# Pattern A — TOML-level root override
root = "/var/lib/myapp/pure"

[[repo]]
name = "platform"
kind = "purem"
path = "platform.purem"   # resolves to /var/lib/myapp/pure/platform.purem

# Pattern B — absolute paths per entry
[[repo]]
name = "platform"
kind = "purem"
path = "/var/lib/myapp/pure/platform.purem"
```

Both make the classpath byte-loadable without any side-channel.

The signature deliberately does NOT accept a `root: &Path` parameter:
passing both bytes AND a path would imply a hybrid model where Java
extracts the TOML to disk anyway, in which case the existing file-
loading path (`legend_cli::classpath::load_classpath`) is the honest
API. Bytes mode is for "TOML is fully self-describing" usage.

Forwarder for the downstream cdylib (mirror in
[`examples/mydsl-jni-extension/src/lib.rs`](../../examples/mydsl-jni-extension/src/lib.rs)):

```rust
use jni::objects::JByteArray;

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContextWithClasspath<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    classpath_bytes: JByteArray<'local>,
) -> jlong {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContextWithClasspath(
        env,
        class,
        classpath_bytes,
    )
}
```

The example crate's `EXPECTED_SYMBOLS` (in `tests/symbols.rs`) now
includes this entry; downstream cdylibs that copy the template and
don't add the forwarder will fail their own symbol-parity test at
`cargo test` time.

[`compile_classpath_bytes`]: ../../crates/core-platform-pure/src/classpath.rs

---

## Stable API reference

The types below are public and form the de-facto downstream contract.
Breaking changes to these will land as `MAJOR` version bumps; everything
else is best-effort backwards-compatible.

| Crate | Public surface |
|---|---|
| `legend-pure-runtime` | `native::{NativeFunction, NativeRegistry, RuntimeExtension, RUNTIME_EXTENSIONS, ExtensionStateStore}`, `value::Value`, `eval::{Evaluator, EvalContextTrait, Evaluated}`, `builder::EvaluatorBuilder`, `heap::{RuntimeHeap, ObjectHandle}`, `dsl::{DSLPopulator, DSLPopulationCtx, DSL_POPULATORS, discovered_populators, run_populators}`, `error::{PureException, PureRuntimeError}` |
| `legend-pure-parser-pure` | `model::{PureModel, Element, DSLInstance}`, `extension::{CompilerExtension, COMPILER_EXTENSIONS, discovered_compiler_extensions, CompileExtensionScope, DeclareCtx, DefineCtx, ValidateCtx, lower_and_infer_expression}`, `pipeline::{compile, compile_with_extensions}`, `types::{TypeExpr, Multiplicity, ResolvedType, Parameter, ValueSpec, ExprKind}` |
| `legend-pure-ide` | `IdeExtension`, `IDE_EXTENSIONS`, `discovered_ide_extensions`, `Reference`, `RefKind`, `ReferenceIndex`, `build_reference_index`, `build_reference_index_with_ide_extensions`, `walk_references` |
| `legend-pure-parser-parser` | `ParserContext`, `IslandParser`, `ISLAND_PARSERS`, `discovered_island_parsers`, `SectionParser`, `SECTION_PARSERS`, `discovered_section_parsers`, `parse`, `parse_with_islands`, `parse_with_sections`, `error::ParseError` |
| `legend-pure-parser-ast` | `dsl::DSLElement`, `island::IslandContent`, `expression::Expression`, `section::SourceFile`, `SourceInfo`, derive macros |
| `legend-pure-core-platform` | `platform::{load_platform, PLATFORM_AUTO_IMPORTS}`, `repo::{Repo, RepoMeta, load}`, `classpath::{Classpath, ResolvedClasspath, ClasspathError, parse_classpath_toml, merge_with_embedded, compile_classpath_bytes}` (the last is the JNI byte-array entry point — see §11.1) |
| `legend-cli` | `main_entry`, `classpath::{resolve_classpath, load_classpath, synthetic_from_snapshots_dir, discover_classpath}` (CLI cascade on top of the shared parser), `diagnostics::CliError` |
| `legend-pure-parser-jni` | `Java_*_nativeInitContext`, `Java_*_nativeInitContextWithClasspath` (byte-array classpath, §11.1), `Java_*_nativeEvaluate`, `Java_*_nativeGetProperty`, `Java_*_nativeGetClassifier`, `Java_*_nativeNew`, `Java_*_nativeFreeContext`, `Java_*_nativeFreeInstance` |

---

## Gaps and roadmap

The extension story is structurally complete and ships every
documented shape end-to-end:

- **Discovery** — every plug-in trait carries a `linkme`
  distributed slice; downstream crates self-register by annotating
  one `static` per impl (Phase 1).
- **In-tree DSL migration** — `dsl-diagram`, `dsl-mapping`,
  `dsl-relational`, `dsl-graph`, `dsl-store`, `dsl-tds`,
  `dsl-path`, and `store-relational` all self-register through
  the slice mechanism (Phase 3 LIGHT + FULL).
- **IDE separation** — `walk_references` is owned by a dedicated
  `IdeExtension` trait in the `legend-pure-ide` crate, so a DSL
  can ship compiler hooks without an IDE participation and vice
  versa (Phase 2 + 2.5).
- **Per-evaluator state + configs** — `CompileExtensionScope`
  threads stateful extensions through ctx without `Sync` bounds;
  `Evaluator::builder().extension_configs(...)` replaced the old
  `OnceLock` global in `store-relational-runtime` (Phase 4).
- **CLI classpath integration** — `test` / `run` / `repl` /
  `snapshot` compile the classpath's repo set via
  `repo::load(resolved.repos, …)` when `--classpath` is explicit;
  `--live` / `--watch` / `--platform-dir` become no-ops with a
  stderr warning (Phase 5).
- **JNI distribution** — `legend-pure-parser-jni` ships as a dual
  `["rlib", "cdylib"]` crate; downstream cdylibs depend on it as
  an rlib + use the [`mydsl-jni-extension`](../../examples/mydsl-jni-extension/)
  forwarder pattern to produce their own
  `lib<name>_pure_jni.{dylib,so,dll}`. The
  `nativeInitContextWithClasspath(byte[])` entry point compiles
  classpath TOML supplied directly from JAR resources without
  filesystem coordination (§11.1).
- **Worked examples** — [`examples/mydsl-extension/`](../../examples/mydsl-extension/)
  and [`examples/mydsl-jni-extension/`](../../examples/mydsl-jni-extension/)
  prove the recipe end-to-end and are CI-built.

The extension API and CLI / JNI distribution paths all work today —
the structural shape this recipe describes is the structural shape
downstream consumers will use post-publication. Open items beyond
the recipe scope (incremental compilation, body parallelization,
full PCT native coverage, etc.) live in
[`BACKLOG.md`](../../BACKLOG.md).