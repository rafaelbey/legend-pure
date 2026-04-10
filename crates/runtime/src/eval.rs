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

//! Expression evaluator — the core interpreter loop.
//!
//! The evaluator walks a compiled Pure model and produces [`Value`]s.
//! It is the central component of the runtime, connecting the compiler's
//! IR to the native function registry, variable context, and object heap.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐     ┌──────────────┐     ┌────────────────┐
//! │  PureModel   │────▶│  Evaluator   │────▶│     Value      │
//! │ (compiler IR)│     │              │     │ (runtime value)│
//! └──────────────┘     │  ┌────────┐  │     └────────────────┘
//!                      │  │Heap    │  │
//!                      │  │Context │  │
//!                      │  │Natives │  │
//!                      │  └────────┘  │
//!                      └──────────────┘
//! ```
//!
//! # Error handling
//!
//! The evaluator uses the lazy call stack pattern: it does **not** maintain
//! a call stack during the happy path. Instead, when an error occurs, each
//! `map_err` in the recursive call chain appends a [`StackFrame`] as the
//! `Err(PureException)` propagates upward. This gives us **zero overhead**
//! on the hot path and rich diagnostics on failure.
//!
//! See `docs/runtime/error_location_design.md` for the full design rationale.
//!
//! # Status
//!
//! This module defines the evaluator struct and its public API.
//! The actual `eval()` implementation requires the compiler's IR types
//! from the `pure` crate, which is currently work-in-progress.

use crate::context::VariableContext;
use crate::error::PureException;
use crate::heap::RuntimeHeap;
use crate::native::NativeRegistry;
use crate::value::Value;

/// Evaluator state — holds mutable context during expression evaluation.
///
/// The evaluator walks the compiler's IR and produces [`Value`]s. It owns:
/// - A [`RuntimeHeap`] for object storage
/// - A [`VariableContext`] for scoped variable bindings
/// - A reference to the [`NativeRegistry`] for built-in function dispatch
///
/// # Lazy call stack
///
/// The evaluator has **no call stack field**. The Pure-level call stack is
/// built lazily via `map_err` only when an error propagates through the
/// recursive `eval()` calls. This means zero overhead on the happy path.
///
/// # Lifetime
///
/// The `'model` lifetime ties the evaluator to the compiled model and
/// native registry, both of which are immutable during evaluation.
pub struct Evaluator<'model> {
    /// The object heap — mutable storage for runtime objects.
    heap: RuntimeHeap,

    /// Scoped variable bindings with O(1) flat-undo.
    context: VariableContext,

    /// Immutable registry of native functions.
    natives: &'model NativeRegistry,
    // NOTE: `model: &'model PureModel` will be added when the compiler
    // crate's PureModel type is defined. For now, the evaluator operates
    // with just natives + heap + context.
}

impl<'model> Evaluator<'model> {
    /// Create a new evaluator.
    ///
    /// The evaluator starts with an empty heap and variable context.
    /// The native registry provides built-in function implementations.
    #[must_use]
    pub fn new(natives: &'model NativeRegistry) -> Self {
        Self {
            heap: RuntimeHeap::new(),
            context: VariableContext::new(),
            natives,
        }
    }

    /// Access the object heap.
    #[must_use]
    pub fn heap(&self) -> &RuntimeHeap {
        &self.heap
    }

    /// Mutably access the object heap.
    pub fn heap_mut(&mut self) -> &mut RuntimeHeap {
        &mut self.heap
    }

    /// Access the variable context.
    #[must_use]
    pub fn context(&self) -> &VariableContext {
        &self.context
    }

    /// Mutably access the variable context.
    pub fn context_mut(&mut self) -> &mut VariableContext {
        &mut self.context
    }

    /// Access the native function registry.
    #[must_use]
    pub fn natives(&self) -> &NativeRegistry {
        self.natives
    }

    /// Call a native function by name with the given arguments.
    ///
    /// This is the primary dispatch mechanism for built-in functions.
    /// The evaluator looks up the function in the registry, executes it,
    /// and wraps any error with source location.
    ///
    /// # Errors
    /// Returns `PureException` if the function is not found or if
    /// execution fails.
    #[allow(clippy::result_large_err)] // PureException is intentionally rich
    pub fn call_native(&self, name: &str, args: &[Value]) -> Result<Value, PureException> {
        let func = self.natives.get_or_err(name).map_err(PureException::from)?;
        func.execute(args).map_err(PureException::from)
    }
}

impl std::fmt::Debug for Evaluator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Evaluator")
            .field("heap_objects", &self.heap.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluator_create() {
        let registry = NativeRegistry::standard();
        let eval = Evaluator::new(&registry);
        assert_eq!(eval.heap().len(), 0);
    }

    #[test]
    fn evaluator_call_native_plus() {
        let registry = NativeRegistry::standard();
        let eval = Evaluator::new(&registry);
        let result = eval
            .call_native("plus", &[Value::Integer(2), Value::Integer(3)])
            .unwrap();
        assert_eq!(result, Value::Integer(5));
    }

    #[test]
    fn evaluator_call_native_missing() {
        let registry = NativeRegistry::standard();
        let eval = Evaluator::new(&registry);
        let result = eval.call_native("nonexistent", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn evaluator_call_native_type_error() {
        let registry = NativeRegistry::standard();
        let eval = Evaluator::new(&registry);
        let result = eval.call_native("plus", &[Value::Integer(1), Value::String("x".into())]);
        assert!(result.is_err());
    }

    #[test]
    fn evaluator_standard_registry_has_functions() {
        let registry = NativeRegistry::standard();
        assert!(registry.get("plus").is_some());
        assert!(registry.get("minus").is_some());
        assert!(registry.get("equal").is_some());
        assert!(registry.get("and").is_some());
        assert!(registry.get("size").is_some());
        assert!(registry.get("length").is_some());
        assert!(!registry.is_empty());
    }
}
