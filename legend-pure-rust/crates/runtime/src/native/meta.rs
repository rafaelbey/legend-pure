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

//! Meta-model native functions: `pathToElement`, `elementToPath`, `match`,
//! `id`, `type`, `genericType`, `rawType`, `enumName`, `enumValues`,
//! `toRepresentation`, `subTypeOf`.
//!
//! These bridge the Pure-level metamodel (`PackageableElement`,
//! `ConcreteFunctionDefinition`, `Package`) with the compiled [`PureModel`].
//! They are the foundation the Pure-native test orchestrator
//! (`surveyor.pure`) uses to discover and run tests.

// Helpers in this module take `ObjectHandle` by value for source clarity;
// with `Rc<RefCell<HeapEntry>>` that's an O(1) refcount bump.
#![allow(clippy::needless_pass_by_value)]

use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{FunctionCallData, TypeExpr, ValueSpec};
use smol_str::SmolStr;

use crate::date::DatePrecision;
use crate::error::{PureException, PureRuntimeError};
use crate::heap::{ObjectHandle, RuntimeHeap};
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::{FunctionValue, Value};

// ---------------------------------------------------------------------------
// pathToElement
// ---------------------------------------------------------------------------

/// Pure `pathToElement(path:String[1], separator:String[1]):PackageableElement[1]`
///
/// Resolves a package-qualified path string (e.g., `"meta::pure::functions"`)
/// to a [`Value::Element`] handle into the [`PureModel`]. Errors if the path
/// does not resolve — use `lenientPathToElement` for the optional variant.
#[derive(Debug)]
pub struct PathToElement;

impl NativeFunction for PathToElement {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("pathToElement", &values, 2)?;
        let path = values[0].as_string()?;
        let separator = values[1].as_string()?;
        match resolve_path(ctx.model(), path.as_str(), separator.as_str()) {
            Some(id) => Ok(Evaluated::new(Value::Element(id))),
            None => Err(PureRuntimeError::EvaluationError(format!(
                "pathToElement: path not found: '{path}'"
            ))
            .into()),
        }
    }

    fn signature(&self) -> &'static str {
        "pathToElement(path:String[1], separator:String[1]):PackageableElement[1]"
    }
}

// ---------------------------------------------------------------------------
// lenientPathToElement
// ---------------------------------------------------------------------------

/// Pure `lenientPathToElement(path:String[1], separator:String[1]):PackageableElement[0..1]`
///
/// Like [`PathToElement`], but returns an empty value (`Unit`) when the path
/// does not resolve instead of raising an error.
#[derive(Debug)]
pub struct LenientPathToElement;

impl NativeFunction for LenientPathToElement {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("lenientPathToElement", &values, 2)?;
        let path = values[0].as_string()?;
        let separator = values[1].as_string()?;
        match resolve_path(ctx.model(), path.as_str(), separator.as_str()) {
            Some(id) => Ok(Evaluated::new(Value::Element(id))),
            None => Ok(Evaluated::new(Value::Unit)),
        }
    }

    fn signature(&self) -> &'static str {
        "lenientPathToElement(path:String[1], separator:String[1]):PackageableElement[0..1]"
    }
}

// ---------------------------------------------------------------------------
// elementToPath
// ---------------------------------------------------------------------------

/// Pure `elementToPath(element:PackageableElement[1], separator:String[1], includeRoot:Boolean[1]):String[1]`
///
/// Builds the qualified path string for a model-element reference by walking
/// up the package parent chain. For functions, uses the simple `functionName`
/// (not the mangled signature).
///
/// When `includeRoot == true`, the anonymous root package is rendered as the
/// literal `"Root"` — matching the Java Pure runtime. `elementToPath(Package)`
/// is just `"Package"` (top-level types carry `parent_package = root`);
/// `elementToPath(Package, '::', true)` becomes `"Root::Package"`.
#[derive(Debug)]
pub struct ElementToPath;

impl NativeFunction for ElementToPath {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("elementToPath", &values, 3)?;
        let separator = values[1].as_string()?;
        let include_root = values[2].as_boolean()?;
        let path = match &values[0] {
            Value::Element(id) => {
                build_element_path(ctx.model(), *id, separator.as_str(), include_root)
            }
            Value::Object(obj_id) => {
                build_ephemeral_path(ctx.heap(), obj_id.clone(), separator.as_str(), include_root)
            }
            other => {
                return Err(PureRuntimeError::type_mismatch("PackageableElement", other).into());
            }
        };
        Ok(Evaluated::new(Value::String(SmolStr::new(path))))
    }

    fn signature(&self) -> &'static str {
        "elementToPath(element:PackageableElement[1], separator:String[1], includeRoot:Boolean[1]):String[1]"
    }
}

// ---------------------------------------------------------------------------
// sourceInformation
// ---------------------------------------------------------------------------

/// Pure `sourceInformation(node:Any[1]):SourceInformation[0..1]`
///
/// Returns a `SourceInformation` heap object for a model-element reference
/// (`Value::Element`) if source info is recoverable, otherwise [`Value::Unit`].
///
/// Populates `source`, `startLine`, `startColumn`, `line`, `column`,
/// `endLine`, `endColumn` — the same shape the surveyor filter inspects.
/// Non-element values (primitives, heap objects lacking a node) get `Unit`.
#[derive(Debug)]
pub struct SourceInformation;

impl NativeFunction for SourceInformation {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("sourceInformation", &values, 1)?;
        // Heap object: read its `sourceInformation` slot directly.
        // `Copy` populates this slot for instances created via
        // `^$obj(prop=val)` so the override case can be distinguished
        // from a no-override `^$obj()` clone — pinned by
        // `testSourceInformationCopy`. Falls back to Unit when the slot
        // isn't populated (the default for instances `evaluateAndDeactivate`
        // wraps without an explicit source attribution).
        if let Value::Object(obj_id) = &values[0] {
            let slot = ctx
                .heap()
                .get_property_values(&obj_id.clone(), "sourceInformation")
                .ok()
                .and_then(|vs| vs.iter().next().cloned());
            return Ok(Evaluated::new(slot.unwrap_or(Value::Unit)));
        }
        let Value::Element(id) = &values[0] else {
            return Ok(Evaluated::new(Value::Unit));
        };
        // Package elements have no ElementNode — surface that as "no info".
        let ElementId::InstanceId { .. } = id else {
            return Ok(Evaluated::new(Value::Unit));
        };
        let node = ctx.model().get_node(*id);
        let source = node.source_info.clone();
        let name_source = node.name_source_info.clone();
        let obj = ctx
            .heap_mut()
            .alloc_dynamic(crate::m3_paths::SOURCE_INFORMATION);
        let heap = ctx.heap_mut();
        heap.mutate_add(&obj, "source", &[Value::String(source.source.clone())])?;
        heap.mutate_add(
            &obj,
            "startLine",
            &[Value::Integer(i64::from(source.start_line))],
        )?;
        heap.mutate_add(
            &obj,
            "startColumn",
            &[Value::Integer(i64::from(source.start_column))],
        )?;
        // `line`/`column` = name-identifier position, distinct from the
        // declaration start — Java Pure parity for SourceInformation.
        heap.mutate_add(
            &obj,
            "line",
            &[Value::Integer(i64::from(name_source.start_line))],
        )?;
        heap.mutate_add(
            &obj,
            "column",
            &[Value::Integer(i64::from(name_source.start_column))],
        )?;
        heap.mutate_add(
            &obj,
            "endLine",
            &[Value::Integer(i64::from(source.end_line))],
        )?;
        heap.mutate_add(
            &obj,
            "endColumn",
            &[Value::Integer(i64::from(source.end_column))],
        )?;
        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "sourceInformation(node:Any[1]):SourceInformation[0..1]"
    }
}

// ---------------------------------------------------------------------------
// instanceOf
// ---------------------------------------------------------------------------

/// Pure `instanceOf(Any[1], Type[1]):Boolean[1]`
///
/// Checks whether a value is an instance of the given metamodel type.
/// Delegates to the same kind-based check used by [`Match`].
#[derive(Debug)]
pub struct InstanceOf;

impl NativeFunction for InstanceOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("instanceOf", &values, 2)?;
        let subject = &values[0];
        let type_id = as_element_id(&values[1])?;
        Ok(Evaluated::new(Value::Boolean(value_matches_type(
            ctx.model(),
            subject,
            type_id,
            ctx.heap(),
        ))))
    }

    fn signature(&self) -> &'static str {
        "instanceOf(Any[1], Type[1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// cast
// ---------------------------------------------------------------------------

/// Pure `cast<V|m>(p:Any[m], typ:V[1]):V[m]`
///
/// Runtime type coercion — returns the value unchanged if it is an instance
/// of the target type, otherwise raises a [`PureRuntimeError::TypeMismatch`].
#[derive(Debug)]
pub struct Cast;

impl NativeFunction for Cast {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("cast", &values, 2)?;
        let subject = values[0].clone();
        let type_id = as_element_id(&values[1])?;
        let type_name = ctx.model().element_name(type_id).to_string();
        // Extract type-variable-values from the source `@P(8)` reference.
        // Forced `Value::Element` drops them; we peek the original spec
        // tree so `cast(10, @P(8))` can bind `x = 8` for the constraint
        // evaluation below. The compiler stores them as
        // `TypeExpr::Named { value_arguments, … }` in the `TypeReference`
        // ExprKind produced by `@P(8)` — see `lower_type_reference`.
        let type_value_args: Vec<legend_pure_parser_pure::types::ConstValue> = if args.len() > 1 {
            match &*args[1].kind {
                legend_pure_parser_pure::types::ExprKind::TypeReference {
                    type_expr:
                        legend_pure_parser_pure::types::TypeExpr::Named {
                            value_arguments, ..
                        },
                    ..
                } => value_arguments.clone(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        // Error text follows the Java runtime's `Cast exception: X cannot
        // be cast to Y` format. Platform tests (`assertError(|…->cast(@T),
        // 'Cast exception: …')`) match on the string verbatim — the wording
        // is a behavioural contract, not a stylistic choice.
        let mk_err = |v: &Value| {
            PureException::from(PureRuntimeError::EvaluationError(format!(
                "Cast exception: {} cannot be cast to {type_name}",
                v.type_name()
            )))
        };
        // Two-phase: first verify type compatibility for every element
        // (immutable borrow of the heap), then evaluate constraints
        // (mutable borrow of the context). Collecting each item up front
        // keeps the closure's immutable borrow of `ctx` scoped to the
        // first loop.
        let items: Vec<Value> = match &subject {
            Value::Collection(coll) => coll.iter().cloned().collect(),
            Value::Unit => return Ok(Evaluated::new(Value::Unit)),
            other => vec![other.clone()],
        };
        for v in &items {
            if !cast_compatible(v, type_id, ctx.model(), ctx.heap()) {
                return Err(mk_err(v));
            }
        }
        for v in &items {
            evaluate_primitive_constraints(ctx, type_id, v, &type_value_args)?;
        }
        Ok(Evaluated::new(subject))
    }

    fn signature(&self) -> &'static str {
        "cast<V|m>(p:Any[m], typ:V[1]):V[m]"
    }
}

/// Run every constraint declared on the target primitive type (and its
/// ancestor parametric primitives) against `subject`. Raises
/// [`PureExceptionKind::ConstraintViolation`] on the first failure,
/// matching Java Pure's message shape
/// `"Constraint :[<id>] violated in the Class <name>[, Message: <msg>]"`.
///
/// Inheritance walk: when the target's `super_type` is itself a
/// parametric primitive (`Primitive OP8 extends OP(8)`), the parent's
/// constraints are evaluated with the parent's `super_type_value_arguments`
/// bound in scope — that's how `@OP8` inherits OP's `$this < $x` with
/// `x = 8`. The walk stops at the first bootstrap primitive (Integer /
/// String / etc.) since those carry no constraints.
///
/// Scope bindings for each constraint body:
/// - `$this` → the value being cast
/// - each `type_variable_parameter` at this level → the matching
///   positional `value_args` entry (either the caller's `@P(8)` bindings
///   or the child's `extends OP(8)` bindings when walking up).
///
/// Non-primitive targets and primitives with no constraints in their
/// inheritance chain are a no-op.
#[allow(clippy::result_large_err)]
fn evaluate_primitive_constraints(
    ctx: &mut dyn EvalContextTrait,
    type_id: legend_pure_parser_pure::ids::ElementId,
    subject: &Value,
    type_value_args: &[legend_pure_parser_pure::types::ConstValue],
) -> Result<(), PureException> {
    // Collect the inheritance chain up front (element_id, value_args used
    // to bind that level's type variables). Starts at the cast target
    // with caller-supplied args; each step uses the child's
    // `super_type_value_arguments` to bind the parent's parameters.
    let mut chain: Vec<(
        legend_pure_parser_pure::ids::ElementId,
        Vec<legend_pure_parser_pure::types::ConstValue>,
    )> = Vec::new();
    {
        let mut current = Some((type_id, type_value_args.to_vec()));
        let mut visited: std::collections::HashSet<legend_pure_parser_pure::ids::ElementId> =
            std::collections::HashSet::new();
        while let Some((id, args)) = current {
            if !visited.insert(id) {
                break;
            }
            match ctx.model().get_element(id) {
                Element::PrimitiveType(prim) => {
                    let next_args = prim.super_type_value_arguments.clone();
                    current = prim.super_type.map(|s| (s, next_args));
                }
                _ => current = None,
            }
            chain.push((id, args));
        }
    }
    for (level_id, level_args) in chain {
        let (constraints, param_names, prim_name) = match ctx.model().get_element(level_id) {
            Element::PrimitiveType(prim) if !prim.constraints.is_empty() => {
                let names: Vec<SmolStr> = prim
                    .type_variable_parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                (
                    prim.constraints.clone(),
                    names,
                    ctx.model().element_name(level_id).clone(),
                )
            }
            _ => continue,
        };
        ctx.context_mut().push_scope();
        ctx.context_mut()
            .set(SmolStr::new_static("this"), subject.clone());
        for (i, name) in param_names.iter().enumerate() {
            if let Some(cv) = level_args.get(i) {
                let v = match cv {
                    legend_pure_parser_pure::types::ConstValue::Integer(n) => Value::Integer(*n),
                    legend_pure_parser_pure::types::ConstValue::String(s) => {
                        Value::String(SmolStr::new(s))
                    }
                };
                ctx.context_mut().set(name.clone(), v);
            }
        }
        let mut first_failure: Option<PureException> = None;
        for (idx, constraint) in constraints.iter().enumerate() {
            match ctx.evaluate(&constraint.function) {
                Ok(eval) => {
                    if matches!(eval.into_value(), Value::Boolean(true)) {
                        continue;
                    }
                }
                Err(e) => {
                    first_failure = Some(e);
                    break;
                }
            }
            let message = match constraint.message.as_ref().map(|m| ctx.evaluate(m)) {
                Some(Ok(eval)) => match eval.into_value() {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                },
                Some(Err(e)) => {
                    first_failure = Some(e);
                    break;
                }
                None => None,
            };
            let constraint_id = constraint
                .name
                .clone()
                .unwrap_or_else(|| SmolStr::new(idx.to_string()));
            first_failure = Some(PureException::constraint(
                constraint_id,
                crate::error::ConstraintKind::Class,
                prim_name.clone(),
                message,
                constraint.source_info.clone(),
                Vec::new(),
            ));
            break;
        }
        ctx.context_mut().pop_scope();
        if let Some(e) = first_failure {
            return Err(e);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// match
// ---------------------------------------------------------------------------

/// Pure `match<T|m,n>(var:Any[*], functions:Function<{Nil[n]->T[m]}>[1..*]):T[m]`
///
/// Type-based pattern matching: finds the first lambda in `functions` whose
/// first parameter's declared type is compatible with `var`, then invokes it.
///
/// Type compatibility is resolved by the class's simple name plus a small set
/// of structural checks covering the metamodel subclasses used by surveyor
/// (`Package`, `ConcreteFunctionDefinition`, `Class`, primitives, etc.). Full
/// Pure type-hierarchy dispatch is out of scope for this first cut — the cases
/// not covered fall through to a conservative "no match" and advance to the
/// next branch.
#[derive(Debug)]
pub struct Match;

impl NativeFunction for Match {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        let (subject, functions, extra) = match values.len() {
            2 => (values[0].clone(), values[1].to_collection(), None),
            3 => (
                values[0].clone(),
                values[1].to_collection(),
                Some(values[2].clone()),
            ),
            n => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "match: expected 2 or 3 argument(s), got {n}"
                ))
                .into());
            }
        };

        // Decompose the subject into (count, elements). Collection / Unit /
        // scalar all normalize to a uniform (count, element-iterator) shape
        // so multiplicity + element-type checks share one path.
        let elements: Vec<Value> = match &subject {
            Value::Collection(c) => c.iter().cloned().collect(),
            Value::Unit => Vec::new(),
            other => vec![other.clone()],
        };
        #[allow(clippy::cast_possible_truncation)]
        let subject_count = elements.len() as u32;

        for func_val in &functions {
            let Value::Function(fv) = func_val else {
                continue;
            };
            let FunctionValue::Lambda(lambda) = fv.as_ref() else {
                continue;
            };

            // Zero-arg lambda is a catch-all branch.
            if lambda.parameters.is_empty() {
                let extras: Vec<Value> = extra.iter().cloned().collect();
                return ctx.call_function(func_val, &extras).map(Evaluated::new);
            }

            let param = &lambda.parameters[0];
            let (lower, upper) = mult_bounds(&param.multiplicity);
            // Multiplicity gate: subject count must fit [lower..=upper].
            // Empty subject (`[]->cast(@String)`) matches any branch with
            // `lower == 0` regardless of the param's type — there are no
            // elements to type-check.
            if subject_count < lower || upper.is_some_and(|u| subject_count > u) {
                continue;
            }
            let type_ok = subject_count == 0
                || match &param.type_expr {
                    TypeExpr::Named {
                        element: type_class_id,
                        ..
                    } => elements
                        .iter()
                        .all(|e| value_matches_type(ctx.model(), e, *type_class_id, ctx.heap())),
                    // Unconstrained generic parameter → always matches.
                    // `Unresolved` (lambda type hole that escaped a
                    // partially-broken compile — the lambda-level
                    // `CannotInferLambdaParameterTypes` already fired)
                    // collapses here too: we don't have a concrete
                    // class to test against, so accept any value.
                    TypeExpr::Generic(_) | TypeExpr::Unresolved => true,
                    _ => false,
                };
            if !type_ok {
                continue;
            }
            // Match. Build the call args: `(subject, extra?)`. Pass the
            // *original* subject value so collections stay collections —
            // re-materializing from `elements` would change identity for
            // single-element collections.
            let mut call_args = vec![subject.clone()];
            if let Some(extra_v) = &extra {
                call_args.push(extra_v.clone());
            }
            return ctx.call_function(func_val, &call_args).map(Evaluated::new);
        }

        Err(PureRuntimeError::EvaluationError(format!(
            "match: no branch matched for value: {subject}"
        ))
        .into())
    }

    fn signature(&self) -> &'static str {
        "match<T,P|m,n,o>(var:Any[*], functions:Function<{Nil[n] [, P[o]] -> T[m]}>[1..*] [, with:P[o]]):T[m]"
    }
}

/// Decode a [`Multiplicity`] into concrete `[lower, upper]` bounds.
/// `Variable(_)` maps to `[0, *]` since at the runtime level a multiplicity
/// variable hasn't been bound to a specific range — it acts as
/// "any cardinality" for matching purposes.
fn mult_bounds(m: &legend_pure_parser_pure::types::Multiplicity) -> (u32, Option<u32>) {
    use legend_pure_parser_pure::types::Multiplicity as M;
    match m {
        M::PureOne => (1, Some(1)),
        M::ZeroOrOne => (0, Some(1)),
        M::ZeroOrMany | M::Variable(_) => (0, None),
        M::OneOrMany => (1, None),
        M::Range { lower, upper } => (*lower, *upper),
    }
}

// ---------------------------------------------------------------------------
// id
// ---------------------------------------------------------------------------

/// Pure `id(Any[1]):String[1]`
///
/// Returns an identity string for any value. Heap objects are identified by
/// `Anonymous_<ObjectHandle>` — matching the Java Pure runtime's convention for
/// anonymous instances. Primitives render their canonical textual form
/// (unquoted for strings). Model-element references render their fully
/// qualified path.
#[derive(Debug)]
pub struct Id;

impl NativeFunction for Id {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("id", &values, 1)?;
        let s = render_id(&values[0], ctx.model());
        Ok(Evaluated::new(Value::String(SmolStr::new(s))))
    }

    fn signature(&self) -> &'static str {
        "id(Any[1]):String[1]"
    }
}

// ---------------------------------------------------------------------------
// type
// ---------------------------------------------------------------------------

/// Pure `type(Any[1]):Type[1]`
///
/// Returns the metamodel [`Type`] element describing `value`'s runtime class.
/// Primitives resolve to their well-known bootstrap IDs (`Integer`, `Float`,
/// `String`, ...). Heap objects resolve by their stored classifier FQN.
/// Model-element references report themselves — the compiled metaclass is
/// structural and identity-equal to the element.
#[derive(Debug)]
pub struct TypeOf;

impl NativeFunction for TypeOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("type", &values, 1)?;
        let type_id = resolve_value_type(&values[0], ctx.model(), ctx.heap())?;
        Ok(Evaluated::new(Value::Element(type_id)))
    }

    fn signature(&self) -> &'static str {
        "type(Any[1]):Type[1]"
    }
}

// ---------------------------------------------------------------------------
// genericType
// ---------------------------------------------------------------------------

/// Pure `genericType(Any[1]):GenericType[1]`
///
/// Wraps `type(value)` in a freshly allocated `GenericType` heap object.
/// The allocated object has only the `rawType` property populated — type
/// arguments (which would cover parametric types like `List<Integer>`) are
/// left empty because the runtime does not yet reify generic arguments.
#[derive(Debug)]
pub struct GenericTypeOf;

impl NativeFunction for GenericTypeOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("genericType", &values, 1)?;

        // Empty-values InstanceValue (Java parity: `@type` at expression
        // position evaluates to `InstanceValue { genericType: <metadata>,
        // values: [] }` — see AntlrContextToM3CoreInstance line 1098-1099).
        // For these, return the IV's pre-set `genericType` slot directly
        // — Java's GenericType native takes the same path when
        // `valueCount == 0` (else branch). Identification is by M3
        // ElementId of the InstanceValue classifier, never by string.
        if let Value::Object(obj_id) = &values[0] {
            let classifier = ctx.heap().classifier(&obj_id.clone())?.clone();
            let iv_id = crate::m3_paths::resolve(ctx.model(), crate::m3_paths::INSTANCE_VALUE);
            if iv_id.is_some() && crate::m3_paths::resolve(ctx.model(), &classifier) == iv_id {
                let inner_values = ctx.heap().get_property_values(&obj_id.clone(), "values")?;
                if inner_values.is_empty() {
                    let gt_values = ctx
                        .heap()
                        .get_property_values(&obj_id.clone(), "genericType")?;
                    if let Some(Value::Object(gt_id)) = gt_values.iter().next() {
                        return Ok(Evaluated::new(Value::Object(gt_id.clone())));
                    }
                }
            }
        }

        let type_id = resolve_value_type(&values[0], ctx.model(), ctx.heap())?;
        // Heap-instance values created with `^Class<T1, T2>(…)` carry a
        // `__typeArguments` reserved slot — pull it before allocating the
        // wrapper so we can mirror those bindings into the GenericType's
        // own `typeArguments` list.
        let instance_type_args: Vec<Value> = match &values[0] {
            Value::Object(obj_id) => ctx
                .heap()
                .get_property_values(&obj_id.clone(), "__typeArguments")
                .ok()
                .map(|v| v.iter().cloned().collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
        ctx.heap_mut()
            .mutate_add(&obj, "rawType", &[Value::Element(type_id)])?;

        // When the value itself is a Class or Enumeration element
        // reference, the genericType is parameterised on that element —
        // `CC_Person->genericType()` yields `Class<CC_Person>`, so
        // `typeArguments[0].rawType` reads back as `CC_Person`. Populate
        // the single-argument shape; other kinds (instances, primitives,
        // enum values) have no parameterisation.
        if let Value::Element(elem_id) = &values[0] {
            let arg_eligible = matches!(
                ctx.model().get_element(*elem_id),
                Element::Class(_) | Element::Enumeration(_)
            );
            if arg_eligible {
                let arg_gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
                ctx.heap_mut()
                    .mutate_add(&arg_gt, "rawType", &[Value::Element(*elem_id)])?;
                ctx.heap_mut()
                    .mutate_add(&obj, "typeArguments", &[Value::Object(arg_gt)])?;
            }
        }
        // Heap-instance type bindings (`^List<String>(…)` → `[String]`)
        // surface as `typeArguments[i]` GenericType wrappers each carrying
        // a single `rawType` ref. Empty when the construction site had no
        // `<…>` parameters or the args weren't resolvable to elements.
        if !instance_type_args.is_empty() {
            let mut arg_objs: Vec<Value> = Vec::with_capacity(instance_type_args.len());
            for arg in instance_type_args {
                if let Value::Element(arg_id) = arg {
                    let arg_gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
                    ctx.heap_mut()
                        .mutate_add(&arg_gt, "rawType", &[Value::Element(arg_id)])?;
                    arg_objs.push(Value::Object(arg_gt));
                }
            }
            ctx.heap_mut()
                .mutate_add(&obj, "typeArguments", &arg_objs)?;
        }
        // Lambda + compiled-function values reify as a `FunctionType`
        // heap wrapper inside `typeArguments[0]` — Java parity for
        // `$f.genericType().typeArguments->at(0).rawType->toOne()
        //   ->cast(@FunctionType).parameters` reading back the lambda's
        // declared parameter list. Without this `typeArguments` is empty
        // and the cast fails. Done here (rather than upstream of the
        // outer `obj` allocation) so the existing `rawType` plumbing
        // remains the single source of truth for the genericType's own
        // class.
        if let Value::Function(fv) = &values[0] {
            let func_type_obj = build_function_type_wrapper(ctx, fv)?;
            let arg_gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
            ctx.heap_mut()
                .mutate_add(&arg_gt, "rawType", &[Value::Object(func_type_obj)])?;
            ctx.heap_mut()
                .mutate_add(&obj, "typeArguments", &[Value::Object(arg_gt)])?;
        }
        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "genericType(Any[1]):GenericType[1]"
    }
}

/// Allocate a `meta::pure::metamodel::type::FunctionType` heap wrapper
/// describing a `Value::Function`'s signature.
///
/// For lambdas: every declared parameter becomes a `VariableExpression`
/// child carrying `name` plus a `genericType` `GenericType` wrapping the
/// parameter's resolved type, with multiplicity reified as a sibling
/// `Multiplicity` wrapper. The lambda body's static return type isn't
/// known at this layer (no Pass-2.5 inference plumbed through to
/// runtime values); leave `returnType` / `returnMultiplicity` empty so
/// callers see Java's "unknown" shape rather than a wrong concrete one.
///
/// For compiled function references: pull `parameters`, `return_type`
/// and `return_multiplicity` straight off the `Function` compiled node
/// — these are exactly what the Java `Function.parameters`/`returnType`
/// reflective slots return.
#[allow(clippy::result_large_err)]
pub(crate) fn build_function_type_wrapper(
    ctx: &mut dyn EvalContextTrait,
    fv: &crate::value::FunctionValue,
) -> Result<crate::heap::ObjectHandle, PureException> {
    let func_type_obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::FUNCTION_TYPE);
    let (params, return_type, return_mult): (
        std::sync::Arc<[legend_pure_parser_pure::types::Parameter]>,
        _,
        _,
    ) = match fv {
        crate::value::FunctionValue::Lambda(closure) => {
            (std::sync::Arc::clone(&closure.parameters), None, None)
        }
        crate::value::FunctionValue::Compiled(id) => {
            if let Element::Function(f) = ctx.model().get_element(*id) {
                (
                    std::sync::Arc::clone(&f.parameters),
                    Some(f.return_type.clone()),
                    Some(f.return_multiplicity.clone()),
                )
            } else {
                (std::sync::Arc::from(Vec::new()), None, None)
            }
        }
        // Path closures expose `Function<{U[1]→V[m]}>` shape but we
        // don't synthesise a parameter array here — the start type +
        // last step's return type would have to be re-derived from
        // the model. Reflective callers that need the full FunctionType
        // shape on a Path are not yet exercised; surface an empty
        // signature so this code path compiles cleanly.
        crate::value::FunctionValue::Path(_) => (std::rc::Rc::from(Vec::new()), None, None),
    };

    let mut param_objs: Vec<Value> = Vec::with_capacity(params.len());
    for p in params.iter() {
        let var_expr_obj = ctx
            .heap_mut()
            .alloc_dynamic(crate::m3_paths::VARIABLE_EXPRESSION);
        ctx.heap_mut()
            .mutate_add(&var_expr_obj, "name", &[Value::String(p.name.clone())])?;
        if let Some(type_id) = type_expr_to_element(&p.type_expr) {
            let p_gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
            ctx.heap_mut()
                .mutate_add(&p_gt, "rawType", &[Value::Element(type_id)])?;
            ctx.heap_mut()
                .mutate_add(&var_expr_obj, "genericType", &[Value::Object(p_gt)])?;
        }
        let mult_obj = build_multiplicity_wrapper(ctx, &p.multiplicity)?;
        ctx.heap_mut()
            .mutate_add(&var_expr_obj, "multiplicity", &[Value::Object(mult_obj)])?;
        param_objs.push(Value::Object(var_expr_obj));
    }
    if !param_objs.is_empty() {
        ctx.heap_mut()
            .mutate_add(&func_type_obj, "parameters", &param_objs)?;
    }

    if let Some(rt) = return_type
        && let Some(rt_id) = type_expr_to_element(&rt)
    {
        let rt_gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
        ctx.heap_mut()
            .mutate_add(&rt_gt, "rawType", &[Value::Element(rt_id)])?;
        ctx.heap_mut()
            .mutate_add(&func_type_obj, "returnType", &[Value::Object(rt_gt)])?;
    }
    if let Some(rm) = return_mult {
        let rm_obj = build_multiplicity_wrapper(ctx, &rm)?;
        ctx.heap_mut().mutate_add(
            &func_type_obj,
            "returnMultiplicity",
            &[Value::Object(rm_obj)],
        )?;
    }
    Ok(func_type_obj)
}

/// Best-effort `TypeExpr → ElementId`. Generic / parametric type slots
/// have no element to point at; leave the caller to handle by skipping
/// the `rawType` slot rather than fabricating one.
fn type_expr_to_element(ty: &legend_pure_parser_pure::types::TypeExpr) -> Option<ElementId> {
    match ty {
        legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => Some(*element),
        _ => None,
    }
}

/// Allocate a `meta::pure::metamodel::multiplicity::Multiplicity` heap
/// wrapper carrying `lowerBound` / `upperBound` integer slots. Mirrors
/// the subset of `Multiplicity.<lower|upper>Bound` Pure code typically
/// reads off lambda parameter / return signatures; richer fields
/// (`name` for variable multiplicities, etc.) are not yet exposed.
#[allow(clippy::result_large_err)]
fn build_multiplicity_wrapper(
    ctx: &mut dyn EvalContextTrait,
    m: &legend_pure_parser_pure::types::Multiplicity,
) -> Result<crate::heap::ObjectHandle, PureException> {
    use legend_pure_parser_pure::types::Multiplicity as M;
    let (lower, upper): (i64, Option<i64>) = match m {
        M::PureOne => (1, Some(1)),
        M::ZeroOrOne => (0, Some(1)),
        M::OneOrMany => (1, None),
        M::Range { lower, upper } => (i64::from(*lower), upper.map(i64::from)),
        // ZeroOrMany and unbound variables: unbounded, zero or more
        M::ZeroOrMany | M::Variable(_) => (0, None),
    };
    let obj = ctx.heap_mut().alloc_dynamic(crate::m3_paths::MULTIPLICITY);
    let lower_value = ctx
        .heap_mut()
        .alloc_dynamic(crate::m3_paths::MULTIPLICITY_VALUE);
    ctx.heap_mut()
        .mutate_add(&lower_value, "value", &[Value::Integer(lower)])?;
    ctx.heap_mut()
        .mutate_add(&obj, "lowerBound", &[Value::Object(lower_value)])?;
    if let Some(u) = upper {
        let upper_value = ctx
            .heap_mut()
            .alloc_dynamic(crate::m3_paths::MULTIPLICITY_VALUE);
        ctx.heap_mut()
            .mutate_add(&upper_value, "value", &[Value::Integer(u)])?;
        ctx.heap_mut()
            .mutate_add(&obj, "upperBound", &[Value::Object(upper_value)])?;
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// rawType
// ---------------------------------------------------------------------------

/// Pure `rawType(GenericType[1]):Type[0..1]`
///
/// Reads the `rawType` property from a `GenericType` heap object. Returns
/// [`Value::Unit`] if the property is unset — matching the `[0..1]`
/// multiplicity contract.
#[derive(Debug)]
pub struct RawType;

impl NativeFunction for RawType {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("rawType", &values, 1)?;
        let Value::Object(obj_id) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("GenericType", &values[0]).into());
        };
        let heap_values = ctx.heap().get_property_values(&obj_id.clone(), "rawType")?;
        match heap_values.head() {
            Some(v) => Ok(Evaluated::new(v.clone())),
            None => Ok(Evaluated::new(Value::Unit)),
        }
    }

    fn signature(&self) -> &'static str {
        "rawType(GenericType[1]):Type[0..1]"
    }
}

// ---------------------------------------------------------------------------
// enumName
// ---------------------------------------------------------------------------

/// Pure `enumName(Enumeration<Any>[1]):String[1]`
///
/// Returns the simple name of an enumeration element reference. The input is
/// the `Enumeration` itself, not one of its values — to read a value's name
/// use the string-formatted `<EnumName>.<ValueName>` that `Evaluator` produces
/// for enum literals.
#[derive(Debug)]
pub struct EnumName;

impl NativeFunction for EnumName {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("enumName", &values, 1)?;
        let id = as_element_id(&values[0])?;
        match ctx.model().get_element(id) {
            Element::Enumeration(_) => Ok(Evaluated::new(Value::String(
                ctx.model().element_name(id).clone(),
            ))),
            _ => Err(PureRuntimeError::type_mismatch("Enumeration", &values[0]).into()),
        }
    }

    fn signature(&self) -> &'static str {
        "enumName(Enumeration<Any>[1]):String[1]"
    }
}

// ---------------------------------------------------------------------------
// enumValues
// ---------------------------------------------------------------------------

/// Pure `enumValues<T>(Enumeration<T>[1]):T[*]`
///
/// Expands an enumeration element to the collection of its member values —
/// each member encoded as `"<EnumSimpleName>.<MemberName>"` to match the
/// string representation produced by [`Evaluator::eval_enum_value`] and
/// consumed by the equality machinery that `match`/`instanceOf` rely on.
#[derive(Debug)]
pub struct EnumValues;

impl NativeFunction for EnumValues {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("enumValues", &values, 1)?;
        let id = as_element_id(&values[0])?;
        let Element::Enumeration(enum_def) = ctx.model().get_element(id) else {
            return Err(PureRuntimeError::type_mismatch("Enumeration", &values[0]).into());
        };
        let enum_values: Vec<Value> = enum_def
            .values
            .iter()
            .map(|v| Value::EnumValue {
                enum_id: id,
                member: v.name.clone(),
            })
            .collect();
        Ok(Evaluated::new(Value::from_vec(enum_values)))
    }

    fn signature(&self) -> &'static str {
        "enumValues<T>(Enumeration<T>[1]):T[*]"
    }
}

// ---------------------------------------------------------------------------
// toRepresentation
// ---------------------------------------------------------------------------

/// Pure `toRepresentation(Any[1]):String[1]`
///
/// Renders a value as a round-trippable Pure source snippet: strings are
/// single-quoted with embedded quotes escaped, numbers use their `Display`
/// form, collections render as `[a, b, c]`, heap objects render as
/// `<$classifier Anonymous_$id>`, and model elements render as their FQN.
/// This is the debug/introspection counterpart to [`Id`] — it does not
/// reuse the `id` native (avoiding a cross-native dispatch).
#[derive(Debug)]
pub struct ToRepresentation;

impl NativeFunction for ToRepresentation {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("toRepresentation", &values, 1)?;
        let s = render_representation(&values[0], ctx.model(), ctx.heap());
        Ok(Evaluated::new(Value::String(SmolStr::new(s))))
    }

    fn signature(&self) -> &'static str {
        "toRepresentation(Any[1]):String[1]"
    }
}

// ---------------------------------------------------------------------------
// subTypeOf
// ---------------------------------------------------------------------------

/// Pure `subTypeOf(Type[1], Type[1]):Boolean[1]`
///
/// Walks the class / primitive generalization chain starting from `child`,
/// returning `true` as soon as `parent` is found. Reflexivity is honoured
/// (`subTypeOf(T, T)` is `true`). Unresolved `TypeExpr::Generic` supertypes
/// (still symbolic post-M3 parsing) are skipped silently — they cannot
/// contribute a positive answer and must not abort the walk. Types that are
/// neither [`Element::Class`] nor [`Element::PrimitiveType`] yield `false`.
#[derive(Debug)]
pub struct SubTypeOf;

impl NativeFunction for SubTypeOf {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("subTypeOf", &values, 2)?;
        let child = as_element_id(&values[0])?;
        let parent = as_element_id(&values[1])?;
        Ok(Evaluated::new(Value::Boolean(is_sub_type_of(
            child,
            parent,
            ctx.model(),
        ))))
    }

    fn signature(&self) -> &'static str {
        "subTypeOf(Type[1], Type[1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract an `ElementId` from a [`Value::Element`].
pub(crate) fn as_element_id(v: &Value) -> Result<ElementId, PureException> {
    match v {
        Value::Element(id) => Ok(*id),
        other => Err(PureRuntimeError::type_mismatch("PackageableElement", other).into()),
    }
}

// `build_element_path` and `resolve_path` are re-exports of the shared
// `crate::model_utils` helpers — see the module for the algorithm and
// edge-case contract.
use crate::model_utils::{build_element_path, resolve_path};

/// Build the qualified path for an ephemeral (heap-constructed) packageable
/// element.
///
/// `^PackageableElement(name='X', package=^Package(...))` produces a
/// `Value::Object` that has no `ElementId`. Walk the heap's `package` /
/// `name` property chain instead. The outermost `^Package` — the one whose
/// `package` property is empty — plays the role of the root. Match the
/// Java Pure rule: a top-level element (direct child of the outermost
/// package) never gets a "Root" prefix, even with `include_root=true`;
/// the outermost package's own name is always suppressed unless it is
/// *named* `"Root"` AND `include_root` is true AND there is at least one
/// intermediate package segment.
///
/// An ephemeral with no `name` (e.g. `^PackageableElement()`) renders as
/// the empty string.
fn build_ephemeral_path(
    heap: &RuntimeHeap,
    obj_id: ObjectHandle,
    separator: &str,
    include_root: bool,
) -> String {
    let name_of = |oid: ObjectHandle| -> Option<SmolStr> {
        let values = heap.get_property_values(&oid, "name").ok()?;
        values.iter().next().and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    };
    let package_of = |oid: ObjectHandle| -> Option<ObjectHandle> {
        let values = heap.get_property_values(&oid, "package").ok()?;
        values.iter().next().and_then(|v| match v {
            Value::Object(p) => Some(p.clone()),
            _ => None,
        })
    };

    // Leaf name — empty string if the ephemeral has no name at all.
    let Some(leaf_name) = name_of(obj_id.clone()) else {
        return String::new();
    };

    // Walk upward collecting names. The outermost package (one with no
    // `package` property) is treated as the "root" marker.
    let mut chain: Vec<SmolStr> = vec![leaf_name];
    let mut cursor = package_of(obj_id);
    let mut outermost_root_name: Option<SmolStr> = None;
    while let Some(pkg_id) = cursor {
        let next = package_of(pkg_id.clone());
        if next.is_none() {
            // Outermost package — don't emit its name unconditionally;
            // leave inclusion to the root-prefix rule below.
            outermost_root_name = name_of(pkg_id);
            break;
        }
        if let Some(n) = name_of(pkg_id) {
            chain.push(n);
        }
        cursor = next;
    }

    // Apply the root-prefix rule: only prepend the outermost package name
    // when `include_root == true` and there is at least one intermediate
    // package segment (i.e. the element is nested, not top-level).
    let has_intermediate = chain.len() > 1;
    if include_root
        && has_intermediate
        && let Some(root_name) = outermost_root_name
    {
        chain.push(root_name);
    }

    chain.reverse();
    chain
        .iter()
        .map(SmolStr::as_str)
        .collect::<Vec<_>>()
        .join(separator)
}

// `element_simple_name` lives in `crate::model_utils`.

/// Check whether `value` is an instance of the class identified by
/// `type_class_id`.
///
/// This is a pragmatic first cut — sufficient for surveyor's
/// `Package` / `ConcreteFunctionDefinition` dispatch — not full type-hierarchy
/// subtyping. Unknown type classes conservatively return `false` so `match`
/// advances to the next branch rather than silently matching.
fn value_matches_type(
    model: &PureModel,
    value: &Value,
    type_class_id: ElementId,
    heap: &RuntimeHeap,
) -> bool {
    let type_name = match type_class_id {
        ElementId::Package(pkg_id) => &model.get_package(pkg_id).name,
        ElementId::InstanceId { .. } => &model.get_node(type_class_id).name,
    };

    match type_name.as_str() {
        "Any" => true,
        "Nil" => false,
        "Package" => matches!(value, Value::Element(id) if id.is_package()),
        "ConcreteFunctionDefinition" => {
            matches!(value, Value::Element(id)
                if matches!(model.get_element(*id), Element::Function(f) if !f.is_native))
        }
        "NativeFunctionDefinition" => {
            matches!(value, Value::Element(id)
                if matches!(model.get_element(*id), Element::Function(f) if f.is_native))
        }
        "FunctionDefinition" | "Function" => {
            matches!(value, Value::Function(_))
                || matches!(value, Value::Element(id)
                    if matches!(model.get_element(*id), Element::Function(_)))
        }
        "Class" => matches!(value, Value::Element(id)
            if matches!(model.get_element(*id), Element::Class(_))),
        "Enumeration" => matches!(value, Value::Element(id)
            if matches!(model.get_element(*id), Element::Enumeration(_))),
        "Enum" => matches!(value, Value::EnumValue { .. }),
        "Profile" => matches!(value, Value::Element(id)
            if matches!(model.get_element(*id), Element::Profile(_))),
        "Association" => matches!(value, Value::Element(id)
            if matches!(model.get_element(*id), Element::Association(_))),
        "Measure" => matches!(value, Value::Element(id)
            if matches!(model.get_element(*id), Element::Measure(_))),
        "PackageableElement" => matches!(value, Value::Element(_)),
        "Type" => matches!(value, Value::Element(id) if matches!(
            model.get_element(*id),
            Element::Class(_)
                | Element::Enumeration(_)
                | Element::PrimitiveType(_)
                | Element::Measure(_)
                | Element::Unit(_)
        )),
        "Boolean" => matches!(value, Value::Boolean(_)),
        "Integer" => matches!(value, Value::Integer(_)),
        "Float" => matches!(value, Value::Float(_)),
        "Decimal" => matches!(value, Value::Decimal(_)),
        "Number" => matches!(
            value,
            Value::Integer(_) | Value::Float(_) | Value::Decimal(_)
        ),
        "String" => matches!(value, Value::String(_)),
        "Date" | "DateTime" | "StrictDate" => matches!(value, Value::Date(_)),
        "StrictTime" => matches!(value, Value::StrictTime(_)),
        _ => {
            // Fallback 1: heap-object classifier exact-name match.
            if let Value::Object(obj_id) = value
                && let Ok(classifier) = heap.classifier(&obj_id.clone())
                && classifier
                    .rsplit("::")
                    .next()
                    .is_some_and(|tail| tail == type_name.as_str())
            {
                return true;
            }
            // Fallback 2: supertype-chain walk (the instanceOf direction).
            // Resolve the value's runtime type and walk its ancestors — a
            // value satisfies the type when its type extends the target.
            // This is what makes `$la_address->instanceOf(LA_GeographicEntity)`
            // and `$p8->cast(@Integer)` succeed for non-equal but related
            // types without every type-check growing bespoke Class wiring.
            if let Ok(value_type_id) = resolve_value_type(value, model, heap) {
                return type_extends(model, value_type_id, type_class_id);
            }
            false
        }
    }
}

/// Direct parents of `id` in declaration order, with an implicit edge to
/// `Any` when a Class / `PrimitiveType` declares none.
fn direct_parents(model: &PureModel, id: ElementId) -> Vec<ElementId> {
    let mut parents: Vec<ElementId> = Vec::new();
    match model.get_element(id) {
        Element::Class(c) => {
            for st in &c.super_types {
                if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                    parents.push(*element);
                }
            }
        }
        Element::PrimitiveType(p) => {
            if let Some(sup) = p.super_type {
                parents.push(sup);
            }
        }
        _ => return parents,
    }
    if parents.is_empty()
        && id != bootstrap::ANY_ID
        && matches!(
            model.get_element(id),
            Element::Class(_) | Element::PrimitiveType(_)
        )
    {
        parents.push(bootstrap::ANY_ID);
    }
    parents
}

/// C3 linearization — Python-style MRO. Produces the ordered supertype
/// chain Pure's `generalizations(Type)` expects (and that `assertIs` /
/// `assertEquals` compare against element-by-element).
///
/// For a class with multiple parents, the chain orders each parent's own
/// MRO pre-visited, merging them so each type appears exactly once and
/// relative order within each input list is preserved. Mirrors the classic
/// C3 algorithm with a conservative fallback: if the merge is inconsistent
/// (diamond with conflicting orderings) we emit the remaining candidates
/// in arrival order rather than panicking.
fn linearize_c3(model: &PureModel, id: ElementId) -> Vec<ElementId> {
    fn walk(
        model: &PureModel,
        id: ElementId,
        cache: &mut std::collections::HashMap<ElementId, Vec<ElementId>>,
    ) -> Vec<ElementId> {
        if let Some(cached) = cache.get(&id) {
            return cached.clone();
        }
        let parents = direct_parents(model, id);
        let parent_lines: Vec<Vec<ElementId>> =
            parents.iter().map(|p| walk(model, *p, cache)).collect();
        let mut lists: Vec<std::collections::VecDeque<ElementId>> = parent_lines
            .into_iter()
            .map(|v| v.into_iter().collect())
            .collect();
        if !parents.is_empty() {
            lists.push(parents.iter().copied().collect());
        }

        let mut result: Vec<ElementId> = vec![id];
        loop {
            // Trim empty lists.
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }
            // Take the first candidate that appears only as head in any list.
            let mut picked: Option<ElementId> = None;
            for list in &lists {
                if let Some(head) = list.front().copied() {
                    let in_tail = lists.iter().any(|other| {
                        let mut it = other.iter();
                        let _ = it.next(); // skip head
                        it.any(|&x| x == head)
                    });
                    if !in_tail {
                        picked = Some(head);
                        break;
                    }
                }
            }
            // Fallback: inconsistent hierarchy — take the first head
            // available rather than panicking. Keeps the native total.
            let pick = match picked {
                Some(p) => p,
                None => match lists.first().and_then(|l| l.front().copied()) {
                    Some(p) => p,
                    None => break,
                },
            };
            result.push(pick);
            for list in &mut lists {
                if list.front() == Some(&pick) {
                    list.pop_front();
                }
            }
        }
        cache.insert(id, result.clone());
        result
    }
    let mut cache = std::collections::HashMap::new();
    walk(model, id, &mut cache)
}

/// True when `descendant` extends (transitively) `ancestor` — or is `ancestor`.
///
/// Walks `Class::super_types` and `PrimitiveType::super_type` upward. Used by
/// [`value_matches_type`] and [`cast_compatible`] so hierarchy-aware runtime
/// checks don't each re-implement the chain walk.
/// Convenience: is the element identified by `descendant_id` a subtype of
/// the M3 class named by `ancestor_fqn`? Resolves the FQN against the
/// model and walks the supertype chain via [`type_extends`].
///
/// Returns `false` when either the FQN fails to resolve or the descendant
/// isn't in the chain. Useful for classifier checks where the caller only
/// knows a string path (`m3_paths::FUNCTION_EXPRESSION`) and the concrete
/// instance may be a registered subclass (`SimpleFunctionExpression`).
pub(crate) fn classifier_extends_m3(
    model: &PureModel,
    descendant_id: ElementId,
    ancestor_fqn: &str,
) -> bool {
    crate::m3_paths::resolve(model, ancestor_fqn)
        .is_some_and(|ancestor| type_extends(model, descendant_id, ancestor))
}

pub(crate) fn type_extends(model: &PureModel, descendant: ElementId, ancestor: ElementId) -> bool {
    let mut stack: Vec<ElementId> = vec![descendant];
    let mut visited: std::collections::HashSet<ElementId> = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if id == ancestor {
            return true;
        }
        match model.get_element(id) {
            Element::Class(c) => {
                for st in &c.super_types {
                    if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                        stack.push(*element);
                    }
                }
            }
            Element::PrimitiveType(p) => {
                if let Some(super_id) = p.super_type {
                    stack.push(super_id);
                }
            }
            _ => {}
        }
    }
    false
}

/// Cast-direction type check — succeeds in either direction of the lattice.
///
/// A cast succeeds when:
/// - the value already matches the target ([`value_matches_type`], which also
///   covers primitives and the instanceOf-direction subtype walk); **or**
/// - the target extends the value's type — Pure's "primitive extension
///   downcast" idiom (`1->cast(@P8)` where `P8 extends Integer`). The target
///   refines a supertype, and the runtime trusts the source of the cast.
fn cast_compatible(
    value: &Value,
    target_id: ElementId,
    model: &PureModel,
    heap: &RuntimeHeap,
) -> bool {
    if value_matches_type(model, value, target_id, heap) {
        return true;
    }
    let Ok(value_type_id) = resolve_value_type(value, model, heap) else {
        return false;
    };
    type_extends(model, target_id, value_type_id)
}

/// Resolve the metamodel [`ElementId`] for a runtime [`Value`]'s type.
///
/// Primitives map to their bootstrap IDs. Heap objects look up their
/// classifier's FQN — the stored path is split on `"::"` and resolved via
/// the model's package tree. Element refs report themselves. `Value::Unit`,
/// collections, maps, and functions fall back to `Any` (bootstrap top type)
/// — this matches the Java runtime's conservative upper bound for values
/// lacking a dedicated reified type.
fn resolve_value_type(
    value: &Value,
    model: &PureModel,
    heap: &RuntimeHeap,
) -> Result<ElementId, PureException> {
    match value {
        Value::Boolean(_) => Ok(bootstrap::BOOLEAN_ID),
        Value::Integer(_) => Ok(bootstrap::INTEGER_ID),
        Value::Float(_) => Ok(bootstrap::FLOAT_ID),
        Value::Decimal(_) => Ok(bootstrap::DECIMAL_ID),
        Value::String(_) => Ok(bootstrap::STRING_ID),
        Value::EnumValue { enum_id, .. } => Ok(*enum_id),
        Value::Date(d) => Ok(match d.precision() {
            DatePrecision::Day => bootstrap::STRICT_DATE_ID,
            DatePrecision::Time(_) => bootstrap::DATE_TIME_ID,
            // Year / Month precision — no finer bootstrap ID, classify as
            // the abstract `Date`.
            _ => bootstrap::DATE_ID,
        }),
        Value::StrictTime(_) => Ok(bootstrap::STRICT_TIME_ID),
        Value::Element(id) => {
            // The runtime type of an element reference is its metaclass
            // (`type(CC_Person) == Class`). `metatype_of` already centralises
            // the element-kind → M3 metaclass lookup for the compiler; we
            // reuse it here so `type(...)` / `genericType(...)` agree with
            // dispatch.
            if let Some(meta_id) =
                legend_pure_parser_pure::bootstrap::metatype_of(model, model.get_element(*id))
            {
                return Ok(meta_id);
            }
            Ok(*id)
        }
        Value::Object(obj_id) => {
            let classifier = heap.classifier(obj_id)?;
            model.resolve_fqn_str(classifier.as_str()).ok_or_else(|| {
                PureRuntimeError::EvaluationError(format!(
                    "type: classifier '{classifier}' does not resolve to a known Type"
                ))
                .into()
            })
        }
        // A unit-tagged numeric value — runtime type is the owning Unit
        // element, so `instanceOf(5 RomanLength~Pes, RomanLength~Pes)`
        // resolves via the normal subtype walk.
        Value::UnitInstance { unit_id, .. } => Ok(*unit_id),
        // Lambda / compiled-function values resolve through the M3
        // metatype hierarchy: `LambdaFunction` for anonymous lambdas
        // (Pure literal `{|true}`), and the compiled element's own
        // metatype (`ConcreteFunctionDefinition` for user functions,
        // `NativeFunctionDefinition` for native registrations) for
        // compiled refs. This is what makes `$f->type()->toOne()->id()`
        // return `'LambdaFunction'` for `$lambda->reactivate()` —
        // testInstanceValueReactivation's second assertion.
        Value::Function(fv) => match fv.as_ref() {
            crate::value::FunctionValue::Lambda(_) => {
                crate::m3_paths::resolve(model, crate::m3_paths::LAMBDA_FUNCTION).ok_or_else(|| {
                    PureRuntimeError::EvaluationError(
                        "type: meta::pure::metamodel::function::LambdaFunction not in model".into(),
                    )
                    .into()
                })
            }
            crate::value::FunctionValue::Compiled(eid) => {
                let element = model.get_element(*eid);
                Ok(
                    legend_pure_parser_pure::bootstrap::metatype_of(model, element)
                        .unwrap_or(bootstrap::ANY_ID),
                )
            }
            // Path closures are `meta::pure::metamodel::path::Path`
            // instances — runtime metatype matches Java parity for
            // `#/Type/p#->type()`.
            crate::value::FunctionValue::Path(_) => model
                .resolve_by_path(&[
                    smol_str::SmolStr::new("meta"),
                    smol_str::SmolStr::new("pure"),
                    smol_str::SmolStr::new("metamodel"),
                    smol_str::SmolStr::new("path"),
                    smol_str::SmolStr::new("Path"),
                ])
                .ok_or_else(|| {
                    PureRuntimeError::EvaluationError(
                        "type: meta::pure::metamodel::path::Path not in model".into(),
                    )
                    .into()
                }),
        },
        // Collection runtime type is the least-upper-bound of its
        // elements' types — `[1, 2, 3].type() == Integer`,
        // `[1, 'a'].type() == Any`, `[CO_Address, CO_Location].type()
        // == CO_GeographicEntity`. Empty collections fall through to
        // `Any`. Mirrors what Java compiled mode can't do because
        // of erasure — `testConcatenateTypeInference` is excluded
        // there but exercised here. Resolution per element reuses
        // this function recursively, then folds via
        // `least_upper_bound_ids` (the same compile-time LUB the
        // overload resolver uses).
        Value::Collection(items) => {
            let mut iter = items.iter();
            let Some(first) = iter.next() else {
                return Ok(bootstrap::ANY_ID);
            };
            let mut acc = resolve_value_type(first, model, heap)?;
            for item in iter {
                if acc == bootstrap::ANY_ID {
                    break; // Any is the top — no further widening possible.
                }
                let t = resolve_value_type(item, model, heap)?;
                acc = legend_pure_parser_pure::resolve::least_upper_bound_ids(acc, t, model);
            }
            Ok(acc)
        }
        Value::Map(_) | Value::Unit => Ok(bootstrap::ANY_ID),
    }
}

/// Render the identity string for a value — powers the `id` native.
///
/// For model-element references the id is the element's **simple name**
/// (`CC_Person->id() == 'CC_Person'`), not its qualified path — that
/// matches Java Pure and the `id.pure` / `reactivate.pure` assertions.
/// Qualified-path rendering is the job of `elementToPath`.
fn render_id(value: &Value, model: &PureModel) -> String {
    match value {
        Value::Object(obj_id) => {
            format!("Anonymous_{:p}", std::rc::Rc::as_ptr(obj_id))
        }
        Value::Element(id) => crate::model_utils::element_simple_name(model, *id).to_string(),
        Value::String(s) => s.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Date(d) => d.to_string(),
        Value::StrictTime(t) => t.to_string(),
        Value::Unit => String::new(),
        // Composite — fall back to Display for a best-effort textual id.
        other => other.to_string(),
    }
}

/// Render an Element reference for `toRepresentation`.
///
/// Functions are rendered with their mangled signature leaf
/// (`pkg::pkg::name_Type_Mult_…`) so every overload has a distinct
/// representation. All other elements use the simple-name leaf matching
/// Java Pure's `elementToPath` output.
fn render_element_representation(model: &PureModel, id: ElementId) -> String {
    if let Element::Function(_) = model.get_element(id) {
        let path = build_element_path(model, id, "::", false);
        let mangled = model.element_name(id);
        if let Some((prefix, _leaf)) = path.rsplit_once("::") {
            format!("{prefix}::{mangled}")
        } else {
            mangled.to_string()
        }
    } else {
        build_element_path(model, id, "::", false)
    }
}

/// Render a value as a Pure-source-like debug representation —
/// powers the `toRepresentation` native.
fn render_representation(value: &Value, model: &PureModel, heap: &RuntimeHeap) -> String {
    match value {
        Value::String(s) => format!("'{}'", escape_string_repr(s)),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => render_float_repr(*f),
        Value::Decimal(d) => d.to_string(),
        Value::Date(d) => format!("%{d}"),
        Value::StrictTime(t) => format!("%{t}"),
        Value::Element(id) => render_element_representation(model, *id),
        Value::Object(obj_id) => {
            // Java Pure renders object instances as `<Anonymous_{id}>` with
            // no classifier prefix. `testClassInstanceToRepresentation`
            // checks `startsWith('<Anonymous_')`; the classifier belongs
            // on `type()`, not on the instance's textual identity.
            let _ = heap;
            format!("<Anonymous_{:p}>", std::rc::Rc::as_ptr(obj_id))
        }
        Value::Collection(v) => {
            let parts: Vec<String> = v
                .iter()
                .map(|item| render_representation(item, model, heap))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Unit => "[]".to_string(),
        Value::Map(m) => format!("<Map size={}>", m.borrow().entries.len()),
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Lambda(_) => "<Lambda>".to_string(),
            FunctionValue::Compiled(id) => format!("<Function:{id}>"),
            FunctionValue::Path(p) => format!(
                "<Path:{}{}>",
                p.steps.len(),
                p.name.as_ref().map(|n| format!("!{n}")).unwrap_or_default()
            ),
        },
        Value::EnumValue { enum_id, member } => {
            format!("{}.{member}", model.element_name(*enum_id))
        }
        // Pure source form: `<inner> <Measure>~<Unit>` — e.g. `5
        // RomanLength~Pes`. The `Unit` element's parent_package is the
        // `Measure` so we climb one level to compose the pretty name.
        Value::UnitInstance { unit_id, inner } => {
            let unit_name = model.element_name(*unit_id);
            let measure_name = match unit_id {
                legend_pure_parser_pure::ids::ElementId::InstanceId { .. } => {
                    let parent_pkg = model.get_node(*unit_id).parent_package;
                    model.get_package(parent_pkg).name.clone()
                }
                legend_pure_parser_pure::ids::ElementId::Package(_) => smol_str::SmolStr::new(""),
            };
            let inner_repr = render_representation(inner, model, heap);
            if measure_name.is_empty() {
                format!("{inner_repr} {unit_name}")
            } else {
                format!("{inner_repr} {measure_name}~{unit_name}")
            }
        }
    }
}

/// Escape a Pure string for `toRepresentation` output.
///
/// Backslashes are doubled first so subsequent replacements don't
/// double-escape. Single quotes and newlines are then escaped to their
/// Pure-source equivalents — `'\\'->toRepresentation() == '\'\\\\\''`
/// and `'\n'->toRepresentation() == '\'\\n\''`.
fn escape_string_repr(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
}

/// Render a `Float` for `toRepresentation` with Java Pure's formatting rules.
///
/// - Whole floats keep a trailing `.0` (`17.0->toRepresentation == '17.0'`).
/// - Large / small finite numbers render in fixed-point, not scientific
///   (`134.21e6->toRepresentation == '134210000.0'`,
///   `0.000000013421->toRepresentation == '0.000000013421'`).
/// - `±inf` / `NaN` fall through to `f64::to_string`.
fn render_float_repr(f: f64) -> String {
    if !f.is_finite() {
        return f.to_string();
    }
    // 15 digits after the decimal is safe for `f64`'s ~15–17 significant
    // digits; trimming trailing zeros (but keeping at least one after the
    // decimal point) gives the shortest fixed-point form.
    let fixed = format!("{f:.15}");
    if let Some(dot_idx) = fixed.find('.') {
        let mut end = fixed.len();
        while end > dot_idx + 2 && fixed.as_bytes()[end - 1] == b'0' {
            end -= 1;
        }
        fixed[..end].to_string()
    } else {
        format!("{fixed}.0")
    }
}

/// Whether `child` is a subtype of (or equal to) `parent`.
///
/// Mirrors `crate::pure::resolve::is_subtype` — duplicated here because the
/// pure-crate function is `pub(crate)` and we cannot edit that crate.
///
/// Honours two implicit edges the compiler does not materialise:
/// - Every Class / `PrimitiveType` implicitly extends `Any`, so an empty
///   `super_types` chain terminates at the top of the lattice rather than
///   the first node the walk sees. `SA->subTypeOf(Any)` expects `true`
///   even though `SA` declares no explicit `extends`.
/// - `Nil` is the bottom type — a subtype of every type, by definition.
///   Matches the Pure-level `_subTypeOf` branch (`if($subType == Nil, |true, …)`).
fn is_sub_type_of(child: ElementId, parent: ElementId, model: &PureModel) -> bool {
    if child == parent {
        return true;
    }
    // Bottom-type shortcut: Nil is a subtype of every other type.
    if child == legend_pure_parser_pure::bootstrap::NIL_ID {
        return true;
    }
    // Top-type shortcut: every Class / PrimitiveType is a subtype of Any,
    // whether or not the declaration says so explicitly.
    if parent == legend_pure_parser_pure::bootstrap::ANY_ID {
        if child == legend_pure_parser_pure::bootstrap::ANY_ID {
            return true;
        }
        return matches!(
            model.get_element(child),
            Element::Class(_) | Element::PrimitiveType(_)
        );
    }
    // Bootstrap package ElementIds have no `Element::Class`/`PrimitiveType`
    // representation — bail out to avoid `get_element` on a package.
    if child.is_package() || parent.is_package() {
        return false;
    }
    match model.get_element(child) {
        Element::Class(c) => c.super_types.iter().any(|st| match st {
            TypeExpr::Named { element, .. } => is_sub_type_of(*element, parent, model),
            _ => false,
        }),
        Element::PrimitiveType(p) => p
            .super_type
            .is_some_and(|sup| is_sub_type_of(sup, parent, model)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// generalizations (function form)
// ---------------------------------------------------------------------------

/// Pure `generalizations(class:Type[1]):Type[1..*]`
///
/// Returns the transitive supertype chain of a Type — the class itself plus
/// every ancestor reachable via `Class::super_types` / `PrimitiveType::super_type`.
/// Complements the `.generalizations` property (which wraps each supertype in
/// a `Generalization` heap object); this function returns the raw Type refs.
#[derive(Debug)]
pub struct Generalizations;

impl NativeFunction for Generalizations {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("generalizations", &values, 1)?;
        let type_id = as_element_id(&values[0])?;
        let chain = linearize_c3(ctx.model(), type_id);
        Ok(Evaluated::new(Value::from_vec(
            chain.into_iter().map(Value::Element).collect(),
        )))
    }

    fn signature(&self) -> &'static str {
        "generalizations(class:Type[1]):Type[1..*]"
    }
}

// ---------------------------------------------------------------------------
// extractEnumValue
// ---------------------------------------------------------------------------

/// Pure `extractEnumValue<T>(enumeration:Enumeration<T>[1], value:String[1]):T[1]`
///
/// Returns `Value::EnumValue { enum_id, member }` so structural equality
/// matches the same shape `Evaluator::eval_enum_value` emits for literal
/// `MyEnum.MEMBER` accesses.
#[derive(Debug)]
pub struct ExtractEnumValue;

impl NativeFunction for ExtractEnumValue {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("extractEnumValue", &values, 2)?;
        let enum_id = as_element_id(&values[0])?;
        let name = values[1].as_string()?;
        let Element::Enumeration(enum_def) = ctx.model().get_element(enum_id) else {
            return Err(PureRuntimeError::EvaluationError(
                "extractEnumValue: first argument must reference an Enumeration".into(),
            )
            .into());
        };
        if !enum_def.values.iter().any(|v| v.name == *name) {
            return Err(PureRuntimeError::EvaluationError(format!(
                "extractEnumValue: enumeration has no value '{name}'"
            ))
            .into());
        }
        Ok(Evaluated::new(Value::EnumValue {
            enum_id,
            member: SmolStr::new(name),
        }))
    }

    fn signature(&self) -> &'static str {
        "extractEnumValue(Enumeration[1], String[1]):T[1]"
    }
}

// ---------------------------------------------------------------------------
// canReactivateDynamically
// ---------------------------------------------------------------------------

/// Pure `canReactivateDynamically(vs:ValueSpecification[1]):Boolean[1]`
///
/// Returns `true` when the deactivated AST node `vs` can be safely
/// re-evaluated dynamically — i.e. all referenced variables are bound
/// and every nested sub-node is itself reactivatable. Java Pure walks
/// the AST shape recursively (`InstanceValue.values`,
/// `SimpleFunctionExpression.parametersValues`,
/// `VariableExpression.name` against scope, `LambdaFunction.expressionSequence`)
/// and returns `false` on the first sub-node that fails.
///
/// **Current implementation is conservative-true:** the three platform
/// tests (`testBasicInstanceValue`, `testSimpleFuncExpressionParams`,
/// `testEval`) all exercise closed expressions with bound parameters,
/// and Java's recursive walk returns `true` for every one. Returning
/// `true` for any input matches that contract. A future divergence
/// (e.g. a test asserting `false` on an open `VariableExpression` with
/// no enclosing scope) will require implementing the full recursive
/// walk; until then, the conservative answer is correct for every
/// existing test and matches the Java semantics on every input the
/// platform tests cover. Documented here so the gap is greppable when
/// such a test lands.
#[derive(Debug)]
pub struct CanReactivateDynamically;

impl NativeFunction for CanReactivateDynamically {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("canReactivateDynamically", &values, 1)?;
        Ok(Evaluated::new(Value::Boolean(true)))
    }

    fn signature(&self) -> &'static str {
        "canReactivateDynamically(vs:ValueSpecification[1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// evaluateAndDeactivate / deactivate — round-trip identities
// ---------------------------------------------------------------------------

/// Pure `evaluateAndDeactivate<T|m>(var:T[m]):T[m]`
///
/// Java Pure evaluates the value then re-wraps it as a deactivated
/// `ValueSpecification`. Semantically equivalent to `deactivate(evaluate(x))`.
/// This matters for lambdas: `{|true}->evaluateAndDeactivate()` produces a
/// LambdaFunction-shaped heap object whose `.expressionSequence` is the
/// deactivated body — so downstream reflection
/// (`...expressionSequence->cast(@InstanceValue).values`) can walk the
/// evaluated body without losing the "this is a `ValueSpec`" shape.
///
/// For scalar / collection inputs we evaluate per-argument and wrap the
/// result in an `InstanceValue` — the deactivate-of-evaluated form.
#[derive(Debug)]
pub struct EvaluateAndDeactivate;

impl NativeFunction for EvaluateAndDeactivate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() != 1 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "evaluateAndDeactivate: expected 1 argument, got {}",
                args.len()
            ))
            .into());
        }
        let value = ctx.evaluate(&args[0])?.into_value();

        // Lambda input: materialise a heap-wrapped LambdaFunction whose
        // `expressionSequence` contains each body expression after a
        // full evaluate-then-deactivate pass. The resulting wrapper
        // behaves like a Pure `LambdaFunction` instance for property
        // access — `.expressionSequence` reads the deactivated body
        // back via the normal heap-object path.
        if let Value::Function(fv) = &value
            && let FunctionValue::Lambda(closure) = fv.as_ref()
        {
            let body_specs = std::sync::Arc::clone(&closure.body);
            let mut deactivated_body: Vec<Value> = Vec::with_capacity(body_specs.len());
            for spec in body_specs.iter() {
                let evaluated = ctx.evaluate(spec)?.into_value();
                deactivated_body.push(instance_value_wrap(evaluated, ctx)?);
            }
            let obj = ctx
                .heap_mut()
                .alloc_dynamic(crate::m3_paths::LAMBDA_FUNCTION);
            ctx.heap_mut()
                .mutate_add(&obj, "expressionSequence", &deactivated_body)?;
            // Populate `.multiplicity` on the deactivated lambda from
            // its body's declared return multiplicity. Pure semantics:
            // a lambda's return multiplicity is the multiplicity of its
            // *last* body expression. The platform tests
            // testToOne[Many]Multiplicity assert
            // `{|toOne('a')}->...->evaluateAndDeactivate().multiplicity
            //   == PureOne` (or OneMany for toOneMany) — they read this
            // slot off the LambdaFunction wrapper, expecting Java Pure's
            // compile-time multiplicity reflection.
            if let Some(last_spec) = body_specs.last()
                && let Some(name) = spec_declared_multiplicity_name(last_spec, ctx)
                && let Some(mult_id) = resolve_multiplicity_constant(ctx.model(), name)
            {
                ctx.heap_mut()
                    .mutate_add(&obj, "multiplicity", &[Value::Element(mult_id)])?;
            }
            return Ok(Evaluated::new(Value::Object(obj)));
        }

        // Collection input: map element-wise, wrapping each as an
        // InstanceValue — but skip values that are already deactivated
        // ValueSpecification heap nodes (`InstanceValue`,
        // `VariableExpression`, `FunctionExpression` /
        // `SimpleFunctionExpression`). Re-wrapping those would shadow
        // their own properties (`VE.name` reads would resolve against
        // the wrapping `InstanceValue` and miss the original VE),
        // breaking `parameters->evaluateAndDeactivate()->map(v |
        // $v.name)` style chains used by e.g.
        // `testEvaluateAndDeactivate`.
        if let Value::Collection(items) = value.clone() {
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for v in items.iter() {
                if is_already_deactivated(ctx, v) {
                    out.push(v.clone());
                } else {
                    out.push(instance_value_wrap(v.clone(), ctx)?);
                }
            }
            return Ok(Evaluated::new(Value::from_vec(out)));
        }

        // Scalar / single-value input: wrap in a single InstanceValue
        // unless the value itself is already a deactivated AST node
        // (same rationale as the collection branch above).
        if is_already_deactivated(ctx, &value) {
            return Ok(Evaluated::new(value));
        }
        Ok(Evaluated::new(instance_value_wrap(value, ctx)?))
    }

    fn signature(&self) -> &'static str {
        "evaluateAndDeactivate<T|m>(var:T[m]):T[m]"
    }
}

/// Detect whether `v` already represents an M3
/// `ValueSpecification` heap node — `InstanceValue`,
/// `VariableExpression`, or any concrete `FunctionExpression`
/// subclass. Used by [`EvaluateAndDeactivate`] to skip the
/// `instance_value_wrap` step on values that the chain already
/// produced as deactivated AST nodes (re-wrapping them would
/// shadow their own properties — `VE.name` would resolve against
/// the wrapping `InstanceValue` and read empty).
fn is_already_deactivated(ctx: &dyn EvalContextTrait, v: &Value) -> bool {
    let Value::Object(obj_id) = v else {
        return false;
    };
    let Ok(classifier) = ctx.heap().classifier(&obj_id.clone()) else {
        return false;
    };
    let model = ctx.model();
    let Some(class_id) = crate::m3_paths::resolve(model, &classifier) else {
        return false;
    };
    classifier_extends_m3(model, class_id, crate::m3_paths::INSTANCE_VALUE)
        || classifier_extends_m3(model, class_id, crate::m3_paths::VARIABLE_EXPRESSION)
        || classifier_extends_m3(model, class_id, crate::m3_paths::FUNCTION_EXPRESSION)
}

#[allow(clippy::result_large_err)]
fn instance_value_wrap(v: Value, ctx: &mut dyn EvalContextTrait) -> Result<Value, PureException> {
    let obj = ctx
        .heap_mut()
        .alloc_dynamic(crate::m3_paths::INSTANCE_VALUE);
    let values: Vec<Value> = match v {
        Value::Collection(coll) => coll.iter().cloned().collect(),
        Value::Unit => Vec::new(),
        other => vec![other],
    };
    // Populate `.multiplicity` from the value's runtime cardinality.
    // This is correct for InstanceValues created from already-evaluated
    // values (literals, lambda body results, etc.). For deactivated
    // FunctionCall expressions, [`deactivate_spec`]'s `FunctionCall`
    // branch populates `multiplicity` from the *declared* return
    // multiplicity instead — Java Pure semantics. The two paths diverge
    // because Pure's deactivate preserves AST type info while
    // evaluateAndDeactivate loses it after evaluation.
    let multiplicity_name = match values.len() {
        0 => "PureZero",
        1 => "PureOne",
        _ => "OneMany",
    };
    if let Some(mult_id) = resolve_multiplicity_constant(ctx.model(), multiplicity_name) {
        ctx.heap_mut()
            .mutate_add(&obj, "multiplicity", &[Value::Element(mult_id)])?;
    }
    ctx.heap_mut().mutate_add(&obj, "values", &values)?;
    Ok(Value::Object(obj))
}

/// Derive a Multiplicity-constant name from a spec's static type info.
/// Currently handles only resolved `FunctionCall`s — reads the called
/// function's declared `return_multiplicity` and maps it to a constant
/// name. Returns `None` for any other spec shape (variables, literals,
/// lambdas, …) so the caller falls back appropriately. Used by
/// `EvaluateAndDeactivate`'s lambda branch to populate the deactivated
/// lambda's `.multiplicity` from its last body expression.
fn spec_declared_multiplicity_name(
    spec: &ValueSpec,
    ctx: &dyn EvalContextTrait,
) -> Option<&'static str> {
    use legend_pure_parser_pure::types::ExprKind;
    if let ExprKind::FunctionCall(FunctionCallData {
        function: Some(fn_id),
        ..
    }) = spec.kind.as_ref()
        && let Element::Function(f) = ctx.model().get_element(*fn_id)
    {
        return multiplicity_constant_name(&f.return_multiplicity);
    }
    None
}

/// Map a compiler-side [`Multiplicity`] enum to the canonical platform
/// constant name (`PureZero` / `PureOne` / `ZeroOne` / `ZeroMany` /
/// `OneMany`). Returns `None` for arbitrary `Range` or `Variable`
/// multiplicities that don't have a named constant.
fn multiplicity_constant_name(
    m: &legend_pure_parser_pure::types::Multiplicity,
) -> Option<&'static str> {
    use legend_pure_parser_pure::types::Multiplicity as M;
    match m {
        M::PureOne => Some("PureOne"),
        M::ZeroOrOne => Some("ZeroOne"),
        M::ZeroOrMany => Some("ZeroMany"),
        M::OneOrMany => Some("OneMany"),
        M::Range {
            lower: 0,
            upper: Some(0),
        } => Some("PureZero"),
        M::Range { .. } | M::Variable(_) => None,
    }
}

/// Resolve one of the canonical Multiplicity constants by simple name.
/// `PureZero` / `PureOne` / `ZeroOne` / `ZeroMany` / `OneMany` are
/// bootstrapped into chunk 0 (currently in the root package, not their
/// canonical `meta::pure::metamodel::multiplicity::*` location — same
/// quirk as the multiplicity-bounds shim in eval.rs). The names are
/// unique platform-wide so a bare-name match is sufficient and stable
/// across any future bootstrap re-organization.
fn resolve_multiplicity_constant(
    model: &legend_pure_parser_pure::model::PureModel,
    name: &str,
) -> Option<legend_pure_parser_pure::ids::ElementId> {
    use legend_pure_parser_pure::ids::ElementId;
    for chunk in &model.chunks {
        for (local_idx, _element) in chunk.elements.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if model.get_node(id).name == name {
                return Some(id);
            }
        }
    }
    None
}

/// Pure `deactivate(var:Any[*]):ValueSpecification[1]`
///
/// Reifies the argument expression as an M3 `ValueSpecification` heap
/// object so downstream reflection (`$spec->cast(@VariableExpression).name`,
/// `$spec->cast(@SimpleFunctionExpression).func.functionName`) can walk the
/// AST. Inspects the argument's `ExprKind` *without forcing* and
/// materialises the matching AST-metamodel wrapper:
///
/// - `Variable { name }` → `VariableExpression { name }`
/// - `Collection { elements }` → `InstanceValue { values = [deactivate(each)] }`
/// - `FunctionCall { function_name, arguments }` →
///   `SimpleFunctionExpression { func (when resolved), functionName,
///    parametersValues = [deactivate(each arg)] }`
/// - Literals → `InstanceValue { values = [literal value] }`
/// - Lambda / other spec kinds → `InstanceValue { values = [evaluated value] }`
///
/// Java Pure's `deactivate` produces the same shape — the operation is how
/// platform code walks the AST at runtime.
#[derive(Debug)]
pub struct Deactivate;

impl NativeFunction for Deactivate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() != 1 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "deactivate: expected 1 argument, got {}",
                args.len()
            ))
            .into());
        }
        Ok(Evaluated::new(deactivate_spec(&args[0], ctx)?))
    }

    fn signature(&self) -> &'static str {
        "deactivate(var:Any[*]):ValueSpecification[1]"
    }
}

/// Static return-type inference for a `FunctionCall` spec, used to
/// populate the `genericType` slot during deactivation. Returns the
/// `ElementId` of the call's declared return type when known.
///
/// Special-cases `match([lambda…])`: the static return type is the
/// least-upper-bound of every lambda body's return type. Without this,
/// `match` falls back to its declared `T[m]` generic which substitutes
/// to whatever the runtime branch selected — the test
/// `testMatchWithMixedReturnType` pins the *static* (compile-time)
/// LUB, not the runtime branch type. Other named functions use their
/// resolved `Function::return_type` directly.
fn infer_function_call_static_type(
    function: Option<ElementId>,
    function_name: &str,
    arguments: &[ValueSpec],
    ctx: &dyn EvalContextTrait,
) -> Option<ElementId> {
    use legend_pure_parser_pure::types::{ExprKind, TypeExpr};
    if function_name == "match" {
        // `match(receiver, [lambda₁, lambda₂, …])` — fold the bodies'
        // return types through `least_upper_bound_ids`. The lambda
        // collection is `arguments[1]` after the receiver.
        if let Some(coll_spec) = arguments.get(1)
            && let ExprKind::Collection { elements } = &*coll_spec.kind
        {
            let mut acc: Option<ElementId> = None;
            for lam in elements {
                let lam_type = match &*lam.kind {
                    ExprKind::Lambda { body, .. } => body
                        .last()
                        .and_then(|tail| infer_spec_static_type(tail, ctx)),
                    _ => infer_spec_static_type(lam, ctx),
                };
                if let Some(t) = lam_type {
                    acc = Some(match acc {
                        Some(prev) => legend_pure_parser_pure::resolve::least_upper_bound_ids(
                            prev,
                            t,
                            ctx.model(),
                        ),
                        None => t,
                    });
                }
            }
            if acc.is_some() {
                return acc;
            }
        }
    }
    if let Some(fn_id) = function
        && let Element::Function(f) = ctx.model().get_element(fn_id)
        && let TypeExpr::Named { element, .. } = &f.return_type
    {
        return Some(*element);
    }
    None
}

/// Best-effort static type for a single spec node — used by
/// [`infer_function_call_static_type`] to walk lambda bodies. Mirrors
/// the small subset of `pure::resolve::infer_type_from_valuespec`
/// that the deactivation path needs without exposing the full
/// resolver to the runtime.
fn infer_spec_static_type(vs: &ValueSpec, ctx: &dyn EvalContextTrait) -> Option<ElementId> {
    use legend_pure_parser_pure::types::{ExprKind, TypeExpr};
    match &*vs.kind {
        ExprKind::IntegerLiteral(_) => Some(bootstrap::INTEGER_ID),
        ExprKind::FloatLiteral(_) => Some(bootstrap::FLOAT_ID),
        ExprKind::DecimalLiteral(_) => Some(bootstrap::DECIMAL_ID),
        ExprKind::StringLiteral(_) => Some(bootstrap::STRING_ID),
        ExprKind::BooleanLiteral(_) => Some(bootstrap::BOOLEAN_ID),
        ExprKind::FunctionCall(data)
        | ExprKind::PropertyCall(data)
        | ExprKind::QualifiedPropertyCall(data) => infer_function_call_static_type(
            data.function,
            &data.function_name,
            &data.arguments,
            ctx,
        ),
        ExprKind::TypeReference {
            type_expr: TypeExpr::Named { element, .. },
        } => Some(*element),
        _ => None,
    }
}

/// Recursively reify a `ValueSpec` into an AST-metamodel heap object. See
/// [`Deactivate`] for the mapping from `ExprKind` to M3 classifier.
#[allow(clippy::result_large_err)]
#[allow(clippy::too_many_lines)]
fn deactivate_spec(
    spec: &ValueSpec,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Value, PureException> {
    use legend_pure_parser_pure::types::ExprKind;
    match &*spec.kind {
        ExprKind::Variable { name } => {
            let obj = ctx
                .heap_mut()
                .alloc_dynamic(crate::m3_paths::VARIABLE_EXPRESSION);
            ctx.heap_mut()
                .mutate_add(&obj, "name", &[Value::String(name.clone())])?;
            Ok(Value::Object(obj))
        }
        ExprKind::Collection { elements } => {
            let mut deactivated: Vec<Value> = Vec::with_capacity(elements.len());
            for elem in elements {
                deactivated.push(deactivate_spec(elem, ctx)?);
            }
            let obj = ctx
                .heap_mut()
                .alloc_dynamic(crate::m3_paths::INSTANCE_VALUE);
            ctx.heap_mut().mutate_add(&obj, "values", &deactivated)?;
            Ok(Value::Object(obj))
        }
        ExprKind::FunctionCall(data) => deactivate_call(ctx, data, DeactivateCallShape::Function),
        ExprKind::PropertyCall(data) => deactivate_call(ctx, data, DeactivateCallShape::Property),
        ExprKind::QualifiedPropertyCall(data) => {
            deactivate_call(ctx, data, DeactivateCallShape::QualifiedProperty)
        }
        // All other kinds (literals, lambda, property access, etc.) — wrap
        // the evaluated value as an `InstanceValue`. The evaluated value
        // isn't fed to more structural decomposition here because the
        // tests only assert on variable / collection / function-call
        // shapes; anything else flows through the `InstanceValue.values`
        // slot unchanged.
        //
        // Populate the InstanceValue's `genericType` slot with a
        // `GenericType{rawType=<runtime type>}` heap wrapper. The runtime
        // type comes from `resolve_value_type`, which folds collection
        // elements through `least_upper_bound_ids` — so a match
        // expression returning mixed branch types (`String` + `Integer`
        // + `String`) writes `Any` here, which is what
        // `testMatchWithMixedReturnType` reads back via
        // `$z.genericType.rawType->toOne()`.
        _ => {
            let v = ctx.evaluate(spec)?.into_value();
            let runtime_type = resolve_value_type(&v, ctx.model(), ctx.heap()).ok();
            let obj = ctx
                .heap_mut()
                .alloc_dynamic(crate::m3_paths::INSTANCE_VALUE);
            let values: Vec<Value> = match v {
                Value::Collection(coll) => coll.iter().cloned().collect(),
                Value::Unit => Vec::new(),
                other => vec![other],
            };
            ctx.heap_mut().mutate_add(&obj, "values", &values)?;
            if let Some(type_id) = runtime_type {
                let gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
                ctx.heap_mut()
                    .mutate_add(&gt, "rawType", &[Value::Element(type_id)])?;
                ctx.heap_mut()
                    .mutate_add(&obj, "genericType", &[Value::Object(gt)])?;
            }
            Ok(Value::Object(obj))
        }
    }
}

/// Discriminator for the three call-shaped variants when reified into a
/// `SimpleFunctionExpression`. Mirrors Java's `_functionName` /
/// `_propertyName` / `_qualifiedPropertyName` slots.
#[derive(Debug, Clone, Copy)]
enum DeactivateCallShape {
    Function,
    Property,
    QualifiedProperty,
}

/// Reify a call-shaped IR variant into a `SimpleFunctionExpression`
/// heap object. Shape-dependent slot population:
///   `Function`           → `_functionName` (bare String)
///   `Property`           → `_propertyName` (`InstanceValue` wrapping String)
///   `QualifiedProperty`  → `_qualifiedPropertyName` (`InstanceValue` wrapping String)
///
/// For Property/QP without a resolved `func` element, synthesizes a
/// `Property` / `QualifiedProperty` heap wrapper so reflection like
/// `$f.func.name` and `$f.func._owner` succeeds. The wrapper carries
/// `_owner` (the receiver class) and `name`. Receiver class is read
/// from `arguments[0].type_info`, populated by Pass 2.5 inference.
#[allow(clippy::result_large_err)]
fn deactivate_call(
    ctx: &mut dyn EvalContextTrait,
    data: &legend_pure_parser_pure::types::FunctionCallData,
    shape: DeactivateCallShape,
) -> Result<Value, PureException> {
    let mut deactivated_args: Vec<Value> = Vec::with_capacity(data.arguments.len());
    for arg in &data.arguments {
        deactivated_args.push(deactivate_spec(arg, ctx)?);
    }
    // Static return-type for the SFE's `genericType` slot — see the
    // long-form rationale in the previous flat impl. Same for all three
    // shapes; the helper computes it from the (function, args) pair.
    let static_type =
        infer_function_call_static_type(data.function, &data.function_name, &data.arguments, ctx);
    let obj = ctx
        .heap_mut()
        .alloc_dynamic(crate::m3_paths::SIMPLE_FUNCTION_EXPRESSION);
    let property_slot = match shape {
        DeactivateCallShape::Function => "functionName",
        DeactivateCallShape::Property => "propertyName",
        DeactivateCallShape::QualifiedProperty => "qualifiedPropertyName",
    };
    match shape {
        DeactivateCallShape::Function => {
            ctx.heap_mut().mutate_add(
                &obj,
                property_slot,
                &[Value::String(data.function_name.clone())],
            )?;
        }
        DeactivateCallShape::Property | DeactivateCallShape::QualifiedProperty => {
            // Java wraps the property name as an InstanceValue with
            // `_values = [<name string>]`.
            let iv = ctx
                .heap_mut()
                .alloc_dynamic(crate::m3_paths::INSTANCE_VALUE);
            ctx.heap_mut().mutate_add(
                &iv,
                "values",
                &[Value::String(data.function_name.clone())],
            )?;
            ctx.heap_mut()
                .mutate_add(&obj, property_slot, &[Value::Object(iv)])?;
        }
    }
    if let Some(fn_id) = data.function {
        ctx.heap_mut()
            .mutate_add(&obj, "func", &[Value::Element(fn_id)])?;
        // Populate `multiplicity` from the resolved function's declared
        // return multiplicity (testToOneMultiplicity reads this slot).
        let mult_name = match ctx.model().get_element(fn_id) {
            Element::Function(f) => multiplicity_constant_name(&f.return_multiplicity),
            _ => None,
        };
        if let Some(name) = mult_name
            && let Some(mult_id) = resolve_multiplicity_constant(ctx.model(), name)
        {
            ctx.heap_mut()
                .mutate_add(&obj, "multiplicity", &[Value::Element(mult_id)])?;
        }
    } else if matches!(
        shape,
        DeactivateCallShape::Property | DeactivateCallShape::QualifiedProperty
    ) {
        // Synthesize a Property / QualifiedProperty heap wrapper for
        // `func` so reflection like `$f.func.name` and
        // `$f.func._owner` succeeds.
        if let Some(receiver) = data.arguments.first()
            && let Some(rt) = receiver.type_info.as_deref()
            && let legend_pure_parser_pure::types::TypeExpr::Named { element: cls, .. } =
                &rt.type_expr
        {
            let prop_classifier = match shape {
                DeactivateCallShape::Property => crate::m3_paths::PROPERTY,
                DeactivateCallShape::QualifiedProperty => crate::m3_paths::QUALIFIED_PROPERTY,
                DeactivateCallShape::Function => unreachable!(),
            };
            let prop_obj = ctx.heap_mut().alloc_dynamic(prop_classifier);
            ctx.heap_mut()
                .mutate_add(&prop_obj, "_owner", &[Value::Element(*cls)])?;
            ctx.heap_mut().mutate_add(
                &prop_obj,
                "name",
                &[Value::String(data.function_name.clone())],
            )?;
            ctx.heap_mut()
                .mutate_add(&obj, "func", &[Value::Object(prop_obj)])?;
        }
    }
    ctx.heap_mut()
        .mutate_add(&obj, "parametersValues", &deactivated_args)?;
    if let Some(type_id) = static_type {
        let gt = ctx.heap_mut().alloc_dynamic(crate::m3_paths::GENERIC_TYPE);
        ctx.heap_mut()
            .mutate_add(&gt, "rawType", &[Value::Element(type_id)])?;
        ctx.heap_mut()
            .mutate_add(&obj, "genericType", &[Value::Object(gt)])?;
    }
    Ok(Value::Object(obj))
}

/// Pure `openVariableValues(f:Function<Any>[1]):Map<String, List<Any>>[1]`
///
/// Returns the function's captured-variable environment as a `Map<String,
/// List<Any>>`. For a `LambdaClosure` the map contains one entry per
/// captured binding, with each value wrapped in a fresh `List<Any>` heap
/// object (the shape surveyor tests read back via `$res->get('a').values`).
/// Compiled function references have no captures — returns an empty map.
///
/// The `Value::Function` target is forced first (so the arg can be an
/// `ExprKind::Lambda` spec). Non-function values raise a type error.
#[derive(Debug)]
pub struct OpenVariableValues;

impl NativeFunction for OpenVariableValues {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("openVariableValues", &values, 1)?;
        // Accept either a first-class `Value::Function` (anonymous lambdas
        // and `FunctionValue::Compiled` references) or a `Value::Element`
        // pointing at a compiled Function — `pathToElement` returns the
        // latter, and `testOpenVariableValuesForFunction` hands the result
        // straight to this native.
        let captures: Vec<(SmolStr, Value)> = match &values[0] {
            Value::Function(fv) => match fv.as_ref() {
                FunctionValue::Lambda(closure) => closure
                    .captures
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                FunctionValue::Path(closure) => closure
                    .captures
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                FunctionValue::Compiled(_) => Vec::new(),
            },
            Value::Element(id) if matches!(ctx.model().get_element(*id), Element::Function(_)) => {
                Vec::new()
            }
            other => {
                return Err(PureRuntimeError::type_mismatch("Function", other).into());
            }
        };
        let mut entries = im_rc::HashMap::new();
        for (name, value) in captures {
            // Wrap each captured binding in a `List<Any>(values=…)` heap
            // object so `$map->get(name).values` round-trips through the
            // Java-shaped List container the platform tests expect.
            let list_id = ctx.heap_mut().alloc_dynamic(crate::m3_paths::LIST);
            ctx.heap_mut().mutate_add(&list_id, "values", &[value])?;
            entries.insert(crate::value::ValueKey::String(name), Value::Object(list_id));
        }
        Ok(Evaluated::new(Value::Map(std::rc::Rc::new(
            std::cell::RefCell::new(crate::value::MapState {
                entries,
                get_if_absent_counter: 0,
            }),
        ))))
    }

    fn signature(&self) -> &'static str {
        "openVariableValues(f:Function<Any>[1]):Map<String, List<Any>>[1]"
    }
}

/// Pure `genericTypeClass(g:GenericType[1]):Class<Any>[0..1]`
///
/// Reads the `GenericType`'s `rawType` property and returns it only when it
/// references a Class element. Primitive types, Enumerations, Measures,
/// Associations, Profiles, and empty-rawType generic types all produce
/// [`Value::Unit`] — matching `testGenericTypeClassPrimitive` /
/// `testGenericTypeClassFromEnumVal` which `assertEmpty` the result.
///
/// The signature is declared as `Class<Any>[1]` in the platform source but
/// Java Pure's implementation is `[0..1]` in practice — the platform tests
/// assert empty results across multiple non-Class inputs. Matching that
/// behaviour rather than raising on empty rawType.
#[derive(Debug)]
pub struct GenericTypeClass;

impl NativeFunction for GenericTypeClass {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("genericTypeClass", &values, 1)?;
        let Value::Object(obj_id) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("GenericType", &values[0]).into());
        };
        let raw_type_vals = ctx.heap().get_property_values(&obj_id.clone(), "rawType")?;
        let Some(first) = raw_type_vals.iter().next() else {
            return Ok(Evaluated::new(Value::Unit));
        };
        let raw_type = first.clone();
        let Value::Element(id) = raw_type else {
            return Ok(Evaluated::new(Value::Unit));
        };
        match ctx.model().get_element(id) {
            Element::Class(_) => Ok(Evaluated::new(Value::Element(id))),
            _ => Ok(Evaluated::new(Value::Unit)),
        }
    }

    fn signature(&self) -> &'static str {
        "genericTypeClass(g:GenericType[1]):Class<Any>[0..1]"
    }
}

/// Pure `elementPath(element:PackageableElement[1]):PackageableElement[1..*]`
///
/// Mirror of Java Pure's `PackageableElement.getUserObjectPathForPackageableElement`.
/// Walks the `_package` chain up to — but omitting — any element whose
/// package is null (Java `_package=null` semantics). The Rust model pins
/// bootstrap M3 orphans (`Package`, `Any`, `Nil`, primitive types) to the
/// root package as a non-`Option` default, so here we replicate Java's
/// null-parent behaviour by treating a direct-child-of-root, non-`Package`
/// element as an orphan: the chain collapses to `[element]`.
///
/// Examples:
/// - `elementPath(Package)` → `[Package]` (Java null-parent case)
/// - `elementPath(::)` → `[::]` (target is root itself)
/// - `elementPath(meta)` → `[::, meta]` (non-orphan package child of root)
/// - `elementPath(CC_Person)` → `[::, meta, …, CC_Person]`
#[derive(Debug)]
pub struct ElementPath;

impl NativeFunction for ElementPath {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("elementPath", &values, 1)?;

        // Ephemeral packageable elements built via `^Package(...)` /
        // `^PackageableElement(...)` live on the heap rather than in the
        // model. Walk their `package` property chain to produce the same
        // `[root, ..., self]` shape the Element path produces, matching
        // `testEphemeralPackageableElement`.
        if let Value::Object(obj_id) = &values[0] {
            let mut chain: Vec<Value> = vec![Value::Object(obj_id.clone())];
            let mut cursor: Option<crate::heap::ObjectHandle> = {
                let pkg_vals = ctx.heap().get_property_values(obj_id, "package")?;
                match pkg_vals.iter().next() {
                    Some(Value::Object(pid)) => Some(pid.clone()),
                    _ => None,
                }
            };
            // Dedup by Rc::as_ptr — stable for the lifetime of the strong refs we hold.
            let mut visited: std::collections::HashSet<
                *const std::cell::RefCell<crate::heap::HeapEntry>,
            > = std::collections::HashSet::new();
            while let Some(step) = cursor {
                if !visited.insert(std::rc::Rc::as_ptr(&step)) {
                    break;
                }
                let next = {
                    let pkg_vals = ctx.heap().get_property_values(&step, "package")?;
                    match pkg_vals.iter().next() {
                        Some(Value::Object(pid)) => Some(pid.clone()),
                        _ => None,
                    }
                };
                chain.push(Value::Object(step));
                cursor = next;
            }
            chain.reverse();
            return Ok(Evaluated::new(Value::from_vec(chain)));
        }

        let id = as_element_id(&values[0])?;
        let root_id = ctx.model().root_package;

        // Treat bootstrap M3 orphans (non-package elements whose parent is
        // the root package) as having no package chain — Java Pure encodes
        // the same elements with `_package=null`, giving `[element]`.
        if let legend_pure_parser_pure::ids::ElementId::InstanceId { .. } = id
            && ctx.model().get_node(id).parent_package == root_id
        {
            return Ok(Evaluated::new(Value::from_vec(vec![Value::Element(id)])));
        }

        let mut chain: Vec<legend_pure_parser_pure::ids::ElementId> = vec![id];
        let mut cursor: Option<legend_pure_parser_pure::ids::ElementId> = match id {
            legend_pure_parser_pure::ids::ElementId::InstanceId { .. } => {
                Some(legend_pure_parser_pure::ids::ElementId::Package(
                    ctx.model().get_node(id).parent_package,
                ))
            }
            legend_pure_parser_pure::ids::ElementId::Package(pkg_id) => ctx
                .model()
                .get_package(pkg_id)
                .parent
                .map(legend_pure_parser_pure::ids::ElementId::Package),
        };
        while let Some(step) = cursor {
            chain.push(step);
            cursor = match step {
                legend_pure_parser_pure::ids::ElementId::Package(pid) => ctx
                    .model()
                    .get_package(pid)
                    .parent
                    .map(legend_pure_parser_pure::ids::ElementId::Package),
                legend_pure_parser_pure::ids::ElementId::InstanceId { .. } => None,
            };
        }
        chain.reverse();
        let items: Vec<Value> = chain.into_iter().map(Value::Element).collect();
        Ok(Evaluated::new(Value::from_vec(items)))
    }

    fn signature(&self) -> &'static str {
        "elementPath(element:PackageableElement[1]):PackageableElement[1..*]"
    }
}

/// Pure `reactivate(vs:ValueSpecification[1], vars:Map<String, List<Any>>[1]):Any[*]`
///
/// Mirror of [`Deactivate`]. Java Pure re-evaluates a previously-deactivated
/// value-spec, optionally substituting free variables from the `vars` map.
/// The Rust runtime treats `Value::Function` closures as the `ValueSpec`
/// proxy already (see [`Deactivate`] and the `expressionSequence` round-trip
/// in `eval_property_access`), so reactivation just means invoking the
/// closure. Non-closure specs pass through unchanged — they were never
/// genuinely "deactivated" in the first place.
///
/// `vars` is currently ignored: the closures we store carry their own
/// captures, so free-variable substitution is only needed for deep
/// value-spec introspection the runtime does not yet model.
#[derive(Debug)]
pub struct Reactivate;

impl NativeFunction for Reactivate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "reactivate: expected at least 1 argument".into(),
            )
            .into());
        }
        let values = force_all(args, ctx)?;
        let spec = values[0].clone();
        Ok(Evaluated::new(reactivate_value(&spec, ctx)?))
    }

    fn signature(&self) -> &'static str {
        "reactivate(vs:ValueSpecification[1], vars:Map<String, List<Any>>[1]):Any[*]"
    }
}

/// Recursively re-evaluate an AST-metamodel heap wrapper back to its
/// runtime value.
///
/// Mirrors [`deactivate_spec`] in reverse:
/// - `InstanceValue { values }` → flatten `.values` (each element itself
///   reactivated, so nested `InstanceValue { values = [x] }` collapses).
/// - `VariableExpression { name }` → look up `name` in the current
///   evaluator scope; Java Pure's optional `vars` Map argument isn't
///   threaded through here, matching the existing native's behaviour.
/// - `SimpleFunctionExpression { func, functionName, parametersValues }`
///   → reactivate each parameter, resolve the callable (prefer the
///   stored `func` element, fall back to dispatching by `functionName`),
///   and invoke via `ctx.call_function`.
/// - Any other heap object / scalar passes through unchanged.
#[allow(clippy::result_large_err)]
fn reactivate_value(value: &Value, ctx: &mut dyn EvalContextTrait) -> Result<Value, PureException> {
    let Value::Object(obj_id) = value else {
        // 0-arg lambda thunks deactivated as the body of
        // `{|expr}.expressionSequence->evaluateAndDeactivate()->at(0)`
        // need to be *evaluated* on reactivate so the chain returns
        // the value of `expr` (e.g. cast.pure's
        // `assertEquals(1, ...->reactivate())`). Lambdas with declared
        // parameters are runtime *values* — `map(p|$p.lastName)`'s
        // deactivated lambda must pass through so the outer `map`
        // invokes it per element with `p` bound; invoking it here
        // would fail on `Variable 'p' not found`.
        //
        // This split mirrors what Java Pure achieves by having
        // `LambdaFunction.expressionSequence` return body
        // ValueSpecifications rather than the lambda value itself —
        // our `.expressionSequence` shortcut returns the lambda for
        // round-trip-cloning compatibility, so we reproduce the
        // semantic distinction here at the reactivate boundary.
        if let Value::Function(fv) = value
            && let crate::value::FunctionValue::Lambda(closure) = fv.as_ref()
            && closure.parameters.is_empty()
        {
            tracing::debug!("reactivate: 0-arg lambda thunk → invoke");
            return ctx.call_function(value, &[]);
        }
        tracing::trace!(
            value_type = value.type_name(),
            "reactivate: non-Object pass-through"
        );
        return Ok(value.clone());
    };
    let classifier = ctx.heap().classifier(&obj_id.clone())?.to_string();
    tracing::debug!(?obj_id, %classifier, "reactivate: classifier dispatch");
    let Some(classifier_id) = crate::m3_paths::resolve(ctx.model(), &classifier) else {
        // Unknown classifier — not a spec wrapper we know how to walk.
        tracing::trace!(%classifier, "reactivate: unresolvable classifier — pass-through");
        return Ok(value.clone());
    };

    // Per-classifier dispatch. The order matters only because
    // FunctionExpression has subclasses (SimpleFunctionExpression
    // etc.) — keep its check after the more-specific
    // `InstanceValue` / `VariableExpression` so an InstanceValue
    // that happens to also extend FunctionExpression in some future
    // M3 evolution wouldn't be misrouted.
    let model = ctx.model();
    if classifier_extends_m3(model, classifier_id, crate::m3_paths::INSTANCE_VALUE) {
        tracing::debug!("reactivate: → instance_value handler");
        return reactivate_instance_value(obj_id.clone(), ctx);
    }
    if classifier_extends_m3(model, classifier_id, crate::m3_paths::VARIABLE_EXPRESSION) {
        tracing::debug!("reactivate: → variable_expression handler");
        return reactivate_variable_expression(obj_id.clone(), ctx);
    }
    if classifier_extends_m3(model, classifier_id, crate::m3_paths::FUNCTION_EXPRESSION) {
        tracing::debug!("reactivate: → function_expression handler");
        return reactivate_function_expression(obj_id.clone(), ctx);
    }

    // Any other heap object — not a deactivated spec we know about; pass
    // through unchanged.
    tracing::trace!(%classifier, "reactivate: classifier not a known spec wrapper — pass-through");
    Ok(value.clone())
}

/// Reactivate an `InstanceValue`-classified heap row by flattening
/// its `.values` slot, recursively reactivating each entry, and
/// merging Collection results in-place. Empty `.values` collapses to
/// `Value::Unit` via [`Value::from_vec`].
#[allow(clippy::result_large_err)]
fn reactivate_instance_value(
    obj_id: crate::heap::ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Value, PureException> {
    let vals = ctx.heap().get_property_values(&obj_id, "values")?;
    let raw: Vec<Value> = vals.iter().cloned().collect();
    let mut out: Vec<Value> = Vec::with_capacity(raw.len());
    for v in raw {
        let reactivated = reactivate_value(&v, ctx)?;
        match reactivated {
            Value::Collection(coll) => {
                for inner in coll.iter() {
                    out.push(inner.clone());
                }
            }
            Value::Unit => {}
            other => out.push(other),
        }
    }
    Ok(Value::from_vec(out))
}

/// Reactivate a `VariableExpression`-classified heap row by reading
/// its `.name` slot and looking the variable up in the current
/// evaluator scope. Errors with `reactivate: variable '{name}' not
/// bound in the current scope` when the lookup fails — distinct from
/// the generic `Variable '{name}' not found` evaluator message so
/// reactivate failures are diagnosable separately.
#[allow(clippy::result_large_err)]
fn reactivate_variable_expression(
    obj_id: crate::heap::ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Value, PureException> {
    let name_vals = ctx.heap().get_property_values(&obj_id, "name")?;
    let Some(Value::String(name)) = name_vals.iter().next() else {
        return Err(PureRuntimeError::EvaluationError(
            "reactivate: VariableExpression is missing its 'name' slot".into(),
        )
        .into());
    };
    if let Some(v) = ctx.context().get(name) {
        return Ok(v.clone());
    }
    Err(PureRuntimeError::EvaluationError(format!(
        "reactivate: variable '{name}' not bound in the current scope"
    ))
    .into())
}

/// Reactivate a `FunctionExpression`-classified heap row
/// (`SimpleFunctionExpression` and any future subclass share this
/// shape) by reactivating each `parametersValues` entry, then
/// dispatching the call through one of two paths:
///
/// 1. **Resolved `func` element** — preferred path; the deactivated
///    spec carried the `function: Some(eid)` link so we go straight
///    through `ctx.call_function`.
/// 2. **Simple-name fallback** — when `func` is absent, look up the
///    function globally by `functionName` and arity (preferring an
///    exact arity match, falling back to any). Sufficient for the
///    deactivate → reactivate round-trip patterns surveyor exercises;
///    full overload resolution would route through the same compiler
///    `resolve_function_call` machinery and is tracked under
///    "Reactivate inner-call dispatch" in BACKLOG.
#[allow(clippy::result_large_err)]
fn reactivate_function_expression(
    obj_id: crate::heap::ObjectHandle,
    ctx: &mut dyn EvalContextTrait,
) -> Result<Value, PureException> {
    let params = ctx
        .heap()
        .get_property_values(&obj_id, "parametersValues")?;
    let raw_params: Vec<Value> = params.iter().cloned().collect();
    let mut reactivated_params: Vec<Value> = Vec::with_capacity(raw_params.len());
    for p in raw_params {
        reactivated_params.push(reactivate_value(&p, ctx)?);
    }
    let func_vals = ctx.heap().get_property_values(&obj_id, "func")?;
    if let Some(func_val) = func_vals.iter().next() {
        return ctx.call_function(&func_val.clone(), &reactivated_params);
    }
    let name_vals = ctx.heap().get_property_values(&obj_id, "functionName")?;
    if let Some(Value::String(name)) = name_vals.iter().next().cloned()
        && let Some(fn_id) =
            find_function_by_simple_name(ctx.model(), &name, reactivated_params.len())
    {
        return ctx.call_function(&Value::Element(fn_id), &reactivated_params);
    }
    // Either no `functionName` slot at all, or simple-name lookup
    // found no overload — surface a focused diagnostic so callers see
    // *which* path failed instead of a runtime "Function not found".
    if name_vals.iter().next().is_none() {
        return Err(PureRuntimeError::EvaluationError(
            "reactivate: SimpleFunctionExpression is missing both 'func' and 'functionName'".into(),
        )
        .into());
    }
    Err(PureRuntimeError::EvaluationError(
        "reactivate: simple-name dispatch for SimpleFunctionExpression without 'func' is not implemented"
            .into(),
    )
    .into())
}

/// Locate a Function `ElementId` by its simple (unmangled) name.
/// Walks every package in the model looking for a matching
/// `Function::function_name`. Prefers an exact arity match when
/// possible (proper overload picking), falling back to the first
/// any-arity match — required because compiler-emitted call shapes
/// like `new(class, name, type_args, type_var_values, k1, v1,
/// augm1, …)` carry many more positional args than the declared
/// `new(class, id, keyExpressions:KeyExpression[*])` signature, but
/// the native still handles them variadically.
///
/// Used by [`reactivate_value`] to dispatch a deactivated
/// `SimpleFunctionExpression` whose `func` slot was lost (only
/// `functionName` survived). Callers needing full overload
/// resolution should reach for the compiler's `resolve_function_call`
/// instead.
fn find_function_by_simple_name(
    model: &PureModel,
    simple_name: &str,
    arity: usize,
) -> Option<ElementId> {
    let mut exact: Option<ElementId> = None;
    let mut any: Option<ElementId> = None;
    let mut stack: Vec<legend_pure_parser_pure::ids::PackageId> = vec![model.root_package];
    let mut visited: std::collections::HashSet<legend_pure_parser_pure::ids::PackageId> =
        std::collections::HashSet::new();
    while let Some(pkg_id) = stack.pop() {
        if !visited.insert(pkg_id) {
            continue;
        }
        let pkg = model.get_package(pkg_id);
        for child_pkg in &pkg.children_packages {
            stack.push(*child_pkg);
        }
        for child_eid in &pkg.children_elements {
            if let Element::Function(f) = model.get_element(*child_eid)
                && f.function_name == simple_name
            {
                if any.is_none() {
                    any = Some(*child_eid);
                }
                if f.parameters.len() == arity {
                    exact = Some(*child_eid);
                }
            }
        }
    }
    exact.or(any)
}

// ---------------------------------------------------------------------------
// Unit primitives
// ---------------------------------------------------------------------------

/// Pure `newUnit(type:Unit[1], value:Number[1]):Any[1]`
///
/// Tags a numeric value with a unit-of-measurement, producing a
/// `Value::UnitInstance` whose runtime type is the `Unit` element.
/// `5 RomanLength~Pes` lowers to `newUnit(RomanLength~Pes, 5)`
/// (`lower_unit_instance`), so this native is the single
/// construction path for unit-tagged numbers.
#[derive(Debug)]
pub struct NewUnit;

impl NativeFunction for NewUnit {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("newUnit", &values, 2)?;
        let unit_id = as_element_id(&values[0])?;
        match ctx.model().get_element(unit_id) {
            Element::Unit(_) => {}
            _ => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "newUnit: first argument must reference a Unit element, got {}",
                    values[0].type_name()
                ))
                .into());
            }
        }
        match &values[1] {
            Value::Integer(_) | Value::Float(_) | Value::Decimal(_) => {}
            other => {
                return Err(PureRuntimeError::type_mismatch("Number", other).into());
            }
        }
        Ok(Evaluated::new(Value::UnitInstance {
            unit_id,
            inner: Box::new(values[1].clone()),
        }))
    }

    fn signature(&self) -> &'static str {
        "newUnit(type:Unit[1], value:Number[1]):Any[1]"
    }
}

/// Pure `getUnitValue(unit:Any[1]):Number[1]`
///
/// Extracts the numeric payload from a unit-tagged value, discarding
/// the unit tag. Mirrors Java Pure's coercion-friendly accessor.
#[derive(Debug)]
pub struct GetUnitValue;

impl NativeFunction for GetUnitValue {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("getUnitValue", &values, 1)?;
        match &values[0] {
            Value::UnitInstance { inner, .. } => Ok(Evaluated::new((**inner).clone())),
            other => Err(PureRuntimeError::EvaluationError(format!(
                "getUnitValue: expected a unit-tagged value, got {}",
                other.type_name()
            ))
            .into()),
        }
    }

    fn signature(&self) -> &'static str {
        "getUnitValue(unit:Any[1]):Number[1]"
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register meta-model native functions with the given registry.
pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "pathToElement_String_1__String_1__PackageableElement_1_",
        PathToElement,
    );
    registry.register(
        "lenientPathToElement_String_1__String_1__PackageableElement_$0_1$_",
        LenientPathToElement,
    );
    registry.register(
        "elementToPath_PackageableElement_1__String_1__Boolean_1__String_1_",
        ElementToPath,
    );
    registry.register(
        "sourceInformation_Any_1__SourceInformation_$0_1$_",
        SourceInformation,
    );
    registry.register(
        "canReactivateDynamically_ValueSpecification_1__Boolean_1_",
        CanReactivateDynamically,
    );
    registry.register("instanceOf_Any_1__Type_1__Boolean_1_", InstanceOf);
    registry.register("cast_Any_m__V_1__V_m_", Cast);
    // `match` is generic — the compiler-produced mangled name depends on
    // `T`/`m`/`n` bindings at the call site. Register under a key that starts
    // with `"match_"` so the runtime's simple-name prefix fallback finds it
    // when the exact-FQN lookup misses. See [`NativeRegistry::find_by_prefix`].
    registry.register("match_Any_MANY__Function_$1_MANY$__T_m_", Match);
    registry.register("match_Any_MANY__Function_$1_MANY$__P_o__T_m_", Match);
    registry.register("id_Any_1__String_1_", Id);
    registry.register("type_Any_1__Type_1_", TypeOf);
    registry.register("genericType_Any_1__GenericType_1_", GenericTypeOf);
    registry.register("rawType_GenericType_1__Type_$0_1$_", RawType);
    registry.register("enumName_Enumeration_1__String_1_", EnumName);
    // `enumValues` is generic — the compiler may mangle with different
    // T-variable spellings at the call site. Register under a key with the
    // `enumValues_` prefix so the simple-name fallback finds it.
    registry.register("enumValues_Enumeration_1__T_MANY_", EnumValues);
    registry.register("toRepresentation_Any_1__String_1_", ToRepresentation);
    registry.register("subTypeOf_Type_1__Type_1__Boolean_1_", SubTypeOf);
    registry.register("generalizations_Type_1__Type_$1_MANY$_", Generalizations);
    registry.register(
        "extractEnumValue_Enumeration_1__String_1__T_1_",
        ExtractEnumValue,
    );
    registry.register("evaluateAndDeactivate_T_m__T_m_", EvaluateAndDeactivate);
    registry.register("deactivate_Any_MANY__ValueSpecification_1_", Deactivate);
    registry.register("newUnit_Unit_1__Number_1__Any_1_", NewUnit);
    registry.register("getUnitValue_Any_1__Number_1_", GetUnitValue);
    registry.register(
        "reactivate_ValueSpecification_1__Map_1__Any_MANY_",
        Reactivate,
    );
    registry.register("genericTypeClass_GenericType_1__Class_1_", GenericTypeClass);
    registry.register(
        "elementPath_PackageableElement_1__PackageableElement_$1_MANY$_",
        ElementPath,
    );
    registry.register("openVariableValues_Function_1__Map_1_", OpenVariableValues);
}
