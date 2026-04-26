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
//! - [`EvalContextTrait`] — type-erased handle to the evaluator passed to
//!   native functions so they can force argument expressions, invoke
//!   callables, and reach the heap.
//! - [`Evaluated`] — the activator result; wraps [`Value`] and will grow
//!   inferred type, span, and multiplicity metadata over time without
//!   churning native signatures.
//!
//! # Expression-activator model
//!
//! Natives receive **unevaluated** [`ValueSpec`]s. They call
//! `ctx.evaluate(&spec)` on every argument they actually need, in whatever
//! order matches their semantics. This mirrors Java Pure's
//! `findValueSpecificationExecutor(...).execute(...)` pattern:
//!
//! - Eager primitives (`plus`, `equal`) evaluate every argument up front.
//! - Short-circuit natives (`and`, `or`, `if`) evaluate arguments on demand
//!   and skip unused branches entirely — no more zero-param lambda wrapping.
//! - Lambda-receiving natives (`map`, `filter`, `fold`) first evaluate the
//!   lambda spec to a `Value::Function`, then invoke it per element via
//!   [`EvalContextTrait::call_function`].

use std::collections::HashMap;
use std::fmt;

#[cfg(test)]
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::{ExprKind, ValueSpec};
use rust_decimal::Decimal;
use smol_str::SmolStr;

use crate::context::VariableContext;
use crate::error::{PureException, PureRuntimeError};
use crate::heap::RuntimeHeap;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Evaluated — forced-expression carrier
// ---------------------------------------------------------------------------

/// The result of forcing a Pure `ValueSpec`.
///
/// `Evaluated` wraps a runtime [`Value`] and carves out a place for the
/// activation metadata we plan to add over time (inferred type, source
/// span, multiplicity, … ). Native functions never see `Value` directly
/// on the hot path — they take and return `Evaluated`, so adding new
/// fields later won't churn 125 signatures a second time.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    value: Value,
}

impl Evaluated {
    /// Wrap a `Value` as a forced-expression result.
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    /// Borrow the underlying `Value`.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Consume into the bare `Value`.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.value
    }

    /// Interpret as a boolean. Errors if the underlying `Value` is not boolean.
    ///
    /// # Errors
    /// Returns a `PureException` wrapping the `Value::as_boolean` error.
    pub fn as_boolean(&self) -> Result<bool, PureException> {
        self.value.as_boolean().map_err(Into::into)
    }

    /// Interpret as an integer. Errors if the underlying `Value` is not `Integer`.
    ///
    /// # Errors
    /// Returns a `PureException` if the value is not an integer.
    pub fn as_integer(&self) -> Result<i64, PureException> {
        self.value.as_integer().map_err(Into::into)
    }

    /// Interpret as a float, promoting `Integer` automatically.
    ///
    /// # Errors
    /// Returns a `PureException` if the value is neither float nor integer.
    pub fn as_float(&self) -> Result<f64, PureException> {
        self.value.as_float().map_err(Into::into)
    }

    /// Interpret as a string. Errors if the underlying `Value` is not `String`.
    ///
    /// # Errors
    /// Returns a `PureException` if the value is not a string.
    pub fn as_string(&self) -> Result<&SmolStr, PureException> {
        self.value.as_string().map_err(Into::into)
    }

    /// Interpret as a decimal. Errors if the underlying `Value` is not `Decimal`.
    ///
    /// # Errors
    /// Returns a `PureException` if the value is not a decimal.
    pub fn as_decimal(&self) -> Result<Decimal, PureException> {
        self.value.as_decimal().map_err(Into::into)
    }

    /// Flatten into a collection view. Scalar values become 1-element vectors.
    #[must_use]
    pub fn to_collection(&self) -> im_rc::Vector<Value> {
        self.value.to_collection()
    }
}

impl From<Value> for Evaluated {
    fn from(value: Value) -> Self {
        Self { value }
    }
}

// ---------------------------------------------------------------------------
// EvalContextTrait — expression activator
// ---------------------------------------------------------------------------

/// Type-erased handle to the evaluator, passed to native functions.
///
/// This is the object-safe counterpart of the concrete `EvalContext` struct
/// (defined in [`eval`](crate::eval)). It mirrors Java Pure's expression-
/// activator pattern: the native receives raw `ValueSpec`s and calls
/// [`evaluate`](Self::evaluate) on each one it wants to force.
pub trait EvalContextTrait {
    /// Force a `ValueSpec` to a runtime [`Evaluated`] under the current scope.
    ///
    /// This is the sole entry point natives use to evaluate argument
    /// expressions — the equivalent of Java's
    /// `FunctionExecutionInterpreted.findValueSpecificationExecutor(...).execute(...)`.
    ///
    /// # Errors
    /// Propagates any `PureException` raised while evaluating the expression.
    fn evaluate(&mut self, spec: &ValueSpec) -> Result<Evaluated, PureException>;

    /// Access the variable context immutably.
    fn context(&self) -> &VariableContext;

    /// Access the variable context mutably.
    fn context_mut(&mut self) -> &mut VariableContext;

    /// Access the runtime heap immutably.
    fn heap(&self) -> &RuntimeHeap;

    /// Access the runtime heap mutably.
    fn heap_mut(&mut self) -> &mut RuntimeHeap;

    /// Invoke a Pure callable (lambda or compiled function) with already-
    /// forced argument values.
    ///
    /// Use this after forcing a callable expression via [`evaluate`](Self::evaluate):
    /// the native materializes the `Value::Function` first, then invokes it
    /// per-element with the runtime values it wants to bind.
    ///
    /// # Errors
    /// Returns the callee's full [`PureException`] unchanged, so semantic
    /// kinds (`AssertionFailed`, `ConstraintViolation`, …) survive the
    /// native→user-code boundary.
    fn call_function(&mut self, callable: &Value, args: &[Value]) -> Result<Value, PureException>;

    /// Look up a qualified property by `name` on the receiver's class
    /// (walking generalizations) and invoke it with `$this` bound to the
    /// receiver and `args` bound to its parameters.
    ///
    /// Returns `Ok(None)` when the receiver is not a heap object, or when
    /// no QP of that name+arity exists anywhere on the class hierarchy.
    /// Mirrors Java Pure's `_Class.findQualifiedPropertyWithNoExplicit\
    /// ArgsUsingGeneralization` + `executeLambdaFromNative` pair (see
    /// `legend-pure-runtime-java-engine-interpreted/.../ToString.java:50`).
    /// The generic `toString` native uses this to dispatch to per-class
    /// `toString()` definitions in platform `.pure` source rather than
    /// hardcoding classifier names in the runtime.
    ///
    /// # Errors
    /// Propagates any `PureException` raised while evaluating the QP body.
    fn invoke_qualified_property(
        &mut self,
        receiver: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>, PureException>;

    /// Access the compiled Pure model (element lookup, type resolution).
    fn model(&self) -> &PureModel;

    /// Emit console output through the evaluator's configured sink.
    ///
    /// Pure's `print`/`println` natives call this instead of writing to
    /// stdout directly, so embedders (CLI, LSP, DAP, tests) can capture
    /// or redirect output.
    fn console_output(&mut self, msg: &str);
}

// ---------------------------------------------------------------------------
// NativeFunction trait
// ---------------------------------------------------------------------------

/// A native (built-in) Pure function implemented in Rust.
///
/// Each native is a zero-sized struct implementing this trait. The evaluator
/// hands it raw `ValueSpec`s plus an [`EvalContextTrait`] activator; the
/// native decides which arguments to force (and in what order) and returns
/// an [`Evaluated`] result.
///
/// Simple natives (`plus`, `equal`) force every argument up front. Short-
/// circuit natives (`and`, `or`, `if`) force on demand so unused branches
/// stay unevaluated. Lambda-receiving natives (`map`, `filter`, `fold`)
/// force the lambda spec to a `Value::Function`, then invoke it per-element
/// via [`EvalContextTrait::call_function`].
pub trait NativeFunction: fmt::Debug {
    /// Execute the native against its raw `ValueSpec` arguments.
    ///
    /// The native is responsible for calling `ctx.evaluate(&arg)` on every
    /// argument it needs. The evaluator does **not** pre-evaluate anything —
    /// that's the point of the activator pattern.
    ///
    /// # Errors
    /// Return a `PureException` for any failure (argument-count mismatch,
    /// type error, propagated sub-expression error). The evaluator wraps
    /// the returned exception with a stack frame before it bubbles up.
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException>;

    /// The Pure function signature, for documentation and error messages.
    ///
    /// Example: `"plus(Integer[1], Integer[1]): Integer[1]"`
    fn signature(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// NativeRegistry
// ---------------------------------------------------------------------------

/// Registry of native functions, keyed by mangled function FQN.
///
/// The evaluator uses this to dispatch calls to built-in functions.
/// Functions are registered at startup and the registry is immutable
/// during evaluation.
///
/// Keys are mangled function names matching Java's
/// `ConcreteFunctionDefinitionNameProcessor` format:
/// `funcName_ParamType_Mult__ReturnType_Mult_`
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

    /// Look up a native function by simple name prefix.
    ///
    /// Searches for a registered function whose FQN key starts with
    /// `"{simple_name}_"`. Fallback path for operator calls the compiler
    /// lowered with simple names before FQN resolution.
    #[must_use]
    pub fn find_by_prefix(&self, simple_name: &str) -> Option<&dyn NativeFunction> {
        let prefix = format!("{simple_name}_");
        self.functions
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .min_by_key(|(key, _)| *key)
            .map(|(_, func)| func.as_ref())
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
    /// This includes arithmetic, comparison, boolean, string, collection,
    /// and language-level (let, if, map, filter, fold, assert) operations.
    #[must_use]
    pub fn standard() -> Self {
        let mut registry = Self::new();
        arithmetic::register(&mut registry);
        comparison::register(&mut registry);
        boolean::register(&mut registry);
        string::register(&mut registry);
        collection::register(&mut registry);
        lang::register(&mut registry);
        testing::register(&mut registry);
        meta::register(&mut registry);
        math::register(&mut registry);
        datetime::register(&mut registry);
        relation::register(&mut registry);
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
/// Generic over argument type `T` so it works for both `&[Value]` (legacy
/// callers still in-flight) and `&[ValueSpec]` (the new native API).
///
/// # Errors
/// Returns a `PureException` wrapping an `EvaluationError` if the count is wrong.
pub fn expect_args<T>(func_name: &str, args: &[T], expected: usize) -> Result<(), PureException> {
    if args.len() != expected {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{func_name}: expected {expected} argument(s), got {}",
            args.len()
        ))
        .into());
    }
    Ok(())
}

/// Validate that at least `min` arguments were provided.
///
/// # Errors
/// Returns a `PureException` if fewer than `min` arguments are present.
pub fn expect_min_args<T>(func_name: &str, args: &[T], min: usize) -> Result<(), PureException> {
    if args.len() < min {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{func_name}: expected at least {min} argument(s), got {}",
            args.len()
        ))
        .into());
    }
    Ok(())
}

/// Force every argument spec to a concrete [`Value`].
///
/// Shared by eager natives — arithmetic, comparison, collection (non-lambda),
/// string, math, datetime, meta, and most of lang. Short-circuit natives
/// (`and`, `or`, `if`, `assert`) and lambda-receiving natives
/// (`map`, `filter`, `fold`, `match`) call `ctx.evaluate` on individual
/// args themselves and do not use this helper.
///
/// # Errors
/// Propagates the first `PureException` raised while forcing an argument.
pub(crate) fn force_all(
    args: &[ValueSpec],
    ctx: &mut dyn EvalContextTrait,
) -> Result<Vec<Value>, PureException> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(ctx.evaluate(a)?.into_value());
    }
    Ok(out)
}

/// Force a `ValueSpec`, auto-invoking zero-parameter `Lambda` wrappers.
///
/// Pure signatures that use `Function<{->T[m]}>` parameters (e.g. `if`'s
/// branches, `assert`'s message) arrive at the native as zero-parameter
/// `Lambda` specs: the compiler wraps the argument expression in a `|expr`
/// closure so evaluation can be deferred. Under the new activator model the
/// native forces the spec itself, which would normally yield the closure
/// `Value::Function(...)`.
///
/// This helper detects that shape and calls the closure with no arguments to
/// produce the branch's actual value, matching the old `defer_execution +
/// eval_lambda(&arg, &[])` behaviour without re-introducing the flag.
pub fn force_thunk(
    spec: &ValueSpec,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Evaluated, PureException> {
    if let ExprKind::Lambda { parameters, .. } = &*spec.kind
        && parameters.is_empty()
    {
        let lambda_val = ctx.evaluate(spec)?.into_value();
        return Ok(Evaluated::new(ctx.call_function(&lambda_val, &[])?));
    }
    ctx.evaluate(spec)
}

// ===========================================================================
// Native function modules
// ===========================================================================

/// Arithmetic native functions: `plus`, `minus`, `times`, `divide`, etc.
pub mod arithmetic;

/// Comparison native functions: `equal`, `lessThan`, etc.
pub mod comparison;

/// Heap-aware structural equality used by `equal` and `<<equality.Key>>`
/// map-key lookups.
pub mod equality;

/// Boolean native functions: `and`, `or`, `not`.
pub mod boolean;

/// String native functions: `plus` (concat), `length`, `substring`, etc.
pub mod string;

/// Collection native functions: `size`, `at`, `first`, `last`, etc.
pub mod collection;

/// Language-level natives: `letFunction`, `if`.
pub mod lang;

/// Test assertion natives: `assert`.
pub mod testing;

/// Meta-model natives: `pathToElement`, `elementToPath`, `match`, `id`,
/// `type`, `genericType`, `rawType`, `enumName`, `enumValues`,
/// `toRepresentation`, `subTypeOf`.
pub mod meta;

/// Math natives: `floor`, `ceiling`, `round`, `sign`, `sqrt`, `cbrt`, `exp`,
/// `log`, `log10`, `pow`, trig functions, `toFloat`, `toDecimal`,
/// `parseInteger`/`Float`/`Boolean`.
pub mod math;

/// Date/time natives: `now`, `today`, `year`, `monthNumber`, `dayOfMonth`,
/// `hour`, `minute`, `second`, `datePart`, `dateDiff`, `adjust`, `hasX`
/// predicates, `parseDate`, `date(...)` constructors.
pub mod datetime;

/// Relation natives: `addColumns` and (future) related operators on
/// `RelationType` / `Column` / `ColSpecArray` heap shapes.
pub mod relation;

// ---------------------------------------------------------------------------
// Test helpers — literal ValueSpec builders + MockCtx
// ---------------------------------------------------------------------------

/// Construct a [`ValueSpec`] wrapping an [`IntegerLiteral`](ExprKind::IntegerLiteral).
#[cfg(test)]
pub(crate) fn lit_int(n: i64) -> ValueSpec {
    spec(ExprKind::IntegerLiteral(n))
}

/// Construct a [`ValueSpec`] wrapping a [`FloatLiteral`](ExprKind::FloatLiteral).
#[cfg(test)]
pub(crate) fn lit_float(f: f64) -> ValueSpec {
    spec(ExprKind::FloatLiteral(f))
}

/// Construct a [`ValueSpec`] wrapping a [`DecimalLiteral`](ExprKind::DecimalLiteral).
#[cfg(test)]
pub(crate) fn lit_decimal(d: rust_decimal::Decimal) -> ValueSpec {
    spec(ExprKind::DecimalLiteral(d))
}

/// Construct a [`ValueSpec`] wrapping a [`BooleanLiteral`](ExprKind::BooleanLiteral).
#[cfg(test)]
pub(crate) fn lit_bool(b: bool) -> ValueSpec {
    spec(ExprKind::BooleanLiteral(b))
}

/// Construct a [`ValueSpec`] wrapping a [`StringLiteral`](ExprKind::StringLiteral).
#[cfg(test)]
pub(crate) fn lit_str(s: &str) -> ValueSpec {
    spec(ExprKind::StringLiteral(s.into()))
}

/// Construct a [`ValueSpec`] wrapping a [`Collection`](ExprKind::Collection) literal.
#[cfg(test)]
pub(crate) fn lit_collection(elements: Vec<ValueSpec>) -> ValueSpec {
    spec(ExprKind::Collection { elements })
}

#[cfg(test)]
fn spec(kind: ExprKind) -> ValueSpec {
    ValueSpec {
        kind: Box::new(kind),
        source_info: SourceInfo::new("<test>", 1, 1, 1, 1),
        type_info: None,
    }
}

/// Mock evaluation context for unit-testing native functions.
///
/// [`MockCtx::evaluate`] supports **only** literal `ExprKind` shapes —
/// `IntegerLiteral`, `FloatLiteral`, `DecimalLiteral`, `StringLiteral`,
/// `BooleanLiteral`, and `Collection` of those. Every other variant
/// panics with a pointer to `crates/runtime/tests/eval_tests.rs`, which
/// is the home for any native test that needs real evaluator support
/// (variables, lambda bodies, function calls, heap, model lookups, …).
///
/// All other `EvalContextTrait` methods panic — if your native uses the
/// heap, model, or call_function, the test must live in the integration
/// file with a real `Evaluator`.
#[cfg(test)]
pub(crate) struct MockCtx;

#[cfg(test)]
impl EvalContextTrait for MockCtx {
    fn evaluate(&mut self, spec: &ValueSpec) -> Result<Evaluated, PureException> {
        let v = match &*spec.kind {
            ExprKind::IntegerLiteral(n) => Value::Integer(*n),
            ExprKind::FloatLiteral(f) => Value::Float(*f),
            ExprKind::DecimalLiteral(d) => Value::Decimal(*d),
            ExprKind::StringLiteral(s) => Value::String(s.clone()),
            ExprKind::BooleanLiteral(b) => Value::Boolean(*b),
            ExprKind::Collection { elements } => {
                let mut pv = im_rc::Vector::new();
                for e in elements {
                    pv.push_back(self.evaluate(e)?.into_value());
                }
                Value::Collection(Box::new(pv))
            }
            _ => unreachable!(
                "MockCtx::evaluate only supports literal ExprKinds; move this test to \
                 crates/runtime/tests/eval_tests.rs where a real Evaluator is available"
            ),
        };
        Ok(Evaluated::new(v))
    }

    fn context(&self) -> &VariableContext {
        unreachable!("MockCtx::context should never be called in simple native tests")
    }
    fn context_mut(&mut self) -> &mut VariableContext {
        unreachable!("MockCtx::context_mut should never be called in simple native tests")
    }
    fn heap(&self) -> &RuntimeHeap {
        unreachable!("MockCtx::heap should never be called in simple native tests")
    }
    fn heap_mut(&mut self) -> &mut RuntimeHeap {
        unreachable!("MockCtx::heap_mut should never be called in simple native tests")
    }
    fn call_function(&mut self, _c: &Value, _a: &[Value]) -> Result<Value, PureException> {
        unreachable!(
            "MockCtx::call_function should never be called in simple native tests; \
             move this test to eval_tests.rs"
        )
    }
    fn invoke_qualified_property(
        &mut self,
        _receiver: &Value,
        _name: &str,
        _args: &[Value],
    ) -> Result<Option<Value>, PureException> {
        // No model in MockCtx — there can be no class to host a QP, so
        // the structural answer is "no such QP". Real-evaluator-backed
        // tests live in eval_tests.rs and exercise the full dispatch.
        Ok(None)
    }
    fn model(&self) -> &PureModel {
        unreachable!("MockCtx::model should never be called in simple native tests")
    }
    fn console_output(&mut self, _msg: &str) {
        // Silent sink — unit tests for side-effect-free natives don't observe
        // console output, and routing to stdout would pollute cargo-test output.
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
            _args: &[ValueSpec],
            _ctx: &mut dyn EvalContextTrait,
        ) -> Result<Evaluated, PureException> {
            Ok(Evaluated::new(self.0.clone()))
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
        let result = func.execute(&[], &mut MockCtx).unwrap();
        assert_eq!(result.into_value(), Value::Integer(42));
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
        let args = vec![lit_int(1), lit_int(2)];
        assert!(expect_args("test", &args, 2).is_ok());
        assert!(expect_args("test", &args, 3).is_err());
    }

    #[test]
    fn expect_min_args_validates() {
        let args = vec![lit_int(1)];
        assert!(expect_min_args("test", &args, 1).is_ok());
        assert!(expect_min_args("test", &args, 2).is_err());
    }

    #[test]
    fn mock_ctx_evaluates_literal() {
        let mut ctx = MockCtx;
        assert_eq!(
            ctx.evaluate(&lit_int(7)).unwrap().into_value(),
            Value::Integer(7)
        );
        assert_eq!(
            ctx.evaluate(&lit_bool(true)).unwrap().into_value(),
            Value::Boolean(true)
        );
        assert_eq!(
            ctx.evaluate(&lit_str("hi")).unwrap().into_value(),
            Value::String("hi".into())
        );
    }

    #[test]
    fn mock_ctx_evaluates_collection() {
        let mut ctx = MockCtx;
        let spec = lit_collection(vec![lit_int(1), lit_int(2), lit_int(3)]);
        let v = ctx.evaluate(&spec).unwrap().into_value();
        match v {
            Value::Collection(pv) => {
                assert_eq!(pv.len(), 3);
                assert_eq!(pv[0], Value::Integer(1));
                assert_eq!(pv[2], Value::Integer(3));
            }
            other => panic!("expected Collection, got {other:?}"),
        }
    }
}
