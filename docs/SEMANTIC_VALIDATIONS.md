# Semantic Validations — Deferred from Parser

These validations were performed by the **Java Pure parser** during parsing, but have been intentionally deferred to the **semantic/compiler layer** in the Rust implementation. The parser produces a valid AST — these rules must be enforced by a dedicated validation pass.

For the parser design rationale, see the module-level doc in `crates/parser/src/parser.rs`.

## Tracking

| ID | Validation | Java Parser Behavior | Rust Parser Behavior | Compiler Action |
|----|-----------|---------------------|---------------------|-----------------|
| SV-001 | Function test name mismatch | Parse error | Parser accepts; AST stores the invocation as-is | Error |
| SV-002 | `->subType` not at root of graph fetch | Parse error | Parser accepts at any depth; AST preserves structure | Error |
| SV-003 | Lambda parameter type inference | Fabricates `Any[*]` during parsing | `Parameter.type_ref = None`, `multiplicity = None` | Infer types |

---

## SV-001: Function Test Name Mismatch

**Grammar:**
```pure
function my::hello(): String[1] { 'Hello' }
{
    myTest | goodbye() => 'Hello';
}
```

**Why deferred:** The test block is syntactically valid — `name | expr => expr;`. Whether `goodbye()` matches the owning function `hello()` is a **name resolution** concern that requires the compiler's symbol table.

**Parser test:** `test_error_cases::function_test_name_mismatch` — asserts `parse_ok` with snapshot.

---

## SV-002: SubType Not at Root Level

**Grammar:**
```pure
#{
    my::Firm {
        name {
            ->subType(@my::SubType) { SubTypeName }
        }
    }
}#
```

**Why deferred:** `->subType(@Type) { fields }` is syntactically identical at any nesting depth. The restriction to root-level-only is a **structural constraint** on graph fetch trees that has no syntactic basis.

**Parser test:** `test_graph_fetch::subtype_not_at_root_error` — asserts `parse_ok` with snapshot.

---

## SV-003: Lambda Parameter Type Inference

**Grammar:**
```pure
x->filter({y | $y > 0})
```

**Why deferred:** In `{y | ...}`, the parameter `y` has no explicit type annotation. The Java parser fabricated `Any[*]`, but choosing a default type is **type inference** — a compiler responsibility.

**AST representation:** `Parameter.type_ref` and `Parameter.multiplicity` are `Option<T>`. Fully typed parameters (e.g., `function f(x: String[1])`) use `Some(...)`. Untyped lambda parameters use `None`.

---

## Implementation Notes

When implementing the semantic layer, create a validation pass that walks the AST:

```rust
// Semantic validations to implement:
//
// 1. FunctionTestNameValidator — for each FunctionDef with tests,
//    verify that every test assertion's invocation calls the
//    owning function.
//
// 2. GraphFetchTreeValidator — walk graph fetch tree expressions
//    and verify that ->subType only appears at the root level
//    (direct children of the root class).
//
// 3. TypeInferencePass — infer types for lambda parameters where
//    type_ref is None, based on the enclosing expression context.
```
