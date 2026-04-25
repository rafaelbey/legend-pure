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

//! Core language native functions: `letFunction`, `if`, `new`, `copy`, `print`.
//!
//! These are the fundamental control-flow and object-construction primitives
//! that the Pure language desugars into. `letFunction` binds variables; `if`
//! provides short-circuiting conditionals; `new` constructs class instances
//! from the `^Class(prop=val)` syntax; `copy` produces a modified clone.

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectId;
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

// ---------------------------------------------------------------------------
// letFunction
// ---------------------------------------------------------------------------

/// Pure `letFunction(String[1], T[m]): T[m]`
///
/// The compiler desugars `let x = expr` → `FunctionCall("letFunction", [name, value])`.
/// This native binds the value to the variable name in the current scope
/// and returns the value.
#[derive(Debug)]
pub struct LetFunction;

impl NativeFunction for LetFunction {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("letFunction", &values, 2)?;
        let name = values[0].as_string()?;
        let value = values[1].clone();
        ctx.context_mut().set(name.clone(), value.clone());
        Ok(Evaluated::new(value))
    }

    fn signature(&self) -> &'static str {
        "letFunction(String[1], T[m]): T[m]"
    }
}

// ---------------------------------------------------------------------------
// if
// ---------------------------------------------------------------------------

/// Pure `if(Boolean[1], Function<{->T[m]}>[1], Function<{->T[m]}>[1]): T[m]`
///
/// Short-circuiting conditional. Forces each argument on demand so only the
/// taken branch is evaluated. The condition and branches are raw `ValueSpec`s
/// — the native calls `ctx.evaluate` directly in the right order.
#[derive(Debug)]
pub struct If;

impl NativeFunction for If {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("if", args, 3)?;
        let cond = crate::native::force_thunk(&args[0], ctx)?.as_boolean()?;
        let branch = if cond { &args[1] } else { &args[2] };
        // Branches are typed `Function<{->T[m]}>` — the compiler lowers them
        // as zero-parameter `Lambda` wrappers. `force_thunk` unwraps that so
        // the caller gets the branch's value, not the closure itself.
        crate::native::force_thunk(branch, ctx)
    }

    fn signature(&self) -> &'static str {
        "if(Boolean[1], Function<{->T[m]}>[1], Function<{->T[m]}>[1]): T[m]"
    }
}

// ---------------------------------------------------------------------------
// eval — call a function value with 0-7 arguments
// ---------------------------------------------------------------------------

/// Pure `eval(func:Function<{...->V[m]}>[1], ...params):V[m]`
///
/// All 8 platform `eval` overloads (0–7 extra parameters) share this single
/// implementation: extract `args[0]` as the callable and pass `args[1..]` as
/// the arguments via `ctx.call_function`.
#[derive(Debug)]
pub struct Eval;

impl NativeFunction for Eval {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "eval: expected at least 1 argument (the function)".into(),
            )
            .into());
        }
        let values = force_all(args, ctx)?;
        let func = &values[0];
        let params = &values[1..];
        Ok(Evaluated::new(ctx.call_function(func, params)?))
    }

    fn signature(&self) -> &'static str {
        "eval(Function[1], ...): Any[*]"
    }
}

// ---------------------------------------------------------------------------
// evaluate — call a function with parameters wrapped in List<Any> objects
// ---------------------------------------------------------------------------

/// Pure `evaluate(func:Function<Any>[1], params:List<Any>[*]):Any[*]`
///
/// Sibling of [`Eval`] with a different calling convention: each function
/// parameter arrives as a `List<Any>(values=...)` heap object instead of
/// a raw arg. We unpack each list's `values` property and flatten them into
/// the final arg vector passed to [`EvalContextTrait::call_function`].
///
/// Pattern seen across the platform: the surveyor builds a lambda clone via
/// `^LambdaFunction(expressionSequence=$fn.expressionSequence)` and invokes
/// it with `->evaluate([])` (no params) or
/// `->evaluate([^List<Any>(values=x), ^List<Any>(values=[y,z])])` (two lists).
#[derive(Debug)]
pub struct Evaluate;

impl NativeFunction for Evaluate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "evaluate: expected at least 1 argument (the function)".into(),
            )
            .into());
        }
        let values = force_all(args, ctx)?;
        let func = values[0].clone();

        // Each remaining arg is either a `List<Any>` heap object with a
        // `values` property, a collection of such lists (multi-valued
        // `List<Any>[*]`), or `Unit` (empty). Flatten into the function's
        // positional parameters: one `List.values` = one parameter binding.
        let mut params: Vec<Value> = Vec::new();
        for rest in &values[1..] {
            match rest {
                Value::Unit => {}
                Value::Collection(v) => {
                    for entry in v.iter() {
                        push_list_values(ctx, entry, &mut params)?;
                    }
                }
                other => push_list_values(ctx, other, &mut params)?,
            }
        }
        Ok(Evaluated::new(ctx.call_function(&func, &params)?))
    }

    fn signature(&self) -> &'static str {
        "evaluate(func:Function<Any>[1], params:List<Any>[*]):Any[*]"
    }
}

/// Read a single `List<Any>` argument's `values` property and append the
/// flattened contents to `out`. Non-object inputs are treated as a direct
/// scalar parameter binding (surveyor sometimes passes raw values).
fn push_list_values(
    ctx: &mut dyn EvalContextTrait,
    v: &Value,
    out: &mut Vec<Value>,
) -> Result<(), PureException> {
    match v {
        Value::Object(id) => {
            let values = ctx.heap().get_property_values(*id, "values")?;
            // A List<Any>.values of multiplicity [*] reduces to a single
            // bound positional parameter: the collection itself (or the
            // scalar, if single). This matches Java Pure's List-packing
            // convention — the receiver function sees one param per List.
            let mut copy: Vec<Value> = Vec::with_capacity(values.len());
            for val in values.iter() {
                copy.push(val.clone());
            }
            out.push(Value::from_vec(copy));
            Ok(())
        }
        Value::Unit => Ok(()),
        other => {
            out.push(other.clone());
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// print — write a value to stdout (used by println)
// ---------------------------------------------------------------------------

/// Pure `print(param:Any[*], max:Integer[1]):Nil[0]`
///
/// Writes each argument's [`Display`](std::fmt::Display) form to stdout,
/// then returns [`Value::Unit`]. The `max` parameter is accepted for
/// signature compatibility and ignored (the Java runtime uses it to cap
/// graph-walk depth when rendering objects; the Rust interpreter currently
/// always uses the scalar `Display`).
#[derive(Debug)]
pub struct Print;

impl NativeFunction for Print {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("print", &values, 2)?;
        match &values[0] {
            Value::Collection(v) => {
                for item in v.iter() {
                    ctx.console_output(&format!("{item}"));
                }
            }
            Value::Unit => {}
            other => ctx.console_output(&format!("{other}")),
        }
        Ok(Evaluated::new(Value::Unit))
    }

    fn signature(&self) -> &'static str {
        "print(param:Any[*], max:Integer[1]):Nil[0]"
    }
}

// ---------------------------------------------------------------------------
// new — object construction
// ---------------------------------------------------------------------------

/// Pure `new<T>(class:Class<T>[1], id:String[1], keyExpressions:KeyExpression[*]):T[1]`
///
/// Constructs a runtime instance of the given class and seeds its properties.
///
/// The compiler's `^Class(prop1=val1, prop2=val2)` desugaring calls this
/// native with flattened arguments:
///
/// ```text
/// new(<Element: class>, "ClassName", "prop1", val1, "prop2", val2, ...)
/// ```
///
/// The second argument (class simple name) is ignored — we classify the
/// heap entry by the class's full path so property lookups through
/// `Value::Object` surface the right metatype. Extra property assignments
/// after the two fixed positionals are read as flat key/value pairs.
#[derive(Debug)]
pub struct New;

impl NativeFunction for New {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        if values.len() < 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "new: expected at least 2 arguments (class, id), got {}",
                values.len()
            ))
            .into());
        }
        let class_id = match &values[0] {
            Value::Element(id) => *id,
            other => {
                return Err(PureRuntimeError::type_mismatch("Class", other).into());
            }
        };
        let _id = &values[1]; // ignored — Pure's Java impl uses this only as a debug label
        // Two call shapes converge on this native:
        //
        //   * Compiler-emitted `^Class<T1, T2>(prop=val, …)` — args[2] is a
        //     Collection of resolved type-argument elements (or empty when
        //     no `<…>` was declared); args[3..] is the flat key/value pair
        //     sequence. Detected by args[2] being a Collection whose entries
        //     (when present) are `Value::Element`.
        //
        //   * Pure-source `new(class, id)` / `new(class, id, [keyExprs])` —
        //     direct calls to the platform's declared overloads. No type
        //     arguments; args[2] (when present) is a Collection of
        //     `KeyExpression` heap objects per the M3 signature. Full
        //     KeyExpression decoding isn't implemented yet — surface the
        //     shape and skip property hydration.
        let mut type_args: Vec<Value> = Vec::new();
        let mut kvs_offset = values.len(); // default: no kvs
        if values.len() >= 3 {
            match &values[2] {
                // Empty `^Class()` constructions lower the position-2 type-arg
                // Collection to `Value::Unit` (zero-element from_vec).
                // Treat as the compiler-emitted shape with no type args.
                Value::Unit => {
                    kvs_offset = 3;
                }
                Value::Collection(coll) => {
                    let all_elements = coll.iter().all(|v| matches!(v, Value::Element(_)));
                    if all_elements {
                        type_args = coll.iter().cloned().collect();
                        kvs_offset = 3;
                    } else {
                        // Pure-source `new(class, id, [keyExprs])` — leave kvs
                        // untouched; KeyExpression hydration is a separate
                        // backlog item.
                        kvs_offset = values.len();
                    }
                }
                // Single Element at position 2 — compiler-emitted shape
                // where exactly one type arg was provided (`from_vec` collapses
                // a one-element vector to its scalar).
                Value::Element(_) => {
                    type_args = vec![values[2].clone()];
                    kvs_offset = 3;
                }
                _ => {
                    // Old-style flat key/value pairs starting at position 2 —
                    // compiler no longer emits this shape, but tolerate it
                    // for any direct callers.
                    kvs_offset = 2;
                }
            }
        }
        let classifier = class_fqn(ctx.model(), class_id);

        // Shortcut: `^LambdaFunction(expressionSequence = <lambda>)` is the
        // surveyor's lambda-cloning idiom. When the construction's only
        // keyword is `expressionSequence` carrying a single Function value,
        // return that Function directly instead of allocating a heap object.
        // This keeps the round-trip `$fn.expressionSequence` →
        // `^LambdaFunction(expressionSequence=…)` → `->evaluate([])`
        // working without requiring full `ValueSpecification` heap modelling
        // on the lambda body. See boolean/and.pure's
        // `testShortCircuitInDynamicEvaluation` for the motivating pattern.
        if is_lambda_function_class(ctx.model(), class_id)
            && let Some(fn_val) = try_lambda_shortcut(&values[kvs_offset..])
        {
            return Ok(Evaluated::new(fn_val));
        }

        let obj = ctx.heap_mut().alloc_dynamic(classifier);
        if !type_args.is_empty() {
            ctx.heap_mut()
                .mutate_set(obj, "__typeArguments", &type_args)?;
        }
        apply_key_value_pairs(ctx, obj, &values[kvs_offset..])?;
        populate_association_inverses(ctx, obj, class_id, &values[kvs_offset..])?;
        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "new<T>(class:Class<T>[1], id:String[1], keyExpressions:KeyExpression[*]):T[1]"
    }
}

// ---------------------------------------------------------------------------
// copy — structural update of an existing instance
// ---------------------------------------------------------------------------

/// Pure `copy<T>(source:T[1], keyExpressions:KeyExpression[*]):T[1]`
///
/// The compiler's `^$source(prop=val)` desugaring calls this native with
/// flat key/value pairs after the source:
///
/// ```text
/// copy(<Object:...>, "prop1", val1, "prop2", val2, ...)
/// ```
///
/// Allocates a new heap object that shares the source's classifier, copies
/// every property from the source, then applies the overrides.
#[derive(Debug)]
pub struct Copy;

impl NativeFunction for Copy {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        if values.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "copy: expected at least 1 argument (source)".into(),
            )
            .into());
        }

        // Copying a `Value::Element` — `^$functionRef()` in Pure. Elements
        // aren't heap-backed, so we materialise a fresh heap Object
        // classified by the element's metatype (`ConcreteFunctionDefinition`,
        // `Class`, etc. via `bootstrap::metatype_of`). Subsequent introspection
        // (`$copy->genericType().rawType`) then resolves to the same metatype
        // the original's element reflection would produce.
        if let Value::Element(elem_id) = &values[0] {
            let element = ctx.model().get_element(*elem_id);
            let Some(meta_id) =
                legend_pure_parser_pure::bootstrap::metatype_of(ctx.model(), element)
            else {
                return Err(PureRuntimeError::EvaluationError(
                    "copy: element has no M3 metatype — cannot materialise".into(),
                )
                .into());
            };
            let classifier =
                crate::model_utils::build_element_path(ctx.model(), meta_id, "::", false);
            let obj = ctx.heap_mut().alloc_dynamic(classifier);
            apply_key_value_pairs(ctx, obj, &values[1..])?;
            return Ok(Evaluated::new(Value::Object(obj)));
        }

        let source_id = values[0].as_object()?;

        // Snapshot classifier + properties before we take a mutable heap borrow.
        let classifier: String = ctx.heap().classifier(source_id)?.to_owned();
        let names = ctx.heap().property_names(source_id)?;
        let mut original_props: Vec<(SmolStr, Vec<Value>)> = Vec::with_capacity(names.len());
        for name in names {
            let prop_values: Vec<Value> = ctx
                .heap()
                .get_property_values(source_id, name.as_str())?
                .iter()
                .cloned()
                .collect();
            original_props.push((name, prop_values));
        }

        let obj = ctx.heap_mut().alloc_dynamic(classifier.clone());
        // Split the supplied kvs into plain (no `.`) and dotted-path
        // (`address.name = 'X'`) groups. Plain kvs go through the normal
        // mutate_set; path kvs need a per-first-segment deep-clone so
        // `^$pierre(address.name='Somewhere')` lands on a fresh Address
        // object instead of mutating the source's nested instance.
        let (plain_kvs, path_kvs) = partition_path_kvs(&values[1..])?;

        // Path-property updates rebind a top-level property (`firm`) to
        // a fresh clone — the carried-over inverse population for that
        // property would otherwise add the new copy to the *source's*
        // nested-object inverse list (e.g. add `$bob` to the OLD
        // `gsJC.employees`). Track the first-segment names to exclude
        // them from carried inverse population.
        let path_overrides: std::collections::HashSet<SmolStr> = {
            let mut set = std::collections::HashSet::new();
            let mut i = 0;
            while i < path_kvs.len() {
                if let Ok(k) = path_kvs[i].as_string()
                    && let Some((head, _)) = k.split_once('.')
                {
                    set.insert(SmolStr::new(head));
                }
                i += 2;
            }
            set
        };

        // Flatten carried-over properties into a `[key1, val1, key2, val2, …]`
        // slice compatible with `populate_association_inverses` — the copy
        // should appear in every association inverse its source belonged
        // to (e.g. a copied Person is appended to `firm.employees` so
        // `assertSameElements([$bob, $pierre], $firmX.employees)` holds).
        // Skip any property the user is path-overriding; the inverse for
        // those is established by the path-property handler against the
        // freshly-cloned nested object.
        let mut carried_kvs: Vec<Value> = Vec::with_capacity(original_props.len() * 2);
        for (name, prop_values) in &original_props {
            ctx.heap_mut().mutate_add(obj, name.as_str(), prop_values)?;
            if path_overrides.contains(name) {
                continue;
            }
            carried_kvs.push(Value::String(name.clone()));
            carried_kvs.push(Value::from_vec(prop_values.clone()));
        }

        apply_key_value_pairs(ctx, obj, &plain_kvs)?;
        apply_path_property_updates(ctx, source_id, obj, &path_kvs)?;

        if let Some(class_id) = crate::m3_paths::resolve(ctx.model(), &classifier) {
            populate_association_inverses(ctx, obj, class_id, &carried_kvs)?;
            populate_association_inverses(ctx, obj, class_id, &plain_kvs)?;
            // Re-establish inverses for path-overridden top-level
            // properties — read the freshly-set value off the new
            // object and feed it through the same mechanism so the
            // cloned nested object's inverse-side list is updated.
            for head in &path_overrides {
                let new_vals = ctx.heap().get_property_values(obj, head.as_str())?;
                let assigned = Value::from_vec(new_vals.iter().cloned().collect());
                let synthetic = vec![Value::String(head.clone()), assigned];
                populate_association_inverses(ctx, obj, class_id, &synthetic)?;
            }
        }

        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "copy<T>(source:T[1], keyExpressions:KeyExpression[*]):T[1]"
    }
}

/// Pure
/// `dynamicNew(class:Class<Any>[1], kvs:KeyValue[*]):Any[1]` /
/// `dynamicNew(gt:GenericType[1], kvs:KeyValue[*]):Any[1]`
///
/// Reflective `new` — the class (or its `GenericType` wrapper) is
/// resolved at runtime, and property assignments come in as a
/// collection of `KeyValue` heap objects with `key: String[1]` and
/// `value: Any[*]`. Java Pure's variants additionally accept property-
/// override / default-override / post-init hook functions; those
/// overloads aren't handled here yet (registered separately under
/// their own mangled keys once implementation lands).
///
/// Defaulting behaviour — when a property on the target class declares
/// a `default_value` expression and the caller didn't supply a
/// `KeyValue` for that property, the default is evaluated in the
/// current scope and stored on the new instance. Caller-supplied
/// values always win over defaults (mirrors `^Class(prop=val)` +
/// default interaction in the compiler).
#[derive(Debug)]
pub struct DynamicNew;

impl NativeFunction for DynamicNew {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        if values.len() < 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "dynamicNew: expected at least 2 arguments (class/genericType, kvs), got {}",
                values.len()
            ))
            .into());
        }

        // Resolve the target class — accept a direct Class Element or a
        // `GenericType` heap wrapper whose `rawType` points at one.
        let class_id = match &values[0] {
            Value::Element(id) => *id,
            Value::Object(obj_id) => {
                let raw_type_vals = ctx.heap().get_property_values(*obj_id, "rawType")?;
                match raw_type_vals.iter().next() {
                    Some(Value::Element(id)) => *id,
                    _ => {
                        return Err(PureRuntimeError::EvaluationError(
                            "dynamicNew: GenericType wrapper has no resolved rawType".into(),
                        )
                        .into());
                    }
                }
            }
            other => {
                return Err(PureRuntimeError::type_mismatch("Class or GenericType", other).into());
            }
        };
        if !matches!(ctx.model().get_element(class_id), Element::Class(_)) {
            return Err(PureRuntimeError::EvaluationError(format!(
                "dynamicNew: element {} is not a Class",
                ctx.model().element_name(class_id)
            ))
            .into());
        }

        let classifier = class_fqn(ctx.model(), class_id);
        let obj = ctx.heap_mut().alloc_dynamic(classifier);

        // Snapshot the KeyValue entries up front so subsequent heap
        // mutations (applying defaults, overlaying values) don't trip
        // up the iteration.
        let kvs = values[1].to_collection();
        let mut supplied: Vec<(SmolStr, Vec<Value>)> = Vec::with_capacity(kvs.len());
        let mut supplied_keys: std::collections::HashSet<SmolStr> =
            std::collections::HashSet::with_capacity(kvs.len());
        for kv in kvs.iter() {
            let Value::Object(kv_id) = kv else {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "dynamicNew: expected KeyValue, got {}",
                    kv.type_name()
                ))
                .into());
            };
            let key_vals = ctx.heap().get_property_values(*kv_id, "key")?;
            let Some(Value::String(key)) = key_vals.iter().next() else {
                return Err(PureRuntimeError::EvaluationError(
                    "dynamicNew: KeyValue.key is missing or not a String".into(),
                )
                .into());
            };
            let value_vals = ctx.heap().get_property_values(*kv_id, "value")?;
            let flat: Vec<Value> = value_vals.iter().cloned().collect();
            supplied_keys.insert(key.clone());
            supplied.push((key.clone(), flat));
        }

        // Apply per-property defaults for any property the caller didn't
        // override. Walk the supertype chain child-first so that a
        // redeclared property on a subclass wins over the parent's
        // default (Pure MRO: most-specific class owns the effective
        // default). Evaluate each `default_value` expression in the
        // current context — it's a regular ValueSpec.
        let default_specs: Vec<(SmolStr, legend_pure_parser_pure::types::ValueSpec)> = {
            let mut acc: Vec<(SmolStr, legend_pure_parser_pure::types::ValueSpec)> = Vec::new();
            let mut seen: std::collections::HashSet<SmolStr> = supplied_keys.clone();
            let mut visited: std::collections::HashSet<ElementId> =
                std::collections::HashSet::new();
            let mut stack: Vec<ElementId> = vec![class_id];
            while let Some(id) = stack.pop() {
                if !visited.insert(id) {
                    continue;
                }
                let Element::Class(c) = ctx.model().get_element(id) else {
                    continue;
                };
                for p in &c.properties {
                    if !seen.insert(p.name.clone()) {
                        continue;
                    }
                    if let Some(expr) = &p.default_value {
                        acc.push((p.name.clone(), expr.clone()));
                    }
                }
                for st in &c.super_types {
                    if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                        stack.push(*element);
                    }
                }
            }
            acc
        };
        for (name, spec) in default_specs {
            let v = ctx.evaluate(&spec)?.into_value();
            let flat: Vec<Value> = match v {
                Value::Collection(coll) => coll.iter().cloned().collect(),
                Value::Unit => Vec::new(),
                other => vec![other],
            };
            ctx.heap_mut().mutate_set(obj, name.as_str(), &flat)?;
        }

        // Overlay caller-supplied bindings. Build a flat [k, v, k, v]
        // slice so we can reuse `populate_association_inverses` for
        // bidirectional-association maintenance.
        let mut assoc_kvs: Vec<Value> = Vec::with_capacity(supplied.len() * 2);
        for (key, flat) in supplied {
            ctx.heap_mut().mutate_set(obj, key.as_str(), &flat)?;
            assoc_kvs.push(Value::String(key));
            assoc_kvs.push(Value::from_vec(flat));
        }
        populate_association_inverses(ctx, obj, class_id, &assoc_kvs)?;

        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "dynamicNew(class:Class[1]|GenericType[1], kvs:KeyValue[*]):Any[1]"
    }
}

/// Recognise the `LambdaFunction` / `Function` M3 classes used by Pure's
/// `^LambdaFunction(expressionSequence = …)` clone idiom.
///
/// Compares by resolved [`ElementId`] against the canonical
/// `meta::pure::metamodel::function::{Function,LambdaFunction}` paths — not
/// by textual suffix match on the classifier string — so unrelated classes
/// named `LambdaFunction` in user code don't trigger the shortcut.
fn is_lambda_function_class(
    model: &legend_pure_parser_pure::model::PureModel,
    class_id: legend_pure_parser_pure::ids::ElementId,
) -> bool {
    crate::m3_paths::resolve(model, crate::m3_paths::LAMBDA_FUNCTION) == Some(class_id)
        || crate::m3_paths::resolve(model, crate::m3_paths::FUNCTION) == Some(class_id)
}

/// Detect the `(expressionSequence, <single Function>)` key/value pair
/// emitted by `^LambdaFunction(expressionSequence = $fn.expressionSequence)`.
///
/// Returns the underlying `Value::Function` when the shortcut applies,
/// so `New` can return it directly instead of allocating a heap object.
fn try_lambda_shortcut(kvs: &[Value]) -> Option<Value> {
    if kvs.len() != 2 {
        return None;
    }
    let Value::String(key) = &kvs[0] else {
        return None;
    };
    if key != "expressionSequence" {
        return None;
    }
    match &kvs[1] {
        Value::Function(_) => Some(kvs[1].clone()),
        Value::Collection(v) if v.len() == 1 && matches!(v[0], Value::Function(_)) => {
            Some(v[0].clone())
        }
        _ => None,
    }
}

/// Populate association reverse-links for every Association-backed property
/// just assigned to `obj`.
///
/// When Pure code writes `^Person(firm=firmX)`, the `Employment` association
/// dictates that `firmX.employees` must also contain the new person. Java
/// Pure maintains this invariant implicitly; the Rust heap does not, so we
/// walk the key/value pairs after construction and inject the reverse edge.
///
/// Algorithm per `(key, value)` pair:
/// 1. Look up `association_properties[class_id]` — a list of
///    `(association_id, prop_idx)` where `assoc.properties[prop_idx]` has
///    type = `class_id` (i.e., points back AT us).
/// 2. In each such association with exactly two properties, the property
///    at `1 - prop_idx` is the one *injected into* `class_id` (the one
///    the caller may have just set).
/// 3. If that injected property's name matches `key` and the assigned
///    value is a heap `Value::Object`, append `obj` to the target
///    object's `assoc.properties[prop_idx].name` slot — the inverse.
#[allow(clippy::result_large_err)]
fn populate_association_inverses(
    ctx: &mut dyn EvalContextTrait,
    obj: ObjectId,
    class_id: ElementId,
    kvs: &[Value],
) -> Result<(), PureException> {
    if kvs.len() % 2 != 0 {
        return Ok(()); // Arity already validated by apply_key_value_pairs
    }

    // Snapshot the association entries once — cheap O(n) per property but we
    // avoid re-borrowing the model mid-loop.
    let entries: Vec<(ElementId, usize)> = ctx.model().association_properties(class_id).to_vec();
    if entries.is_empty() {
        return Ok(());
    }

    // For each forward assignment key, find the matching injected property
    // and its inverse, then mutate the target object's inverse slot.
    let mut i = 0;
    while i < kvs.len() {
        let Value::String(key) = &kvs[i] else {
            i += 2;
            continue;
        };
        let assigned = kvs[i + 1].clone();
        for (assoc_id, prop_idx_pointing_to_self) in &entries {
            let Element::Association(assoc) = ctx.model().get_element(*assoc_id) else {
                continue;
            };
            if assoc.properties.len() != 2 {
                continue; // n-ary associations: inverse isn't a single peer
            }
            let injected_idx = 1 - *prop_idx_pointing_to_self;
            let injected_name = assoc.properties[injected_idx].name.clone();
            if injected_name.as_str() != key.as_str() {
                continue;
            }
            let inverse_name = assoc.properties[*prop_idx_pointing_to_self].name.clone();

            let targets: Vec<ObjectId> = match &assigned {
                Value::Object(id) => vec![*id],
                Value::Collection(v) => v
                    .iter()
                    .filter_map(|x| match x {
                        Value::Object(id) => Some(*id),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for target in targets {
                ctx.heap_mut()
                    .mutate_add(target, inverse_name.as_str(), &[Value::Object(obj)])?;
            }
        }
        i += 2;
    }
    Ok(())
}

/// Walk a flat `[key1, val1, key2, val2, ...]` slice and `mutate_set` each
/// pair onto `obj`. Collections are expanded so property storage stays flat.
///
/// Uses set (replace) rather than add (append) semantics: `^$src(prop='new')`
/// in Pure replaces `prop`'s value with `'new'`, it does not append. `New`
/// constructs fresh empty objects where this distinction is moot, but
/// `Copy` pre-populates from the source and relies on the reset behaviour
/// so the override doesn't leak the original value into the slot.
fn apply_key_value_pairs(
    ctx: &mut dyn EvalContextTrait,
    obj: ObjectId,
    kvs: &[Value],
) -> Result<(), PureException> {
    if kvs.len() % 2 != 0 {
        return Err(PureRuntimeError::EvaluationError(format!(
            "object construction: expected key/value pairs, got {} extra arguments",
            kvs.len()
        ))
        .into());
    }
    let mut i = 0;
    while i < kvs.len() {
        let key = kvs[i].as_string()?.clone();
        let value = kvs[i + 1].clone();
        let values: Vec<Value> = match value {
            Value::Collection(coll) => coll.iter().cloned().collect(),
            Value::Unit => Vec::new(),
            other => vec![other],
        };
        ctx.heap_mut().mutate_set(obj, key.as_str(), &values)?;
        i += 2;
    }
    Ok(())
}

/// Split a `[key1, val1, key2, val2, ...]` slice into two parallel
/// flat-pair slices: the plain keys (no `.`) and the dotted-path
/// keys (`address.name`, `firm.legalName`). Both keep the same flat
/// shape so `apply_key_value_pairs` and `apply_path_property_updates`
/// can consume them uniformly.
#[allow(clippy::result_large_err)]
fn partition_path_kvs(kvs: &[Value]) -> Result<(Vec<Value>, Vec<Value>), PureException> {
    if kvs.len() % 2 != 0 {
        return Err(PureRuntimeError::EvaluationError(format!(
            "copy: expected key/value pairs, got {} extra arguments",
            kvs.len()
        ))
        .into());
    }
    let mut plain: Vec<Value> = Vec::new();
    let mut path: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < kvs.len() {
        let key = kvs[i].as_string()?;
        let bucket = if key.contains('.') {
            &mut path
        } else {
            &mut plain
        };
        bucket.push(kvs[i].clone());
        bucket.push(kvs[i + 1].clone());
        i += 2;
    }
    Ok((plain, path))
}

/// Apply dotted-path property updates (`address.name = 'Somewhere'`,
/// `firm.legalName = 'FirmX'`) to the freshly-allocated copy.
///
/// For each unique first segment, deep-clone the source's nested object
/// once and apply every update keyed under that segment to the clone.
/// The new object's first-segment property is then set to the clone —
/// this is what makes `assertIsNot($pierre.address, $pierre2.address)`
/// hold even though only the leaf field was mentioned. Multi-level
/// paths (`a.b.c`) recurse: the `b.c = v` update is applied as a
/// path-property on the clone of `a`.
#[allow(clippy::result_large_err)]
fn apply_path_property_updates(
    ctx: &mut dyn EvalContextTrait,
    source_id: ObjectId,
    target_id: ObjectId,
    kvs: &[Value],
) -> Result<(), PureException> {
    use std::collections::BTreeMap;

    // Group `(first_segment → [(rest_path, value), …])` so multiple
    // updates against the same first segment share a single clone.
    let mut groups: BTreeMap<SmolStr, Vec<(SmolStr, Value)>> = BTreeMap::new();
    let mut i = 0;
    while i < kvs.len() {
        let key = kvs[i].as_string()?.clone();
        let value = kvs[i + 1].clone();
        let (head, tail) = match key.split_once('.') {
            Some((h, t)) => (SmolStr::new(h), SmolStr::new(t)),
            None => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "copy: expected dotted-path key, got '{key}'"
                ))
                .into());
            }
        };
        groups.entry(head).or_default().push((tail, value));
        i += 2;
    }

    for (head, updates) in groups {
        let nested = ctx.heap().get_property_values(source_id, head.as_str())?;
        let mut cloned: Vec<Value> = Vec::with_capacity(nested.len());
        for v in nested.iter() {
            let Value::Object(inner_id) = v else {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "copy: cannot path-set '{head}.…' on a non-object property value"
                ))
                .into());
            };
            let clone_id = clone_heap_object(ctx, *inner_id)?;
            // Group updates between leaf-set ('name = "X"') and
            // further-nested path-set ('address.name = "X"' against this
            // clone). Multi-level paths recurse via
            // `apply_path_property_updates`.
            let mut leaf_kvs: Vec<Value> = Vec::new();
            let mut nested_path_kvs: Vec<Value> = Vec::new();
            for (rest, val) in &updates {
                let bucket = if rest.contains('.') {
                    &mut nested_path_kvs
                } else {
                    &mut leaf_kvs
                };
                bucket.push(Value::String(rest.clone()));
                bucket.push(val.clone());
            }
            apply_key_value_pairs(ctx, clone_id, &leaf_kvs)?;
            if !nested_path_kvs.is_empty() {
                apply_path_property_updates(ctx, *inner_id, clone_id, &nested_path_kvs)?;
            }
            cloned.push(Value::Object(clone_id));
        }
        ctx.heap_mut()
            .mutate_set(target_id, head.as_str(), &cloned)?;
    }
    Ok(())
}

/// Allocate a fresh heap Object that mirrors `source_id` — same
/// classifier and a copy of every property. Used by the path-property
/// path so a `^$src(nested.field=val)` update lands on a new nested
/// instance rather than mutating the source's.
#[allow(clippy::result_large_err)]
fn clone_heap_object(
    ctx: &mut dyn EvalContextTrait,
    source_id: ObjectId,
) -> Result<ObjectId, PureException> {
    let classifier = ctx.heap().classifier(source_id)?.to_owned();
    let names = ctx.heap().property_names(source_id)?;
    let mut snapshot: Vec<(SmolStr, Vec<Value>)> = Vec::with_capacity(names.len());
    for name in names {
        let prop_values: Vec<Value> = ctx
            .heap()
            .get_property_values(source_id, name.as_str())?
            .iter()
            .cloned()
            .collect();
        snapshot.push((name, prop_values));
    }
    let clone_id = ctx.heap_mut().alloc_dynamic(classifier);
    for (name, vs) in snapshot {
        ctx.heap_mut().mutate_set(clone_id, name.as_str(), &vs)?;
    }
    Ok(clone_id)
}

/// Compute the qualified path for a class element (used as heap classifier).
fn class_fqn(model: &PureModel, id: ElementId) -> String {
    let mut segments: Vec<SmolStr> = Vec::new();
    segments.push(match model.get_element(id) {
        Element::Function(f) => f.function_name.clone(),
        _ => model.element_name(id).clone(),
    });
    let mut parent = match id {
        ElementId::Package(pkg_id) => model.get_package(pkg_id).parent,
        ElementId::InstanceId { .. } => Some(model.get_node(id).parent_package),
    };
    while let Some(pkg_id) = parent {
        let pkg = model.get_package(pkg_id);
        if pkg.parent.is_none() {
            // Skip the unnamed root package.
            break;
        }
        segments.push(pkg.name.clone());
        parent = pkg.parent;
    }
    segments.reverse();
    segments
        .iter()
        .map(SmolStr::as_str)
        .collect::<Vec<_>>()
        .join("::")
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register core language native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("letFunction_String_1__T_m__T_m_", LetFunction);
    registry.register("if_Boolean_1__Function_1__Function_1__T_m_", If);
    registry.register("print_Any_MANY__Integer_1__Nil_0_", Print);
    // `new` and `copy` are invoked through the simple-name prefix fallback —
    // the compiler lowers `^Class(...)` / `^$src(...)` with raw `"new"` /
    // `"copy"` function names and no resolved ElementId, so we register
    // under a key that begins with the simple name.
    registry.register("new_Class_1__String_1__KeyExpression_MANY__T_1_", New);
    registry.register("copy_T_1__KeyExpression_MANY__T_1_", Copy);
    // Basic dynamicNew — Class / GenericType receivers, no override hooks.
    // The hook-bearing overloads (property / default / post-init lambdas)
    // share the same mangled `dynamicNew_*` family and are registered
    // against the same native; unused hook args are silently discarded
    // because the basic construction path doesn't invoke them.
    registry.register("dynamicNew_Class_1__KeyValue_MANY__Any_1_", DynamicNew);
    registry.register(
        "dynamicNew_GenericType_1__KeyValue_MANY__Any_1_",
        DynamicNew,
    );
    // Override-hook overloads — additional `Function<{...}>[0..1]`
    // parameters (property override / default override / post-init
    // hook) follow the KeyValue collection. The basic `DynamicNew`
    // impl silently ignores args past position 1, so the hook-bearing
    // tests flip provided none of them actually exercise the hooks.
    // Invoking the hooks properly is tracked separately.
    registry.register(
        "dynamicNew_Class_1__KeyValue_MANY__Function_$0_1$__Function_$0_1$__Any_$0_1$__Any_1_",
        DynamicNew,
    );
    registry.register(
        "dynamicNew_GenericType_1__KeyValue_MANY__Function_$0_1$__Function_$0_1$__Any_$0_1$__Any_1_",
        DynamicNew,
    );

    // eval — 8 arities (0–7 extra parameters).
    // Mangled names derived from the platform source in essential/lang/eval/eval.pure
    // via the same mangling rules as ast::element::FunctionSignature::mangled_name.
    // eval — 8 arities (0–7 extra parameters).
    // Mangled names derived from the platform source in essential/lang/eval/eval.pure.
    registry.register("eval_Function_1__V_m_", Eval);
    registry.register("eval_Function_1__T_n__V_m_", Eval);
    registry.register("eval_Function_1__T_n__U_p__V_m_", Eval);
    registry.register("eval_Function_1__T_n__U_p__W_q__V_m_", Eval);
    registry.register("eval_Function_1__T_n__U_p__W_q__X_r__V_m_", Eval);
    registry.register("eval_Function_1__T_n__U_p__W_q__X_r__Y_s__V_m_", Eval);
    registry.register("eval_Function_1__T_n__U_p__W_q__X_r__Y_s__Z_t__V_m_", Eval);
    // 7-extra-arg variant — type variables S[n],T[o],... match the Pure source declaration
    registry.register(
        "eval_Function_1__S_n__T_o__U_p__W_q__X_r__Y_s__Z_t__V_m_",
        Eval,
    );

    // evaluate(func, List<Any>[*]) — companion of eval with parameter lists
    // packed into `List<Any>` heap objects. See `evaluate.pure` platform
    // source. Mangled name matches the stored FQN.
    registry.register("evaluate_Function_1__List_MANY__Any_MANY_", Evaluate);
}
