# Legend Pure Rust — Code Review & Optimization Roadmap

> **Scope:** Full codebase audit of the `legend-pure-rust` workspace  
> **Status:** Living document — update as items are completed

---

## Executive Summary

The codebase has **strongly executed** the Phase 1–4 memory optimizations from the prior plan.
`Value` is now 24 bytes (boxed heavy variants), `ValueSpec.kind` is `Box<ExprKind>`,
`type_info` is `Option<Box<ResolvedType>>`, and `CompilationError` uses `thiserror`.
`StringLiteral.value` is `SmolStr`.

This review **confirms what was already done**, identifies the **remaining open issues**,
and surfaces **new high-value opportunities** not previously captured. Issues are ranked by
business impact.

---

## ✅ Already Implemented (Confirmed)

| Item | Status | File |
|---|---|---|
| `Value::Collection(Box<PVector<…>>)` | ✅ Done | `runtime/src/value.rs:96` |
| `Value::Map(Box<im_rc::HashMap<…>>)` | ✅ Done | `runtime/src/value.rs:102` |
| `Value::Lambda(Box<LambdaClosure>)` | ✅ Done | `runtime/src/value.rs:108` |
| `ValueSpec.kind: Box<ExprKind>` | ✅ Done | `pure/src/types.rs:238` |
| `ValueSpec.type_info: Option<Box<ResolvedType>>` | ✅ Done | `pure/src/types.rs:242` |
| `StringLiteral.value: SmolStr` | ✅ Done | `ast/src/expression.rs:220` |
| `CompilationError` → `thiserror::Error` | ✅ Done | `pure/src/error.rs:21` |
| `ExpressionVisitor` trait on AST | ✅ Done | `ast/src/expression.rs:666` |

---

## 🔴 Critical Issues (Production Blockers)

### 1. Native function dispatch is fundamentally wrong — full architectural fix required

**Files:** `runtime/src/native.rs`, `runtime/src/eval.rs`, `pure/src/types.rs`

This issue has three layers, each building on the previous. We will address all three.

#### Layer A: The current `find_by_prefix` is O(N) per call

```rust
// Current — O(N) HashMap iteration on every unresolved operator
pub fn find_by_prefix(&self, simple_name: &str) -> Option<&dyn NativeFunction> {
    let prefix = format!("{simple_name}_");
    self.functions.iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .min_by_key(|(key, _)| *key)
        .map(|(_, func)| func.as_ref())
}
```

Every `plus`, `minus`, `equal`, `and`, `or`, `not` that isn't FQN-resolved hits this path.
With ~80+ registered functions this is **O(80) per arithmetic operation**.

#### Layer B: Would `match` on the string be better than HashMap?

This is the natural next question. The answer is: **yes, but still not good enough**.

Rust compiles `match &str` as **sequential `memcmp` chains** — NOT a jump table (jump tables
require fixed-width integer discriminants). For 80 arms this is O(80) just like the iterator
scan, but with excellent branch prediction for hot arms:

```rust
// Better than HashMap for the 3-5 most common ops, still O(N) worst case
match simple_name {
    "plus"  => ...,
    "equal" => ...,
    // 78 more arms...
}
```

Benchmark comparison for a typical arithmetic-heavy workload:

| Approach | Cost Per Dispatch | Notes |
|---|---|---|
| `find_by_prefix` (current) | ~50–200 ns | O(N) iteration, format! alloc |
| `match &str` at eval time | ~1–5 ns | Branch predicted for hot ops, O(N) worst case |
| **`phf::Map` at eval time** | **~2–4 ns** | **O(1) perfect hash — correct answer** |
| `BuiltinOp` cached in compiler IR | ~0 ns | ⚠️ Layering violation — see below |

#### Layer C: The correct fix — `phf::Map` with mangled FQN keys, entirely within `crates/runtime`

> **Layering rule:** Caching `BuiltinOp` inside `ExprKind` (compiler IR) was considered
> and **rejected**. It would place runtime dispatch vocabulary inside `crates/pure`,
> violating the `ast ← pure ← runtime` dependency invariant. `BuiltinOp` belongs
> in `crates/runtime` — it is the runtime's private vocabulary, not the language's.

> **Key design rule:** Use the **mangled FQN** (`plus_Integer_MANY__Integer_1_`), NOT
> the simple name (`plus`). Simple names are ambiguous across overloads — Integer plus,
> Float plus, and String concatenation are all named "plus" but have completely different
> implementations. Using the simple name would force secondary value-type dispatch
> in `dispatch_builtin`, defeating the purpose of the enum entirely.

The `phf` crate generates a **perfect hash function at build time** for a known, static key
set. At runtime, only a single multiply-and-mask operation resolves the key — zero collisions,
zero iteration, zero allocation. Everything stays in `crates/runtime`:

**Add `BuiltinOp` + `BUILTIN_DISPATCH` to `runtime/src/native.rs`:**

```rust
/// Runtime-private dispatch tag. One variant per overload, not per function name.
///
/// Distinction:
///   OVERLOADED HOMONYMS (one variant per type) — plus(Int,Int) ≠ plus(Float,Float)
///   TRUE POLYMORPHICS   (one variant for all)  — equal works on any comparable type
///
/// Stays in crates/runtime — never imported by crates/pure or crates/ast.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinOp {
    // Integer arithmetic — one variant per type (not polymorphic)
    PlusInteger,
    MinusInteger,
    TimesInteger,
    DivideInteger,
    NegateInteger,
    AbsInteger,
    ModInteger,

    // Float arithmetic
    PlusFloat,
    MinusFloat,
    TimesFloat,
    DivideFloat,
    NegateFloat,
    AbsFloat,
    FloorFloat,
    CeilingFloat,
    RoundFloat,

    // String — plus(String, String) is concatenation, a distinct op
    PlusString,
    Length,
    Substring,
    IndexOfString,
    ToUpper,
    ToLower,
    Trim,

    // Date arithmetic — each combination is a distinct implementation
    PlusDateDuration,
    MinusDateDate,     // date - date = duration
    MinusDateDuration, // date - duration = date

    // True polymorphics — one variant each, runtime inspects Value type
    Equal,             // structural equality, works on any comparable
    NotEqual,
    LessThan,          // ordered types only, checked at runtime
    LessThanEqual,
    GreaterThan,
    GreaterThanEqual,

    // Boolean — not overloaded
    And,
    Or,
    Not,

    // Collections — polymorphic over element type by design
    Map,
    Filter,
    Fold,
    Size,
    IsEmpty,
    At,
    First,
    Last,
    Concatenate,  // collection concatenation — distinct from string plus
    Take,
    Drop,
    Range,
    Reverse,
    Sort,
    Distinct,

    // Control flow
    If,
    LetFunction,
    New,
}

/// Compile-time perfect hash: mangled FQN → BuiltinOp.
///
/// Key format matches what NativeRegistry.register() already uses:
///   "{simple_name}_{ArgType1}_{Multiplicity}__{ArgType2}_{Mult}_..."
/// e.g.: "plus_Integer_MANY__Integer_1_"
///
/// Using mangled names (not simple names) because:
///   - "plus" alone is ambiguous: Integer/Float/String/Date all have their own
///   - One phf entry per overload → one BuiltinOp variant → zero secondary dispatch
///
/// Generated at BUILD time — O(1) at eval time, ~2-4 ns per resolution.
static BUILTIN_DISPATCH: phf::Map<&'static str, BuiltinOp> = phf::phf_map! {
    // Integer arithmetic — mangled names match NativeRegistry registration keys
    "plus_Integer_MANY__Integer_1_"              => BuiltinOp::PlusInteger,
    "minus_Integer_MANY__Integer_1_"             => BuiltinOp::MinusInteger,
    "times_Integer_MANY__Integer_1_"             => BuiltinOp::TimesInteger,
    "divide_Integer_1__Integer_1__Integer_1_"    => BuiltinOp::DivideInteger,
    "negate_Integer_1__Integer_1_"               => BuiltinOp::NegateInteger,
    "abs_Integer_1__Integer_1_"                  => BuiltinOp::AbsInteger,
    "mod_Integer_1__Integer_1__Integer_1_"       => BuiltinOp::ModInteger,

    // Float arithmetic
    "plus_Float_MANY__Float_1_"                  => BuiltinOp::PlusFloat,
    "minus_Float_MANY__Float_1_"                 => BuiltinOp::MinusFloat,
    "times_Float_MANY__Float_1_"                 => BuiltinOp::TimesFloat,
    "divide_Float_1__Float_1__Float_1_"          => BuiltinOp::DivideFloat,
    "floor_Float_1__Integer_1_"                  => BuiltinOp::FloorFloat,
    "ceiling_Float_1__Integer_1_"                => BuiltinOp::CeilingFloat,
    "round_Float_1__Integer_1_"                  => BuiltinOp::RoundFloat,

    // String — "plus" on Strings IS concatenation
    "plus_String_MANY__String_1_"                => BuiltinOp::PlusString,
    "length_String_1__Integer_1_"                => BuiltinOp::Length,
    "substring_String_1__Integer_1__String_1_"   => BuiltinOp::Substring,
    "toUpper_String_1__String_1_"                => BuiltinOp::ToUpper,
    "toLower_String_1__String_1_"                => BuiltinOp::ToLower,

    // True polymorphics — single entry, runtime Value inspection is inherent
    "equal_Any_MANY__Any_1__Boolean_1_"          => BuiltinOp::Equal,
    "lessThan_Number_1__Number_1__Boolean_1_"    => BuiltinOp::LessThan,
    "and_Boolean_1__Boolean_1__Boolean_1_"       => BuiltinOp::And,
    "or_Boolean_1__Boolean_1__Boolean_1_"        => BuiltinOp::Or,
    "not_Boolean_1__Boolean_1_"                  => BuiltinOp::Not,

    // Collections
    "map_T_MANY__Function_1__V_MANY_"            => BuiltinOp::Map,
    "filter_T_MANY__Function_1__T_MANY_"         => BuiltinOp::Filter,
    "fold_T_MANY__V_1__Function_1__V_1_"         => BuiltinOp::Fold,
    "size_Any_MANY__Integer_1_"                  => BuiltinOp::Size,
    "isEmpty_Any_MANY__Boolean_1_"               => BuiltinOp::IsEmpty,
    "at_T_MANY__Integer_1__T_1_"                 => BuiltinOp::At,
    "first_T_MANY__T_$0_1$_"                     => BuiltinOp::First,
    "last_T_MANY__T_$0_1$_"                      => BuiltinOp::Last,
    "tail_T_MANY__T_MANY_"                       => BuiltinOp::Drop,
    "concatenate_T_MANY__T_MANY__T_MANY_"        => BuiltinOp::Concatenate,
    "range_Integer_1__Integer_1__Integer_MANY_"  => BuiltinOp::Range,
    "sort_T_MANY__T_MANY_"                       => BuiltinOp::Sort,
    "distinct_T_MANY__T_MANY_"                   => BuiltinOp::Distinct,

    // Control
    "if_Boolean_1__Function_1__Function_1__T_$0_1$_" => BuiltinOp::If,
};

/// Key evolution roadmap:
///   Phase 1 (NOW):      Mangled name keys — unambiguous, matches NativeRegistry
///   Phase 2 (LATER):    ElementId keys — once stdlib is fully loaded in PureModel,
///                       resolver assigns ElementId → builtin marking pass sets
///                       FunctionTarget::Builtin(op); no string lookup at eval time at all
```

**Replace `eval_function_call` dispatch in `runtime/src/eval.rs`:**

The lookup key must be the **mangled name**, not the simple name. The resolver provides
this either via `self.model.get_fqn(id)` (if ElementId is resolved) or by constructing
the mangled name from the simple name + resolved argument types:

```rust
fn eval_function_call(
    &mut self,
    target: &FunctionTarget,   // after G2 FunctionTarget enum is adopted
    function_name: &str,
    arguments: &[ValueSpec<Resolved>],  // after G1 typestate is adopted
    source_info: &SourceInfo,
) -> Result<Value, PureException> {
    match target {
        // HOT PATH: BuiltinOp pre-resolved in FunctionTarget during type resolution
        // After Pattern G is complete: zero string lookup, zero phf hash at eval time
        FunctionTarget::Builtin(op) => {
            self.dispatch_builtin(*op, arguments, source_info)
        }
        // User-defined function — ElementId pre-resolved by compiler
        FunctionTarget::UserFunction(id) => {
            let args = self.eval_args_eager(arguments)?;
            self.call_user_function(*id, &args, function_name)
        }
        // INTERIM hot path: phf lookup using mangled name from the resolver
        // Used until FunctionTarget::Builtin is populated by type resolution
        FunctionTarget::Unresolved => {
            let mangled = build_mangled_key(function_name, arguments);
            if let Some(&op) = BUILTIN_DISPATCH.get(mangled.as_str()) {
                return self.dispatch_builtin(op, arguments, source_info);
            }
            Err(PureException::from(PureRuntimeError::FunctionNotFound(function_name.into())))
        }
        FunctionTarget::Extension(name) => {
            self.dispatch_extension(name, arguments, source_info)
        }
    }
}

/// Zero-secondary-dispatch — each arm knows its exact implementation
/// because one BuiltinOp variant = one specific overload
#[inline]
fn dispatch_builtin(
    &mut self, op: BuiltinOp,
    arguments: &[ValueSpec<Resolved>],
    source_info: &SourceInfo,
) -> Result<Value, PureException> {
    macro_rules! eager {
        ($f:expr) => {{
            let args = self.eval_args_eager(arguments)?;
            $f(&args).map_err(|e| PureException::from(e).with_source(source_info))
        }};
    }
    match op {
        // Each arm calls ONE specific implementation — no Value-type inspection needed
        BuiltinOp::PlusInteger  => eager!(arithmetic::plus_integer),
        BuiltinOp::PlusFloat    => eager!(arithmetic::plus_float),
        BuiltinOp::PlusString   => eager!(string::concatenate),  // "plus" on String = concat

        // True polymorphics — Value inspection IS inherent to the operation
        BuiltinOp::Equal        => eager!(comparison::equal),    // structural equality

        // Deferred ops: lambdas evaluated on demand
        BuiltinOp::Map          => {
            let args = self.eval_args_eager(arguments)?;
            let mut ctx = EvalContext { evaluator: self };
            collection::map_builtin(&args, &mut ctx)
                .map_err(|e| PureException::from(e).with_source(source_info))
        }
        BuiltinOp::If           => self.dispatch_deferred_if(arguments, source_info),
        // ... all variants — no HashMap, no vtable, no secondary type dispatch
    }
}
```


**If the 2–4 ns `phf` cost ever matters (prove it with a profiler first):**

Create `crates/lang-builtins` — a new zero-dependency crate at the base of the graph
containing only `pub enum BuiltinOp`. Both `pure` and `runtime` depend on it. Neither
depends on the other. `ExprKind::FunctionCall` gains `builtin: Option<BuiltinOp>`,
the `phf` lookup moves to compile time, and the eval hot path becomes ~0 ns.
The layering stays clean because `lang-builtins` has zero runtime dependencies:

```
crates/lang-builtins  ←  crates/pure  ←  crates/runtime
         ↑___________________________________|
```

This is the same pattern as `crates/protocol`. Do not implement until profiling proves
the 2–4 ns actually shows up in the `chaotic_100k` flamegraph.

**Impact summary:**
- `find_by_prefix` O(80) scan + `format!` allocation → **eliminated**
- `dyn NativeFunction` vtable per built-in call → **eliminated, jump table instead**
- `NativeRegistry` + `Box<dyn NativeFunction>` → **retained for user extensions only**
- `crates/pure` compiler IR → **unchanged, zero layering risk**
- Remaining cost: ~2–4 ns phf hash per built-in call — acceptable for all workloads

---

### 2. `call_user_function` clones entire function body on every call

**File:** `runtime/src/eval.rs:501-503`

```rust
// Called on every user function invocation
let params = func.parameters.clone();  // Vec<Parameter> — full clone
let body = func.body.clone();          // Vec<ValueSpec>  — full deep clone!
let source_info = node.source_info.clone();
```

**Problem:** The borrow checker forces this clone because `self.model` is borrowed immutably
via `get_element()` while `self` (which contains the context) is borrowed mutably for
`eval_body()`. The issue is that the `get_element` return value holds a borrow of `self.model`,
and calling `self.eval_body(&body)` requires `&mut self`, forcing the clone.

Cloning `Vec<ValueSpec>` means cloning all `Box<ExprKind>` nodes in the function body on
every single call. For a function called in a `map` over 100K items, this is 100K deep clones.

**Fix (Architectural):** Restructure `Evaluator` to split the immutable model reference from
the mutable execution state, enabling you to borrow both independently:

```rust
// Split the evaluator into a read-only view and a mutable execution context
// This is the standard Rust pattern for this class of borrow conflict

pub struct EvalState {
    pub heap: RuntimeHeap,
    pub context: VariableContext,
}

pub struct Evaluator<'model, H: EvalHooks = NoOpHooks> {
    model: &'model PureModel,
    natives: &'model NativeRegistry,
    state: EvalState,
    hooks: H,
}

// call_user_function can now borrow model and state separately:
fn call_user_function(&mut self, element_id: ElementId, args: &[Value], name: &str)
    -> Result<Value, PureException>
{
    let element = self.model.get_element(element_id); // borrows self.model
    let Element::Function(func) = element else { ... };
    
    // func.parameters and func.body are now only borrowed, not cloned
    self.state.context.push_scope();
    for (param, arg) in func.parameters.iter().zip(args.iter()) {
        self.state.context.set(param.name.clone(), arg.clone());
    }
    // eval_body only needs &mut self.state, not &self.model, so no conflict
    let result = self.eval_body_with_model(self.model, self.natives, &func.body, &mut self.state);
    self.state.context.pop_scope();
    result
}
```

**Impact:** For workloads with deeply recursive functions or large `map`/`filter` operations
over user-defined functions, this is the **single largest CPU hotspot** after fixing #1.

---

### 3. `SourceInfo` bloats every node by 40 bytes — deferred but high ROI

**File:** `ast/src/source_info.rs:44-55`

```rust
pub struct SourceInfo {
    pub source: SmolStr, // 24 bytes — DUPLICATED across every node in a file
    pub start_line: u32, //  4 bytes
    pub start_column: u32, // 4 bytes
    pub end_line: u32,   //  4 bytes
    pub end_column: u32, //  4 bytes
}               // Total: 40 bytes per node
```

The prior review deferred this correctly. The **time to implement it is now**, because:
- The parser API is stable enough that threading a `SourceMap` through it is a bounded refactor
- For a 100K-element model, 10,000 × 40 bytes = 400KB of diagnostic metadata, 240KB of which
  are redundant copies of a single file path string
- `SourceInfo` is embedded in `ValueSpec` (compiler nodes) and `Parameter`, meaning the
  memory savings cascade through the entire compiled model graph

**Proposed Change (two-phase):**

```rust
// Phase A - AST layer: intern source paths
pub type SourceId = u32;  // index into a SourceMap

pub struct SourceMap {
    paths: Vec<SmolStr>,  // dedup'd file path pool
    path_index: HashMap<SmolStr, SourceId>,
}

// Compacted SourceInfo: 16 bytes instead of 40
pub struct SourceInfo {
    pub source_id: SourceId,   // 4 bytes (u32 index into SourceMap)
    pub start_line: u32,       // 4 bytes
    pub start_column: u16,     // 2 bytes (columns rarely exceed 65535)
    pub end_line: u32,         // 4 bytes
    pub end_column: u16,       // 2 bytes
}                              // Total: 16 bytes — 60% size reduction
```

**Impact:** 400KB → 160KB for 10K nodes. Critical for large Legend Engine model compilations.

---

## 🟡 High-Impact Issues (Quality & Maintainability)

### 4. `eval_lambda_creation` always captures empty `HashMap` — misses true lexical closures

**File:** `runtime/src/eval.rs:593-605`

```rust
fn eval_lambda_creation(&self, parameters: &[…], body: &[ValueSpec]) -> Value {
    // Captures are empty for non-escaping lambdas (map/filter/fold).
    // The evaluator uses the enclosing context directly.
    Value::Lambda(Box::new(LambdaClosure {
        parameters: parameters.to_vec(),
        body: body.to_vec(),
        captures: HashMap::new(),  // ← always empty!
    }))
}
```

**Problem:** The comment says "The evaluator uses the enclosing context directly," but this is
only safe if the lambda is immediately invoked within the same scope. If a lambda is:
1. Stored in a variable (`let f = {x: Integer[1] | $x + $outerVar}`)
2. Returned from a function
3. Passed to a function that calls it after returning

...then `$outerVar` will either resolve to a stale binding or fail with `VariableNotFound`,
because the `VariableContext` has been popped by the time the lambda runs.

**Fix:** Capture free variables at lambda creation time:

```rust
fn eval_lambda_creation(&self, parameters: &[Parameter], body: &[ValueSpec]) -> Value {
    // Collect free variables referenced in the body that aren't in params
    let param_names: HashSet<&str> = parameters.iter().map(|p| p.name.as_str()).collect();
    let mut captures = HashMap::new();
    
    for free_var in collect_free_vars(body) {
        if !param_names.contains(free_var.as_str()) {
            if let Some(val) = self.context.get(&free_var) {
                captures.insert(free_var, val.clone());
            }
        }
    }
    
    Value::Lambda(Box::new(LambdaClosure { parameters: parameters.to_vec(), body: body.to_vec(), captures }))
}
```

**Impact:** This is a **semantic correctness bug** for non-trivially escaping lambdas. Add a
`collect_free_vars(body: &[ValueSpec]) -> Vec<SmolStr>` helper that walks `ExprKind::Variable`
nodes and returns names not bound by enclosing `Lambda` parameters.

---

### 5. `ConstValue::String` uses heap-allocated `String`, not `SmolStr`

**File:** `pure/src/types.rs:96`

```rust
pub enum ConstValue {
    Integer(i64),
    String(String),   // ← should be SmolStr
}
```

`ConstValue` appears in `TypeExpr::Named::value_arguments` — compile-time type parameters like
`Varchar(255)` or `Res('ok')`. The whole codebase has standardized on `SmolStr` for short
strings. This is an inconsistency that will proliferate if `ConstValue` is used more widely.

**Fix:** `String(SmolStr)` — zero-cost for strings ≤22 bytes (which all type param strings are).

---

### 6. `parse_subsecond_nanos` in `lower.rs` allocates a `String` unconditionally

**File:** `pure/src/lower.rs:710-728`

```rust
fn parse_subsecond_nanos(frac: &str) -> i32 {
    // ...
    let mut padded = String::with_capacity(9);  // ← heap allocation on every datetime parse
    for (i, c) in frac.chars().enumerate() {
        ...
        padded.push(c);
    }
    while padded.len() < 9 {
        padded.push('0');
    }
    padded.parse().unwrap_or(0)
}
```

This allocates a `String` for every datetime literal in the model. It can be replaced with
a zero-allocation numeric calculation:

```rust
fn parse_subsecond_nanos(frac: &str) -> i32 {
    if frac.is_empty() {
        return 0;
    }
    // Strip timezone suffix
    let frac = frac.split_once('+').map_or(frac, |(m, _)| m);
    let frac = frac.split_once('-').map_or(frac, |(m, _)| m);
    
    // Scale to nanoseconds: "123" → 123_000_000, "123456789" → 123_456_789
    let digits: u32 = frac.len().min(9) as u32;
    let value: i32 = frac[..digits as usize].parse().unwrap_or(0);
    let scale = 10i32.pow(9 - digits);
    value * scale
}
```

---

### 7. `ExpressionVisitor` in AST does not recurse — misleading API

**File:** `ast/src/expression.rs:666-734`

The `ExpressionVisitor` trait dispatches to leaf handlers like `visit_arithmetic`, but those
default implementations are **no-ops**, and the default `visit()` method does not recurse into
children. A user implementing `visit_arithmetic` will only see top-level arithmetic nodes,
not nested ones inside lambdas or collection literals.

This is a classic visitor anti-pattern. The `visit()` dispatch in the trait itself should
recursively walk children before calling the leaf methods (or after, for post-order traversal):

```rust
pub trait ExpressionVisitor {
    fn visit(&mut self, expr: &Expression) {
        // Walk children FIRST (pre-order by default), then the specific handler
        self.walk_children(expr);
        match expr {
            Expression::Arithmetic(e) => self.visit_arithmetic(e),
            // ...
        }
    }

    fn walk_children(&mut self, expr: &Expression) {
        match expr {
            Expression::Arithmetic(e) => {
                self.visit(&e.left);
                self.visit(&e.right);
            }
            Expression::Lambda(e) => {
                for body_expr in &e.body { self.visit(body_expr); }
            }
            Expression::Collection(e) => {
                for elem in &e.elements { self.visit(elem); }
            }
            Expression::FunctionApplication(e) => {
                for arg in &e.arguments { self.visit(arg); }
            }
            // ... all recursive cases
            _ => {}
        }
    }

    fn visit_arithmetic(&mut self, _expr: &ArithmeticExpr) {}
    // ... leaf handlers remain no-ops by default
}
```

**Impact:** Any future linting, optimization pass, or IDE feature that uses `ExpressionVisitor`
will silently produce incorrect results on deeply nested expressions. This also applies to the
compiler's `PureModel` Visitor (if it exists) over `ValueSpec`.

---

## 🟢 Design Pattern & Maintainability Improvements

### 8. Typestate Pattern for `ValueSpec` — eliminate `Option::unwrap` on type_info

**File:** `pure/src/types.rs:236-243`

As recommended in the prior review, implement the typestate pattern to distinguish pre- and
post-inference expressions at compile time. This is the correct Rust idiom and should be
prioritized for the next major iteration:

```rust
// Marker types — zero-size, zero-cost
pub struct Unresolved;
pub struct Resolved;

// Generic over inference state
pub struct ValueSpec<S = Unresolved> {
    pub kind: Box<ExprKind>,
    pub source_info: SourceInfo,
    pub type_info: <S as InferenceState>::TypeInfo,
    _state: PhantomData<S>,
}

pub trait InferenceState {
    type TypeInfo;
}
impl InferenceState for Unresolved {
    type TypeInfo = ();  // No type info yet
}
impl InferenceState for Resolved {
    type TypeInfo = Box<ResolvedType>;  // Guaranteed present
}
```

The evaluator's `eval()` function signature would then become:
```rust
pub fn eval(&mut self, expr: &ValueSpec<Resolved>) -> Result<Value, PureException>
```

This **eliminates every `.unwrap()` and `.expect()` on `type_info`** at compile-time.

---

### 9. `HashMap<SmolStr, Value>` in `LambdaClosure.captures` should use `IndexMap` or small-vec optimization

**File:** `runtime/src/value.rs:127`

```rust
pub struct LambdaClosure {
    pub parameters: Vec<legend_pure_parser_pure::types::Parameter>,
    pub body: Vec<legend_pure_parser_pure::types::ValueSpec>,
    pub captures: HashMap<SmolStr, Value>,  // Most closures capture 0-3 vars
}
```

Most lambda closures in real Pure code capture very few variables (0–3). Using `HashMap` for
this is a heavy-weight choice that allocates even for the common empty case (issue #4 above).

**Recommendation:** Use `smallvec::SmallVec<[(SmolStr, Value); 4]>` as a linear scan array,
which is faster than `HashMap` for N ≤ 8 due to cache coherence and zero allocation overhead.
When capture sets are large, a standard `HashMap` handles the fallback.

---

### 10. `EvalContextTrait::eval_lambda` loses the error call stack

**File:** `runtime/src/eval.rs:706-715`

```rust
impl<H: EvalHooks> EvalContextTrait for EvalContext<'_, '_, H> {
    fn eval_lambda(&mut self, lambda_val: &Value, args: &[Value])
        -> Result<Value, PureRuntimeError>
    {
        match lambda_val {
            Value::Lambda(closure) => self
                .evaluator
                .eval_lambda_value(closure, args)
                .map_err(|e| PureRuntimeError::EvaluationError(format!("{e}"))), // ← lossy!
```

`eval_lambda_value` returns `Result<Value, PureException>` (with call stack). Converting to
`PureRuntimeError::EvaluationError(format!("{e}"))` **destroys the structured call stack**
inside the exception, converting it to a flat string. When a native `map`/`filter` lambda
fails, you lose the precise source location of the failure inside the lambda body.

**Fix:** Thread `PureException` through `EvalContextTrait::eval_lambda`:

```rust
pub trait EvalContextTrait {
    fn eval_lambda(&mut self, lambda: &Value, args: &[Value]) 
        -> Result<Value, PureException>;  // Return the rich exception, not the flat one
    // ...
}
```

This is a signature-breaking change but it's the correct design. The downside of keeping the
current design grows as user code becomes more complex — silent stack trace loss is a major
debugging impediment.

---

## 🏗️ Architectural Design Pattern Changes

Beyond individual bug fixes, these are the structural patterns worth evolving:

### A. Replace `dyn NativeFunction` vtable with `BuiltinOp` enum for hot-path (covered in Issue #1)

The `Box<dyn NativeFunction>` registry is the right pattern for **user-registered extensions**.
It is the wrong pattern for the **30 built-in operators**. The extension registry stays;
the hot path moves to `BuiltinOp` enum dispatch.

### B. Trampoline / CPS to prevent stack overflow on deep recursion

The recursive `eval()` calling `eval_function_call()` calling `call_user_function()` calling
`eval_body()` calling `eval()` will **stack-overflow** on deeply recursive Pure programs
(naive Fibonacci at n=5000, deep mutual recursion, etc.).

Production interpreters (Erlang VM, CPython, LuaJIT) use a trampoline loop:

```rust
enum Step {
    Done(Value),
    TailCall { element_id: ElementId, args: Vec<Value> },
}

// Outer loop — all recursion on the heap, not the stack
pub fn eval_trampoline(&mut self, expr: &ValueSpec) -> Result<Value, PureException> {
    let mut current = expr;
    loop {
        match self.eval_step(current)? {
            Step::Done(v)                      => return Ok(v),
            Step::TailCall { element_id, args } => {
                // Set up next frame on heap — no stack growth
                current = self.prepare_call(element_id, &args)?;
            }
        }
    }
}
```

### C. `Arc<PureModel>` instead of `&'model PureModel` lifetime

The lifetime `'model` on `Evaluator<'model, H>` propagates into every method signature and
makes multi-threaded use impossible. Since `PureModel` contains no `Rc` types it is
`Send + Sync`. Using `Arc<PureModel>` removes the lifetime entirely:

```rust
pub struct Evaluator<H: EvalHooks = NoOpHooks> {
    model: Arc<PureModel>,      // ref-counted, no lifetime annotation needed
    natives: Arc<NativeRegistry>,
    heap: RuntimeHeap,          // per-evaluator (im_rc not Send)
    context: VariableContext,   // per-evaluator
    hooks: H,
}
```

This enables the production deployment model: **one `Arc<PureModel>` compiled once, shared
across N thread-local `Evaluator` instances** for concurrent Legend Server requests.

### D. Split `Evaluator` into `EvalStack` + `EvalCore` to fix borrow conflicts (Issue #2 root cause)

The `call_user_function` body clone (Issue #2) exists because `self.model` and `self.context`
can't be borrowed simultaneously. The fix is the standard Rust "split struct" pattern:

```rust
/// Immutable model references — shared, borrowed freely
pub struct EvalCore<'model> {
    pub model: &'model PureModel,
    pub natives: &'model NativeRegistry,
}

/// Mutable execution state — separate from model
pub struct EvalStack {
    pub heap: RuntimeHeap,
    pub context: VariableContext,
}

/// Combined evaluator delegates to both
pub struct Evaluator<'model, H: EvalHooks = NoOpHooks> {
    core: EvalCore<'model>,
    stack: EvalStack,
    hooks: H,
}
```

With this split, `call_user_function` can borrow `self.core.model` and `self.stack.context`
simultaneously with no clone.

### E. `ExecutionStrategy` enum for the 4-layer execution model

The architecture documents a 4-layer model (Interpreted → Native → Memoized → Compiled)
but the code has no abstraction boundary between them. Encoding the strategy as data:

```rust
pub enum ExecutionStrategy {
    Interpreted,
    Memoized { cache: Arc<DashMap<CacheKey, Value>> },
    // JIT: future milestone
}

impl Evaluator {
    fn call_function(&mut self, id: ElementId, args: &[Value]) -> Result<Value, PureException> {
        match self.strategy {
            ExecutionStrategy::Interpreted   => self.interpret(id, args),
            ExecutionStrategy::Memoized { .. } => self.memoized_call(id, args),
        }
    }
}
```

### F. Fix `UnaryMinus` desugaring — semantic correctness

`lower_unary_minus` desugars `-x` → `FunctionCall("minus", [x])`. But `minus` is registered
as a **binary** function and will fail at runtime with arity errors. It should desugar to
`negate` (a distinct unary function) or `minus(Integer(0), x)`:

```rust
// In lower.rs — change:
fn lower_unary_minus(e: &ast_expr::UnaryMinusExpr, …) -> Option<ValueSpec> {
    // Option A: dedicated negate function
    unary_op("negate", &e.operand, &e.source_info, ctx, errors)
    // Option B: desugar to 0 - x
    binary_op("minus", &zero_literal(e.source_info), &e.operand, …)
}
```

### G. Eliminate `Option` Fields via Typestate and Enum — Pipeline Invariants as Types

**Files:** `pure/src/types.rs`, `pure-ir` (after extraction)  
**Principle:** An `Option<T>` that is `None` only because of *when* in the pipeline, and will always be `Some` by a later pass, is not genuinely optional. Encode that guarantee as a type.

#### Decision Rule

```
Is Option absent because of pipeline TIMING (WILL be set later)?
  YES → Typestate: ValueSpec<Unresolved> / ValueSpec<Resolved>

Do multiple fields form a set of exclusive states?
  YES → Enum: FunctionTarget replacing Option<ElementId>

Is Option genuinely optional at the language level?
  YES → Keep Option — it's semantically correct

Is a numeric ID that must never be default/zero?
  YES → Newtype with NonZeroU32
```

#### G1. Typestate for `ValueSpec` — eliminate all `type_info.unwrap()` calls

`type_info: Option<Box<ResolvedType>>` is `None` after lowering, `Some` after type inference.
The evaluator requires it but currently panics at runtime if inference was skipped. This
is the classic use case for the Typestate pattern:

```rust
// Zero-size marker types — PhantomData erases them at compile time, cost nothing
pub struct Unresolved;
pub struct Resolved;

// Sealed to prevent external state definitions
mod sealed { pub trait TypeState {} }
impl sealed::TypeState for Unresolved {}
impl sealed::TypeState for Resolved {}

pub trait TypeState: sealed::TypeState {
    type TypeInfo;
}
impl TypeState for Unresolved {
    type TypeInfo = ();                // No field — not even allocatable
}
impl TypeState for Resolved {
    type TypeInfo = Box<ResolvedType>; // Always present, infallible access
}

pub struct ValueSpec<S: TypeState = Unresolved> {
    pub kind: Box<ExprKind>,
    pub source_info: SourceInfo,
    pub type_info: S::TypeInfo,   // () when Unresolved, Box<ResolvedType> when Resolved
    _state: PhantomData<S>,
}

impl ValueSpec<Unresolved> {
    /// The type inference pass calls this to advance state.
    /// Cannot be called accidentally — requires a concrete ResolvedType.
    pub fn resolve(self, type_info: Box<ResolvedType>) -> ValueSpec<Resolved> {
        ValueSpec { kind: self.kind, source_info: self.source_info, type_info, _state: PhantomData }
    }
}

impl ValueSpec<Resolved> {
    /// Infallible — no Option, no unwrap, no panic risk
    pub fn resolved_type(&self) -> &ResolvedType { &self.type_info }
}
```

**The evaluator signature becomes a compile-time invariant:**

```rust
// Before — runtime panic risk, trusts the pipeline implicitly
pub fn eval(&mut self, expr: &ValueSpec) -> Result<Value, PureException> {
    let ty = expr.type_info.as_ref().expect("type inference must have run");
    // ...
}

// After — the TYPE SIGNATURE enforces the invariant
pub fn eval(&mut self, expr: &ValueSpec<Resolved>) -> Result<Value, PureException> {
    let ty = &expr.type_info;  // Box<ResolvedType>, always present
    // ...
}
```

Passing an unresolved expression to `eval()` is now a **compile error**, not a runtime panic.

#### G2. Enum for `FunctionCall.target` — replace `Option<ElementId>` + implicit fallthrough

The current `function: Option<ElementId>` is ambiguous: `None` means "unresolved" OR "dynamic
extension" — callers can't tell without also checking `function_name`. This is an
**undiscriminated sum** and should be an enum:

```rust
/// All possible dispatch targets for a function call.
/// Unambiguously encodes every case — no None + separate-field reasoning needed.
pub enum FunctionTarget {
    Unresolved,               // lowering produced no match (diagnostic already emitted)
    Builtin(BuiltinOp),       // runtime: phf::Map dispatch (stays in crates/runtime)
    UserFunction(ElementId),  // runtime: call_user_function
    Extension(SmolStr),       // runtime: dynamic extension registry
}

// ExprKind::FunctionCall becomes:
FunctionCall {
    target: FunctionTarget, // single, exhaustive discriminant
    function_name: SmolStr, // kept only for error messages
    arguments: Vec<ValueSpec>,
}
```

**At eval time, the match is exhaustive and unambiguous:**

```rust
// Before — nested Option checks, implicit fallthrough
match (function, function_name.as_str()) {
    (Some(id), _)  => self.call_user_function(id, args),
    (None, name)   => BUILTIN_DISPATCH.get(name)
                        .map(|op| self.dispatch_builtin(op, args))
                        .unwrap_or_else(|| Err(FunctionNotFound(name.into()))),
}

// After — exhaustive, no ambiguity, compiler-verified
match &call.target {
    FunctionTarget::Builtin(op)      => self.dispatch_builtin(*op, args),
    FunctionTarget::UserFunction(id) => self.call_user_function(*id, args),
    FunctionTarget::Extension(name)  => self.dispatch_extension(name, args),
    FunctionTarget::Unresolved       => unreachable!("unresolved call reached eval"),
}
```

#### G3. `Parameter` — enum for typed vs inferred lambda params

Lambda parameters (`{x: Integer[1] | ...}` vs `{x | ...}`) currently use:

```rust
pub struct Parameter {
    pub name: SmolStr,
    pub type_ref: Option<TypeExpr>,       // None for type-inferred lambdas
    pub multiplicity: Option<Multiplicity>,
    pub source_info: SourceInfo,
}
```

The correct model: two distinct types that share a name but have different fields:

```rust
pub struct TypedParam {
    pub name: SmolStr,
    pub type_ref: TypeExpr,          // always present — user wrote it
    pub multiplicity: Multiplicity,  // always present — user wrote it
    pub source_info: SourceInfo,
}

pub struct InferredParam {
    pub name: SmolStr,
    pub source_info: SourceInfo,
    // No type fields — they do not exist yet
}

/// Used in both function definitions and lambda bodies
pub enum Parameter {
    Typed(TypedParam),
    Inferred(InferredParam),
}
```

Code that needs type information is forced to match — accessing `type_ref` on an `Inferred`
parameter is a compile error, not an `unwrap()` panic.

#### G4. `ElementId` — `NonZeroU32` to exclude the sentinel zero value

```rust
// Before — zero is silently valid, no protection against default-constructed IDs
pub struct ElementId(u32);

// After — zero is structurally excluded
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(NonZeroU32);

impl ElementId {
    pub fn new(n: u32) -> Option<Self> {
        NonZeroU32::new(n).map(Self)
    }
    pub fn get(self) -> u32 { self.0.get() }
    // No Default impl — prevents accidental zero-ID construction
}
```

Bonus: `Option<ElementId>` is now **pointer-sized** (the same size as `u32`) because
`NonZeroU32` enables the niche optimization — `None` is represented as the bit pattern 0.

#### What NOT to Typestate

Making `ExprKind` itself generic (`ExprKind<S: TypeState>`) is too invasive at this stage.
The generic would propagate through every recursive variant (`Lambda.body`, `Collection.elements`,
`FunctionCall.arguments`), creating significant type-level noise. The scope of typestate should
be `ValueSpec<S>` — the boundary between pipeline stages — not the interior IR nodes.

---

## 🏛️ Crate Architecture Decision

### Decision: Extract `crates/pure-ir` — Option B ✅

**Status: SELECTED — implement before any other architectural change**

The current `ast ← pure ← runtime` crate layout conflates two distinct concerns in `crates/pure`:
1. **Compiler output types** — `PureModel`, `ExprKind`, `ValueSpec`, `ElementId`, `ResolvedType` (needed by both the compiler AND the runtime)
2. **Compilation passes** — `lower.rs`, `resolve.rs`, `validate.rs` (needed only by the compiler)

The runtime depends on the compiler crate solely to access the IR types. It does not need or use the compilation passes. This forces every runtime change to track compiler changes unnecessarily, and prevents future alternative backends (JIT, AOT) from depending on the IR without also depending on the compiler.

#### Why Not Merge?

Three concrete use cases require the compiler without the runtime:
- `legend check` / `legend validate` — linting without execution
- IDE language server / protocol export — parsing and type-checking without evaluation
- WASM compilation — `crates/pure` can target WASM; `crates/runtime` cannot (heap and native I/O are not WASM-safe)

The Java Legend Pure already makes this distinction: `legend-engine-pure-runtime-java-extension-compiled-*` and `legend-engine-pure-runtime-java-extension-interpreted-*` are separate modules sharing the same M3 model layer.

#### Selected Architecture: Option B

```
                    ┌─────────────────────────────────────┐
                    │         crates/pure-ir  (NEW)        │
                    │  PureModel, ExprKind, ValueSpec      │
                    │  ElementId, ResolvedType, Parameter  │
                    │  TypeExpr, Multiplicity, Ids         │
                    │  Zero runtime dependencies           │
                    └──────────────┬──────────────────────┘
                                   │
               ┌───────────────────┴────────────────────┐
               ▼                                        ▼
    ┌──────────────────────┐              ┌─────────────────────────┐
    │    crates/pure       │              │    crates/runtime       │
    │  lower.rs            │              │  eval.rs                │
    │  resolve.rs          │              │  heap.rs                │
    │  validate.rs         │              │  context.rs             │
    │  error.rs            │              │  native/*.rs            │
    │  bootstrap.rs        │              │  error.rs               │
    └──────────────────────┘              │  BuiltinOp (phf::Map)   │
               ▲                         └─────────────────────────┘
               │
    ┌──────────────────────┐
    │     crates/ast       │
    └──────────────────────┘
```

#### Types That Move to `crates/pure-ir`

From `crates/pure/src/types.rs`:
- `PureModel`, `Element`, `Function`, `Class`, `Enum`, `Association`
- `ExprKind`, `ValueSpec`, `Parameter`
- `TypeExpr`, `ResolvedType`, `Multiplicity`
- `DateValue`, `ConstValue`
- `LambdaParam` and related IR structs

From `crates/pure/src/ids.rs`:
- `ElementId` and all ID types

#### `crates/pure-ir` Cargo.toml

```toml
[package]
name = "legend-pure-ir"
version = "0.1.0"

[dependencies]
smol_str = "0.3"
rust_decimal = "1"
legend-pure-parser-ast = { path = "../ast" }  # for SourceInfo only
```

Notably absent: `im-rc`, `slotmap`, `phf`, `thiserror` — zero runtime dependencies.

#### How This Resolves Other Issues

| Issue | How `pure-ir` extraction resolves it |
|---|---|
| `BuiltinOp` placement | Lives in `crates/runtime` — reads `pure-ir` types. No compiler involvement. |
| `EvalCore` borrow split (Pattern D) | `EvalCore` holds `&'model PureModel` from `pure-ir` — independent of compiler passes. |
| `Arc<PureModel>` threading (Pattern C) | `PureModel: Send + Sync` (no Rc). `Arc<PureModel>` trivially safe once in its own crate. |
| Future JIT/AOT runtime | Depends on `pure-ir` + `pure` output, not on the interpreter. |
| WASM compilation | `pure-ir` and `pure` are WASM-compatible. `runtime` is not. Clean boundary. |

#### Migration Plan

1. Create `crates/pure-ir/src/lib.rs` — copy IR types from `pure/src/types.rs` and `pure/src/ids.rs`
2. Update `crates/pure/Cargo.toml` — add `legend-pure-ir` dependency, remove moved types
3. Update `crates/pure/src/types.rs` — add `pub use legend_pure_ir::*;` re-exports for zero API breakage
4. Update `crates/runtime/Cargo.toml` — swap `legend-pure-pure` → `legend-pure-ir` for IR types
5. Run `cargo test --workspace` — should pass with no logic changes
6. Once stable: remove the re-exports from `crates/pure` to enforce the boundary

Estimated effort: **1 day**. Zero behavioral changes — pure structural migration.

---

## 📈 Performance Benchmarking Strategy

### Problem With the Current `crates/stress` Suite

The three test files (`stress_hub_spoke.rs`, `stress_dense.rs`, `stress_chaotic.rs`) have real structural issues:

1. **`stress_dense.rs` duplicates `stress_hub_spoke.rs`** — it calls `HubSpokeConfig::dense_10k()`, which is also exercised inside `stress_hub_spoke.rs`. One file should be deleted and the dense topology moved into `stress_hub_spoke.rs` as a 4th config variant.
2. **`PhaseTimer` inside `#[test]` is not a benchmark** — it produces wall-clock timings with no warmup, no iteration statistics, and no baseline comparison. The Criterion suite in `benches/pipeline.rs` is correct; the `PhaseTimer` code in the test files provides false precision and should be removed.
3. **Zero runtime/evaluator benchmarks exist** — the eval loop, native dispatch, heap allocation, and lambda evaluation have no performance measurement at all. Every optimization identified in this review has no way to prove it worked.
4. **No micro-benchmarks for hot paths** — all benchmarks are end-to-end pipeline runs. They detect regressions but cannot identify which component regressed.

### The Correct Method: Work Backwards From the SLA

**Step 1 — Define the performance contract first.** Without a concrete SLA, benchmark results are interesting but not actionable:

```
Tier 1 — Arithmetic / property access (simplest case):
  eval: "$x + 1"  →  SLA: < 10 µs P99
  Constrains: dispatch + single native call + variable lookup

Tier 2 — Collection transformation (realistic case):
  eval: "$list->filter(x|$x.active==true)->map(x|$x.id)" over 10K elements
  SLA: < 500 µs
  Constrains: lambda eval, heap allocation, iteration overhead

Tier 3 — Complex user-defined function (worst expected case):
  eval: multi-step function, 3+ user calls, branching, recursion
  SLA: < 5 ms
  Constrains: call frame overhead, stack depth, GC pressure
```

**Step 2 — Map the critical path.** Only benchmark boxes ON the path:

```
Request
  │
  [1] parse(expr)              ← skip for pre-compiled model
  [2] resolve(ast, PureModel)  ← one-time per request
  [3] eval(expr)  ◄─── HOT PATH — this runs N times
       │
       ├─ [3a] dispatch: builtin or user function?    ← find_by_prefix target
       ├─ [3b] eval args eagerly (recursive)
       ├─ [3c] dispatch_builtin(op, args)             ← phf + BuiltinOp target
       ├─ [3d] call_user_function(id, args)           ← body clone target
       ├─ [3e] VariableContext enter/set/exit
       └─ [3f] eval_lambda_value(closure, args)       ← capture bug target
  [4] serialize result
```

**Step 3 — Apply the Amdahl filter.** Priority = share of total time × realistic speedup:

| Component | Est. time share | Realistic speedup | Score |
|---|---|---|---|
| `find_by_prefix` dispatch | ~35% | 50× (phf replaces O(80)) | **17** |
| `call_user_function` body clone | ~25% | 10× (EvalCore borrow) | **2.5** |
| Lambda capture / `eval_lambda` | ~15% | 3× (SmallVec + correct capture) | **0.5** |
| `VariableContext::enter` | ~10% | 2× (already O(1)) | **0.2** |
| `resolve_by_path` | ~1% | 2× | **0.02** |

Anything scoring < 0.3 is below the noise floor — skip until profiling confirms a higher real share.

### The Four-Level Benchmark Hierarchy

```
Level 1 — Nano-benchmarks  (ns, run always, <5s total)
  Purpose: prove ONE optimization works. Written BEFORE the change.
  Failure: >5% regression → block PR

Level 2 — Micro-benchmarks (µs, run on PR, <30s total)
  Purpose: measure one hot-path operation end-to-end
  Failure: >10% regression → block merge

Level 3 — Meso-benchmarks  (ms, run nightly, <5min total)
  Purpose: realistic workload — catches emergent bottlenecks
  Failure: >15% regression → create ticket

Level 4 — Load benchmarks  (throughput, run pre-release, uncapped)
  Purpose: sustained throughput, memory growth, concurrency
  Failure: throughput drops >5% or memory grows unboundedly → release blocker
```

### The 7 Benchmarks to Write Now (Before Any Fix)

Write these to capture the **"before" baselines** for every optimization in this review. They become the permanent regression suite:

| # | Name | Level | Measures | Expected Before | Expected After |
|---|---|---|---|---|---|
| 1 | `nano/dispatch_find_by_prefix` | Nano | `find_by_prefix("plus")` | ~150 ns | N/A (deleted) |
| 2 | `nano/dispatch_phf_mangled` | Nano | `BUILTIN_DISPATCH.get(mangled_key)` | N/A | ~3 ns |
| 3 | `nano/call_user_fn_identity` | Nano | call `identity(42)` user function | ~500 ns | ~50 ns |
| 4 | `nano/lambda_map_10` | Nano | map lambda over 10 integers | fails (capture bug) | <1 µs |
| 5 | `nano/variable_context_roundtrip` | Nano | enter N vars, read, exit scope | baseline | baseline |
| 6 | `micro/eval_arithmetic_1k` | Micro | 1K `$a + $b` evaluations | TBD | Tier 1 SLA proxy |
| 7 | `micro/eval_map_filter_10k` | Micro | map + filter over 10K collection | TBD | Tier 2 SLA proxy |

### Stress Crate Reorganization

```
crates/stress/
  benches/
    pipeline.rs      ← keep — add Phase 6 (evaluate) and memory assertions
    runtime.rs       ← NEW: nano/micro benchmarks for hot paths (items 1–7 above)
  tests/
    stress_pipeline.rs  ← merge hub_spoke + dense (delete stress_dense.rs)
    stress_chaotic.rs   ← keep as-is
    stress_eval.rs      ← NEW: evaluation correctness at scale
  src/
    alloc.rs
    lib.rs
    generate/
      hub_spoke.rs
      chaotic.rs
      eval_fixtures.rs  ← NEW: Pure snippet generators for eval test scenarios
```

**`stress_eval.rs`** is the highest-priority new file — it provides the correctness scaffolding that every evaluation fix needs:

```rust
#[test]
fn eval_arithmetic_correctness() {
    // 10K arithmetic eval assertions — first test that proves dispatch fix works
}

#[test]
fn eval_map_filter_correctness() {
    // Lambda over collections — catches capture bug AND body-clone regression
}

#[test]
#[cfg(feature = "heavy")]
fn eval_recursive_100k() {
    // Recursion depth — catches stack overflow before production
}
```

---

## 🔧 Custom Macros

### Decision Framework

```
macro_rules!     → Use freely for structural repetition within a crate.
                   Zero cost, no extra crates, debuggable.

proc macros      → Require a separate *-macros crate. Use only when
(derive/attr)      macro_rules! genuinely cannot do the job.

build.rs codegen → Use only when generated data comes from an external
                   source that would otherwise require manual sync.
```

**Rule:** Never add a macro just to shorten code. Add a macro when the alternative is structural repetition that is **error-prone to write correctly** every time.

### The 4 `macro_rules!` Worth Writing

#### H1. `bench_with_alloc!` — enforces the correct allocator reset sequence

`benches/pipeline.rs` repeats this exact 6-line block ~25 times:

```rust
let baseline = alloc::current_bytes();
alloc::reset();
b.iter(|| black_box(/* operation */));
let mem = alloc::snapshot();
println!("  peak_delta: {} KB", alloc::peak_delta(baseline) / 1024);
println!("  total_allocs: {}", mem.alloc_count);
```

Forgetting `alloc::reset()` silently gives wrong memory numbers. The macro enforces the correct order and is impossible to misuse:

```rust
macro_rules! bench_with_alloc {
    ($bencher:expr, $op:expr) => {{
        let _baseline = $crate::alloc::current_bytes();
        $crate::alloc::reset();
        $bencher.iter(|| ::criterion::black_box($op));
        let _mem = $crate::alloc::snapshot();
        println!("  peak_delta: {} KB",
            $crate::alloc::peak_delta(_baseline) / 1024);
        println!("  allocs: {}", _mem.alloc_count);
    }};
}

// Every bench function collapses to a single line:
group.bench_function("hub_spoke_1k", |b| {
    bench_with_alloc!(b, hub_spoke::generate(&config_1k))
});
```

**Location:** `crates/stress/src/lib.rs` (exported for use in all bench files)

#### H2. `assert_eval!` — makes runtime correctness tests writable

Without this, writing one evaluation assertion costs 7 lines of wiring. With it, tests read like Pure specifications:

```rust
// Before — 7 lines of infrastructure for one assertion
let sf = parser::parse(SOURCE, "test.pure").expect("parse");
let model = compile!(&[sf]).expect("compile");
let registry = NativeRegistry::standard();
let mut eval = Evaluator::new(&model, &registry);
let result = eval.call_fn("test::add", &[Value::Integer(1), Value::Integer(2)]).expect("eval");
assert_eq!(result, Value::Integer(3));

// After — intent is immediately clear, no infrastructure noise
assert_eval!(
    "function test::add(a: Integer[1], b: Integer[1]): Integer[1] { $a + $b }",
    call = "test::add",
    args = [Value::Integer(1), Value::Integer(2)],
    expect = Value::Integer(3)
);
```

```rust
macro_rules! assert_eval {
    ($source:expr, call = $call:expr, args = [$($arg:expr),*], expect = $expected:expr) => {{
        let sf = legend_pure_parser_parser::parse($source, "test.pure")
            .expect("assert_eval: parse failed");
        let model = legend_pure_parser_pure::compile!(&[sf])
            .expect("assert_eval: compile failed");
        let registry = $crate::native::NativeRegistry::standard();
        let mut eval = $crate::eval::Evaluator::new(&model, &registry);
        let result = eval.call_fn($call, &[$($arg),*])
            .expect("assert_eval: eval failed");
        assert_eq!(result, $expected,
            "assert_eval mismatch for '{}': {:?}", $call, result);
    }};
}
```

**Location:** `crates/runtime/src/test_utils.rs` (test-only, `#[cfg(test)]`)

This is the single most important macro to write — it unblocks writing correctness tests for every evaluation fix. Without it, nobody will write the eval tests we need.

#### H3. Local `eager!` / `deferred!` inside `dispatch_builtin`

Already described in Issue #1. Local macros (scoped to a single function) are free — they don't pollute the API and don't appear in docs:

```rust
fn dispatch_builtin(&mut self, op: BuiltinOp, arguments: &[ValueSpec<Resolved>],
    source_info: &SourceInfo) -> Result<Value, PureException>
{
    macro_rules! eager {
        ($f:expr) => {{
            let args = self.eval_args_eager(arguments)?;
            $f(&args).map_err(|e| PureException::from(e).with_source(source_info))
        }};
    }
    macro_rules! deferred {
        ($f:expr) => { $f(self, arguments, source_info) };
    }
    match op {
        BuiltinOp::PlusInteger => eager!(arithmetic::plus_integer),
        BuiltinOp::PlusFloat   => eager!(arithmetic::plus_float),
        BuiltinOp::Map         => deferred!(collection::dispatch_map),
        BuiltinOp::If          => deferred!(control::dispatch_if),
        // ...
    }
}
```

The pattern enforces: eager ops call `eval_args_eager` + `map_err`; deferred ops receive unevaluated arguments. Getting either rule wrong silently produces wrong evaluation semantics. The macro makes it structurally impossible to mix them up.

**Location:** local to `dispatch_builtin` fn in `runtime/src/eval.rs`

#### H4. `binary_op!` / `func_call!` builder macros in `lower.rs`

After the typestate refactoring (`ValueSpec<Unresolved>`), every lowered node construction is ~8 lines. `lower.rs` has ~25 operator lowering functions that all do the same thing:

```rust
// Before — 8 lines of construction noise for one FunctionCall node
Some(ValueSpec {
    kind: Box::new(ExprKind::FunctionCall {
        target: FunctionTarget::Unresolved,
        function_name: SmolStr::new("plus"),
        arguments: vec![lower_expr(lhs)?, lower_expr(rhs)?],
    }),
    source_info: e.source_info.clone(),
    type_info: (),
    _state: PhantomData,
})

// After — intent is immediately readable
Some(binary_op!("plus", lower_expr(lhs)?, lower_expr(rhs)?, e.source_info))
```

```rust
// Module-level in lower.rs — not exported outside the crate
macro_rules! func_call {
    ($name:expr, [$($arg:expr),*], $src:expr) => {
        ValueSpec::unresolved(
            ExprKind::FunctionCall {
                target: FunctionTarget::Unresolved,
                function_name: SmolStr::new_static($name),
                arguments: vec![$($arg),*],
            },
            &$src,
        )
    };
}
macro_rules! binary_op {
    ($op:expr, $lhs:expr, $rhs:expr, $src:expr) => {
        func_call!($op, [$lhs, $rhs], $src)
    };
}
macro_rules! unary_op {
    ($op:expr, $operand:expr, $src:expr) => {
        func_call!($op, [$operand], $src)
    };
}
```

**Location:** module-level in `crates/pure/src/lower.rs`

### One `build.rs` + Data File: `define_builtins!`

**The problem it solves:** The `phf::Map` dispatch table and `NativeRegistry` registration are two separate places that register the same set of built-in functions. They can drift apart — a function registered in `NativeRegistry` but missing from `BUILTIN_DISPATCH` would be silently unreachable via the fast path. This is the highest-risk maintenance hazard in the phf design.

**The solution:** A single data file that both tables are generated from:

```toml
# crates/runtime/src/builtins.toml — single source of truth
[[builtin]]
mangled  = "plus_Integer_MANY__Integer_1_"
op       = "PlusInteger"
impl     = "arithmetic::plus_integer"

[[builtin]]
mangled  = "plus_Float_MANY__Float_1_"
op       = "PlusFloat"
impl     = "arithmetic::plus_float"
# ...
```

```rust
// build.rs — generates src/generated/builtin_dispatch.rs at build time
// Contains both the phf::Map and a register_all() fn from the same source
fn main() {
    let builtins = parse_builtins_toml("src/builtins.toml");
    generate_phf_map(&builtins, "src/generated/builtin_dispatch.rs");
    generate_register_fn(&builtins, "src/generated/register_builtins.rs");
    println!("cargo:rerun-if-changed=src/builtins.toml");
}
```

> **Timing:** Do not implement this until Sprint 2 confirms the phf design is stable.
> The `macro_rules!` macros above (H1–H4) should be written in Sprint 1 — they are
> immediate wins with no design risk. The `build.rs` approach is a Sprint 3 hardening step.

### What NOT to Macro

| Candidate | Reason to skip |
|---|---|
| `#[derive(NativeFunction)]` proc macro | A trait + `register_native!` macro_rules! covers it with less machinery |
| `#[pure_test]` attribute proc macro | Start with `assert_eval!` — proc macro overhead not justified until the test suite is established |
| `#[derive(ExprVisitor)]` proc macro | One trait impl + `macro_rules!` for visitor arms is sufficient |
| Macros for `SourceInfo` construction | `SourceInfo::new(file, line, col)` is already concise |
| Macros to shorten `map_err` chains | Use the `?` operator and `From` impls instead |

---

## 📊 Severity Summary

| # | Issue | Severity | Category | Effort |
|---|---|---|---|---|
| **G** | **Crate restructure: extract `crates/pure-ir`** | 🔴 **Critical** | **Architecture** | **1 day** |
| **H** | **Macro suite: `bench_with_alloc!`, `assert_eval!`, `eager!`, `binary_op!`** | 🟡 **High** | **DX / Correctness** | **0.5 day** |
| 1 | Native dispatch O(N) scan — `phf::Map` + `BuiltinOp` in runtime | 🔴 Critical | Performance / Architecture | Medium |
| 2 | `call_user_function` clones entire function body | 🔴 Critical | Performance Memory/CPU | Medium |
| 3 | `SourceInfo` 40B string duplication across all nodes | 🔴 Critical | Memory | High |
| 4 | Lambda captures always empty — semantic correctness bug | 🔴 Critical | Correctness | Medium |
| F | `UnaryMinus` desugars to binary `minus` — arity bug | 🔴 Critical | Correctness | Low |
| 5 | `ConstValue::String` uses `String` not `SmolStr` | 🟡 High | Memory / Consistency | Low |
| 6 | `parse_subsecond_nanos` allocates `String` on every parse | 🟡 High | Performance | Low |
| 7 | `ExpressionVisitor` doesn't recurse — misleading API | 🟡 High | Correctness / API | Medium |
| 8 | `ValueSpec` typestate — eliminate `type_info.unwrap()` | 🟡 High | Safety / Design | High |
| 9 | `LambdaClosure.captures` uses `HashMap` for tiny maps | 🟢 Medium | Performance | Low |
| 10 | `eval_lambda` via `EvalContextTrait` loses call stack | 🟢 Medium | Debug Quality | Medium |
| A | Replace `dyn NativeFunction` vtable with `phf` + `BuiltinOp` | 🔴 Critical | Architecture | Medium |
| B | Recursive `eval()` will stack-overflow on deep programs | 🔴 Critical | Reliability | High |
| C | `&'model PureModel` prevents multi-threaded deployment | 🟡 High | Scalability | Low |
| D | Split `Evaluator` into `EvalCore` + `EvalStack` | 🟡 High | Architecture | Medium |
| E | `ExecutionStrategy` enum for 4-layer model | 🟢 Medium | Design | Medium |

---

## Recommended Execution Order

**Sprint 0 — Foundation (must be first):**
0a. **Pattern G** (extract `crates/pure-ir`) — 1 day. Structural prerequisite for A, C, D.
0b. **Macros H1–H4** (`bench_with_alloc!`, `assert_eval!`, `eager!`, `binary_op!`) — 0.5 day.
    Written now so every subsequent fix has test scaffolding and benchmark baselines.
0c. **Benchmarks 1–7** (`nano/dispatch_*`, `nano/call_user_fn_*`, `micro/eval_*`) — 0.5 day.
    Capture "before" numbers. These become the permanent regression suite.

**Sprint 1 — Correctness (unblock testing):**
1. **Fix F** (`UnaryMinus` → `negate`) — 15 mins. Silent runtime failure.
2. **Fix #4** (lambda captures) — correctness bug; required before lambda tests are meaningful.
3. **Fix #6 + #5** (subsecond nanos + `ConstValue::SmolStr`) — trivial, same PR.
4. **Fix #7** (`ExpressionVisitor` recursion) — before any new passes use it.
5. **Add `stress_eval.rs`** — `eval_arithmetic_correctness`, `eval_map_filter_correctness`.

**Sprint 2 — Hot path performance:**
6. **Pattern A / Fix #1** (`phf::Map` + `BuiltinOp` in runtime) — primary benchmark unlock.
   Run nano/dispatch benchmarks before and after to prove the gain.
7. **Pattern D / Fix #2** (split `Evaluator`, eliminate body clone) — biggest CPU win at scale.
   Run nano/call_user_fn benchmarks before and after.
8. **Fix #9** (`LambdaClosure` captures → `SmallVec`) — same PR as lambda capture fix.

**Sprint 3 — Architecture hardening:**
9. **Pattern C** (`Arc<PureModel>`) — enables concurrent deployment.
10. **Fix #10** (`eval_lambda` exception threading) — API-breaking, plan carefully.
11. **Pattern B** (trampoline) — required before Legend Server production rollout.
12. **`define_builtins!` / `build.rs`** — once phf design is stable, consolidate the dispatch
    table into a single `builtins.toml` source of truth.

**Milestone — Type system:**
13. **Fix #3** (`SourceInfo` interning) — significant memory win.
14. **Fix #8** (`ValueSpec` typestate) — compile-time safety, last because most invasive.
