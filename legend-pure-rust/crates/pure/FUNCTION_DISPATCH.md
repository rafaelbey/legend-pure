# Function Dispatch — Design & Status

This document tracks the function overload resolution strategy for the Pure
compiler. Function calls in Pure are resolved by **simple name** and then
narrowed by parameter matching. This is the Rust counterpart of the Java
`FunctionExpressionMatcher`.

## Architecture

```
resolve_function_call(ptr, call_args, source_info, ctx)
  │
  ├─ 1. Search by simple name (Function.function_name)
  │     across root package + all import scopes
  │
  ├─ 2. Filter by parameter count
  │     f.parameters.len() == call_args.len()
  │
  ├─ 3. Filter by parameter type compatibility    ← TODO
  │     arg_type is subtype of param_type
  │
  ├─ 4. Narrow by multiplicity specificity        ← TODO
  │     prefer [1] match over [*] match
  │
  └─ 5. Pick best candidate or report ambiguity
```

### Key Separation

- **`resolve_element_ptr`** — resolves by exact mangled element name
  (the element's identity in the graph). Used for type references,
  annotations, property access.

- **`resolve_function_call`** — resolves by simple name + dispatch.
  Used only for `FunctionApplication` and `ArrowFunction` lowering.

## Feature Matrix

| Feature | Status | Notes |
|---|---|---|
| Filter by param count | ✅ Implemented | `f.parameters.len() == arg_count` |
| Filter by param type (concrete) | 🔲 Planned | Match `Number` vs `Date` vs `String` |
| Filter by multiplicity | 🔲 Planned | Match `[1]` vs `[0..1]` vs `[*]` |
| Subtype matching | 🔲 Planned | `Integer` matches `Number` param |
| Generic params (T, V) | ⚠️ Partial | Treated as `Any` — always matches |
| Generic unification | 🔲 Future | Propagate `Z` across params |
| Lambda param inference | 🔲 Future | Infer types from expected function type |
| Coercion (e.g., Int→Float) | 🔲 Future | Implicit numeric widening |
| Return type influence | 🔲 Future | Expected return type narrows candidates |

## Prerequisites

### Pass 2a/2b Pipeline Split

Type-based dispatch requires all function **signatures** (parameter types,
return types) to be fully resolved before any function **body** is compiled.
Current pipeline resolves everything in a single Pass 2.

**Required change**: Split Pass 2 into:

- **Pass 2a — Resolve Signatures**: resolve parameter types, multiplicities,
  and return types for all functions/native functions. Shells are hydrated
  with real types.

- **Pass 2b — Resolve Bodies**: compile expression bodies. At this point all
  function signatures are available for dispatch matching.

## Dispatch Algorithm

### Step 1: Compile Arguments

Lower each argument expression through the existing pipeline. The compiled
`ValueSpec` carries `type_expr` and `multiplicity` from the type system.

### Step 2: Type Compatibility Filter

For each candidate function, check param-by-param:

| Arg Type | Param Type | Result |
|---|---|---|
| Same type | Same type | ✅ Exact match (score 2) |
| Subtype | Supertype | ✅ Subtype match (score 1) |
| Any concrete | `TypeExpr::Generic` | ✅ Generic match (score 0) |
| Any concrete | `Any` | ✅ Wildcard match (score 0) |
| Incompatible | Incompatible | ❌ Eliminate |

### Step 3: Multiplicity Narrowing

Among type-compatible candidates, score by multiplicity specificity:

| Arg Mult | Param Mult | Score |
|---|---|---|
| `[1]` | `[1]` | 2 (exact) |
| `[1]` | `[0..1]` | 1 (compatible) |
| `[1]` | `[*]` | 0 (loose) |
| `[0..1]` | `[0..1]` | 2 (exact) |
| `[*]` | `[*]` | 2 (exact) |

Highest total score wins. Tie = ambiguity error.

## Known Gaps & Compromises

### Generic Unification (Deferred)

`contains<Z>(Z[*], Z[1])` treats `Z` as `Any`, so it always matches.
A concrete overload like `contains(String[1], String[1])` should score
higher than the generic version. When both match, prefer concrete.

### Lambda Parameter Types (Deferred)

Untyped lambda params (`x | $x + 1`) are typed as `Any`. The Java
compiler infers lambda param types from the expected `Function<{...}>`
type at the call site.

### Numeric Coercion (Deferred)

The Java compiler supports implicit widening: `Integer` → `Number`,
`Integer` → `Float`. Not implemented.

## Current Error Counts (Reference)

```
Total errors: 535
  468 AmbiguousImport
   65 UnresolvedElement
    2 ParseFailure
```

Top ambiguous functions (same param count, needs type/mult dispatch):
```
  71 isNotEmpty   — mult difference ([*] vs [0..1])
  44 map          — generic overloads
  44 elementToPath — overloads
  32 assert       — overloads
  29 lessThan     — type difference (Number vs Date vs String)
  27 contains     — cross-package + generics
```
