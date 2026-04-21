---
name: Known divergences between docs, designs and code
description: Places where the Rust port has drifted from its stated design, or deliberately deviates from the Java reference — watch for silent regressions
type: project
---

**1. `ExprKind` desugars operators — contrary to DESIGN.md.**

`crates/pure/DESIGN.md` §11 and `docs/runtime/convergence_analysis.md` both say: "Don't desugar expressions. Keep `Expression` structurally parallel to `ast::Expression`." But `crates/pure/src/types.rs` `ExprKind` (lines 245–352) declares: "Operators are desugared to function calls. `let` is `FunctionCall("letFunction", ...)`, `new` is `FunctionCall("new", ...)`. `Group(...)` is eliminated." And `crates/pure/src/lower.rs` lines 70–77 do exactly that: `Arithmetic` → `lower_arithmetic` → `FunctionCall`.

Why: it's simpler to have one dispatch path in the evaluator. The AST still preserves the non-desugared form (`crates/ast/src/expression.rs:43`), so Pure→AST composition is unaffected — but a future Pure→*PureModel*→AST emission path would need to un-desugar. If source-faithful emission becomes a goal, this decision must be revisited or the docs updated.

**2. Enum value evaluation returns a `Value::String`.**

`crates/runtime/src/eval.rs:591-594` implements `eval_enum_value` by formatting `"EnumName.VALUE"` into a `SmolStr`. That's a placeholder — Java has proper enum-value identity with stereotype/profile access. Any test that compares enums by identity, accesses enum tags, or dispatches on enum type will diverge.

**3. `Unit` is promoted to a package-level `Element::Unit`, not nested under `Measure`.**

Java M3 stores Units as children of their parent Measure. The Rust port allocates `Element::Unit` shells in Pass 1 with names like `Measure~UnitName` (`crates/pure/src/pipeline.rs:370`). Works for name resolution but non-canonical. `BACKLOG.md` marks this P1 "Unit as child of Measure."

**4. Chunk 0 / M3 metamodel is hardcoded in Rust, not parsed from m3.pure.**

Java parses `m3.pure` with a dedicated M4 parser at runtime. Rust hardcodes the M3 metamodel in `crates/pure/src/bootstrap.rs` + `m3_parser.rs` (1396 lines!). Faster and type-safe, but:
- Any M3 metamodel change requires a Rust code change (not a `.pure` edit).
- Subtle bootstrap behaviour (e.g., classifier generic types with deep nesting as in m3.pure:213) may diverge.
- `crates/pure/BACKLOG.md` flags "M3 property types are placeholder `Any`" and "M3 parser missing constraints/qualified properties/derived properties" — so the hardcoded bootstrap is known-incomplete.

**5. Generic type parameters are treated as `Any` in dispatch.**

`FUNCTION_DISPATCH.md` Feature Matrix marks "Generic params (T, V)" as ⚠️ Compromise: always matches. True generic unification (Java's `TypeInferenceObserver` with backtracking) is deferred P2. Direct consequence: overloaded generic functions (`map<T>(col: T[*], ...)` vs variants) can match incorrectly — visible as the 21 `map` ambiguities in the current error count.

**6. The evaluator's native dispatch has a fallback `find_by_prefix`.**

`crates/runtime/src/native.rs:207` — if a function's mangled FQN doesn't match a registered native, the evaluator falls back to the **first match** (sorted) starting with `simple_name_`. This hides dispatch bugs: a wrongly compiled `plus_xxx_` that should resolve to Number plus could silently resolve to String plus, or vice versa. Correct when compiler dispatch works; dangerous when it doesn't. See `eval.rs:400` comment "This handles unresolved operators (where function is None) AND standard library natives…".

**7. Multiplicity representation is non-uniform.**

`docs/runtime/multiplicity_and_iteration.md` commits to "scalar values are genuinely scalar" (unlike Java which wraps everything). Cost: every stdlib function that accepts `T[*]` must handle both `Value::Integer(42)` (implicit `[1]`) and `Value::Collection(...)`. The `to_one`, `to_zero_one`, `to_collection` helpers exist on `Value` (`crates/runtime/src/value.rs:337-420`) but natives must remember to call them — easy to forget, silent semantic drift when missed.

**8. `PureDate` arithmetic uses `jiff` UTC-stored calendar math.**

Java uses a variable-precision date that preserves the input precision (year-only, month-only, etc.). `PureDate` (`crates/runtime/src/date.rs`, 755 lines) wraps `jiff::civil::DateTime` — year/month-only precision is tracked but timezone semantics and formatting edge cases are their own test surface. Watch leap-year / DST / month-end adjust cases carefully.

**9. Parser error recovery policy.**

`crates/parser/` supports `PartialParse` on error. The compile path accepts `partial.source_file` in tests (`crates/runtime/tests/eval_tests.rs:129`). That means compile errors don't stop the pipeline — a test that should fail to parse may silently get a partial model. PCT-style tests relying on precise compilation errors will need explicit error assertions, not just "Ok/Err".
