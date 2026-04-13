// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Native (built-in) Pure functions implemented in Rust.
//!
//! Native functions are Pure functions whose implementation is provided by the
//! runtime rather than by user-written Pure code. Examples include arithmetic
//! operators (`plus`, `minus`), collection operations (`size`, `at`), and
//! assertion primitives (`fail`, `assert`).
//!
//! # Architecture
//!
//! - [`NativeFunction`] — trait that each built-in function implements.
//! - [`NativeRegistry`] — lookup table mapping qualified function names to
//!   their implementations.
//! - [`EvalContextTrait`] — type-erased handle to the evaluator, passed to
//!   native functions so they can invoke lambdas, access variables, etc.
//!
//! The evaluator calls [`NativeRegistry::get`] to dispatch to native functions.
//! Native functions receive their arguments as `&[Value]` and an
//! `&mut dyn EvalContextTrait` to access the evaluator. The evaluator wraps
//! any error with source location to produce a [`PureException`](crate::error::PureException).
//!
//! # Lambda-dependent functions
//!
//! Functions like `map`, `filter`, `fold`, and `if` use `ctx.eval_lambda()`
//! to invoke lambda bodies. Functions like `if` also use
//! [`NativeFunction::defer_execution`] to short-circuit argument evaluation.

use std::collections::HashMap;
use std::fmt;

use smol_str::SmolStr;

use crate::context::VariableContext;
use crate::error::PureRuntimeError;
use crate::heap::RuntimeHeap;
use crate::value::Value;

// ---------------------------------------------------------------------------
// EvalContextTrait — type-erased evaluator handle for native functions
// ---------------------------------------------------------------------------

/// Type-erased handle to the evaluator, passed to native functions.
///
/// This is the object-safe counterpart of the concrete `EvalContext` struct
/// (defined in [`eval`](crate::eval)). It allows `NativeFunction` to remain
/// object-safe (stored as `Box<dyn NativeFunction>`) while still having
/// access to the evaluator's capabilities.
///
/// Mirrors Java's pattern where `NativeFunction.execute()` receives
/// `FunctionExecutionInterpreted`, `VariableContext`, and `ProcessorSupport`.
pub trait EvalContextTrait {
    /// Evaluate a lambda closure with the given arguments.
    ///
    /// # Errors
    /// Returns `PureRuntimeError` if evaluation fails or the value is not a lambda.
    fn eval_lambda(&mut self, lambda: &Value, args: &[Value]) -> Result<Value, PureRuntimeError>;

    /// Access the variable context immutably.
    fn context(&self) -> &VariableContext;

    /// Access the variable context mutably.
    fn context_mut(&mut self) -> &mut VariableContext;

    /// Access the runtime heap immutably.
    fn heap(&self) -> &RuntimeHeap;

    /// Access the runtime heap mutably.
    fn heap_mut(&mut self) -> &mut RuntimeHeap;
}

// ---------------------------------------------------------------------------
// NativeFunction trait
// ---------------------------------------------------------------------------

/// A native (built-in) Pure function implemented in Rust.
///
/// Native functions receive their arguments as a slice of [`Value`]s
/// and an [`EvalContextTrait`] that provides access to the evaluator's
/// capabilities (lambda evaluation, variable context, heap, model).
/// The evaluator wraps any error with source location to produce a
/// `PureException`.
///
/// This mirrors Java's `NativeFunction` which receives
/// `VariableContext`, `FunctionExecutionInterpreted`, and
/// `ProcessorSupport` in its `execute()` method.
///
/// # Implementors
///
/// Each native function is a zero-sized struct implementing this trait.
/// This allows compile-time dispatch and zero allocation for the function
/// objects themselves.
///
/// Simple natives (e.g., `plus`, `equal`) ignore the `ctx` parameter.
/// Lambda-dependent natives (e.g., `map`, `filter`, `if`) use
/// `ctx.eval_lambda()` to invoke lambda bodies.
pub trait NativeFunction: fmt::Debug {
    /// Execute the function with the given arguments and evaluation context.
    ///
    /// Arguments are already evaluated (left-to-right) by the evaluator,
    /// unless [`defer_execution`](Self::defer_execution) returns `true`.
    ///
    /// The `ctx` parameter provides access to the evaluator for:
    /// - Invoking lambda bodies (`ctx.eval_lambda()`)
    /// - Reading/writing variables (`ctx.context_mut()`)
    /// - Accessing the object heap (`ctx.heap_mut()`)
    ///
    /// # Errors
    /// Returns `PureRuntimeError` if argument count/types are wrong or
    /// if the computation fails (e.g., division by zero).
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError>;

    /// The Pure function signature, for documentation and error messages.
    ///
    /// Example: `"plus(Integer[1], Integer[1]): Integer[1]"`
    fn signature(&self) -> &'static str;

    /// Whether this native defers parameter evaluation.
    ///
    /// When `true`, the evaluator passes unevaluated expressions wrapped
    /// as zero-parameter [`LambdaClosure`](crate::value::LambdaClosure)s
    /// instead of fully evaluated values. The native function then
    /// evaluates them on demand via `ctx.eval_lambda()`.
    ///
    /// This enables short-circuiting (e.g., `if` only evaluates the taken
    /// branch) and matches Java's `deferParameterExecution()`.
    ///
    /// Defaults to `false` — most native functions eagerly evaluate args.
    fn defer_execution(&self) -> bool {
        false
    }
}

/// Registry of native functions, keyed by qualified Pure function name.
///
/// The evaluator uses this to dispatch calls to built-in functions.
/// Functions are registered at startup and the registry is immutable
/// during evaluation.
///
/// # Example
///
/// ```ignore
/// use legend_pure_runtime::native::NativeRegistry;
///
/// let registry = NativeRegistry::standard();
/// let plus = registry.get("plus").unwrap();
/// let result = plus.execute(&[Value::Integer(2), Value::Integer(3)], &mut ctx);
/// assert_eq!(result.unwrap(), Value::Integer(5));
/// ```
pub struct NativeRegistry {
    functions: HashMap<SmolStr, Box<dyn NativeFunction>>,
}

impl NativeRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }

    /// Register a native function under the given name.
    ///
    /// If a function with the same name already exists, it is replaced.
    pub fn register(&mut self, name: impl Into<SmolStr>, func: impl NativeFunction + 'static) {
        self.functions.insert(name.into(), Box::new(func));
    }

    /// Look up a native function by name.
    ///
    /// Returns `None` if no function is registered under that name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn NativeFunction> {
        self.functions.get(name).map(AsRef::as_ref)
    }

    /// Look up a native function, returning a `FunctionNotFound` error if missing.
    ///
    /// # Errors
    /// Returns `FunctionNotFound` if the function is not registered.
    pub fn get_or_err(&self, name: &str) -> Result<&dyn NativeFunction, PureRuntimeError> {
        self.get(name)
            .ok_or_else(|| PureRuntimeError::FunctionNotFound(name.into()))
    }

    /// The number of registered functions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Create a registry pre-loaded with all standard Pure native functions.
    ///
    /// This includes arithmetic, comparison, boolean, string, and collection
    /// operations.
    #[must_use]
    pub fn standard() -> Self {
        let mut registry = Self::new();
        arithmetic::register(&mut registry);
        comparison::register(&mut registry);
        boolean::register(&mut registry);
        string::register(&mut registry);
        collection::register(&mut registry);
        registry
    }
}

impl Default for NativeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NativeRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeRegistry")
            .field("count", &self.functions.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Argument validation helpers
// ---------------------------------------------------------------------------

/// Validate that exactly `n` arguments were provided.
///
/// # Errors
/// Returns `EvaluationError` with a descriptive message if the count is wrong.
pub fn expect_args(
    func_name: &str,
    args: &[Value],
    expected: usize,
) -> Result<(), PureRuntimeError> {
    if args.len() != expected {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{func_name}: expected {expected} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

/// Validate that at least `min` arguments were provided.
///
/// # Errors
/// Returns `EvaluationError` if fewer than `min` arguments are present.
pub fn expect_min_args(
    func_name: &str,
    args: &[Value],
    min: usize,
) -> Result<(), PureRuntimeError> {
    if args.len() < min {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{func_name}: expected at least {min} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

// ===========================================================================
// Native function modules
// ===========================================================================

/// Arithmetic native functions: `plus`, `minus`, `times`, `divide`, etc.
pub mod arithmetic;

/// Comparison native functions: `equal`, `lessThan`, etc.
pub mod comparison;

/// Boolean native functions: `and`, `or`, `not`.
pub mod boolean;

/// String native functions: `plus` (concat), `length`, `substring`, etc.
pub mod string;

/// Collection native functions: `size`, `at`, `first`, `last`, etc.
pub mod collection;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// A no-op evaluation context stub for testing native functions.
///
/// All methods panic — this is only suitable for testing natives that
/// don't use the evaluation context (i.e., simple arithmetic, comparison,
/// string, and collection functions that ignore the `ctx` parameter).
#[cfg(test)]
pub(crate) struct NoOpEvalCtx;

#[cfg(test)]
impl EvalContextTrait for NoOpEvalCtx {
    fn eval_lambda(&mut self, _l: &Value, _a: &[Value]) -> Result<Value, PureRuntimeError> {
        unreachable!("NoOpEvalCtx::eval_lambda should never be called in simple native tests")
    }
    fn context(&self) -> &VariableContext {
        unreachable!("NoOpEvalCtx::context should never be called in simple native tests")
    }
    fn context_mut(&mut self) -> &mut VariableContext {
        unreachable!("NoOpEvalCtx::context_mut should never be called in simple native tests")
    }
    fn heap(&self) -> &RuntimeHeap {
        unreachable!("NoOpEvalCtx::heap should never be called in simple native tests")
    }
    fn heap_mut(&mut self) -> &mut RuntimeHeap {
        unreachable!("NoOpEvalCtx::heap_mut should never be called in simple native tests")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial native function for testing the registry.
    #[derive(Debug)]
    struct ConstantFn(Value);

    impl NativeFunction for ConstantFn {
        fn execute(
            &self,
            _args: &[Value],
            _ctx: &mut dyn EvalContextTrait,
        ) -> Result<Value, PureRuntimeError> {
            Ok(self.0.clone())
        }

        fn signature(&self) -> &'static str {
            "constant(): Any[1]"
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = NativeRegistry::new();
        reg.register("myFunc", ConstantFn(Value::Integer(42)));

        let func = reg.get("myFunc").unwrap();
        let result = func.execute(&[], &mut NoOpEvalCtx).unwrap();
        assert_eq!(result, Value::Integer(42));
    }

    #[test]
    fn registry_missing_function() {
        let reg = NativeRegistry::new();
        assert!(reg.get("nope").is_none());
        assert!(reg.get_or_err("nope").is_err());
    }

    #[test]
    fn registry_len() {
        let mut reg = NativeRegistry::new();
        assert!(reg.is_empty());
        reg.register("a", ConstantFn(Value::Unit));
        reg.register("b", ConstantFn(Value::Unit));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn expect_args_validates() {
        let args = vec![Value::Integer(1), Value::Integer(2)];
        assert!(expect_args("test", &args, 2).is_ok());
        assert!(expect_args("test", &args, 3).is_err());
    }

    #[test]
    fn expect_min_args_validates() {
        let args = vec![Value::Integer(1)];
        assert!(expect_min_args("test", &args, 1).is_ok());
        assert!(expect_min_args("test", &args, 2).is_err());
    }
}
