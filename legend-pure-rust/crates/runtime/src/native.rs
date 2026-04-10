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
//!
//! The evaluator calls [`NativeRegistry::get`] to dispatch to native functions.
//! Native functions receive their arguments as `&[Value]` and return
//! `Result<Value, PureRuntimeError>`. The evaluator wraps any error with
//! source location to produce a [`PureException`](crate::error::PureException).
//!
//! # Lambda-dependent functions
//!
//! Functions like `map`, `filter`, and `fold` need the evaluator to invoke
//! lambdas. These cannot be implemented as plain `NativeFunction`s because
//! they need `&mut Evaluator`. They will be handled directly by the evaluator's
//! expression walker, not through this registry.

use std::collections::HashMap;
use std::fmt;

use smol_str::SmolStr;

use crate::error::PureRuntimeError;
use crate::value::Value;

/// A native (built-in) Pure function implemented in Rust.
///
/// Native functions receive their arguments as a slice of [`Value`]s
/// and return a `Result<Value, PureRuntimeError>`. The evaluator
/// wraps any error with source location to produce a `PureException`.
///
/// # Implementors
///
/// Each native function is a zero-sized struct implementing this trait.
/// This allows compile-time dispatch and zero allocation for the function
/// objects themselves.
pub trait NativeFunction: fmt::Debug {
    /// Execute the function with the given arguments.
    ///
    /// Arguments are already evaluated (left-to-right) by the evaluator.
    /// The function should validate argument count and types, returning
    /// appropriate `PureRuntimeError` variants on failure.
    ///
    /// # Errors
    /// Returns `PureRuntimeError` if argument count/types are wrong or
    /// if the computation fails (e.g., division by zero).
    fn execute(&self, args: &[Value]) -> Result<Value, PureRuntimeError>;

    /// The Pure function signature, for documentation and error messages.
    ///
    /// Example: `"plus(Integer[1], Integer[1]): Integer[1]"`
    fn signature(&self) -> &'static str;
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
/// let result = plus.execute(&[Value::Integer(2), Value::Integer(3)]);
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial native function for testing the registry.
    #[derive(Debug)]
    struct ConstantFn(Value);

    impl NativeFunction for ConstantFn {
        fn execute(&self, _args: &[Value]) -> Result<Value, PureRuntimeError> {
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
        let result = func.execute(&[]).unwrap();
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
