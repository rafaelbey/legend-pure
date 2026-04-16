# Function Dispatch — Design & Implementation

This document describes the function overload resolution strategy for the Pure
compiler. Function calls in Pure are resolved by **simple name** and then
narrowed by parameter matching. This is the Rust counterpart of the Java
`FunctionExpressionMatcher`.

## Pipeline Prerequisite: Pass 2a/2b Split

Type-based dispatch requires all function **signatures** to be fully resolved
before any function **body** is compiled.

- **Pass 2a — Resolve Signatures**: Hydrates parameter types, multiplicities,
  and return types for all functions. Bodies are left empty.

- **Pass 2b — Resolve Bodies**: Compiles expression bodies. All function
  signatures are available in the `PureModel` for dispatch matching.
  Function parameters are seeded into `ResolutionContext.variable_types`
  before body lowering.

## Resolution Architecture

```
resolve_function_call(ptr, arg_count, lowered_args, ctx)
  │
  ├─ 1. Search by simple name (Function.function_name)
  │     across root package + all import scopes
  │
  ├─ 2. Filter by parameter count
  │     f.parameters.len() == arg_count
  │     (uses AST arg count, not lowered count — see "Lower First" below)
  │
  ├─ 3. narrow_candidates_by_type(candidates, lowered_args)
  │     ├─ Infer type + multiplicity of each lowered argument
  │     ├─ Score each candidate by type + multiplicity match
  │     ├─ Eliminate incompatible candidates
  │     └─ Return highest-scoring candidate(s)
  │
  └─ 4. If 1 candidate → resolved. If 0 or >1 → error.
```

### Key Separation

- **`resolve_element_ptr`** — resolves by exact mangled element name
  (the element's identity in the graph). Used for type references,
  annotations, property access.

- **`resolve_function_call`** — resolves by simple name + dispatch.
  Used only for `FunctionApplication` and `ArrowFunction` lowering.

## The "Lower First" Pattern

Arguments are **lowered before dispatch**. This gives the resolver access to
the compiled `ValueSpec` of each argument, from which types and multiplicities
can be inferred structurally.

```
lower_function_application(e, ctx, errors):
  1. Lower all args:  arguments = e.arguments.filter_map(lower_expression)
  2. Resolve:         function_id = resolve_function_call(
                          name, e.arguments.len(), &arguments, ctx)
  3. Build:           FunctionCall { function_id, arguments }
```

**Important**: The AST argument count (`e.arguments.len()`) is used for
parameter-count filtering, not the lowered count. If an argument fails to
lower (returns `None` from `filter_map`), the lowered count shrinks, but the
intent at the call site was the original count.

## Type Inference

Types and multiplicities are inferred from the lowered `ExprKind` structure:

### Type Inference (`infer_type_from_valuespec`)

| ExprKind | Inferred Type |
|---|---|
| `IntegerLiteral` | `Integer` |
| `FloatLiteral` | `Float` |
| `DecimalLiteral` | `Decimal` |
| `StringLiteral` | `String` |
| `BooleanLiteral` | `Boolean` |
| `DateLiteral(StrictDate)` | `StrictDate` |
| `DateLiteral(DateTime)` | `DateTime` |
| `DateLiteral(StrictTime)` | `StrictTime` |
| `EnumValue { enum_element }` | The `Enumeration` element |
| `FunctionCall { function }` | Return type of the resolved function |
| `Variable { name }` | Looked up from `variable_types` map |
| `Lambda`, `Collection`, etc. | `None` (unknown) |

### Multiplicity Inference (`infer_multiplicity_from_valuespec`)

| ExprKind | Inferred Multiplicity |
|---|---|
| Any literal, Enum, Lambda | `[1]` |
| `Collection` | `[*]` |
| `FunctionCall { function }` | Return multiplicity of resolved function |
| `Variable { name }` | Looked up from `variable_types` map |
| Others | `None` (unknown) |

### Variable Type Tracking

`ResolutionContext.variable_types` is a `HashMap<SmolStr, (TypeExpr, Multiplicity)>`
populated from three sources:

1. **Function parameters** — seeded in Pass 2b before body lowering
2. **Let bindings** — `let x = expr` infers type from the lowered RHS
3. **Lambda parameters** — registered before body lowering, restored after
   (scoped to prevent leakage into outer scope)

## Scoring Algorithm

Each candidate function is scored per-parameter, then scores are summed.
Incompatible candidates are eliminated. Highest total score wins.

### Type Scoring

| Condition | Score |
|---|---|
| Exact type match (`arg == param`) | +3 |
| Subtype match (`is_subtype(arg, param)`) | +1 |
| Generic/Any param (always compatible) | +0 |
| Incompatible type | **Eliminate** |

### Multiplicity Scoring

| Condition | Score |
|---|---|
| Exact match (`arg_mult == param_mult`) | +4 |
| Compatible, param is `[1]` | +4 (specificity) |
| Compatible, param is `[0..1]` | +3 |
| Compatible, param is `[1..*]` | +2 |
| Compatible, param is `[*]` | +1 |
| Unknown arg mult — still uses specificity | +specificity |
| Incompatible (arg range ⊄ param range) | **Eliminate** |

### Multiplicity Compatibility

An argument's multiplicity is compatible with a parameter's if:
`arg_lower >= param_lower AND arg_upper <= param_upper`

Example: `[1]` (1..1) fits into `[0..1]` (0..1) ✅, `[*]` (0..∞) into `[0..1]` ❌

### Subtype Check

`is_subtype(child, parent)` walks the `super_types` chain on Class and
PrimitiveType elements. `Integer → Number → Any` is a valid chain.

## Feature Matrix

| Feature | Status | Notes |
|---|---|---|
| Filter by param count | ✅ Done | `f.parameters.len() == arg_count` |
| Type-compatible matching | ✅ Done | Exact + subtype scoring |
| Multiplicity narrowing | ✅ Done | Specificity-based scoring |
| Subtype hierarchy walk | ✅ Done | `is_subtype` via `super_types` |
| Variable type tracking | ✅ Done | Params, lets, lambdas |
| Narrowest-match wins | ✅ Done | Uncapped specificity scoring |
| Generic params (T, V) | ⚠️ Compromise | Treated as `Any` — always matches |
| Generic unification | 🔲 Deferred | Propagate `Z` across params |
| Lambda param inference | 🔲 Deferred | Infer from `Function<{...}>` type |
| Numeric coercion | 🔲 Deferred | Implicit widening `Integer → Float` |
| Return type influence | 🔲 Deferred | Expected return type narrows |

## Current Error Counts

```
Total errors: 243  (down from 538, 55% reduction)
  161 AmbiguousImport
   75 UnresolvedElement
    6 ParseFailure
    1 DuplicateElement
```

Top remaining ambiguous:
```
  30 elementToPath  — M3 metaclass type (Type vs PackageableElement vs Function<Any>)
  29 dynamicNew     — M3 metaclass type (Class<Any> vs GenericType)
  21 map            — generic multiplicity param (T[m] vs T[*] vs T[0..1])
  12 assertEquals   — message overloads (String vs Function vs format)
  10 assert         — message overloads
   8 abs            — same-type overloads with multiplicity variants
   8 plus           — cross-package (math vs string)
```
