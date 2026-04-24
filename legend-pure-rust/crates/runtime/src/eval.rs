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
//!                      │  │Hooks   │  │
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
//! # Generic hooks
//!
//! The evaluator is generic over `H: EvalHooks`, enabling zero-cost
//! instrumentation:
//! - [`NoOpHooks`] — production. All hook calls compile to nothing.
//! - `DebugHooks` (future, in `lsp` crate) — IDE debugging with breakpoints.

use std::collections::HashMap;

use im_rc::Vector as PVector;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{DateValue, ExprKind, TypeExpr, ValueSpec};
use smol_str::SmolStr;

use crate::context::VariableContext;
use crate::date::PureDate;
use crate::error::{PureException, PureRuntimeError, StackFrame};
use crate::heap::RuntimeHeap;
use crate::hooks::{EvalHooks, NoOpHooks};
use crate::native::{Evaluated, NativeFunction, NativeRegistry};
use crate::value::{FunctionValue, LambdaClosure, Value};

/// Evaluator state — holds mutable context during expression evaluation.
///
/// The evaluator walks the compiler's IR and produces [`Value`]s. It owns:
/// - A reference to the [`PureModel`] (compiled IR)
/// - A [`RuntimeHeap`] for object storage
/// - A [`VariableContext`] for scoped variable bindings
/// - A reference to the [`NativeRegistry`] for built-in function dispatch
/// - An [`EvalHooks`] instance for instrumentation
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
pub struct Evaluator<'model, H: EvalHooks = NoOpHooks> {
    /// The compiled Pure model (IR).
    model: &'model PureModel,

    /// The object heap — mutable storage for runtime objects.
    heap: RuntimeHeap,

    /// Scoped variable bindings with O(1) flat-undo.
    context: VariableContext,

    /// Immutable registry of native functions.
    natives: &'model NativeRegistry,

    /// Memoised per-class member wrappers (`.properties`,
    /// `.qualifiedProperties`, `.propertiesFromAssociations`). Keyed by
    /// `(class_element_id, member-kind)` — the same query returns the same
    /// `Value::Object(obj_id)` set every time, so
    /// `assertIs(CC_Address.properties->at(0), CC_Address.properties->at(0))`
    /// holds. The wrappers are functionally pure (they read class metadata,
    /// not heap state), so no invalidation is needed for the evaluator's
    /// lifetime.
    member_wrapper_cache: HashMap<(ElementId, &'static str), Vec<Value>>,

    /// Instrumentation hooks (zero-cost for `NoOpHooks`).
    hooks: H,
}

impl<'model> Evaluator<'model, NoOpHooks> {
    /// Create a new evaluator with production (no-op) hooks.
    ///
    /// The evaluator starts with an empty heap and variable context.
    #[must_use]
    pub fn new(model: &'model PureModel, natives: &'model NativeRegistry) -> Self {
        Self {
            model,
            heap: RuntimeHeap::new(),
            context: VariableContext::new(),
            natives,
            member_wrapper_cache: HashMap::new(),
            hooks: NoOpHooks,
        }
    }
}

impl<'model, H: EvalHooks> Evaluator<'model, H> {
    /// Create a new evaluator with custom hooks.
    ///
    /// Use this for debugging or instrumentation. For production use,
    /// prefer [`Evaluator::new`] which uses zero-cost [`NoOpHooks`].
    #[must_use]
    pub fn with_hooks(model: &'model PureModel, natives: &'model NativeRegistry, hooks: H) -> Self {
        Self {
            model,
            heap: RuntimeHeap::new(),
            context: VariableContext::new(),
            natives,
            member_wrapper_cache: HashMap::new(),
            hooks,
        }
    }

    /// Access the compiled model.
    #[must_use]
    pub fn model(&self) -> &'model PureModel {
        self.model
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

    // -----------------------------------------------------------------------
    // Core evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a compiled expression, producing a runtime value.
    ///
    /// This is the core recursive evaluator. It dispatches on `ExprKind`
    /// and produces `Value`s. Errors are wrapped with source location to
    /// form `PureException`s with lazy call stacks.
    ///
    /// # Errors
    /// Returns `PureException` on evaluation failure (type mismatch,
    /// variable not found, function not found, etc.).
    #[allow(clippy::result_large_err)] // PureException is intentionally rich
    pub fn eval(&mut self, expr: &ValueSpec) -> Result<Value, PureException> {
        self.hooks.before_eval(&expr.source_info);

        let result = match &*expr.kind {
            // -- Literals -------------------------------------------------
            ExprKind::IntegerLiteral(n) => Ok(Value::Integer(*n)),
            ExprKind::FloatLiteral(n) => Ok(Value::Float(*n)),
            ExprKind::DecimalLiteral(d) => Ok(Value::Decimal(*d)),
            ExprKind::StringLiteral(s) => Ok(Value::String(s.clone())),
            ExprKind::BooleanLiteral(b) => Ok(Value::Boolean(*b)),
            ExprKind::DateLiteral(dv) => self.eval_date_literal(dv),

            // -- Variable reference ---------------------------------------
            ExprKind::Variable { name } => self
                .context
                .require(name)
                .cloned()
                .map_err(PureException::from),

            // -- Function call (the most complex case) --------------------
            ExprKind::FunctionCall {
                function,
                function_name,
                arguments,
            } => self.eval_function_call(*function, function_name, arguments, &expr.source_info),

            // -- Property access ------------------------------------------
            ExprKind::PropertyAccess { target, property } => {
                self.eval_property_access(target, property)
            }

            // -- Qualified property access --------------------------------
            ExprKind::QualifiedPropertyAccess {
                target,
                property,
                arguments,
            } => self.eval_qualified_property(target, property, arguments),

            // -- Enum value -----------------------------------------------
            ExprKind::EnumValue {
                enum_element,
                value,
            } => Ok(self.eval_enum_value(*enum_element, value)),

            // -- Lambda ---------------------------------------------------
            ExprKind::Lambda { parameters, body } => {
                Ok(self.eval_lambda_creation(parameters, body))
            }

            // -- Collection literal ---------------------------------------
            ExprKind::Collection { elements } => self.eval_collection(elements),

            // -- Type reference -------------------------------------------
            // `@MyClass` / `@ConcreteFunctionDefinition<Any>` needs to survive into
            // runtime values so `cast`, `instanceOf`, and `match` can inspect the
            // target class. A bare structural reference (function type, generic
            // variable) still has no runtime representation — fall back to `Unit`.
            ExprKind::TypeReference { type_expr } => match type_expr {
                legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => {
                    Ok(Value::Element(*element))
                }
                _ => Ok(Value::Unit),
            },
            ExprKind::Column => Ok(Value::Unit),

            // -- Bare element reference -----------------------------------
            // Produce a first-class Element handle so meta-model natives
            // (pathToElement, elementToPath, match) can inspect it. Using
            // the raw ElementId avoids the panic that get_node hits on
            // Package variants.
            ExprKind::PackageableElementRef { element } => Ok(Value::Element(*element)),
        }?;

        self.hooks.after_eval(&expr.source_info, &result);
        Ok(result)
    }

    /// Evaluate a sequence of expressions, returning the last value.
    ///
    /// Pure function bodies are `Vec<Expression>` — the result is the
    ///
    /// # Errors
    /// Returns `PureException` if any underlying expression evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn eval_body(&mut self, body: &[ValueSpec]) -> Result<Value, PureException> {
        let mut result = Value::Unit;
        for expr in body {
            result = self.eval(expr)?;
        }
        Ok(result)
    }

    /// Call a Pure function by name with pre-evaluated arguments.
    ///
    /// Looks up the function in the model by fully qualified name, binds
    /// the arguments to parameters, and evaluates the body.
    ///
    /// # Errors
    /// Returns `PureException` if the function is not found or evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, PureException> {
        let segments: Vec<SmolStr> = name.split("::").map(SmolStr::new).collect();
        // Functions are stored with mangled names; use prefix-matching resolve.
        // Fall back to exact resolve for non-function elements (Package, Class, etc.).
        let element_id = self
            .model
            .resolve_function_by_path(&segments)
            .or_else(|| self.model.resolve_by_path(&segments))
            .ok_or_else(|| PureException::from(PureRuntimeError::FunctionNotFound(name.into())))?;

        self.call_user_function(element_id, args, name)
    }

    /// Call a Pure function by its resolved [`ElementId`] with no arguments.
    ///
    /// This is the primary entry point for integration tests that resolve
    /// functions externally (e.g., via [`PureModel::resolve_function_by_path`]).
    ///
    /// # Errors
    /// Returns `PureException` if the element is not a function or evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn call_user_function_by_id(
        &mut self,
        element_id: ElementId,
    ) -> Result<Value, PureException> {
        let name = self.model.get_node(element_id).name.clone();
        self.call_user_function(element_id, &[], &name)
    }

    /// Call a Pure function by its resolved [`ElementId`] with explicit arguments.
    ///
    /// # Errors
    /// Returns `PureException` if the element is not a function or evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn call_user_function_by_id_with_args(
        &mut self,
        element_id: ElementId,
        args: &[Value],
    ) -> Result<Value, PureException> {
        let name = self.model.get_node(element_id).name.clone();
        self.call_user_function(element_id, args, &name)
    }

    // -----------------------------------------------------------------------
    // Date literal evaluation
    // -----------------------------------------------------------------------

    #[allow(clippy::result_large_err, clippy::unused_self)]
    fn eval_date_literal(&self, dv: &DateValue) -> Result<Value, PureException> {
        match dv {
            DateValue::StrictDate { year, month, day } => {
                let date = match (month, day) {
                    (None, _) => PureDate::year(*year),
                    (Some(m), None) => PureDate::year_month(*year, *m),
                    (Some(m), Some(d)) => PureDate::strict_date(*year, *m, *d),
                };
                let date = date.map_err(|e| {
                    PureException::from(PureRuntimeError::EvaluationError(format!(
                        "Invalid date literal: {e}"
                    )))
                })?;
                Ok(Value::Date(date))
            }
            DateValue::DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                subsecond_nanos,
                subsecond_digits,
                has_seconds,
                tz_offset_minutes,
            } => {
                // Pure stores every DateTime in UTC. Shift by the source TZ
                // offset so `%…T20:54-0500` lands at `…T01:54+0000`. We
                // build an intermediate `jiff::civil::DateTime`, subtract
                // the source offset to obtain UTC, and then hand the
                // resulting components to `PureDate::datetime`.
                let civil = jiff::civil::DateTime::new(
                    *year,
                    *month,
                    *day,
                    *hour,
                    *minute,
                    *second,
                    *subsecond_nanos,
                )
                .map_err(|e| {
                    PureException::from(PureRuntimeError::EvaluationError(format!(
                        "Invalid datetime literal: {e}"
                    )))
                })?;
                let shifted = if let Some(offset) = tz_offset_minutes {
                    civil
                        .checked_sub(jiff::Span::new().minutes(i64::from(*offset)))
                        .map_err(|e| {
                            PureException::from(PureRuntimeError::EvaluationError(format!(
                                "Invalid datetime timezone shift: {e}"
                            )))
                        })?
                } else {
                    civil
                };
                let precision = if *subsecond_digits > 0 {
                    crate::date::TimePrecision::Subsecond(*subsecond_digits)
                } else if *has_seconds {
                    crate::date::TimePrecision::Second
                } else {
                    crate::date::TimePrecision::Minute
                };
                let date = PureDate::datetime(
                    shifted.year(),
                    shifted.month(),
                    shifted.day(),
                    shifted.hour(),
                    shifted.minute(),
                    shifted.second(),
                    shifted.subsec_nanosecond(),
                    precision,
                )
                .map_err(|e| {
                    PureException::from(PureRuntimeError::EvaluationError(format!(
                        "Invalid datetime literal: {e}"
                    )))
                })?;
                Ok(Value::Date(date))
            }
            DateValue::StrictTime {
                hour,
                minute,
                second,
                subsecond_nanos,
                subsecond_digits: _,
            } => {
                let time = crate::date::StrictTime::new(*hour, *minute, *second, *subsecond_nanos)
                    .map_err(|e| {
                        PureException::from(PureRuntimeError::EvaluationError(format!(
                            "Invalid time literal: {e}"
                        )))
                    })?;
                Ok(Value::StrictTime(time))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Function call dispatch
    // -----------------------------------------------------------------------

    /// Dispatch a function call — the most complex evaluation case.
    ///
    /// Strategy (mirrors Java's `FunctionExpressionExecutor`):
    /// 1. Resolve the function's mangled FQN from the compiled model.
    /// 2. Check if the FQN is in the `NativeRegistry` — if so, hand the
    ///    native the raw argument `ValueSpec`s and let it drive its own
    ///    evaluation order through `ctx.evaluate` (the activator pattern).
    /// 3. If the function has a resolved `ElementId`, call the user function.
    /// 4. Error: function not found.
    #[allow(clippy::result_large_err)]
    fn eval_function_call(
        &mut self,
        function: Option<ElementId>,
        function_name: &str,
        arguments: &[ValueSpec],
        source_info: &legend_pure_parser_ast::SourceInfo,
    ) -> Result<Value, PureException> {
        // 1. Resolve the function's mangled FQN for native lookup.
        //    The compiler's Pass 2.1 stores the mangled FQN in ElementNode.name,
        //    so we just read it — zero computation at dispatch time.
        let lookup_key = match function {
            Some(id) => self.model.get_node(id).name.as_str(),
            None => function_name,
        };

        // 2a. Try native dispatch with exact FQN.
        if let Some(native) = self.natives.get(lookup_key) {
            return self.dispatch_native(native, lookup_key, arguments, source_info);
        }

        // 2b. If the compiler resolved the call to a concrete (non-native) function,
        //     dispatch to its body BEFORE the simple-name prefix fallback. A Pure
        //     wrapper like `elementToPath(PackageableElement[1]):String[1]` delegates
        //     to a higher-arity native — letting the prefix fallback fire here would
        //     route the wrapper call straight into the native with the wrong arity.
        //
        //     Native function declarations (no body) intentionally fall through to 2c
        //     so the prefix fallback can still paper over FQN-mangling mismatches for
        //     `if`, `map`, and friends.
        if let Some(element_id) = function
            && matches!(
                self.model.get_element(element_id),
                Element::Function(f) if !f.is_native
            )
        {
            let mut args = Vec::with_capacity(arguments.len());
            for arg in arguments {
                args.push(self.eval(arg)?);
            }
            return self.call_user_function(element_id, &args, function_name);
        }

        // 2c. Unresolved call or native with an FQN mismatch: last-resort
        //     prefix-based native lookup using the simple function name.
        if let Some(native) = self.natives.find_by_prefix(function_name) {
            return self.dispatch_native(native, lookup_key, arguments, source_info);
        }

        // 3. Nothing matched. If we reached here with a resolved element, it
        //    must be a native declaration (no body) for which we have no Rust
        //    implementation — don't call the empty body, that would silently
        //    return `Unit` and mask the missing native. Surface it as a
        //    FunctionNotFound so callers can classify it (PCT skip, CLI error).
        Err(PureException::from(PureRuntimeError::FunctionNotFound(
            lookup_key.into(),
        )))
    }

    // -----------------------------------------------------------------------
    // Native function dispatch
    // -----------------------------------------------------------------------

    /// Dispatch a native function using the expression-activator model.
    ///
    /// Hands the native the raw `ValueSpec` arguments unchanged — each native
    /// drives its own evaluation order through `ctx.evaluate`. Any exception
    /// the native raises is decorated with a stack frame before bubbling up.
    #[allow(clippy::result_large_err)]
    fn dispatch_native(
        &mut self,
        native: &dyn NativeFunction,
        lookup_key: &str,
        arguments: &[ValueSpec],
        source_info: &legend_pure_parser_ast::SourceInfo,
    ) -> Result<Value, PureException> {
        let result = {
            let mut ctx = EvalContext { evaluator: self };
            native.execute(arguments, &mut ctx)
        };

        result.map(Evaluated::into_value).map_err(|e| {
            e.with_frame(StackFrame {
                function_name: lookup_key.into(),
                source: source_info.clone(),
            })
        })
    }

    // -----------------------------------------------------------------------
    // User function evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a user-defined function from the compiled model.
    #[allow(clippy::result_large_err)]
    fn call_user_function(
        &mut self,
        element_id: ElementId,
        args: &[Value],
        function_name: &str,
    ) -> Result<Value, PureException> {
        let element = self.model.get_element(element_id);
        let node = self.model.get_node(element_id);

        let Element::Function(func) = element else {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("{function_name} is not a function"),
            )));
        };

        // Clone the data we need before borrowing self mutably
        let params = func.parameters.clone();
        let body = func.body.clone();
        let source_info = node.source_info.clone();

        self.hooks.enter_function(function_name, &source_info);

        // Push scope, bind parameters
        self.context.push_scope();

        for (param, arg) in params.iter().zip(args.iter()) {
            self.context.set(param.name.clone(), arg.clone());
        }

        // Evaluate body
        let result = self.eval_body(&body);

        // Pop scope (always, even on error)
        self.context.pop_scope();

        self.hooks.leave_function(function_name);

        // Wrap error with call stack frame
        result.map_err(|e| {
            e.with_frame(StackFrame {
                function_name: function_name.into(),
                source: source_info,
            })
        })
    }

    // -----------------------------------------------------------------------
    // Property access
    // -----------------------------------------------------------------------

    #[allow(clippy::result_large_err)]
    fn eval_property_access(
        &mut self,
        target: &ValueSpec,
        property: &str,
    ) -> Result<Value, PureException> {
        let target_val = self.eval(target)?;
        match &target_val {
            Value::Object(id) => {
                // Read as a multi-valued property and normalise via `from_vec`:
                // empty → `Unit`, single → scalar, multiple → `Collection`. This
                // matches Pure's multiplicity semantics — `$tg.before` on a
                // freshly-constructed `^TestGroup(before=[])` must be empty,
                // not a "property not found" error.
                let values = self
                    .heap
                    .get_property_values(*id, property)
                    .map_err(PureException::from)?;
                let collected: Vec<Value> = values.iter().cloned().collect();
                Ok(Value::from_vec(collected))
            }
            Value::Element(id) => {
                let id = *id;
                self.eval_element_property(id, property)
                    .map_err(PureException::from)
            }
            Value::Function(fv) => self
                .eval_function_property(fv, property, &target_val)
                .map_err(PureException::from),
            // Pure auto-maps property access over a collection:
            // `persons.firstName` produces the flattened collection of every
            // `firstName` value across every person in `persons`. Recurse
            // per element and concatenate — `from_vec` normalises the single-
            // vs. many case so scalar receivers keep working.
            Value::Collection(items) => {
                let items = items.clone();
                let mut out: Vec<Value> = Vec::new();
                for item in items.iter() {
                    let v = self.property_access_on_value(item, property)?;
                    match v {
                        Value::Unit => {}
                        Value::Collection(inner) => {
                            for x in inner.iter() {
                                out.push(x.clone());
                            }
                        }
                        other => out.push(other),
                    }
                }
                Ok(Value::from_vec(out))
            }
            _ => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "Property access on non-object value: {}.{}",
                    target_val.type_name(),
                    property
                ),
            ))),
        }
    }

    /// Read `property` off a pre-evaluated [`Value`] — used by the
    /// collection-auto-map path to recurse without re-evaluating the
    /// receiver expression. Mirrors `eval_property_access` without the
    /// wrapper `self.eval(target)` step.
    #[allow(clippy::result_large_err)]
    fn property_access_on_value(
        &mut self,
        value: &Value,
        property: &str,
    ) -> Result<Value, PureException> {
        match value {
            Value::Object(id) => {
                let values = self
                    .heap
                    .get_property_values(*id, property)
                    .map_err(PureException::from)?;
                let collected: Vec<Value> = values.iter().cloned().collect();
                Ok(Value::from_vec(collected))
            }
            Value::Element(id) => self
                .eval_element_property(*id, property)
                .map_err(PureException::from),
            Value::Function(fv) => self
                .eval_function_property(fv, property, value)
                .map_err(PureException::from),
            Value::Collection(items) => {
                let items = items.clone();
                let mut out: Vec<Value> = Vec::new();
                for item in items.iter() {
                    let v = self.property_access_on_value(item, property)?;
                    match v {
                        Value::Unit => {}
                        Value::Collection(inner) => {
                            for x in inner.iter() {
                                out.push(x.clone());
                            }
                        }
                        other => out.push(other),
                    }
                }
                Ok(Value::from_vec(out))
            }
            Value::Unit => Ok(Value::Unit),
            other => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "Property access on non-object value: {}.{property}",
                    other.type_name()
                ),
            ))),
        }
    }

    /// Synthesize Function-metamodel properties for a [`Value::Function`].
    ///
    /// Pure code reads a handful of properties off lambdas and compiled
    /// function refs — most importantly `expressionSequence`, which the
    /// `^LambdaFunction(expressionSequence = $fn.expressionSequence)` idiom
    /// uses to clone a lambda. We return the function itself wrapped as a
    /// single-element collection so the `New` native's LambdaFunction
    /// shortcut can round-trip it back into an invokable `Value::Function`.
    #[allow(clippy::result_large_err)]
    fn eval_function_property(
        &self,
        fv: &FunctionValue,
        property: &str,
        target_val: &Value,
    ) -> Result<Value, PureRuntimeError> {
        match property {
            "expressionSequence" => {
                let mut pv = PVector::new();
                pv.push_back(target_val.clone());
                Ok(Value::Collection(Box::new(pv)))
            }
            "functionName" | "name" => {
                let name = match fv {
                    FunctionValue::Compiled(id) => match self.model.get_element(*id) {
                        Element::Function(f) => f.function_name.clone(),
                        _ => self.model.element_name(*id).clone(),
                    },
                    FunctionValue::Lambda(_) => SmolStr::new_static("<lambda>"),
                };
                Ok(Value::String(name))
            }
            other => Err(PureRuntimeError::EvaluationError(format!(
                "Property '{other}' not supported on Function value"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Element property access
    // -----------------------------------------------------------------------

    /// Read a property of a model-element reference (`Value::Element`).
    ///
    /// Supports the M3 meta-model properties used by the Pure-level test
    /// orchestrator (`surveyor.pure`):
    /// - `name`         → simple name (function name for Functions, not mangled)
    /// - `package`      → parent package as `Value::Element(Package(..))`, or `Unit` for root
    /// - `children`     → for packages, child sub-packages + child elements as a Collection
    /// - `stereotypes`  → for Functions, Classes, etc: a Collection of freshly-allocated
    ///                    heap objects classifying as `meta::pure::metamodel::extension::Stereotype`
    ///                    with `value`/`profile` properties — matches the M3 representation
    ///                    surveyor expects.
    fn eval_element_property(
        &mut self,
        id: ElementId,
        property: &str,
    ) -> Result<Value, PureRuntimeError> {
        match property {
            // `functionName` is M3's name for a Function element's simple
            // name — `Function.functionName: String[1]`. `name` is the
            // generic shared property on `ModelElement` / `PackageableElement`.
            // For Function elements both return the same unmangled simple
            // name; for other kinds only `name` applies.
            "name" | "functionName" => {
                let name = match self.model.get_element(id) {
                    Element::Function(f) => f.function_name.clone(),
                    // Unit elements are stored with a `Measure~Unit`
                    // composite identity to keep names globally unique,
                    // but Pure's `$unit.name` idiom reads the local
                    // unit segment only — matching Java Pure semantics
                    // and the `->find(u | $u.name == 'Stadium')` test
                    // pattern in testNewUnitIndirectUnit.
                    Element::Unit(_) => {
                        let full = self.model.element_name(id);
                        full.rsplit_once('~')
                            .map_or_else(|| full.clone(), |(_, local)| SmolStr::new(local))
                    }
                    _ => self.model.element_name(id).clone(),
                };
                Ok(Value::String(name))
            }
            "package" => {
                let parent = match id {
                    ElementId::Package(pkg_id) => self
                        .model
                        .get_package(pkg_id)
                        .parent
                        .map(ElementId::Package),
                    ElementId::InstanceId { .. } => {
                        Some(ElementId::Package(self.model.get_node(id).parent_package))
                    }
                };
                match parent {
                    Some(pid) => Ok(Value::Element(pid)),
                    None => Ok(Value::Unit),
                }
            }
            "children" => match id {
                ElementId::Package(pkg_id) => {
                    let pkg = self.model.get_package(pkg_id);
                    let mut items: Vec<Value> = Vec::with_capacity(
                        pkg.children_packages.len() + pkg.children_elements.len(),
                    );
                    for &child_pkg_id in &pkg.children_packages {
                        items.push(Value::Element(ElementId::Package(child_pkg_id)));
                    }
                    for &child_eid in &pkg.children_elements {
                        items.push(Value::Element(child_eid));
                    }
                    Ok(Value::from_vec(items))
                }
                ElementId::InstanceId { .. } => Err(PureRuntimeError::EvaluationError(format!(
                    "Property 'children' is only valid for Package elements, got element {id}"
                ))),
            },
            "stereotypes" => {
                // Clone into an owned Vec so we can release the immutable borrow
                // of self.model before we mutate self.heap.
                //
                // Each access allocates fresh heap objects — identity is not preserved
                // across calls (`$f.stereotypes == $f.stereotypes` is false). Surveyor
                // only inspects `.value`/`.profile`, so this is fine. If callers start
                // caring about identity, cache per-(element, property) on first read.
                let stereos: Vec<(ElementId, SmolStr)> = match self.model.get_element(id) {
                    Element::Function(f) => f
                        .stereotypes
                        .iter()
                        .map(|s| (s.profile, s.value.clone()))
                        .collect(),
                    Element::Class(c) => c
                        .stereotypes
                        .iter()
                        .map(|s| (s.profile, s.value.clone()))
                        .collect(),
                    _ => Vec::new(),
                };
                let mut items: Vec<Value> = Vec::with_capacity(stereos.len());
                for (profile, value) in stereos {
                    let obj = self.heap.alloc_dynamic(crate::m3_paths::STEREOTYPE);
                    self.heap
                        .mutate_add(obj, "value", &[Value::String(value)])?;
                    self.heap
                        .mutate_add(obj, "profile", &[Value::Element(profile)])?;
                    items.push(Value::Object(obj));
                }
                Ok(Value::from_vec(items))
            }
            "taggedValues" => {
                let tags: Vec<(ElementId, SmolStr, String)> = match self.model.get_element(id) {
                    Element::Function(f) => f
                        .tagged_values
                        .iter()
                        .map(|t| (t.profile, t.tag.clone(), t.value.clone()))
                        .collect(),
                    Element::Class(c) => c
                        .tagged_values
                        .iter()
                        .map(|t| (t.profile, t.tag.clone(), t.value.clone()))
                        .collect(),
                    _ => Vec::new(),
                };
                let mut items: Vec<Value> = Vec::with_capacity(tags.len());
                for (profile, tag, value) in tags {
                    let obj = self.heap.alloc_dynamic(crate::m3_paths::TAGGED_VALUE);
                    self.heap.mutate_add(obj, "tag", &[Value::String(tag)])?;
                    self.heap
                        .mutate_add(obj, "profile", &[Value::Element(profile)])?;
                    self.heap
                        .mutate_add(obj, "value", &[Value::String(SmolStr::new(value))])?;
                    items.push(Value::Object(obj));
                }
                Ok(Value::from_vec(items))
            }
            "properties" | "qualifiedProperties" | "propertiesFromAssociations" => {
                self.eval_class_member_collection(id, property)
            }
            "generalizations" => {
                // Synthesize `Generalization` heap objects — one per direct
                // supertype. `getAllTypeGeneralisations` and `subTypeOf`
                // walk these. Each wraps a `GenericType` with `rawType`
                // pointing to the supertype element, and `specific`
                // pointing back to `id` (the subtype).
                //
                // Classes / primitives that declare no explicit `extends`
                // implicitly extend `Any` — the compiler does not
                // materialise that edge, so we add it here when the
                // declared chain is empty and the element is not `Any`
                // itself. Without this, the Pure-level helpers
                // (`getAllTypeGeneralisations`, `_subTypeOf(..., Any)`)
                // fail to reach the top of the lattice.
                let declared: Vec<ElementId> = match self.model.get_element(id) {
                    Element::Class(c) => c
                        .super_types
                        .iter()
                        .filter_map(|st| match st {
                            TypeExpr::Named { element, .. } => Some(*element),
                            _ => None,
                        })
                        .collect(),
                    Element::PrimitiveType(p) => p.super_type.into_iter().collect(),
                    _ => Vec::new(),
                };
                let is_typelike = matches!(
                    self.model.get_element(id),
                    Element::Class(_) | Element::PrimitiveType(_)
                );
                let super_eids: Vec<ElementId> = if declared.is_empty()
                    && is_typelike
                    && id != legend_pure_parser_pure::bootstrap::ANY_ID
                {
                    vec![legend_pure_parser_pure::bootstrap::ANY_ID]
                } else {
                    declared
                };
                let mut items = Vec::with_capacity(super_eids.len());
                for super_eid in super_eids {
                    let gt_obj = self.heap.alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
                    self.heap
                        .mutate_add(gt_obj, "rawType", &[Value::Element(super_eid)])?;
                    let gen_obj = self.heap.alloc_dynamic(crate::m3_paths::GENERALIZATION);
                    self.heap
                        .mutate_add(gen_obj, "general", &[Value::Object(gt_obj)])?;
                    self.heap
                        .mutate_add(gen_obj, "specific", &[Value::Element(id)])?;
                    items.push(Value::Object(gen_obj));
                }
                Ok(Value::from_vec(items))
            }
            _ => {
                // Enum value access: `MyEnum.VALUE` where the compiler didn't
                // pre-lower to `ExprKind::EnumValue`. Emit the same
                // first-class `Value::EnumValue` the `ExprKind::EnumValue`
                // branch produces so comparisons and type() agree between
                // the two paths.
                if let Element::Enumeration(enum_def) = self.model.get_element(id)
                    && enum_def.values.iter().any(|v| v.name == property)
                {
                    return Ok(Value::EnumValue {
                        enum_id: id,
                        member: SmolStr::new(property),
                    });
                }
                // Measure introspection — `RomanLength.canonicalUnit` and
                // `.nonCanonicalUnits` reflect the compiled Measure node
                // directly. Used by `newUnit(RomanLength.canonicalUnit->
                // toOne(), 5)` / similar runtime-resolved unit lookups.
                if let Element::Measure(m) = self.model.get_element(id) {
                    match property {
                        "canonicalUnit" => {
                            return Ok(match m.canonical_unit {
                                Some(u) => Value::Element(u),
                                None => Value::Unit,
                            });
                        }
                        "nonCanonicalUnits" => {
                            let items: Vec<Value> = m
                                .non_canonical_units
                                .iter()
                                .copied()
                                .map(Value::Element)
                                .collect();
                            return Ok(Value::from_vec(items));
                        }
                        _ => {}
                    }
                }
                Err(PureRuntimeError::EvaluationError(format!(
                    "Property '{property}' not supported on model element references"
                )))
            }
        }
    }

    /// Surface the `properties` / `qualifiedProperties` / `propertiesFromAssociations`
    /// list of a Class as a Collection of heap-backed wrapper objects.
    ///
    /// Each wrapper is classified so [`apply_callable`](Self::apply_callable) can
    /// recognise it: a `Property` wrapper invoked with one argument performs
    /// a plain property access on that argument (matches Pure's
    /// `$propRef->eval($instance)` idiom). `QualifiedProperty` wrappers are
    /// surfaced but invocation of their bodies is not yet implemented.
    ///
    /// The `_owner` and `name` heap slots carry the class id and member name
    /// so callable wrappers know what to look up. `propertiesFromAssociations`
    /// returns an empty collection until associations are modelled in the
    /// compiler — emitting an empty list beats rejecting the property access
    /// outright, which is Phase 1B's goal.
    #[allow(clippy::result_large_err)]
    fn eval_class_member_collection(
        &mut self,
        id: ElementId,
        property: &str,
    ) -> Result<Value, PureRuntimeError> {
        // Normalise the property name to a static key for cache lookups.
        let kind: &'static str = match property {
            "properties" => "properties",
            "qualifiedProperties" => "qualifiedProperties",
            "propertiesFromAssociations" => "propertiesFromAssociations",
            _ => unreachable!("caller restricts property value"),
        };

        // Cached wrappers: the set of Property / QualifiedProperty heap
        // objects for a Class is functionally pure (it reads only class
        // metadata), so repeat queries return the same ObjectIds. This is
        // what `assertIs($class.properties->at(0), $class.properties->at(0))`
        // relies on.
        if let Some(cached) = self.member_wrapper_cache.get(&(id, kind)) {
            return Ok(Value::from_vec(cached.clone()));
        }

        // Snapshot names before we take the mutable heap borrow — reading
        // the model and writing to the heap can't alias `self`.
        let names: Vec<SmolStr> = match self.model.get_element(id) {
            Element::Class(c) => match kind {
                "properties" => c.properties.iter().map(|p| p.name.clone()).collect(),
                "qualifiedProperties" => c
                    .qualified_properties
                    .iter()
                    .map(|q| q.name.clone())
                    .collect(),
                "propertiesFromAssociations" => Vec::new(),
                _ => unreachable!("kind is one of the three above"),
            },
            _ => Vec::new(),
        };

        let classifier = match kind {
            "qualifiedProperties" => crate::m3_paths::QUALIFIED_PROPERTY,
            _ => crate::m3_paths::PROPERTY,
        };

        let mut items: Vec<Value> = Vec::with_capacity(names.len());
        for name in names {
            let obj = self.heap.alloc_dynamic(classifier);
            self.heap.mutate_add(obj, "name", &[Value::String(name)])?;
            self.heap.mutate_add(obj, "_owner", &[Value::Element(id)])?;
            items.push(Value::Object(obj));
        }
        self.member_wrapper_cache.insert((id, kind), items.clone());
        Ok(Value::from_vec(items))
    }

    // -----------------------------------------------------------------------
    // Qualified property access
    // -----------------------------------------------------------------------

    #[allow(clippy::result_large_err)]
    fn eval_qualified_property(
        &mut self,
        target: &ValueSpec,
        property: &str,
        arguments: &[ValueSpec],
    ) -> Result<Value, PureException> {
        let target_val = self.eval(target)?;

        // Universal `.all` — every Class has an implicit `MyClass.all` that
        // returns every heap instance classified as `MyClass` or any of its
        // subclasses. Mirrors Java Pure's platform-level `.all` accessor.
        if property == "all"
            && let Value::Element(class_id) = target_val
            && matches!(self.model.get_element(class_id), Element::Class(_))
        {
            let target_path =
                crate::model_utils::build_element_path(&self.model, class_id, "::", false);
            let mut instances: Vec<Value> = Vec::new();
            for (obj_id, classifier) in self.heap.iter_classifiers() {
                if classifier == target_path {
                    instances.push(Value::Object(obj_id));
                }
            }
            return Ok(Value::from_vec(instances));
        }

        // Direct QP invocation on a heap instance: `$instance.qp(args)`.
        // Resolve the instance's classifier → Class → `qualified_properties`
        // entry matching `property`, then evaluate the QP body with `this`
        // bound to the instance and positional params bound from `arguments`.
        if let Value::Object(obj_id) = target_val {
            let classifier = self
                .heap
                .classifier(obj_id)
                .map_err(PureException::from)?
                .to_string();
            let segments: Vec<SmolStr> = if classifier.is_empty() {
                Vec::new()
            } else {
                classifier.split("::").map(SmolStr::new).collect()
            };
            if let Some(class_id) = self.model.resolve_by_path(&segments)
                && let Element::Class(class) = self.model.get_element(class_id)
                && let Some(qp) = class
                    .qualified_properties
                    .iter()
                    .find(|q| q.name == property)
            {
                let params = qp.parameters.clone();
                let body = qp.body.clone();
                let mut args_v: Vec<Value> = Vec::with_capacity(arguments.len());
                for arg in arguments {
                    args_v.push(self.eval(arg)?);
                }
                self.context.push_scope();
                self.context
                    .set(SmolStr::new("this"), Value::Object(obj_id));
                for (param, arg) in params.iter().zip(args_v.iter()) {
                    self.context.set(param.name.clone(), arg.clone());
                }
                let result = self.eval_body(&body);
                self.context.pop_scope();
                return result;
            }
        }

        let mut _args = Vec::with_capacity(arguments.len());
        for arg in arguments {
            _args.push(self.eval(arg)?);
        }
        Err(PureException::from(PureRuntimeError::EvaluationError(
            format!("Qualified property access not yet supported: {property}"),
        )))
    }

    // -----------------------------------------------------------------------
    // Enum value
    // -----------------------------------------------------------------------

    fn eval_enum_value(&self, enum_element: ElementId, value: &str) -> Value {
        Value::EnumValue {
            enum_id: enum_element,
            member: SmolStr::new(value),
        }
    }

    // -----------------------------------------------------------------------
    // Lambda creation
    // -----------------------------------------------------------------------

    fn eval_lambda_creation(
        &self,
        parameters: &[legend_pure_parser_pure::types::Parameter],
        body: &[ValueSpec],
    ) -> Value {
        // Reify free-variable captures from the enclosing scope. Non-escaping
        // lambdas (map/filter/fold) still use the live context for evaluation;
        // captures exist so escaping lambdas — returned across a function
        // boundary, reflected via `openVariableValues`, or unpacked by the
        // surveyor — can round-trip their closure environment.
        let free = collect_free_variables(body, parameters);
        let mut captures = HashMap::with_capacity(free.len());
        for name in free {
            if let Some(val) = self.context.get(&name) {
                captures.insert(name, val.clone());
            }
        }
        Value::Function(Box::new(FunctionValue::Lambda(LambdaClosure {
            parameters: parameters.to_vec(),
            body: body.to_vec(),
            captures,
        })))
    }

    // -----------------------------------------------------------------------
    // Collection literal
    // -----------------------------------------------------------------------

    #[allow(clippy::result_large_err)]
    fn eval_collection(&mut self, elements: &[ValueSpec]) -> Result<Value, PureException> {
        if elements.is_empty() {
            return Ok(Value::Unit);
        }

        let mut values = PVector::new();
        for elem in elements {
            let val = self.eval(elem)?;
            // Flatten nested collections (Pure semantics: no nested collections)
            match val {
                Value::Collection(inner) => {
                    for v in inner.iter() {
                        values.push_back(v.clone());
                    }
                }
                Value::Unit => {} // Skip unit values
                other => values.push_back(other),
            }
        }

        if values.is_empty() {
            Ok(Value::Unit)
        } else if values.len() == 1 {
            Ok(values.pop_front().unwrap_or(Value::Unit))
        } else {
            Ok(Value::Collection(Box::new(values)))
        }
    }

    // -----------------------------------------------------------------------
    // Lambda evaluation (called by native functions via EvalContext)
    // -----------------------------------------------------------------------

    /// Evaluate a lambda closure with the given arguments.
    ///
    /// Called by native functions (via [`EvalContext`]) that need to invoke
    /// # Errors
    /// Returns `PureException` if lambda execution explicitly fails or an underlying
    /// expression evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn eval_lambda_value(
        &mut self,
        lambda: &LambdaClosure,
        args: &[Value],
    ) -> Result<Value, PureException> {
        self.context.push_scope();

        // Restore captures
        for (name, value) in &lambda.captures {
            self.context.set(name.clone(), value.clone());
        }

        // Bind parameters
        for (param, arg) in lambda.parameters.iter().zip(args.iter()) {
            self.context.set(param.name.clone(), arg.clone());
        }

        let result = self.eval_body(&lambda.body);

        self.context.pop_scope();

        result
    }

    /// Invoke a `Value::Function` with pre-evaluated arguments.
    ///
    /// Both `FunctionValue::Lambda` (anonymous closure) and `FunctionValue::Compiled`
    /// (named compiled element) are `Function` in Pure's type system and dispatched
    /// through the same entry point. The `FunctionValue` variant drives the internal
    /// strategy — callers need not distinguish.
    ///
    /// # Errors
    /// Returns `PureException` if `callable` is not a `Value::Function`, or if the
    /// underlying evaluation fails.
    #[allow(clippy::result_large_err)]
    pub fn apply_callable(
        &mut self,
        callable: &Value,
        args: &[Value],
    ) -> Result<Value, PureException> {
        match callable {
            Value::Function(fv) => self.eval_function_value(fv, args),
            // `Value::Element(fn_id)` flows in from places like surveyor's
            // `executeTest($t->cast(@Function<...>))` — `cast` preserves the
            // element handle rather than rewrapping it, so we need to
            // promote compiled-function elements to `FunctionValue::Compiled`
            // on the fly. Non-function elements remain a type error.
            Value::Element(id) if matches!(self.model.get_element(*id), Element::Function(_)) => {
                self.eval_function_value(&FunctionValue::Compiled(*id), args)
            }
            // Property wrapper objects synthesised by `eval_class_member_collection`
            // become callable here: `$propRef->eval($instance)` reads
            // `$instance.<name>`, where `<name>` is the property name stored
            // on the wrapper. This is the Pure idiom that `testEvaluateOne`
            // exercises: `LA_Person.properties->filter(...)->toOne()->eval($p)`.
            Value::Object(obj_id) => self.apply_object_callable(*obj_id, args),
            other => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("Expected Function, got {}", other.type_name()),
            ))),
        }
    }

    /// Invoke a heap-object-shaped callable — `Property` wrappers emitted by
    /// `.properties` and `QualifiedProperty` wrappers emitted by
    /// `.qualifiedProperties`. Dispatches by resolving the classifier string
    /// to an M3 `ElementId` and comparing against the canonical
    /// `meta::pure::metamodel::function::property::{Property,QualifiedProperty}`
    /// IDs, so we don't rely on textual suffix matching. Returns a type
    /// error for any other classifier so the common case (a non-function
    /// object) still surfaces the actionable "Expected Function" message.
    #[allow(clippy::result_large_err)]
    fn apply_object_callable(
        &mut self,
        id: crate::heap::ObjectId,
        args: &[Value],
    ) -> Result<Value, PureException> {
        let classifier = self
            .heap
            .classifier(id)
            .map_err(PureException::from)?
            .to_string();
        match callable_wrapper_kind(self.model, &classifier) {
            Some(WrapperKind::Property) => {
                if args.is_empty() {
                    return Err(PureException::from(PureRuntimeError::EvaluationError(
                        "Property invocation expects the instance as its argument".into(),
                    )));
                }
                let name = self.read_wrapper_name(id)?;
                self.apply_property_to_instance(&name, &args[0])
            }
            Some(WrapperKind::QualifiedProperty) => self.apply_qualified_property(id, args),
            None => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("Expected Function, got Object<{classifier}>"),
            ))),
        }
    }

    /// Invoke a `QualifiedProperty` wrapper — reads `name` + `_owner` off the
    /// wrapper, locates the QP definition on the owner Class, binds the first
    /// arg as `this` and the remainder as the QP's declared parameters, then
    /// evaluates the QP body. Mirrors `evaluate_Function_1__List_MANY__Any_MANY_`'s
    /// dispatch path for `LA_Person.qualifiedProperties` entries.
    #[allow(clippy::result_large_err)]
    fn apply_qualified_property(
        &mut self,
        id: crate::heap::ObjectId,
        args: &[Value],
    ) -> Result<Value, PureException> {
        let name = self.read_wrapper_name(id)?;
        let owner_vals = self
            .heap
            .get_property_values(id, "_owner")
            .map_err(PureException::from)?;
        let Some(Value::Element(owner_id)) = owner_vals.front().cloned() else {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                "QualifiedProperty wrapper is missing its '_owner' slot".into(),
            )));
        };
        let Element::Class(class) = self.model.get_element(owner_id) else {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("QualifiedProperty owner is not a Class: {owner_id:?}"),
            )));
        };
        let Some(qp) = class.qualified_properties.iter().find(|q| q.name == name) else {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "QualifiedProperty '{name}' not found on class {}",
                    self.model.element_name(owner_id)
                ),
            )));
        };
        if args.is_empty() {
            return Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("QualifiedProperty '{name}' requires a receiver instance"),
            )));
        }
        let params = qp.parameters.clone();
        let body = qp.body.clone();

        self.context.push_scope();
        // `this` — the receiver instance.
        self.context.set(SmolStr::new("this"), args[0].clone());
        // Declared parameters bound positionally from args[1..].
        for (param, arg) in params.iter().zip(args.iter().skip(1)) {
            self.context.set(param.name.clone(), arg.clone());
        }
        let result = self.eval_body(&body);
        self.context.pop_scope();
        result
    }

    /// Read the `name` slot of a Property / QualifiedProperty wrapper as a
    /// plain string, erroring if it is missing or multi-valued.
    #[allow(clippy::result_large_err)]
    fn read_wrapper_name(&self, id: crate::heap::ObjectId) -> Result<SmolStr, PureException> {
        let values = self
            .heap
            .get_property_values(id, "name")
            .map_err(PureException::from)?;
        match values.front() {
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err(PureException::from(PureRuntimeError::EvaluationError(
                "Property wrapper is missing its 'name' slot".into(),
            ))),
        }
    }

    /// Perform a plain property access on `instance` by `name`, returning the
    /// same multiplicity-normalised collection that the field-access path
    /// would produce. Routes Element receivers through the element-property
    /// path so metamodel introspection (`$type->properties()->filter(p |
    /// $p->eval($functionElement) != …)`) can read synthesised Function /
    /// Class fields the same way as direct `$element.name` access.
    #[allow(clippy::result_large_err)]
    fn apply_property_to_instance(
        &mut self,
        name: &str,
        instance: &Value,
    ) -> Result<Value, PureException> {
        match instance {
            Value::Object(obj_id) => {
                let values = self
                    .heap
                    .get_property_values(*obj_id, name)
                    .map_err(PureException::from)?;
                let collected: Vec<Value> = values.iter().cloned().collect();
                Ok(Value::from_vec(collected))
            }
            Value::Element(id) => {
                // Function elements expose a small set of metamodel fields
                // (`expressionSequence`, `functionName`, `name`) via the
                // Function-value path. Route through it first so the
                // `^$func()` → `.properties->eval($func)` metamodel-
                // introspection chain reads the same fields it would off a
                // `Value::Function` receiver.
                if matches!(self.model.get_element(*id), Element::Function(_)) {
                    let fv = FunctionValue::Compiled(*id);
                    let target = Value::Function(Box::new(fv.clone()));
                    if let Ok(v) = self.eval_function_property(&fv, name, &target) {
                        return Ok(v);
                    }
                }
                self.eval_element_property(*id, name)
                    .map_err(PureException::from)
            }
            other => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "Property invocation expects an Object instance, got {}",
                    other.type_name()
                ),
            ))),
        }
    }

    /// Dispatch a `FunctionValue` — anonymous lambda or compiled element.
    #[allow(clippy::result_large_err)]
    fn eval_function_value(
        &mut self,
        fv: &FunctionValue,
        args: &[Value],
    ) -> Result<Value, PureException> {
        match fv {
            FunctionValue::Lambda(closure) => self.eval_lambda_value(closure, args),
            FunctionValue::Compiled(id) => {
                // self.model has 'model lifetime — no conflict with &mut self for EvalContext.
                let mangled: SmolStr = self.model.get_node(*id).name.clone();
                self.dispatch_compiled_function(*id, &mangled, args)
            }
        }
    }

    /// Dispatch a compiled function element — native registry first, then user function.
    ///
    /// `mangled` is pre-read from `model.get_node(id).name` by the caller so this
    /// method does not need to borrow the model (avoiding a double-borrow with `&mut self`).
    ///
    /// Native functions receive `&[ValueSpec]` in the new activator model, but
    /// `apply_callable` has already forced its arguments to `&[Value]`. To
    /// bridge, we bind each pre-forced value under a reserved synthetic name
    /// in a fresh scope and hand the native variable-reference specs — calling
    /// `ctx.evaluate` on them round-trips back to the bound value.
    #[allow(clippy::result_large_err)]
    fn dispatch_compiled_function(
        &mut self,
        id: ElementId,
        mangled: &SmolStr,
        args: &[Value],
    ) -> Result<Value, PureException> {
        // self.natives is &'model NativeRegistry, so the native ref has 'model lifetime —
        // no conflict with the &mut self borrow used to construct EvalContext below.
        let native = self
            .natives
            .get(mangled.as_str())
            .or_else(|| self.natives.find_by_prefix(mangled.as_str()));
        if let Some(native) = native {
            return self.execute_native_with_values(native, args);
        }
        self.call_user_function(id, args, mangled.as_str())
    }

    /// Invoke a native with already-forced `&[Value]` arguments.
    ///
    /// Binds each value under `__native_arg_{i}` in a fresh scope and passes
    /// the native synthesised `Variable { name }` specs. This preserves the
    /// `&[ValueSpec]` execute signature without forcing the caller to
    /// reconstruct source expressions.
    #[allow(clippy::result_large_err)]
    fn execute_native_with_values(
        &mut self,
        native: &dyn NativeFunction,
        args: &[Value],
    ) -> Result<Value, PureException> {
        self.context.push_scope();
        let mut specs = Vec::with_capacity(args.len());
        let placeholder_source = legend_pure_parser_ast::SourceInfo::new("<dispatch>", 0, 0, 0, 0);
        for (i, v) in args.iter().enumerate() {
            let name: SmolStr = format!("__native_arg_{i}").into();
            self.context.set(name.clone(), v.clone());
            specs.push(ValueSpec {
                kind: Box::new(ExprKind::Variable { name }),
                source_info: placeholder_source.clone(),
                type_info: None,
            });
        }
        let result = {
            let mut ctx = EvalContext { evaluator: self };
            native.execute(&specs, &mut ctx)
        };
        self.context.pop_scope();
        result.map(Evaluated::into_value)
    }
}

// ---------------------------------------------------------------------------
// EvalContext — handle passed to native functions
// ---------------------------------------------------------------------------

/// Evaluation context passed to native functions.
///
/// Provides native functions with access to the evaluator's capabilities:
/// evaluating lambda bodies, reading/writing variables, and accessing the
/// object heap. Mirrors Java's `FunctionExecutionInterpreted` + `VariableContext`.
pub struct EvalContext<'a, 'model: 'a, H: EvalHooks = NoOpHooks> {
    /// The underlying evaluator.
    evaluator: &'a mut Evaluator<'model, H>,
}

impl<'model, H: EvalHooks> EvalContext<'_, 'model, H> {
    /// Access the compiled model.
    #[must_use]
    pub fn model(&self) -> &'model PureModel {
        self.evaluator.model
    }
}

impl<H: EvalHooks> crate::native::EvalContextTrait for EvalContext<'_, '_, H> {
    fn evaluate(&mut self, spec: &ValueSpec) -> Result<Evaluated, PureException> {
        self.evaluator.eval(spec).map(Evaluated::new)
    }

    fn context(&self) -> &VariableContext {
        &self.evaluator.context
    }

    fn context_mut(&mut self) -> &mut VariableContext {
        &mut self.evaluator.context
    }

    fn heap(&self) -> &RuntimeHeap {
        &self.evaluator.heap
    }

    fn heap_mut(&mut self) -> &mut RuntimeHeap {
        &mut self.evaluator.heap
    }

    fn call_function(&mut self, callable: &Value, args: &[Value]) -> Result<Value, PureException> {
        self.evaluator.apply_callable(callable, args)
    }

    fn model(&self) -> &legend_pure_parser_pure::model::PureModel {
        self.evaluator.model
    }

    fn console_output(&mut self, msg: &str) {
        self.evaluator.hooks.console_output(msg);
    }
}

impl<H: EvalHooks> std::fmt::Debug for Evaluator<'_, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Evaluator")
            .field("heap_objects", &self.heap.len())
            .field("context_depth", &self.context.depth())
            .finish()
    }
}

/// Collect the free-variable names of a lambda `body` given its declared
/// `parameters`. A "free" variable is any `Variable { name }` reference in
/// the body that isn't introduced by one of the lambda's own parameters or
/// by a `let` / nested-lambda binding along the walk.
///
/// Used at lambda-creation time to snapshot the enclosing scope into the
/// closure's `captures` map so an escaping lambda keeps seeing the values
/// it closed over — `openVariableValues($lambda)` depends on this.
pub(crate) fn collect_free_variables(
    body: &[ValueSpec],
    parameters: &[legend_pure_parser_pure::types::Parameter],
) -> std::collections::HashSet<SmolStr> {
    let mut binders: std::collections::HashSet<SmolStr> =
        parameters.iter().map(|p| p.name.clone()).collect();
    let mut free: std::collections::HashSet<SmolStr> = std::collections::HashSet::new();
    for spec in body {
        walk_free_variables(spec, &mut binders, &mut free);
    }
    free
}

/// Walk a single `ValueSpec`, accumulating free variables into `free` and
/// respecting `let`-desugared bindings in the caller's `binders` set.
fn walk_free_variables(
    spec: &ValueSpec,
    binders: &mut std::collections::HashSet<SmolStr>,
    free: &mut std::collections::HashSet<SmolStr>,
) {
    match &*spec.kind {
        ExprKind::Variable { name } => {
            if !binders.contains(name) {
                free.insert(name.clone());
            }
        }
        ExprKind::FunctionCall {
            function_name,
            arguments,
            ..
        } => {
            // `let x = rhs` lowers to `FunctionCall("letFunction",
            // [StringLiteral("x"), rhs])`. The rhs may reference free
            // variables / captures, so it's visited first. Only then does
            // `x` enter the binder set for the remainder of the body the
            // caller is iterating over.
            if function_name.as_str() == "letFunction" && arguments.len() == 2 {
                if let ExprKind::StringLiteral(binding_name) = &*arguments[0].kind {
                    walk_free_variables(&arguments[1], binders, free);
                    binders.insert(binding_name.clone());
                    return;
                }
            }
            for arg in arguments {
                walk_free_variables(arg, binders, free);
            }
        }
        ExprKind::Lambda {
            parameters: inner_params,
            body: inner_body,
        } => {
            // Nested lambda: its own parameters shadow the outer scope
            // within `inner_body`. Fork the binder set so outer code
            // after the nested lambda still sees the outer view.
            let mut inner_binders = binders.clone();
            for p in inner_params {
                inner_binders.insert(p.name.clone());
            }
            for inner in inner_body {
                walk_free_variables(inner, &mut inner_binders, free);
            }
        }
        ExprKind::PropertyAccess { target, .. } => {
            walk_free_variables(target, binders, free);
        }
        ExprKind::QualifiedPropertyAccess {
            target, arguments, ..
        } => {
            walk_free_variables(target, binders, free);
            for arg in arguments {
                walk_free_variables(arg, binders, free);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                walk_free_variables(e, binders, free);
            }
        }
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::EnumValue { .. }
        | ExprKind::TypeReference { .. }
        | ExprKind::PackageableElementRef { .. }
        | ExprKind::Column => {}
    }
}

/// Classify a heap object by resolving its classifier string to a
/// well-known M3 `ElementId`. Used by `apply_object_callable` to dispatch
/// Property / QualifiedProperty wrappers without relying on textual
/// suffix matching. Returns `None` if the classifier doesn't correspond
/// to one of the callable wrapper classes.
pub(crate) fn callable_wrapper_kind(model: &PureModel, classifier: &str) -> Option<WrapperKind> {
    let id = crate::m3_paths::resolve(model, classifier)?;
    if crate::m3_paths::resolve(model, crate::m3_paths::PROPERTY) == Some(id) {
        Some(WrapperKind::Property)
    } else if crate::m3_paths::resolve(model, crate::m3_paths::QUALIFIED_PROPERTY) == Some(id) {
        Some(WrapperKind::QualifiedProperty)
    } else {
        None
    }
}

/// Callable wrapper classification — see [`callable_wrapper_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WrapperKind {
    /// `meta::pure::metamodel::function::property::Property` — a Class
    /// property reference, invoked as `$propRef->eval($instance)` to read
    /// the named property from `$instance`.
    Property,
    /// `meta::pure::metamodel::function::property::QualifiedProperty` — a
    /// derived/qualified property reference, invoked via
    /// `$qp->evaluate(^List<Any>(values=$instance), …)`.
    QualifiedProperty,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use legend_pure_parser_ast::SourceInfo;
    use legend_pure_parser_pure::bootstrap;
    use legend_pure_parser_pure::types::{ExprKind, ValueSpec};

    use super::*;

    fn test_model() -> PureModel {
        let mut model = PureModel::new();
        let bootstrap_chunk = bootstrap::create_bootstrap_chunk(model.root_package);
        model.chunks.push(bootstrap_chunk);
        model
    }

    fn synth_src() -> SourceInfo {
        SourceInfo::new("<test>", 1, 1, 1, 10)
    }

    fn make_expr(kind: ExprKind) -> ValueSpec {
        ValueSpec {
            kind: Box::new(kind),
            source_info: synth_src(),
            type_info: None,
        }
    }

    #[test]
    fn eval_integer_literal() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::IntegerLiteral(42));
        assert_eq!(eval.eval(&expr).unwrap(), Value::Integer(42));
    }

    #[test]
    fn eval_float_literal() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::FloatLiteral(std::f64::consts::PI));
        assert_eq!(
            eval.eval(&expr).unwrap(),
            Value::Float(std::f64::consts::PI)
        );
    }

    #[test]
    fn eval_string_literal() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::StringLiteral("hello".into()));
        assert_eq!(eval.eval(&expr).unwrap(), Value::String("hello".into()));
    }

    #[test]
    fn eval_boolean_literal() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::BooleanLiteral(true));
        assert_eq!(eval.eval(&expr).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn eval_variable_reference() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        eval.context_mut().push_scope();
        eval.context_mut().set("x", Value::Integer(99));
        let expr = make_expr(ExprKind::Variable { name: "x".into() });
        assert_eq!(eval.eval(&expr).unwrap(), Value::Integer(99));
        eval.context_mut().pop_scope();
    }

    #[test]
    fn eval_variable_not_found() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::Variable {
            name: "nonexistent".into(),
        });
        assert!(eval.eval(&expr).is_err());
    }

    #[test]
    fn eval_native_function_call() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::FunctionCall {
            function: None,
            function_name: "plus_Number_MANY__Number_1_".into(),
            arguments: vec![
                make_expr(ExprKind::IntegerLiteral(2)),
                make_expr(ExprKind::IntegerLiteral(3)),
            ],
        });
        assert_eq!(eval.eval(&expr).unwrap(), Value::Integer(5));
    }

    #[test]
    fn eval_nested_function_calls() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        // plus(2, times(3, 4)) -> 14
        let expr = make_expr(ExprKind::FunctionCall {
            function: None,
            function_name: "plus_Number_MANY__Number_1_".into(),
            arguments: vec![
                make_expr(ExprKind::IntegerLiteral(2)),
                make_expr(ExprKind::FunctionCall {
                    function: None,
                    function_name: "times_Number_MANY__Number_1_".into(),
                    arguments: vec![
                        make_expr(ExprKind::IntegerLiteral(3)),
                        make_expr(ExprKind::IntegerLiteral(4)),
                    ],
                }),
            ],
        });
        assert_eq!(eval.eval(&expr).unwrap(), Value::Integer(14));
    }

    #[test]
    fn eval_collection_literal() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::Collection {
            elements: vec![
                make_expr(ExprKind::IntegerLiteral(1)),
                make_expr(ExprKind::IntegerLiteral(2)),
                make_expr(ExprKind::IntegerLiteral(3)),
            ],
        });
        match eval.eval(&expr).unwrap() {
            Value::Collection(v) => {
                assert_eq!(v.len(), 3);
                assert_eq!(v[0], Value::Integer(1));
                assert_eq!(v[1], Value::Integer(2));
                assert_eq!(v[2], Value::Integer(3));
            }
            other => panic!("Expected Collection, got {other:?}"),
        }
    }

    #[test]
    fn eval_empty_collection() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::Collection { elements: vec![] });
        assert_eq!(eval.eval(&expr).unwrap(), Value::Unit);
    }

    #[test]
    fn eval_lambda_creation() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::Lambda {
            parameters: vec![],
            body: vec![make_expr(ExprKind::IntegerLiteral(42))],
        });
        match eval.eval(&expr).unwrap() {
            Value::Function(fv) => match *fv {
                FunctionValue::Lambda(lc) => {
                    assert_eq!(lc.parameters.len(), 0);
                    assert_eq!(lc.body.len(), 1);
                }
                other => panic!("Expected FunctionValue::Lambda, got {other:?}"),
            },
            other => panic!("Expected Function, got {other:?}"),
        }
    }

    #[test]
    fn eval_function_not_found() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let expr = make_expr(ExprKind::FunctionCall {
            function: None,
            function_name: "nonexistent".into(),
            arguments: vec![],
        });
        assert!(eval.eval(&expr).is_err());
    }

    #[test]
    fn eval_body_returns_last() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        let body = vec![
            make_expr(ExprKind::IntegerLiteral(1)),
            make_expr(ExprKind::IntegerLiteral(2)),
            make_expr(ExprKind::IntegerLiteral(3)),
        ];
        assert_eq!(eval.eval_body(&body).unwrap(), Value::Integer(3));
    }

    #[test]
    fn eval_empty_body() {
        let model = test_model();
        let registry = NativeRegistry::standard();
        let mut eval = Evaluator::new(&model, &registry);
        assert_eq!(eval.eval_body(&[]).unwrap(), Value::Unit);
    }
}
