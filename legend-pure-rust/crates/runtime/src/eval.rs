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
use legend_pure_parser_pure::types::{DateValue, ExprKind, ValueSpec};
use smol_str::SmolStr;

use crate::context::VariableContext;
use crate::date::PureDate;
use crate::error::{PureException, PureRuntimeError, StackFrame};
use crate::heap::RuntimeHeap;
use crate::hooks::{EvalHooks, NoOpHooks};
use crate::native::{NativeFunction, NativeRegistry};
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
                let date = PureDate::strict_date(*year, *month, *day).map_err(|e| {
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
            } => {
                let precision = if *subsecond_nanos > 0 {
                    crate::date::TimePrecision::Subsecond(9)
                } else {
                    crate::date::TimePrecision::Second
                };
                let date = PureDate::datetime(
                    *year,
                    *month,
                    *day,
                    *hour,
                    *minute,
                    *second,
                    *subsecond_nanos,
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
    /// 1. Resolve the function's mangled FQN from the compiled model
    /// 2. Check if the FQN is in the `NativeRegistry`
    ///    - If `native.defer_execution()` → pass unevaluated arg expressions
    ///    - Else → evaluate all arguments left-to-right, pass Values
    ///    - Call native.execute(args, &mut `EvalContext`)
    /// 3. If function has a resolved `ElementId` → call user function
    /// 4. Error: function not found
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

    /// Dispatch a native function, handling both deferred and eager execution.
    #[allow(clippy::result_large_err)]
    fn dispatch_native(
        &mut self,
        native: &dyn NativeFunction,
        lookup_key: &str,
        arguments: &[ValueSpec],
        source_info: &legend_pure_parser_ast::SourceInfo,
    ) -> Result<Value, PureException> {
        if native.defer_execution() {
            // Deferred execution: pass unevaluated expressions as lambdas.
            let deferred_args: Vec<Value> = arguments
                .iter()
                .map(|arg| {
                    let closure = match &*arg.kind {
                        legend_pure_parser_pure::types::ExprKind::Lambda { parameters, body } => {
                            LambdaClosure {
                                parameters: parameters.clone(),
                                body: body.clone(),
                                captures: std::collections::HashMap::new(),
                            }
                        }
                        _ => LambdaClosure {
                            parameters: vec![],
                            body: vec![arg.clone()],
                            captures: std::collections::HashMap::new(),
                        },
                    };
                    Value::Function(Box::new(FunctionValue::Lambda(closure)))
                })
                .collect();

            let result = {
                let mut ctx = EvalContext { evaluator: self };
                native.execute(&deferred_args, &mut ctx)
            };

            return result.map_err(|e| {
                PureException::from(e).with_frame(StackFrame {
                    function_name: lookup_key.into(),
                    source: source_info.clone(),
                })
            });
        }

        // Eager evaluation: evaluate all arguments left-to-right
        let mut args = Vec::with_capacity(arguments.len());
        for arg in arguments {
            args.push(self.eval(arg)?);
        }

        let result = {
            let mut ctx = EvalContext { evaluator: self };
            native.execute(&args, &mut ctx)
        };

        result.map_err(|e| {
            PureException::from(e).with_frame(StackFrame {
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
            _ => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!(
                    "Property access on non-object value: {}.{}",
                    target_val.type_name(),
                    property
                ),
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
            "name" => {
                let name = match self.model.get_element(id) {
                    Element::Function(f) => f.function_name.clone(),
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
                    let obj = self
                        .heap
                        .alloc_dynamic("meta::pure::metamodel::extension::Stereotype");
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
                    let obj = self
                        .heap
                        .alloc_dynamic("meta::pure::metamodel::extension::TaggedValue");
                    self.heap.mutate_add(obj, "tag", &[Value::String(tag)])?;
                    self.heap
                        .mutate_add(obj, "profile", &[Value::Element(profile)])?;
                    self.heap
                        .mutate_add(obj, "value", &[Value::String(SmolStr::new(value))])?;
                    items.push(Value::Object(obj));
                }
                Ok(Value::from_vec(items))
            }
            _ => {
                // Enum value access: `MyEnum.VALUE` where the compiler didn't
                // pre-lower to `ExprKind::EnumValue`. Match the format used by
                // `eval_enum_value` — `"SimpleName.VALUE"` — so equality
                // comparisons between the two paths stay consistent.
                if let Element::Enumeration(enum_def) = self.model.get_element(id)
                    && enum_def.values.iter().any(|v| v.name == property)
                {
                    let simple = self.model.element_name(id);
                    return Ok(Value::String(SmolStr::new(format!("{simple}.{property}"))));
                }
                Err(PureRuntimeError::EvaluationError(format!(
                    "Property '{property}' not supported on model element references"
                )))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Qualified property access
    // -----------------------------------------------------------------------

    #[allow(clippy::result_large_err)]
    #[allow(clippy::used_underscore_binding)]
    fn eval_qualified_property(
        &mut self,
        target: &ValueSpec,
        property: &str,
        arguments: &[ValueSpec],
    ) -> Result<Value, PureException> {
        let _target_val = self.eval(target)?;
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
        let node = self.model.get_node(enum_element);
        Value::String(SmolStr::new(format!("{}.{}", node.name, value)))
    }

    // -----------------------------------------------------------------------
    // Lambda creation
    // -----------------------------------------------------------------------

    #[allow(clippy::unused_self)]
    fn eval_lambda_creation(
        &self,
        parameters: &[legend_pure_parser_pure::types::Parameter],
        body: &[ValueSpec],
    ) -> Value {
        // Captures are empty for non-escaping lambdas (map/filter/fold).
        // The evaluator uses the enclosing context directly.
        Value::Function(Box::new(FunctionValue::Lambda(LambdaClosure {
            parameters: parameters.to_vec(),
            body: body.to_vec(),
            captures: HashMap::new(),
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
            other => Err(PureException::from(PureRuntimeError::EvaluationError(
                format!("Expected Function, got {}", other.type_name()),
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
    #[allow(clippy::result_large_err)]
    fn dispatch_compiled_function(
        &mut self,
        id: ElementId,
        mangled: &SmolStr,
        args: &[Value],
    ) -> Result<Value, PureException> {
        // self.natives is &'model NativeRegistry, so the native ref has 'model lifetime —
        // no conflict with the &mut self borrow used to construct EvalContext below.
        if let Some(native) = self.natives.get(mangled.as_str()) {
            let result = {
                let mut ctx = EvalContext { evaluator: self };
                native.execute(args, &mut ctx)
            };
            return result.map_err(PureException::from);
        }
        // Try prefix-based match (handles simple-name registration in the native registry).
        if let Some(native) = self.natives.find_by_prefix(mangled.as_str()) {
            let result = {
                let mut ctx = EvalContext { evaluator: self };
                native.execute(args, &mut ctx)
            };
            return result.map_err(PureException::from);
        }
        self.call_user_function(id, args, mangled.as_str())
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
    fn eval_lambda(
        &mut self,
        lambda_val: &Value,
        args: &[Value],
    ) -> Result<Value, PureRuntimeError> {
        match lambda_val {
            Value::Function(_) => self
                .evaluator
                .apply_callable(lambda_val, args)
                .map_err(|e| PureRuntimeError::EvaluationError(format!("{e}"))),
            _ => Err(PureRuntimeError::EvaluationError(format!(
                "Expected Function, got {}",
                lambda_val.type_name()
            ))),
        }
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
}

impl<H: EvalHooks> std::fmt::Debug for Evaluator<'_, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Evaluator")
            .field("heap_objects", &self.heap.len())
            .field("context_depth", &self.context.depth())
            .finish()
    }
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
