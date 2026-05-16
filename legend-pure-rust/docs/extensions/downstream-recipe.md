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

> **Status:** the library-level extension SPI is stable. CLI-level
> wiring still requires you to build your own binary that calls
> `Evaluator::new_default_with_extensions(...)` directly — the stock
> `legend` CLI does not yet accept a `--extensions` flag. See
> [Gaps and roadmap](#gaps-and-roadmap) at the end.

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

In-tree templates this guide cites verbatim:

- `crates/store-relational-runtime/` — `RuntimeExtension`
- `crates/dsl-relational/src/parser.rs` — `SectionParser`
- `crates/dsl-relational/src/compiler.rs` — `CompilerExtension`
- `crates/dsl-relational-runtime/src/lib.rs` — `DSLPopulator`
- `crates/cli/src/commands/test.rs` — the runtime wiring exemplar (`NativeRegistry::with_extensions(&[&relational_ext])` + populator setup)

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
legend-pure-parser        = "<version>"   # ParserContext, SectionParser, IslandParser
legend-pure-parser-ast    = "<version>"   # DSLElement, Expression, SourceInfo
legend-pure-parser-pure   = "<version>"   # ValueSpec, type system
smol_str                  = "0.3"
```

Add `clap` + `miette` if you ship your own CLI binary (see §9).

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

**Wiring**: pass it through `NativeRegistry::with_extensions`:

```rust
let registry = NativeRegistry::with_extensions(&[&MyExtension]);
let mut evaluator = Evaluator::new_default(&model, &registry);
```

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
connection per evaluator. Process-wide state needs a different
mechanism (see `store-relational-runtime::set_extension_configs` for one
example).

> **Gap today**: the stock `legend` CLI hard-codes the in-tree
> extensions in `crates/cli/src/commands/test.rs`. To use your
> extension you must build your own CLI binary (§9). Tracked in the
> plan as item B2.

---

## 4. Adding a section grammar (`###MyDsl`)

Implement `SectionParser`. The trait requires `Send + Sync` because the
CLI parallelises file parsing via Rayon — keep your parser stateless
(zero-sized struct is ideal).

```rust
use legend_pure_parser::{ParserContext, SectionParser, error::ParseError};
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
use legend_pure_parser::{ParserContext, IslandParser, error::ParseError};
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
use legend_pure_parser::{parse_with_islands, parse_with_sections};

let islands: Vec<Box<dyn IslandParser>> = vec![Box::new(MyIslandParser)];
let sections: Vec<Box<dyn SectionParser>> = vec![Box::new(MyDslSectionParser)];

let (file, errors) = parse_with_sections(source, "file.pure", &islands, &sections);
```

> **Gap today**: there is no automatic discovery (no `inventory` /
> `linkme` registration). You construct these slices explicitly in
> your binary. Cited as a possible future enhancement, not on the
> short-term roadmap.

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

**Wiring**:

```rust
use legend_pure_pure::pipeline::compile_with_extensions;

let model = compile_with_extensions(
    chunks,
    &[&MyDslExtension],
)?;
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

> **Verify in B4**: confirm `PureModel` exposes a `pub` element-push
> API for external callers. If today's API is `pub(crate)`, the audit
> in plan item B4 promotes it.

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

Note the runtime walks populators in **chunk-element order**, so a
populator can depend on rows allocated by an earlier-running populator
in the same evaluator. The relational stack relies on this for
`RelationalClassMappingDSLPopulator` to find Table rows that
`RelationalDatabaseDSLPopulator` allocated first.

---

## 8. Wiring it together — the minimal custom binary

Until the stock `legend` CLI grows extension flags, you ship your own
binary. The full wiring lifts directly from
`crates/cli/src/commands/test.rs:316-340`:

```rust
use legend_pure_runtime::dsl::{run_populators, DSLPopulator};
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_parser::{parse_with_sections, parse_with_islands};
use legend_pure_pure::pipeline::compile_with_extensions;

fn main() -> anyhow::Result<()> {
    // 1. Parse with your section + island parsers.
    let sections = vec![Box::new(mydsl::MyDslSectionParser) as Box<_>];
    let islands  = vec![Box::new(mydsl::MyIslandParser)     as Box<_>];
    let (parsed_file, parse_errors) = parse_with_sections(
        &source, "input.pure", &islands, &sections,
    );

    // 2. Compile with your CompilerExtension.
    let model = compile_with_extensions(
        chunks_from(parsed_file),
        &[&mydsl::MyDslExtension],
    )?;

    // 3. Build a native registry that includes your extension.
    let registry = NativeRegistry::with_extensions(&[&mydsl::MyExtension]);

    // 4. Construct the evaluator and run your populators.
    let mut evaluator = Evaluator::new_default(&model, &registry);
    let populators: &[&dyn DSLPopulator] = &[&mydsl::MyDslPopulator];
    run_populators(&model, evaluator.heap_mut(), populators);

    // 5. Drive evaluation.
    let result = evaluator.evaluate_fqn("mydsl::tests::run__Boolean_1_", &[])?;
    println!("{:?}", result);
    Ok(())
}
```

> **Gap today**: every external consumer copies this skeleton. Plan
> item B2 will extract a `legend_pure_cli::run_with_extensions(...)`
> helper so your `main.rs` shrinks to ~10 lines and inherits the
> stock CLI's subcommand dispatch, classpath handling, and exit
> codes.

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

> **Gap today**: subcommand bodies still discard the resolved
> classpath (`crates/cli/src/commands/test.rs:186`: `let _ = classpath;`),
> so the stock CLI ignores it for most commands. Until plan item B1
> lands, build your own binary that calls
> `legend_cli::classpath::resolve_classpath` directly and feeds the
> result into `compile_with_extensions` and your evaluator setup.

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

> **Gap today**: there is no in-tree publishing template or
> "downstream extension cookiecutter". Once the example crate at
> `legend-pure-rust/examples/mydsl-extension/` lands (plan item B5)
> it serves as the runnable template.

---

## Stable API reference

The types below are public and form the de-facto downstream contract.
Breaking changes to these will land as `MAJOR` version bumps; everything
else is best-effort backwards-compatible.

| Crate | Public surface |
|---|---|
| `legend-pure-runtime` | `native::{NativeFunction, NativeRegistry, RuntimeExtension, ExtensionStateStore}`, `value::Value`, `eval::{Evaluator, EvalContextTrait, Evaluated}`, `heap::{RuntimeHeap, ObjectHandle}`, `dsl::{DSLPopulator, DSLPopulationCtx, run_populators}`, `error::{PureException, PureRuntimeError}` |
| `legend-pure-pure` | `model::{PureModel, Element, DSLInstance}`, `extension::{CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx, lower_and_infer_expression}`, `pipeline::compile_with_extensions`, `refs::Reference`, `types::{TypeExpr, Multiplicity, ResolvedType, Parameter}` |
| `legend-pure-parser` | `ParserContext`, `IslandParser`, `SectionParser`, `parse_with_islands`, `parse_with_sections`, `error::ParseError` |
| `legend-pure-parser-ast` | `dsl::DSLElement`, `island::IslandContent`, `expression::Expression`, `section::SourceFile`, `SourceInfo`, derive macros |
| `legend-pure-parser-pure` | `model::PureModel`, `types::{ValueSpec, ExprKind}` |

---

## Gaps and roadmap

Today's recipe leans on a custom binary because two CLI integration
items haven't landed yet. The Phase-B plan at
`~/.claude/plans/how-will-be-the-hashed-sparkle.md` sequences them:

1. **B1 — Consume the wired `--classpath` arg** (size S). Subcommand
   bodies stop discarding the resolved classpath and feed it into
   compile/load. Unblocks `legend test --classpath …` for downstream
   repos that ship `.purem` snapshots.
2. **B2 — Factor out `legend_pure_cli::run_with_extensions(...)`**
   (size L). Lets your `main.rs` shrink to ~10 lines and inherit
   subcommand dispatch.
3. **B3 — Repo descriptors + manifest** (size M, already BACKLOG P1).
   Replaces hand-listed paths with a Java-Pure-style
   `repo.definition.json` schema.
4. **B4 — Visibility audit** (size XS). Resolves the duplicate
   `parse_qualified_name` declaration at
   `crates/parser/src/parser/mod.rs:324` / `:434` and confirms every
   `pub(crate)` symbol the example crate reaches for is promoted.
5. **B5 — Worked example crate** (size M). A runnable
   `examples/mydsl-extension/` that this recipe cross-references and
   that CI builds, so the recipe never goes stale.

Until those land, the in-tree DSLs (`crates/dsl-relational/`,
`crates/dsl-mapping/`, `crates/dsl-diagram/`) are the only source of
truth for the extension surface, and external consumers walk the same
trait shapes from a sibling crate.