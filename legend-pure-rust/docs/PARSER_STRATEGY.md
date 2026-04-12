# Parser Architecture Review: Reusability, Scalability & 3rd-Party Tooling

> **Purpose:** Reference document for future parser refactoring. Evaluates the current
> hand-written recursive descent parser against the goals of rule reusability, island
> grammar scalability, and potential adoption of Chumsky or other parser combinator libraries.

---

## 1. Current Architecture Inventory

### 1.1 Crate Layout

```
lexer/     → 911 lines   — Hand-written scanner, produces Vec<Token>
ast/       → 3,479 lines — Typed AST nodes (sections, elements, expressions, types)
parser/    → 2,150 lines — Single monolithic parser.rs + cursor + island plugin system
compose/   → 1,968 lines — AST → source text (roundtrip fidelity)
```

### 1.2 Parser Internal Structure

The parser is a **single `Parser` struct** with ~45 `fn parse_*` methods in one 2,150-line file:

| Layer | Methods | Description |
|---|---|---|
| **Top-level** | `parse_source_file`, `parse_section`, `parse_import`, `parse_element` | Section dispatch |
| **Element parsers** | `parse_class`, `parse_enum`, `parse_profile`, `parse_function`, etc. (8 methods) | One per element keyword |
| **Annotation parsers** | `parse_stereotypes`, `parse_tagged_values`, `parse_constraint`, etc. (6 methods) | Shared across elements |
| **Type parsers** | `parse_type_reference`, `parse_type_spec`, `parse_multiplicity`, `parse_relation_type`, etc. (8 methods) | Shared across elements + expressions |
| **Expression parsers** | `parse_expression` → `parse_or` → `parse_and` → ... → `parse_primary` (12 methods) | Precedence climbing |
| **Lookahead helpers** | `is_lambda_start`, `is_bare_lambda`, `scan_past_type_for_pipe` (3 methods) | Disambiguation |
| **Island dispatch** | Via `ParserContext` bridge to `IslandParser` trait plugins | Currently only graph-fetch |

### 1.3 Island Grammar Plugin System

```
IslandParser (trait)                 ParserContext (bridge)
  tag() → &str                        cursor() → &mut Cursor
  parse(ctx) → Box<dyn IslandContent> parse_expression() → Expression
                                      parse_package_path() → Package
```

The current `ParserContext` exposes exactly **2 reusable parsing rules** to island implementations:
- `parse_expression()` — full expression grammar
- `parse_package_path()` — qualified name `my::pkg::Name`

### 1.4 Error Model

- **Fail-fast:** First error aborts the entire file parse (returns `Err`)
- **No recovery:** No skip-to-next-element or skip-to-sync-token logic
- **Single error:** `ParseError::Unexpected { message, source_info }`

---

## 2. Strengths of Current Design

| Strength | Evidence |
|---|---|
| **Performance** | 28ms for 10K classes sequential, 12ms parallel. Hand-written RD is essentially optimal. |
| **Predictability** | Each `parse_*` method maps 1:1 to a grammar production. Easy to trace and debug. |
| **No external deps** | The parser crate depends only on the lexer and AST crates (+ rayon for parallelism). |
| **Roundtrip fidelity** | Compose crate mirrors parser structure, enabling snapshot testing. |
| **Island plugin system** | The `IslandParser` trait + `ParserContext` is a clean separation of concerns. |

---

## 3. Scalability Concerns

### 3.1 Monolithic Parser File

**Problem:** All 45 methods live in one 2,150-line file. As more sections and element types are added (Mapping, Service, Runtime, Connection, etc.), this file will grow linearly.

**Impact:** Merge conflicts, poor discoverability, unclear ownership.

**Recommendation:** Split `parser.rs` into submodules by concern:

```
parser/
  mod.rs          — Parser struct, parse_source_file, parse_section, parse_element
  annotation.rs   — stereotypes, tagged values, constraints
  class.rs        — parse_class, parse_class_body, parse_property, parse_qualified_property
  enum.rs         — parse_enum
  function.rs     — parse_function, parse_native_function, parse_function_tests
  association.rs  — parse_association
  measure.rs      — parse_measure, parse_unit_def
  profile.rs      — parse_profile
  expression.rs   — parse_expression (entire precedence chain), parse_primary
  type_ref.rs     — parse_type_reference, parse_type_spec, parse_multiplicity
  lambda.rs       — is_lambda_start, is_bare_lambda, scan_past_type_for_pipe, parse_bare_lambda
  helpers.rs      — split_package_name, unquote_string, is_wildcard_ahead
```

This is a **mechanical refactor** — no API or behavior changes. Each submodule would contain `impl Parser { ... }` blocks.

### 3.2 Limited `ParserContext` Surface

**Problem:** Island grammars only get `parse_expression()` and `parse_package_path()`. Future islands (Relational mapping, Service definitions, flatdata) will need:

- `parse_type_reference()` — for mapping column types
- `parse_lambda()` — for filter/constraint expressions in mappings
- `parse_stereotypes()` / `parse_tagged_values()` — annotations are universal
- `parse_multiplicity()` — multiplicities appear in mapping column definitions
- `parse_parameter()` — for function-like constructs

**Recommendation:** Expose these as methods on `ParserContext`:

```rust
impl ParserContext<'_> {
    // Currently exposed:
    pub fn parse_expression(&mut self) -> R<Expression>;
    pub fn parse_package_path(&mut self) -> R<Package>;

    // Proposed additions:
    pub fn parse_type_reference(&mut self) -> R<TypeReference>;
    pub fn parse_multiplicity(&mut self) -> R<Multiplicity>;
    pub fn parse_lambda(&mut self) -> R<Expression>;
    pub fn parse_stereotypes(&mut self) -> R<Vec<StereotypePtr>>;
    pub fn parse_tagged_values(&mut self) -> R<Vec<TaggedValue>>;
    pub fn parse_parameter(&mut self) -> R<Parameter>;
    pub fn parse_parameter_list(&mut self) -> R<Vec<Parameter>>;
    pub fn parse_qualified_name(&mut self) -> R<(Option<Package>, SmolStr, SourceInfo)>;
}
```

These are all **delegation** methods — they call through to the underlying `Parser`. The cost is zero; the benefit is enormous: every rule that's reusable becomes available to every island grammar.

### 3.3 Section Header Dispatch

**Problem:** The current `parse_section()` treats everything as a `###Pure` section and calls `parse_element()`, which hard-codes the 7 element keywords. When `###Mapping`, `###Service`, `###Runtime`, etc. are added, this needs a **section parser registry** analogous to the island parser registry.

**Recommendation:** Introduce a `SectionParser` trait:

```rust
/// Trait for section-level grammar plugins.
///
/// Each section grammar (Pure, Mapping, Service, Runtime, etc.)
/// provides an implementation that parses elements within its section.
pub trait SectionParser: Send + Sync {
    /// The section header this parser handles (e.g., "Pure", "Mapping").
    fn section_name(&self) -> &str;

    /// Parse elements within this section.
    fn parse_elements(
        &self,
        ctx: &mut ParserContext<'_>,
    ) -> Result<Vec<Element>, ParseError>;
}
```

This mirrors the `IslandParser` pattern and allows external crates to register new section grammars without modifying the core parser.

### 3.4 Error Recovery

**Problem:** The parser fails on the first error. For IDE-class usage (IntelliJ plugin, LSP), we need partial parses — returning a valid AST for the successfully-parsed portions and collecting errors for the rest.

**Recommendation (phased):**

| Phase | Approach | Effort |
|---|---|---|
| **Phase 1** (current) | Fail-fast is fine for CLI batch compilation | Done |
| **Phase 2** | Add element-level recovery: on error, skip to next element keyword or `###` section header | Medium |
| **Phase 3** | Add expression-level recovery: on error, skip to next `;` or `}` | High |

For Phase 2, the implementation is straightforward:

```rust
fn parse_section(&mut self) -> R<Section> {
    // ...
    let mut elements = Vec::new();
    let mut errors = Vec::new();
    while !self.cursor.check(TokenKind::SectionHeader) && !self.cursor.check(TokenKind::Eof) {
        match self.parse_element() {
            Ok(elem) => elements.push(elem),
            Err(e) => {
                errors.push(e);
                self.skip_to_next_element(); // new recovery method
            }
        }
    }
    // Return elements + errors together
}
```

---

## 4. Chumsky Evaluation

### 4.1 What Chumsky Offers

| Feature | Status | Relevance |
|---|---|---|
| **Declarative combinators** | Mature (0.10.x/1.0-alpha) | Reduces boilerplate for simple grammars |
| **Built-in Pratt parsing** | `chumsky::pratt` module | Would replace our manual precedence chain |
| **Error recovery** | First-class, automatic | Major benefit for IDE integration |
| **Zero-copy parsing** | New in 0.10.x | Aligns with our `SmolStr` / `Cow` patterns |
| **Extension API** | Allows custom combinators | Could host island grammars |
| **Ariadne integration** | Beautiful diagnostics | Overlaps with our `diagnostics` module |

### 4.2 Risks & Costs

> [!WARNING]
> **Chumsky is still in alpha (0.10.x / 1.0-alpha).** The API is not yet stable and has undergone breaking changes between versions. Adopting it ties us to an evolving dependency.

| Risk | Severity | Mitigation |
|---|---|---|
| **Alpha stability** | High | Wait for 1.0 stable release before production adoption |
| **Compilation times** | High | Chumsky's heavy generic usage causes long compile times; `.boxed()` helps but doesn't eliminate |
| **Migration cost** | Very High | Rewriting 2,150 lines of battle-tested parser logic is a multi-week effort |
| **Performance regression** | Medium | Combinator overhead vs hand-written RD; our benchmarks show ~2.6ms/1K classes, hard to beat |
| **Lookahead complexity** | Medium | Our `is_lambda_start()` / `is_bare_lambda()` multi-token lookahead is awkward in combinator style |
| **Island grammar integration** | Unknown | Chumsky's `extension` API may or may not map cleanly to our `IslandParser` trait |
| **Debug tracing** | Medium | Hand-written parsers have trivial debuggability; combinator stacks are harder to trace |

### 4.3 Recommendation: Targeted Adoption, Not Full Rewrite

> [!IMPORTANT]
> **Do NOT rewrite the parser in Chumsky.** The hand-written RD parser is fast, predictable, and
> battle-tested. A full rewrite would be very high risk with marginal benefit.

Instead, consider **targeted adoption** in specific areas:

#### Option A: Chumsky for Expression Parsing Only (Highest Value)

The expression parser (12 methods, ~500 lines) is the most boilerplate-heavy part of the codebase — the precedence chain from `parse_or` through `parse_multiplicative` is textbook combinator territory. Chumsky's `pratt` module would replace this with ~50 lines of declarative code:

```rust
// Hypothetical: replaces 12 methods with one declaration
let expr = recursive(|expr| {
    let atom = /* parse_primary equivalent */;
    atom.pratt((
        infix(left(1), just(Token::PipePipe), |l, r| Expr::Or(l, r)),
        infix(left(2), just(Token::AmpAmp), |l, r| Expr::And(l, r)),
        infix(left(3), comparison_op, |l, (op,), r| Expr::Cmp(l, op, r)),
        infix(left(4), just(Token::Plus).or(just(Token::Minus)), ...),
        infix(left(5), just(Token::Star).or(just(Token::Slash)), ...),
        prefix(6, just(Token::Bang), |e| Expr::Not(e)),
        prefix(6, just(Token::Minus), |e| Expr::Neg(e)),
        postfix(7, arrow_call, |e, call| Expr::Arrow(e, call)),
        postfix(7, dot_access, |e, field| Expr::Dot(e, field)),
    ))
});
```

**Pros:** Eliminates the mechanical precedence chain, gains error recovery in expressions.
**Cons:** Compilation time increase, mixed paradigm in the parser crate.

#### Option B: Chumsky for New Island/Section Grammars Only

Keep the core Pure parser hand-written. Use Chumsky to implement **new** section/island grammars (Mapping, Service, Runtime) that haven't been written yet. This way:

- Existing code is untouched
- New grammars get error recovery and declarative style
- Island parsers bridge through `ParserContext` as today

**Pros:** No migration risk, best of both worlds.
**Cons:** Two parsing paradigms in the codebase.

#### Option C: Use Winnow Instead of Chumsky

Winnow is the modern successor to Nom, focused on ergonomics and speed. It's more stable than Chumsky (1.x released) and has lower compilation overhead. It provides:
- Stream-based parsing (works with our token stream)
- Good error messages
- No Pratt module (would need manual precedence)

**Pros:** Stable, fast, low compile overhead.
**Cons:** No built-in Pratt parsing, less error recovery than Chumsky.

### 4.4 Decision Matrix

| Criterion | Hand-Written RD | Chumsky | Winnow |
|---|---|---|---|
| **Performance** | ★★★★★ | ★★★★ | ★★★★★ |
| **Compilation speed** | ★★★★★ | ★★☆ | ★★★★ |
| **Error recovery** | ★★ (needs manual work) | ★★★★★ | ★★★ |
| **Debugging** | ★★★★★ | ★★★ | ★★★★ |
| **Boilerplate** | ★★ (precedence chain) | ★★★★★ | ★★★★ |
| **API stability** | ★★★★★ (no dependency) | ★★★ (alpha) | ★★★★★ (1.x stable) |
| **Island/plugin support** | ★★★★ (our trait) | ★★★ (via extension) | ★★★ (manual) |
| **IDE recovery support** | ★★ | ★★★★★ | ★★★ |

---

## 5. Recommended Refactoring Roadmap

### Phase 1: Structural Decomposition (Low Risk, High Impact)

**Goal:** Make the parser maintainable and extensible without any behavioral changes.

1. **Split `parser.rs` into submodules** (§3.1)
   - Each module: one concern, one `impl Parser` block
   - Mechanical refactor, no behavior change
   - Timeline: 1 day

2. **Expand `ParserContext` surface** (§3.2)
   - Expose `parse_type_reference`, `parse_multiplicity`, `parse_stereotypes`, etc.
   - Enables rich island grammars
   - Timeline: 0.5 days

3. **Document the grammar formally** 
   - Create `docs/GRAMMAR.md` with EBNF for every production rule
   - One-to-one mapping between EBNF rule names and `parse_*` method names
   - Timeline: 1 day

### Phase 2: Section Parser Registry (Medium Risk, High Impact)

**Goal:** Enable `###Mapping`, `###Service`, `###Runtime` sections as external plugins.

1. **Implement `SectionParser` trait** (§3.3)
   - Mirror the `IslandParser` pattern
   - Core `PureSectionParser` handles existing `###Pure` section
   - Timeline: 1–2 days

2. **Move `parse_element` dispatch to `PureSectionParser`**
   - `parse_class`, `parse_enum`, etc. move into the Pure section implementation
   - External crates register new `SectionParser` instances

### Phase 3: Error Recovery (Medium Risk, Medium Impact)

**Goal:** Support IDE/LSP partial parsing.

1. **Element-level recovery** (§3.4, Phase 2)
   - On error, skip to next element keyword or section header
   - Return `(Vec<Element>, Vec<ParseError>)` from section parsing
   - Timeline: 1–2 days

2. **Expression-level recovery** (§3.4, Phase 3)
   - On error, skip to next `;` or `}`
   - Emit `Expression::Error(ParseError)` placeholder nodes in the AST
   - Timeline: 2–3 days
   - *Consider Chumsky for this layer specifically*

### Phase 4: Optional Chumsky Adoption (High Risk, Variable Impact)

**Goal:** Evaluate in practice whether Chumsky improves developer velocity for new grammars.

1. **Spike: Implement one new island grammar in Chumsky** (e.g., path expressions `#>{...}#`)
   - Measure: compilation time impact, code clarity, error quality
   - If positive: adopt for future island/section grammars (Option B)
   - If negative: stay fully hand-written

2. **Spike: Replace expression precedence chain with Chumsky Pratt** (Option A)
   - Only if Phase 4.1 is positive
   - Measure: performance regression, compilation time, maintainability

---

## 6. Key Principles for Future Implementors

> [!TIP]
> These principles should guide any parser refactoring or new grammar implementation.

1. **Parser stays syntax-only.** No semantic validation (type checking, name resolution, constraint validation) in the parser. This is documented in `docs/SEMANTIC_VALIDATIONS.md`.

2. **Every `parse_*` method maps to one EBNF production.** When you add a new grammar rule, name the method after the production rule.

3. **Shared rules go through `ParserContext`.** If an island or section grammar needs a common parsing rule (types, expressions, annotations), it **must** use `ParserContext` — never duplicate the logic.

4. **Island parsers must be stateless and `Send + Sync`.** This is enforced by the trait bounds and enables parallel parsing.

5. **Roundtrip fidelity.** Every parser change must be mirrored in the compose crate. If you add `parse_foo()`, add `compose_foo()`.

6. **Test via roundtrip.** Parse → compose → re-parse → assert AST equality. Snapshot tests catch regressions early.

7. **Performance is a feature.** The parser processes 10K classes in ~28ms. Any refactoring that degrades this by >10% needs justification.

---

## 7. Files Referenced

| File | Lines | Purpose |
|---|---|---|
| [parser.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/parser.rs) | 2,151 | Main parser (target of decomposition) |
| [cursor.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/cursor.rs) | 149 | Token cursor abstraction |
| [island.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/island.rs) | 317 | Island parser plugin system + graph fetch |
| [error.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/error.rs) | 81 | Error types (fail-fast, no recovery) |
| [source.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/source.rs) | ~200 | SourceProvider trait (new) |
| [lib.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/parser/src/lib.rs) | ~190 | Public API: `parse()`, `parse_many()`, `ParseOutput` |
| [token.rs](file:///Users/cocobey73/Projects/legend-pure/legend-pure-rust/crates/lexer/src/token.rs) | 296 | Token kinds and keyword table |
