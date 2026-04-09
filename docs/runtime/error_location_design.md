# Runtime Error & Source Location Design

## Problem

When a Pure program fails — through `fail()`, `assert()`, a type mismatch, division
by zero, or any runtime error — the user needs to know:

1. **What** went wrong (the error kind + message)
2. **Where** in their Pure source it happened (file, line, column)
3. **How** they got there (the Pure-level call stack)

Our current `PureRuntimeError` enum carries only the "what" — no source location,
no call stack. This is insufficient for production use.

## How Java Does It

Analyzed from `legend-pure` source at `/Users/cocobey73/Projects/legend-pure`.

### Architecture

```
PureException (abstract)
  ├── sourceInformation: SourceInformation   // where the error originated
  ├── info: String                           // error message
  ├── cause → PureException                  // chained exception (for stack trace)
  │
  ├── PureExecutionException                 // general runtime errors
  │     └── callStack: CoreInstance[]        // Pure-level function call stack
  │
  └── PureAssertFailException                // from fail()/assert()
        └── (extends PureExecutionException)  // same structure, different label
```

### Key Patterns

**1. `functionExpressionCallStack: MutableStack<CoreInstance>`**

Threaded through **every** function call. The `FunctionExpressionExecutor` pushes
the current `FunctionExpression` AST node before dispatch and pops in `finally`:

```java
// FunctionExpressionExecutor.java:57,96
functionExpressionCallStack.push(instance);  // BEFORE function dispatch
try {
    result = executeFunction(..., functionExpressionCallStack, ...);
} finally {
    functionExpressionCallStack.pop();        // ALWAYS pops
}
```

Each `CoreInstance` on the call stack carries `getSourceInformation()` — the source
location of the function call expression in the Pure file.

**2. Error construction — always with location**

Every `throw new PureExecutionException(...)` includes the source info from the
top of the call stack:

```java
throw new PureExecutionException(
    functionExpressionCallStack.peek().getSourceInformation(),  // WHERE
    "Type mismatch: expected Integer, got String",              // WHAT
    functionExpressionCallStack                                 // CALL STACK
);
```

**3. Error enrichment on catch**

`FunctionExecutionInterpreted.executeFunction()` has a cascade of catch blocks
(lines 834-905) that enriches errors lacking source info:

```java
catch (PureAssertFailException e) {
    SourceInformation sourceInfo = functionExpressionCallStack.peek().getSourceInformation();
    if (e.getSourceInformation() == null && sourceInfo != null) {
        throw new PureAssertFailException(sourceInfo, e.getInfo(), functionExpressionCallStack);
    }
    throw e;
}
catch (PureExecutionException e) {
    // Same pattern — enrich with source info if missing
}
catch (Exception e) {
    // Wrap raw exceptions in PureExecutionException
    throw new PureExecutionException(sourceInfo, e.getMessage(), e, functionExpressionCallStack);
}
```

**4. `printPureStackTrace()` — walks the call stack**

`PureExecutionException.printPureStackTrace()` produces output like:

```
Execution error (resource:my/package/model.pure line:15 column:8)
"Type mismatch: expected Integer, got String"
Full Stack:
    my::package::process_1_String_1__Integer_1_     <-     resource:my/package/model.pure line:15 column:8
    my::package::main_1__Any_MANY_                  <-     resource:my/package/main.pure line:5 column:3
```

Each frame prints the function descriptor + source location.

**5. `PureAssertFailException` is a subtype**

`fail()` and `assert()` throw `PureAssertFailException` (extends `PureExecutionException`).
The only difference is `getExceptionName()` returns `"Assert failure"` instead of
`"Execution error"`. Test frameworks use `instanceof` to distinguish assertions from bugs.

---

## Proposed Rust Design

### Core Types

```rust
use legend_pure_parser_ast::SourceInfo;

/// A frame in the Pure-level call stack.
///
/// Each frame represents a function call expression being evaluated,
/// mirroring Java's `functionExpressionCallStack` entries.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Human-readable function identifier.
    /// e.g., "my::package::process" or "Lambda {Integer[1]->String[1]}"
    pub function_name: SmolStr,

    /// Source location of the function call expression.
    pub source: SourceInfo,
}

/// A runtime error with full location context.
///
/// This is the user-facing error type — every runtime error carries
/// the source location where it occurred and the Pure call stack
/// at the time of failure.
#[derive(Debug)]
pub struct PureException {
    /// What went wrong.
    pub kind: PureExceptionKind,

    /// Where in the Pure source the error originated.
    pub source: Option<SourceInfo>,

    /// The Pure-level call stack at the time of the error.
    /// Outermost frame first (reverse of Java's convention — we
    /// collect by cloning the Vec, which is already in push order).
    pub call_stack: Vec<StackFrame>,
}

/// The kind of exception — distinguishes assertions from runtime errors.
///
/// Mirrors Java's `PureExecutionException` vs `PureAssertFailException`.
#[derive(Debug)]
pub enum PureExceptionKind {
    /// A runtime error (type mismatch, property not found, etc.)
    ExecutionError(PureRuntimeError),

    /// An assertion failure from `fail()` or `assert()`.
    /// Test frameworks use this to distinguish expected failures from bugs.
    AssertionFailed(String),
}
```

### Call Stack Management — Lazy Construction (Zero Happy-Path Cost)

Java maintains a `MutableStack<CoreInstance>` that pushes on every function
entry and pops in `finally`. This costs ~10-15ns per call (SmolStr clone +
SourceInfo clone + Vec push/pop). For a tight loop like
`[1..1000000]->map(x | $x + 1)`, that's 10-15ms of pure overhead.

**We do better.** The evaluator has **no call stack field**. Instead, the
call stack is built lazily via `map_err` only when an error propagates up
through the recursive evaluator:

```rust
/// Evaluator state — holds mutable context during expression evaluation.
pub struct Evaluator<'model> {
    model: &'model PureModel,
    heap: RuntimeHeap,
    context: VariableContext,
    // NOTE: no call_stack field — built lazily on error path only
}

impl Evaluator<'_> {
    /// Evaluate a function call expression.
    fn eval_function_call(
        &mut self,
        expr: &FunctionExpression,
    ) -> Result<Value, PureException> {
        // Just call — no push, no pop, no clone on happy path
        self.dispatch_function(expr)
            .map_err(|mut exc| {
                // Error path only: append this frame as the Err unwinds
                exc.call_stack.push(StackFrame {
                    function_name: expr.function_name.clone(),
                    source: expr.source_info().clone(),
                });
                exc
            })
    }
}
```

As the `Err(PureException)` propagates up through the recursive call chain,
each `map_err` appends one frame. The stack builds itself from **innermost
to outermost** — the exact order we store in `call_stack`.

#### Performance Comparison

| Approach | Happy path cost per call | Error path cost | Used by |
|---|---|---|---|
| Push/Pop + snapshot | ~10-15ns (clone + push + pop) | Same + clone Vec | Java |
| **Lazy `map_err`** | **0ns** | ~15ns × stack depth (one-time) | **Rust (ours)** |

The lazy approach is **strictly superior**: zero overhead on the hot path,
and errors are rare events where a few microseconds of stack construction
are invisible.

### Error Propagation Pattern

```rust
// In the evaluator — low-level operations return PureRuntimeError (no location).
// The evaluator creates the initial PureException at the point of failure,
// then each map_err up the chain appends a stack frame:

fn eval_property_access(&mut self, expr: &PropertyAccess) -> Result<Value, PureException> {
    let obj = self.eval_expr(&expr.object)?;
    let obj_id = obj.as_object()
        .map_err(|e| PureException::execution(e, expr.source_info().clone(), Vec::new()))?;

    self.heap.get_property(obj_id, &expr.property)
        .map_err(|e| PureException::execution(e, expr.source_info().clone(), Vec::new()))
}

// Native function `fail()`:
fn native_fail(&self, args: &[Value], source: &SourceInfo) -> Result<Value, PureException> {
    let message = args[0].as_string()
        .map_err(|e| PureException::execution(e, source.clone(), Vec::new()))?;
    Err(PureException::assertion(message.to_string(), source.clone(), Vec::new()))
}
// The empty Vec::new() call_stack gets populated as the Err propagates up
// through each eval_function_call's map_err.
```

### Display Format

Matching the Java output format for familiarity:

```rust
impl fmt::Display for PureException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header: error kind + location
        let kind_name = match &self.kind {
            PureExceptionKind::ExecutionError(_) => "Execution error",
            PureExceptionKind::AssertionFailed(_) => "Assert failure",
        };
        write!(f, "{kind_name}")?;
        if let Some(src) = &self.source {
            write!(f, " (resource:{} line:{} column:{})",
                src.source, src.start_line, src.start_column)?;
        }

        // Message
        match &self.kind {
            PureExceptionKind::ExecutionError(e) => write!(f, "\n\"{e}\"")?,
            PureExceptionKind::AssertionFailed(msg) => write!(f, "\n\"{msg}\"")?,
        };

        // Call stack (if non-empty)
        if !self.call_stack.is_empty() {
            write!(f, "\nFull Stack:")?;
            for frame in self.call_stack.iter().rev() {
                write!(f, "\n    {}     <-     resource:{} line:{} column:{}",
                    frame.function_name,
                    frame.source.source,
                    frame.source.start_line,
                    frame.source.start_column,
                )?;
            }
        }
        Ok(())
    }
}
```

Example output:

```
Execution error (resource:my/package/model.pure line:15 column:8)
"Type mismatch: expected Integer, got String"
Full Stack:
    my::package::process     <-     resource:my/package/model.pure line:15 column:8
    my::package::main        <-     resource:my/package/main.pure line:5 column:3
```

---

## Constraint Violations

Pure has three places where constraints are evaluated — all use the same
`PureExecutionException` with `sourceInformation` + `functionExpressionCallStack`:

### 1. Class constraints (on `new` / `copy`)

When an object is instantiated, `DefaultConstraintHandler.evaluateOneConstraint()`
evaluates each constraint expression from the class hierarchy. If the boolean
result is `false`, it throws:

```java
// DefaultConstraintHandler.java:91
throw new PureExecutionException(
    sourceInformation,                              // location of the ^Class(...) expression
    "Constraint :[" + ruleId + "] violated in the Class " + constraintClass.getName()
        + ", Message: " + messageFunction_result,   // custom message from constraint
    functionExpressionCallStack                     // full call stack
);
```

Key detail: constraints can have a `messageFunction` — a lambda that produces
a custom error message. If present, the interpreter evaluates it and appends
the result to the error.

### 2. Function pre-constraints

```java
// FunctionExecutionInterpreted.java:761
for (CoreInstance constraint : function._preConstraints()) {
    // evaluate constraint expression...
    if (!result) {
        throw new PureExecutionException(
            functionExpressionCallStack.peek().getSourceInformation(),
            "Constraint (PRE):[" + ruleId + "] violated. (Function:" + function.getName() + ")",
            functionExpressionCallStack);
    }
}
```

### 3. Function post-constraints

```java
// FunctionExecutionInterpreted.java:827
for (CoreInstance constraint : function._postConstraints()) {
    // evaluate constraint expression with $return bound to result...
    if (!result) {
        throw new PureExecutionException(
            functionExpressionCallStack.peek().getSourceInformation(),
            "Constraint (POST):[" + ruleId + "] violated. (Function:" + function.getName() + ")",
            functionExpressionCallStack);
    }
}
```

### Coverage in Our Design

All three cases are covered by adding a `ConstraintViolation` variant to
`PureExceptionKind`:

```rust
pub enum PureExceptionKind {
    /// A runtime error (type mismatch, property not found, etc.)
    ExecutionError(PureRuntimeError),

    /// An assertion failure from `fail()` or `assert()`.
    AssertionFailed(String),

    /// A constraint violation — class invariant or function pre/post condition.
    ConstraintViolation {
        /// The constraint name (rule ID).
        constraint_id: SmolStr,
        /// Which kind of constraint: Class, Pre, or Post.
        constraint_kind: ConstraintKind,
        /// The class or function that owns the constraint.
        owner: SmolStr,
        /// Optional custom message from the constraint's `messageFunction`.
        message: Option<String>,
    },
}

/// The kind of constraint that was violated.
pub enum ConstraintKind {
    /// Class invariant — checked on `new` / `copy`.
    Class,
    /// Function pre-condition — checked before function body.
    Pre,
    /// Function post-condition — checked after function body.
    Post,
}
```

This gives us structured data that downstream consumers (IDE, CLI, test
frameworks) can inspect, rather than parsing a message string.

Example output:

```
Execution error (resource:my/package/model.pure line:15 column:8)
"Constraint :[positivePrice] violated in the Class Trade, Message: Price must be > 0"
Full Stack:
    my::package::buildTrade     <-     resource:my/package/model.pure line:15 column:8
    my::package::main           <-     resource:my/package/main.pure line:5 column:3
```

---

## Key Design Decisions

### 1. Two-layer error model

| Layer | Type | Location? | Who creates it? |
|---|---|---|---|
| Inner | `PureRuntimeError` | No | Heap, value conversions, context |
| Outer | `PureException` | Yes | Evaluator only |

Low-level operations (`heap.get_property()`, `value.as_integer()`) return
`PureRuntimeError` — they don't know which expression is being evaluated.
The evaluator wraps these with source info via `PureException::execution()`.

### 2. Lazy call stack — zero happy-path cost

Java pushes/pops on every function call (~10-15ns each). We build the stack
**only when an error occurs**, as the `Err(PureException)` propagates up
through `map_err`. The evaluator has no `call_stack` field at all — the
Rust call stack itself is the implicit structure, and each `map_err` closure
appends a frame only on the error path.

### 3. No evaluator state for call tracking

Unlike Java's `functionExpressionCallStack` that is threaded through every
method, our evaluator needs no additional field for error tracking. This
keeps the evaluator struct lean and the hot path unencumbered.

### 4. `PureRuntimeError` stays as-is

The existing error enum is the "what went wrong" layer. No changes needed.
`PureException` wraps it with the "where" and "how" context.

### 5. `PureExceptionKind::AssertionFailed` is distinct

Matches Java's `PureAssertFailException extends PureExecutionException` —
test frameworks can `match` on the kind to distinguish assertions from bugs.

---

## Changes Required

| File | Change |
|---|---|
| `src/error.rs` | Add `StackFrame`, `PureException`, `PureExceptionKind`, `Display` impl |
| `src/lib.rs` | Re-export `PureException` |
| `Cargo.toml` | Add dep on `legend-pure-parser-ast` (for `SourceInfo`) — already present |

No changes to `PureRuntimeError`, `VariableContext`, `RuntimeHeap`, or `Value`.
