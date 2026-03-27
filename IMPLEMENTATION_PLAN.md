# Rust Pure Parser — Implementation Plan

## Goal

Build a **Rust-based Pure grammar parser** using an **AST-first, TDD-driven** approach. Before any parser code is written, we will: (1) agree on Rust coding practices & tooling, (2) build a strong AST foundation designed for the future compiler, and (3) establish a comprehensive test suite derived from the existing Java grammar tests.

---

## Pillar 1: Coding Practices, Tooling & Project Setup

### Rust Coding Standards

| Practice | Convention |
|----------|-----------|
| **Edition** | Rust 2024 (`edition = "2024"`) |
| **Formatting** | `rustfmt` — enforced in CI, zero-tolerance |
| **Linting** | `clippy` with `#![warn(clippy::all, clippy::pedantic)]` — warnings = errors in CI |
| **Error handling** | `thiserror` for library error types; no `unwrap()`/`expect()` in library code |
| **Naming** | `snake_case` functions/variables, `PascalCase` types, `SCREAMING_SNAKE` constants |
| **Visibility** | Default to `pub(crate)`; only `pub` what's part of the public API |
| **Documentation** | `/// doc comments` on all public items; `#![deny(missing_docs)]` on public crates |
| **Testing** | `#[cfg(test)] mod tests` for unit tests; `tests/` for integration tests |
| **Unsafe** | `#![forbid(unsafe_code)]` on all crates except `jni` bridge |
| **Dependencies** | Minimize — every dependency needs clear justification |

### Code Coverage

| Tool | Purpose |
|------|---------|
| [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) | LLVM source-based coverage instrumentation |
| `llvm-tools-preview` | Rustup component for LLVM coverage tools |

**Target: >90% line coverage across all crates (excluding JNI bridge).**

```bash
# Install
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov

# Run coverage
cargo llvm-cov --workspace --lcov --output-path lcov.info

# Generate HTML report
cargo llvm-cov --workspace --html --output-dir coverage/

# CI gate: fail below 90%
cargo llvm-cov --workspace --fail-under-lines 90
```

Coverage configuration in `Cargo.toml` workspace:
```toml
[workspace.metadata.llvm-cov]
# Exclude JNI bridge from coverage (FFI boundary, tested via Java integration)
exclude = ["legend-pure-parser-jni"]
```

### Library Choices

| Purpose | Library | Rationale |
|---------|---------|-----------|
| **String interning** | [`smol_str`](https://crates.io/crates/smol_str) | 24-byte inline strings; `Clone` is `O(1)` |
| **Error types** | [`thiserror`](https://crates.io/crates/thiserror) | Zero-cost derive for `std::error::Error` |
| **JSON serialization** | [`serde`](https://crates.io/crates/serde) + [`serde_json`](https://crates.io/crates/serde_json) | Protocol crate only — **not** in AST |
| **JNI bridge** | [`jni`](https://crates.io/crates/jni) (0.21+) | De facto Rust-JNI bindings |
| **Plugin discovery** | [`linkme`](https://crates.io/crates/linkme) | Distributed slices for link-time plugin registration |
| **Snapshot testing** | [`insta`](https://crates.io/crates/insta) | Snapshot/golden file testing; `--review` workflow |
| **Benchmarking** | [`criterion`](https://crates.io/crates/criterion) | Statistical benchmarking vs ANTLR4 |
| **Code coverage** | [`cargo-llvm-cov`](https://crates.io/crates/cargo-llvm-cov) | LLVM source-based coverage; CI gate at 90% |
| **Tracing** | [`tracing`](https://crates.io/crates/tracing) + [`tracing-subscriber`](https://crates.io/crates/tracing-subscriber) | Structured, span-based diagnostics; parser rule spans; `log` compatibility |

### Logging & Tracing Strategy

We use the **`tracing`** crate (not `log`) as our diagnostics facade. `tracing` provides **structured spans** that map naturally to parser grammar rules, giving deep observability into the parse flow.

#### Why `tracing` over `log`

| Concern | `log` | `tracing` |
|---------|-------|-----------|
| Discrete events | ✅ | ✅ |
| Structured fields | Limited | ✅ Native key-value pairs |
| Spans (enter/exit) | ❌ | ✅ Grammar rule boundaries |
| Async-aware | ❌ | ✅ |
| `log` compatibility | N/A | ✅ via `tracing-log` bridge |

#### Crate-Level Instrumentation Rules

| Crate | What to instrument | Level |
|-------|--------------------|-------|
| **ast** | None — pure data, no behavior | — |
| **lexer** | Token emission, error recovery | `trace` for each token; `debug` for tokenizer state transitions |
| **parser** | Grammar rule entry/exit, dispatch decisions | `debug` span per rule; `trace` for token consumption |
| **protocol** | AST ↔ JSON conversion | `debug` per element conversion |
| **jni** | JNI call entry/exit, error propagation | `info` for parse calls; `error` for failures |
| **plugins** | Plugin dispatch, island parsing | `debug` for dispatch; `trace` for content |

#### Parser Span Pattern

Each grammar rule produces a `tracing` span, giving a structured call tree:

```rust
use tracing::{debug, trace, instrument};

impl Parser<'_> {
    #[instrument(skip(self), fields(element_type = "class"))]
    fn parse_class(&mut self) -> Result<ClassDef, ParseError> {
        debug!(name = %self.peek_identifier(), "parsing class");
        // ...
        let props = self.parse_properties()?;
        trace!(count = props.len(), "parsed properties");
        // ...
    }

    #[instrument(skip(self), level = "trace")]
    fn parse_properties(&mut self) -> Result<Vec<Property>, ParseError> {
        // each property parsed under this span
    }
}
```

**Runtime output** (with `RUST_LOG=legend_pure_parser=debug`):
```
DEBUG parse_class{element_type="class"}: legend_pure_parser::parser: parsing class, name="Person"
 TRACE parse_properties{}: legend_pure_parser::parser: parsed properties, count=3
```

#### Subscriber Initialization

**Library crates never initialize a subscriber.** Only the JNI bridge or a binary entrypoint does:

```rust
// crates/jni/src/lib.rs — JNI_OnLoad
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // Respects RUST_LOG env var; defaults to warn
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn"))
        )
        .try_init();
}
```

> [!NOTE]
> In production (JNI path), tracing defaults to `warn` — zero overhead for `debug`/`trace` spans. Developers enable verbose tracing via `RUST_LOG=legend_pure_parser=debug` for debugging.

#### Performance Guardrails

- **`trace`-level spans are compiled out** in release builds unless explicitly enabled
- Use `#[instrument(level = "trace")]` for hot paths (token consumption, expression recursion)
- Use `#[instrument(level = "debug")]` for grammar rule entry/exit
- **Never log at `info` or above in hot paths** — `info` is reserved for JNI call boundaries only
- Benchmark with tracing enabled vs disabled to ensure <1% overhead at `warn` level

### Project Documentation

The workspace must include these files for both human developers and AI agents:

#### [NEW] `legend-pure-parser/README.md`

Documents:
- **What this project is** — Rust Pure grammar parser replacing Java/ANTLR4
- **Architecture overview** — layered crate diagram with one-sentence descriptions
- **Quick start** — `cargo build`, `cargo test`, `cargo llvm-cov`
- **Crate map** — table of each crate with purpose, key types, and dependencies
- **Development guide** — how to add a new element type, new expression type, new plugin
- **Testing guide** — how to run tests, update snapshots, generate coverage reports

#### [NEW] `legend-pure-parser/ARCHITECTURE.md`

Deeper architecture reference for AI agents and new developers:
- **Crate dependency graph** (Mermaid)
- **Key traits** — `IslandPlugin`, `SectionPlugin`, `SubParser`, `Emit` with when/how to implement
- **Key macros** — what each declarative/derive macro does and when to use it
- **AST design principles** — why AST ≠ Protocol JSON, how expressions are structured
- **Token → AST flow** — worked example showing source → tokens → AST → JSON for a simple class
- **Decision log** — key technical choices and their rationale (e.g., "why `SmolStr` not `String`")

#### [NEW] `legend-pure-parser/CONTRIBUTING.md`

- Code style rules (link to `rustfmt.toml`, `clippy` config)
- PR checklist: tests, coverage, docs, `cargo fmt`, `cargo clippy`
- How to add a new grammar feature end-to-end

#### [NEW] Per-crate `README.md`

Each crate (`ast/`, `lexer/`, `parser/`, `protocol/`, `jni/`) gets a short `README.md` with:
- Purpose and key types
- Examples of usage
- Links to related crates

---

## Pillar 2: AST-First Design

### Design Philosophy

1. **Parser efficiency** — natural recursive-descent output, not shaped by serialization
2. **Compiler readiness** — types a future Rust compiler consumes directly
3. **Type safety** — Rust enums with data, not stringly-typed maps
4. **Immutability** — constructed once, never mutated after parse

> [!IMPORTANT]
> **No `serde` in the AST crate.** The AST is a pure data model with zero serialization concerns. Only the protocol crate depends on `serde`/`serde_json`.

### Trait & Macro Patterns

#### Core Traits

```rust
/// All AST nodes that carry source location information.
pub trait Spanned {
    fn source_info(&self) -> &SourceInfo;
}

/// All top-level elements that live in a package.
pub trait Packageable: Spanned {
    fn package(&self) -> &PackagePath;
    fn name(&self) -> &Identifier;
    fn path(&self) -> String {  // default impl
        format!("{}::{}", self.package(), self.name())
    }
}

/// Elements that can carry stereotypes and tagged values.
pub trait Annotated {
    fn stereotypes(&self) -> &[StereotypePtr];
    fn tagged_values(&self) -> &[TaggedValue];
}

/// Visitor pattern for AST traversal.
/// Enables compiler passes, linters, and protocol converters to walk the tree.
pub trait ElementVisitor {
    fn visit_class(&mut self, class: &ClassDef);
    fn visit_enum(&mut self, enum_def: &EnumDef);
    fn visit_function(&mut self, func: &FunctionDef);
    fn visit_profile(&mut self, profile: &ProfileDef);
    fn visit_association(&mut self, assoc: &AssociationDef);
    fn visit_measure(&mut self, measure: &MeasureDef);
    fn visit_extension(&mut self, ext: &ExtensionElement);
}

pub trait ExpressionVisitor {
    type Output;
    fn visit_literal(&mut self, lit: &Literal) -> Self::Output;
    fn visit_variable(&mut self, var: &Variable) -> Self::Output;
    fn visit_function_application(&mut self, app: &FunctionApplication) -> Self::Output;
    fn visit_lambda(&mut self, lambda: &Lambda) -> Self::Output;
    // ... one method per Expression variant
}
```

#### Derive & Declarative Macros

```rust
/// Derive macro: auto-implements Spanned for any struct with a `source_info` field.
#[derive(Spanned)]
pub struct ClassDef {
    pub source_info: SourceInfo,
    // ...
}

/// Derive macro: auto-implements Packageable for structs with `package` and `name`.
#[derive(Packageable)]
pub struct ClassDef {
    pub package: PackagePath,
    pub name: Identifier,
    pub source_info: SourceInfo,
    // ...
}

/// Derive macro: auto-implements Annotated for structs with stereotypes/tagged_values.
#[derive(Annotated)]
pub struct ClassDef {
    pub stereotypes: Vec<StereotypePtr>,
    pub tagged_values: Vec<TaggedValue>,
    // ...
}
```

> [!NOTE]
> We start with manual `impl` blocks. If the pattern becomes repetitive across 3+ types, we extract a derive macro. The derive macros live in a `legend-pure-parser-macros` proc-macro crate.

#### Builder Pattern for Tests

```rust
/// Fluent builder for constructing AST nodes in tests
/// (not used in production parsing — parser constructs directly)
impl ClassDef {
    pub fn builder(name: &str) -> ClassDefBuilder { ... }
}

// Usage in tests:
let class = ClassDef::builder("Person")
    .package("model::domain")
    .property("name", "String", Multiplicity::one())
    .stereotype("temporal", "businesstemporal")
    .build();
```

### Key AST Design Decision: Support Type Parameters

> [!IMPORTANT]
> **Divergence from Java:** The current Java parser rejects type parameters (`Class X<T>{}`) with an error. In the Rust parser, we **parse and preserve** type parameters in the AST. This enables future compiler support for generic types without re-parsing. The validation layer (not the parser) can optionally reject them if backward compatibility with the Java engine is required.

### AST Types (Summary)

#### Core Infrastructure

| Type | Purpose |
|------|---------|
| `Identifier` | Alias for `SmolStr` — interned, cheap to clone |
| `PackagePath` | `Vec<Identifier>` — e.g., `meta::pure::profiles::doc` |
| `SourceInfo` | Line/column/offset tracking |
| `TypeReference` | Path + type arguments (`<...>`) + type variable values (`(...)`) |
| `Multiplicity` | `lower: u32`, `upper: Option<u32>` (None = `*`) |

#### Elements

| Type | Fields of Note |
|------|----------------|
| `ClassDef` | `type_parameters`, `super_types`, `properties`, `qualified_properties`, `constraints` |
| `EnumDef` | `values: Vec<EnumValue>` |
| `FunctionDef` | `parameters`, `return_type`, `body`, `tests` |
| `ProfileDef` | `stereotypes: Vec<StringWithSourceInfo>`, `tags: Vec<...>` |
| `AssociationDef` | Two properties (like Class but restricted) |
| `MeasureDef` | `canonical_unit: Option<UnitDef>`, `non_canonical_units` |

#### Expressions

| Variant | Represents |
|---------|-----------|
| `Literal(LiteralKind, SourceInfo)` | Integer, Float, Decimal, String, Boolean, Date |
| `Variable(Identifier, SourceInfo)` | `$name` |
| `PropertyAccess { object, property, params }` | `expr.prop` or `expr.prop(args)` |
| `FunctionApplication { function, params }` | `name(args)` |
| `ArrowApplication { target, function, params }` | `expr->name(args)` |
| `Lambda { params, body }` | `{x \| body}` or `\|body` |
| `Collection(Vec<Expression>)` | `[a, b, c]` |
| `Let { name, value }` | `let x = expr` |
| `Arithmetic { op, left, right }` | `+`, `-`, `*`, `/` |
| `Boolean { op, left, right }` | `&&`, `\|\|` |
| `Comparison { op, left, right }` | `==`, `!=`, `<`, `>`, `<=`, `>=` |
| `Not(Box<Expression>)` | `!expr` |
| `NewInstance { class_path, assignments }` | `^Type(prop=val)` |
| `Cast { expression, target_type }` | `->cast(@Type)` |
| `GraphFetchTree(...)` | `#{Type{props}}#` |
| `ClassInstance(ClassInstance)` | Island grammar plugin results |
| `EnumValue { enum_path, value }` | `Enum.VALUE` |

---

## Pillar 3: TDD — Test-Driven Development

### Testing Strategy

Java grammar tests serve as specification. We group tests into **test modules** by grammar feature to avoid duplication and enable shared fixtures.

### Test Infrastructure

```rust
// tests/helpers.rs — shared test utilities

/// Parse source, assert success, return AST
fn parse_ok(source: &str) -> ParseResult;

/// Parse source, assert error with expected prefix
fn parse_err(source: &str, expected_prefix: &str);

/// Full roundtrip: parse → emit Protocol JSON → snapshot comparison
fn roundtrip(source: &str);

/// Roundtrip with formatting normalization
fn roundtrip_format(expected: &str, input: &str);
```

### Grouped Test Catalog

Tests are organized into modules that share fixtures and reduce duplication:

---

#### Module: `tests::profile` (3 tests)

All from [TestDomainGrammarRoundtrip.java](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-base/legend-engine-core-language-pure/legend-engine-language-pure-grammar/src/test/java/org/finos/legend/engine/language/pure/grammar/test/roundtrip/TestDomainGrammarRoundtrip.java)

| Test | Validates |
|------|-----------|
| `basic` | `stereotypes: [deprecated]; tags: [doc, todo];` |
| `quoted` | `Profile meta::pure::profiles::'with quotes'` |
| `empty` | Profile with empty `{}` body |

Shared fixture: `fn profile_source(name, stereos, tags) -> String`

---

#### Module: `tests::enumeration` (3 tests)

| Test | Validates |
|------|-----------|
| `basic_with_annotations` | Stereotypes + tagged values on enum and values |
| `quoted_names` | `'@'::'my Enum'`, `'Anything e'` |
| `numeric_names` | `'30_360'`, `'30_ACT'` |

---

#### Module: `tests::class` (12 tests)

| Test | Validates |
|------|-----------|
| `basic` | Stereotypes, tagged values, extends, properties, qualified properties |
| `complex_constraints` | Named/unnamed, `~function`, `~enforcementLevel`, `~externalId`, `~message` |
| `aggregation_kinds` | `(shared)`, `(none)`, `(composite)` |
| `multiple_annotations` | Multiple stereotypes and tagged values on class and properties |
| `quoted_annotations` | Quoted profile/stereotype/tag identifiers |
| `escaped_tagged_values` | `'test1\\'s'` in tagged values |
| `quoted_package` | `test::'p a c k a g e'::A` |
| `with_import` | `import anything::*;` resolution |
| `unit_properties` | `NewMeasure~UnitOne[0..1]` |
| `default_values` | String, enum, float, boolean, integer, collection, class instance defaults |
| `sourceinfo_validation` | Exact line/col assertions on all class elements |
| `type_parameters` | `Class X<T>{}` — **parsed successfully** (not rejected) |

Shared fixtures: `fn class_with_properties(...)`, `fn class_with_constraints(...)`

---

#### Module: `tests::association` (3 tests)

| Test | Validates |
|------|-----------|
| `basic` | Two typed properties |
| `aggregation_kinds` | `(shared)`, `(none)` |
| `with_annotations_and_import` | Stereotypes, tagged values, imports |

---

#### Module: `tests::measure` (4 tests)

| Test | Validates |
|------|-----------|
| `convertible` | `*UnitOne: x -> $x`, `UnitTwo: x -> $x * 1000` |
| `non_convertible` | `UnitOne;`, `UnitTwo;` |
| `quoted` | `Measure 'some measure'`, `*'Unit One'` |
| `class_with_unit_properties` | `NewMeasure~UnitOne[0..1]` |

---

#### Module: `tests::function` (10 tests)

| Test | Validates |
|------|-----------|
| `basic_with_body` | Parameters, return type, multi-statement body |
| `date_return_types` | `DateTime`, `StrictDate` literals |
| `overloading` | Same name, different parameter counts |
| `with_new_instance` | `^Type(prop=val)` including nested `^` |
| `quoted_params_and_vars` | `'1,2,3': Integer[3]`, `let '1,2,3' = [...]` |
| `full_path_meta_execution` | `->meta::pure::functions::math::max()` |
| `function_tests` | `{ myTest \| SimpleFunction() => 'Hello World!'; }` |
| `derived_multiple_statements` | `let x = 0; $x;` in qualified properties |
| `with_annotations_and_import` | Stereotypes, tagged values on functions |
| `multi_if_expressions` | Multi-line lambda body inside `if()` |

---

#### Module: `tests::expression` (10 tests)

| Test | Validates |
|------|-----------|
| `arithmetic_precedence` | 18 sub-cases: `(1 - (4 * (2 + 3))) * 4`, division associativity, etc. |
| `boolean_precedence` | `&&` binds tighter than `\|\|` |
| `comparison_with_arithmetic` | `1 + 2 <= 3 - 4` |
| `or_with_arithmetic` | `$this.id->isEmpty() \|\| $this.id >= 0` |
| `cast` | `->cast(@Float)`, `->cast(1.0)`, `->cast('String')` |
| `collection_with_function` | `[(true && ...), false]->oneOf()` |
| `new_instance_nested` | `^goes2(v2=^goes(v='value'))` |
| `let_binding` | `let x = ...` |
| `lambda_variants` | `{x \| body}`, `\|body`, multi-param |
| `enum_value_access` | `EnumType.VALUE` |

---

#### Module: `tests::primitives` (5 tests)

From [TestPrimitives.java](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-base/legend-engine-core-language-pure/legend-engine-language-pure-grammar/src/test/java/org/finos/legend/engine/language/pure/grammar/test/roundtrip/valueSpecification/TestPrimitives.java)

| Test | Validates |
|------|-----------|
| `decimal` | `1.0D`, `[1.0D, 3.0D]` |
| `string` | `'ok'`, `['ok', 'bla']` |
| `integer` | `1`, `[1, 2]` |
| `boolean` | `true`, `[true, false, true]` |
| `mixed` | `[1, 'a', true]` |

---

#### Module: `tests::graph_fetch` (4 tests)

| Test | Validates |
|------|-----------|
| `basic_with_qualifier` | `employeesByFirstName(['Peter']){firstName, lastName}` |
| `subtype_at_root` | `->subType(@FirmSubType){SubTypeName}` |
| `subtype_with_alias` | `'alias1' : SubTypeName` |
| `subtype_not_at_root_error` | Nested `->subType()` → parser error |

---

#### Module: `tests::type_system` (5 tests)

From [TestDomainGrammarArgumentsRoundtrip.java](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-base/legend-engine-core-language-pure/legend-engine-language-pure-grammar/src/test/java/org/finos/legend/engine/language/pure/grammar/test/roundtrip/TestDomainGrammarArgumentsRoundtrip.java), [TestDomainTypeVariablesRoundtrip.java](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-base/legend-engine-core-language-pure/legend-engine-language-pure-grammar/src/test/java/org/finos/legend/engine/language/pure/grammar/test/roundtrip/TestDomainTypeVariablesRoundtrip.java)

| Test | Validates |
|------|-----------|
| `type_arguments` | `Result<String>[1]` in params and return types |
| `cast_with_relation` | `->cast(@Relation<(a:Integer)>)` |
| `type_variable_values` | `Res(1)[1]`, `VARCHAR(200)` |
| `generics_and_variables` | `Res<String>(1,'a')` |
| `relation_column_types` | `X<(a:Integer(200), z:V('ok'))>` |

---

#### Module: `tests::error_cases` (4 tests)

From [TestGrammarParser.java](file:///Users/cocobey73/Projects/legend-engine/legend-engine-core/legend-engine-core-base/legend-engine-core-language-pure/legend-engine-language-pure-grammar/src/test/java/org/finos/legend/engine/language/pure/grammar/test/TestGrammarParser.java)

| Test | Validates |
|------|-----------|
| `unexpected_token` | `asd` before `Class` → error |
| `reserved_keywords` | `Class false::me` → error |
| `invalid_aggregation_kind` | `(tunnel)` → error |
| `function_test_name_mismatch` | Name in test ≠ function name → error |

---

### Test Totals

| Module | Test Count |
|--------|-----------|
| `profile` | 3 |
| `enumeration` | 3 |
| `class` | 12 |
| `association` | 3 |
| `measure` | 4 |
| `function` | 10 |
| `expression` | 10 |
| `primitives` | 5 |
| `graph_fetch` | 4 |
| `type_system` | 5 |
| `error_cases` | 4 |
| **Total** | **63** |

Reduced from 70 by merging related single/collection primitive tests and consolidating duplicated fixtures across class/association/function annotation tests.

---

## Implementation Phases

### Phase 0: Workspace, Standards & Documentation (Day 1)

- [ ] Create `legend-pure-parser/` Cargo workspace
- [ ] Configure `rustfmt.toml`, `clippy.toml`, `.cargo/config.toml`
- [ ] Install & configure `cargo-llvm-cov` with 90% gate
- [ ] Write `README.md`, `ARCHITECTURE.md`, `CONTRIBUTING.md`
- [ ] Write per-crate `README.md` stubs
- [ ] Set up CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo llvm-cov --fail-under-lines 90`

### Phase 1: AST Crate (Days 2-4)

- [ ] Implement all AST types (elements, expressions, types, source info)
- [ ] Define core traits (`Spanned`, `Packageable`, `Annotated`, visitors)
- [ ] Implement builder pattern for test ergonomics
- [ ] Extract derive macros if 3+ types share the pattern
- [ ] Unit tests for AST construction + trait implementations
- [ ] `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`

### Phase 2: Test Scaffolding (Days 5-6)

- [ ] Create 11 test modules with shared fixtures
- [ ] Write all 63 test functions calling `parse_ok` / `parse_err` / `roundtrip`
- [ ] Extract `.pure` corpus files into `tests/corpus/`

### Phase 3: Lexer (Days 7-9)

- [ ] Implement `crates/lexer/` — tokenizer
- [ ] Lexer-specific unit tests (token stream assertions)
- [ ] Coverage must be >90% on lexer crate

### Phase 4: Parser — TDD Loop (Days 10-18)

Priority: Profiles → Enums → Classes → Associations → Measures → Functions → Expressions → Graph Fetch → Type System → Error Cases

### Phase 5: Protocol + Snapshot Tests (Days 19-21)

Protocol v1 model (`crates/protocol/`). See [crates/protocol/DESIGN.md](crates/protocol/DESIGN.md) for design.

- [ ] Protocol v1 structs with serde (TDD: tests first, structs to pass)
- [ ] AST → Protocol conversion
- [ ] Snapshot tests against Java-generated golden files
- [ ] Extension handling (custom Deserialize for unknown `_type`)

### Phase 6: JNI Bridge + Java Integration (Days 22-24)

---

## Verification Plan

```bash
# All tests
cargo test --workspace

# Coverage (must be >90%)
cargo llvm-cov --workspace --fail-under-lines 90

# HTML coverage report
cargo llvm-cov --workspace --html --output-dir coverage/

# Lints
cargo clippy --workspace -- -D warnings
cargo fmt --check

# Snapshot review
cargo insta review
```
