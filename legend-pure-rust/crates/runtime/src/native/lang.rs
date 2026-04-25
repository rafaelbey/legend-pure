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

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ExprKind, TypeExpr, ValueSpec};
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
        // Compiler-emitted `^Class<TArgs>(typeVars)(props)` flows through
        // here as: [class, name, type_args_coll, type_var_values_coll,
        // key1, val1, …]. Pure-source `new(class, id)` /
        // `new(class, id, [keyExprs])` direct calls lack the metadata
        // slots and surface as 2- or 3-arg calls — detect via the shape
        // of position 2.
        let mut type_args: Vec<Value> = Vec::new();
        let mut type_var_values: Vec<Value> = Vec::new();
        let mut kvs_offset = values.len(); // default: no kvs
        if values.len() >= 3 {
            // Probe whether position 2 looks like the compiler-emitted
            // type-args slot (Unit, single Element, or Collection of
            // Elements only) — anything else is a Pure-source call shape
            // and the metadata slots aren't there.
            let pos2_is_type_args = match &values[2] {
                Value::Unit | Value::Element(_) => true,
                Value::Collection(coll) => coll.iter().all(|v| matches!(v, Value::Element(_))),
                _ => false,
            };
            if pos2_is_type_args {
                match &values[2] {
                    Value::Collection(coll) => type_args = coll.iter().cloned().collect(),
                    Value::Element(_) => type_args = vec![values[2].clone()],
                    _ => {}
                }
                if values.len() >= 4 {
                    match &values[3] {
                        Value::Unit => {}
                        Value::Collection(coll) => {
                            type_var_values = coll.iter().cloned().collect();
                        }
                        other => {
                            type_var_values = vec![other.clone()];
                        }
                    }
                    kvs_offset = 4;
                } else {
                    kvs_offset = 3;
                }
            } else if matches!(&values[2], Value::Collection(_)) {
                // Pure-source `new(class, id, [keyExprs])` — KeyExpression
                // hydration is a separate backlog item. Skip the slot.
                kvs_offset = values.len();
            } else {
                // Old-style flat key/value pairs starting at position 2 —
                // compiler no longer emits this shape, but tolerate it.
                kvs_offset = 2;
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
        if !type_var_values.is_empty() {
            ctx.heap_mut()
                .mutate_set(obj, "__typeVariableValues", &type_var_values)?;
        }
        apply_key_value_triples(ctx, obj, &values[kvs_offset..])?;
        populate_association_inverses(ctx, obj, class_id, &values[kvs_offset..])?;
        evaluate_class_constraints(ctx, class_id, obj, &type_var_values)?;
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
        //
        // Java parity: per `legend-pure-runtime-java-engine-interpreted/.../
        // natives/grammar/lang/creation/Copy.java:196-237`, the Java Copy
        // walks `class_getSimplePropertiesByName(classifier)` and for every
        // property NOT being overridden by the caller, copies the source's
        // values onto the new instance via `addValueToProperty(...)` —
        // shallow (shared inner references), eager (no lazy delegation).
        // The literal Java comment is `// The new instance doesn't have
        // the value ... Add (not deep!)`. We mirror that here by walking
        // the metatype + supertypes' property lists, evaluating each
        // against the source via the same property-access path the user
        // would hit via `$f1.<prop>`, and storing the result on the heap.
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
            hydrate_element_to_heap(ctx, *elem_id, meta_id, obj)?;
            apply_key_value_triples(ctx, obj, &values[1..])?;
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
                i += 3;
            }
            set
        };

        // Flatten carried-over properties into a
        // `[key1, val1, augmented1=false, key2, val2, augmented2=false, …]`
        // slice compatible with `populate_association_inverses` — the copy
        // should appear in every association inverse its source belonged
        // to (e.g. a copied Person is appended to `firm.employees` so
        // `assertSameElements([$bob, $pierre], $firmX.employees)` holds).
        // Carried values are unconditionally `=` semantics (we already used
        // `mutate_add` above to seed the slot from the source); the false
        // flag here only matters for the inverse walk's stride.
        // Skip any property the user is path-overriding; the inverse for
        // those is established by the path-property handler against the
        // freshly-cloned nested object.
        let mut carried_kvs: Vec<Value> = Vec::with_capacity(original_props.len() * 3);
        for (name, prop_values) in &original_props {
            ctx.heap_mut().mutate_add(obj, name.as_str(), prop_values)?;
            if path_overrides.contains(name) {
                continue;
            }
            carried_kvs.push(Value::String(name.clone()));
            carried_kvs.push(Value::from_vec(prop_values.clone()));
            carried_kvs.push(Value::Boolean(false));
        }

        apply_key_value_triples(ctx, obj, &plain_kvs)?;
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
                let synthetic = vec![Value::String(head.clone()), assigned, Value::Boolean(false)];
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

        // Overlay caller-supplied bindings. Build a flat
        // `[k, v, augmented=false, …]` triple slice so we can reuse
        // `populate_association_inverses` for bidirectional-association
        // maintenance. The augmented flag is `false` because Java's
        // `KeyValue` shape used by `dynamicNew` doesn't expose `add` to
        // the caller — every overlay is replace-semantics here.
        let mut assoc_kvs: Vec<Value> = Vec::with_capacity(supplied.len() * 3);
        for (key, flat) in supplied {
            ctx.heap_mut().mutate_set(obj, key.as_str(), &flat)?;
            assoc_kvs.push(Value::String(key));
            assoc_kvs.push(Value::from_vec(flat));
            assoc_kvs.push(Value::Boolean(false));
        }
        populate_association_inverses(ctx, obj, class_id, &assoc_kvs)?;

        // Override-bearing overloads land here as
        // `dynamicNew(class|gt, kvs, getterToOne, getterToMany,
        // hiddenPayload [, constraintsManager])`. Allocate a
        // `GetterOverride` heap wrapper carrying every non-empty
        // hook + payload, then bind it to the new instance's
        // `elementOverride` slot so `eval_property_access` can
        // intercept absent property reads via the lambdas. Mirrors
        // Java Pure's `GetterOverride` metamodel object (m3.pure
        // `meta::pure::metamodel::type::GetterOverride`).
        if values.len() >= 5 {
            let getter_to_one = &values[2];
            let getter_to_many = &values[3];
            let hidden_payload = &values[4];
            let any_set = !matches!(getter_to_one, Value::Unit)
                || !matches!(getter_to_many, Value::Unit)
                || !matches!(hidden_payload, Value::Unit);
            if any_set {
                let override_obj = ctx
                    .heap_mut()
                    .alloc_dynamic(crate::m3_paths::GETTER_OVERRIDE);
                if !matches!(getter_to_one, Value::Unit) {
                    ctx.heap_mut().mutate_add(
                        override_obj,
                        "getterOverrideToOne",
                        &[getter_to_one.clone()],
                    )?;
                }
                if !matches!(getter_to_many, Value::Unit) {
                    ctx.heap_mut().mutate_add(
                        override_obj,
                        "getterOverrideToMany",
                        &[getter_to_many.clone()],
                    )?;
                }
                if !matches!(hidden_payload, Value::Unit) {
                    ctx.heap_mut().mutate_add(
                        override_obj,
                        "hiddenPayload",
                        &[hidden_payload.clone()],
                    )?;
                }
                ctx.heap_mut().mutate_add(
                    obj,
                    "elementOverride",
                    &[Value::Object(override_obj)],
                )?;
            }
        }

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

/// Detect the `(expressionSequence, <single Function>, augmented)` triple
/// emitted by `^LambdaFunction(expressionSequence = $fn.expressionSequence)`.
///
/// Returns the underlying `Value::Function` when the shortcut applies,
/// so `New` can return it directly instead of allocating a heap object.
/// The augmented flag is ignored — the shortcut applies only when the
/// triple list has exactly one entry, so there is no carried value to
/// distinguish `=` from `+=` against.
fn try_lambda_shortcut(kvs: &[Value]) -> Option<Value> {
    if kvs.len() != 3 {
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
    if kvs.len() % 3 != 0 {
        return Ok(()); // Arity already validated by apply_key_value_triples
    }

    // Snapshot the association entries once — cheap O(n) per property but we
    // avoid re-borrowing the model mid-loop.
    let entries: Vec<(ElementId, usize)> = ctx.model().association_properties(class_id).to_vec();
    if entries.is_empty() {
        return Ok(());
    }

    // For each forward assignment key, find the matching injected property
    // and its inverse, then mutate the target object's inverse slot. The
    // augmented flag at slot `i+2` is irrelevant to inverse semantics —
    // whether the user wrote `firm = $f` or `firm += $f`, the same
    // reverse-link must be established.
    let mut i = 0;
    while i < kvs.len() {
        let Value::String(key) = &kvs[i] else {
            i += 3;
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
        i += 3;
    }
    Ok(())
}

/// Eager shallow hydration of every M3 reflective property of `meta_id`
/// (and its supertypes) onto the heap object `obj` from the source
/// `Value::Element(elem_id)`.
///
/// Mirrors Java Pure's `Copy.copy(...)` behaviour for Element-source
/// constructions (`^$f1()`): walk every property declared on the
/// classifier's M3 metatype and any ancestor up to `Any`, evaluate the
/// property against the source element via the same property-access
/// path the user would hit (`$f1.<prop>`), and store the resulting
/// values on the new heap object. Empty results (`Value::Unit`) are
/// skipped — the heap returns empty for unset slots anyway, so storing
/// `Unit` is a no-op while still leaving room for the user's
/// subsequent `apply_key_value_triples` overrides to set a real value.
///
/// This delegation through `ctx.evaluate` (rather than re-implementing
/// `eval_element_property` here) means any future Element-side property
/// the runtime learns to compute (e.g. `parameters` once we wire it up
/// from `Function.parameters`) automatically flows into the copy
/// without further changes to this helper.
#[allow(clippy::result_large_err)]
fn hydrate_element_to_heap(
    ctx: &mut dyn EvalContextTrait,
    elem_id: ElementId,
    meta_id: ElementId,
    obj: ObjectId,
) -> Result<(), PureException> {
    // Collect every (de-duplicated, declaration-order) property name
    // declared on the metatype Class or any of its supertypes. Walks
    // the supertype chain breadth-first so subclass redeclarations
    // shadow parent ones at the same name.
    let mut prop_names: Vec<SmolStr> = Vec::new();
    let mut seen: std::collections::HashSet<SmolStr> = std::collections::HashSet::new();
    let mut visited: std::collections::HashSet<ElementId> = std::collections::HashSet::new();
    let mut stack: Vec<ElementId> = vec![meta_id];
    while let Some(cid) = stack.pop() {
        if !visited.insert(cid) {
            continue;
        }
        let Element::Class(c) = ctx.model().get_element(cid) else {
            continue;
        };
        for p in &c.properties {
            if seen.insert(p.name.clone()) {
                prop_names.push(p.name.clone());
            }
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                stack.push(*element);
            }
        }
    }

    // For each property, build a synthetic `<elem>.<prop>` access spec
    // and evaluate it under the current scope. This routes through the
    // evaluator's `eval_element_property` / `eval_function_property` so
    // we don't duplicate that dispatch table here.
    for prop in prop_names {
        let target_spec = ValueSpec {
            kind: Box::new(ExprKind::PackageableElementRef { element: elem_id }),
            source_info: SourceInfo::new("<copy-hydrate>", 0, 0, 0, 0),
            type_info: None,
        };
        let access_spec = ValueSpec {
            kind: Box::new(ExprKind::PropertyAccess {
                target: Box::new(target_spec),
                property: prop.clone(),
            }),
            source_info: SourceInfo::new("<copy-hydrate>", 0, 0, 0, 0),
            type_info: None,
        };
        // Tolerate per-property errors — if the runtime can't compute
        // a particular slot on this element kind, treat it as empty
        // (the heap default) rather than failing the whole copy.
        let value = match ctx.evaluate(&access_spec) {
            Ok(v) => v.into_value(),
            Err(_) => continue,
        };
        let values: Vec<Value> = match value {
            Value::Unit => continue,
            Value::Collection(coll) => coll.iter().cloned().collect(),
            other => vec![other],
        };
        if values.is_empty() {
            continue;
        }
        ctx.heap_mut().mutate_set(obj, prop.as_str(), &values)?;
    }
    Ok(())
}

/// Walk a `[key1, val1, augmented1, key2, val2, augmented2, ...]` slice
/// and apply each triple to `obj`.
///
/// On `augmented == false` (the `=` form) uses `mutate_set` (replace);
/// on `augmented == true` (the `+=` form) uses `mutate_add` (append to
/// any carried-over values). `Copy` pre-populates from the source and
/// relies on `=` to reset the slot so the override doesn't leak the
/// original value, while `+=` is the only way to extend a carried list
/// without restating its prior contents. `New` always allocates a fresh
/// slot, so the distinction is moot at construction time but the
/// compiler still threads the flag through for symmetry.
fn apply_key_value_triples(
    ctx: &mut dyn EvalContextTrait,
    obj: ObjectId,
    kvs: &[Value],
) -> Result<(), PureException> {
    if kvs.len() % 3 != 0 {
        return Err(PureRuntimeError::EvaluationError(format!(
            "object construction: expected (key, value, augmented) triples, got {} extra arguments",
            kvs.len()
        ))
        .into());
    }
    let mut i = 0;
    while i < kvs.len() {
        let key = kvs[i].as_string()?.clone();
        let value = kvs[i + 1].clone();
        let augmented = matches!(&kvs[i + 2], Value::Boolean(true));
        let values: Vec<Value> = match value {
            Value::Collection(coll) => coll.iter().cloned().collect(),
            Value::Unit => Vec::new(),
            other => vec![other],
        };
        if augmented {
            ctx.heap_mut().mutate_add(obj, key.as_str(), &values)?;
        } else {
            ctx.heap_mut().mutate_set(obj, key.as_str(), &values)?;
        }
        i += 3;
    }
    Ok(())
}

/// Split a `[key1, val1, augmented1, key2, val2, augmented2, ...]` slice
/// into two parallel triple slices: the plain keys (no `.`) and the
/// dotted-path keys (`address.name`, `firm.legalName`). Both keep the
/// same triple shape so `apply_key_value_triples` and
/// `apply_path_property_updates` can consume them uniformly.
#[allow(clippy::result_large_err)]
fn partition_path_kvs(kvs: &[Value]) -> Result<(Vec<Value>, Vec<Value>), PureException> {
    if kvs.len() % 3 != 0 {
        return Err(PureRuntimeError::EvaluationError(format!(
            "copy: expected (key, value, augmented) triples, got {} extra arguments",
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
        bucket.push(kvs[i + 2].clone());
        i += 3;
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

    // Group `(first_segment → [(rest_path, value, augmented), …])` so
    // multiple updates against the same first segment share a single
    // clone. The augmented flag flows through to whichever bucket the
    // leaf eventually lands in (`mutate_add` for `+=`, `mutate_set` for
    // `=`).
    let mut groups: BTreeMap<SmolStr, Vec<(SmolStr, Value, bool)>> = BTreeMap::new();
    let mut i = 0;
    while i < kvs.len() {
        let key = kvs[i].as_string()?.clone();
        let value = kvs[i + 1].clone();
        let augmented = matches!(&kvs[i + 2], Value::Boolean(true));
        let (head, tail) = match key.split_once('.') {
            Some((h, t)) => (SmolStr::new(h), SmolStr::new(t)),
            None => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "copy: expected dotted-path key, got '{key}'"
                ))
                .into());
            }
        };
        groups
            .entry(head)
            .or_default()
            .push((tail, value, augmented));
        i += 3;
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
            for (rest, val, augmented) in &updates {
                let bucket = if rest.contains('.') {
                    &mut nested_path_kvs
                } else {
                    &mut leaf_kvs
                };
                bucket.push(Value::String(rest.clone()));
                bucket.push(val.clone());
                bucket.push(Value::Boolean(*augmented));
            }
            apply_key_value_triples(ctx, clone_id, &leaf_kvs)?;
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

/// Run every constraint declared on the given Class against the
/// freshly-constructed instance. Mirrors `Cast`'s primitive-constraint
/// path: bind `$this` to the instance and each
/// `type_variable_parameter` to its `^Class(value)(props)` argument,
/// evaluate the constraint body, raise `ConstraintViolation` on the
/// first failure with the canonical message shape.
///
/// Inheritance walking is intentionally *not* threaded through —
/// constraints declared on parent classes are not yet evaluated. The
/// remaining failing platform tests (testNewWithConstraintExtended /
/// similar) require the same chain logic the primitive path has;
/// tracked under the constraint-runtime backlog item.
#[allow(clippy::result_large_err)]
fn evaluate_class_constraints(
    ctx: &mut dyn EvalContextTrait,
    class_id: ElementId,
    obj: ObjectId,
    type_var_values: &[Value],
) -> Result<(), PureException> {
    let (constraints, param_names, class_name) = match ctx.model().get_element(class_id) {
        Element::Class(class) if !class.constraints.is_empty() => {
            let names: Vec<SmolStr> = class
                .type_variable_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            (
                class.constraints.clone(),
                names,
                ctx.model().element_name(class_id).clone(),
            )
        }
        _ => return Ok(()),
    };
    ctx.context_mut().push_scope();
    ctx.context_mut()
        .set(SmolStr::new_static("this"), Value::Object(obj));
    for (name, value) in param_names.iter().zip(type_var_values.iter()) {
        ctx.context_mut().set(name.clone(), value.clone());
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
            class_name.clone(),
            message,
            constraint.source_info.clone(),
            Vec::new(),
        ));
        break;
    }
    ctx.context_mut().pop_scope();
    match first_failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
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
// getHiddenPayload — read the GetterOverride wrapper's payload slot
// ---------------------------------------------------------------------------

/// Pure `getHiddenPayload(o:Any[1]):Any[0..1]`
///
/// Reads `o.elementOverride.hiddenPayload` — the opaque slot
/// `dynamicNew(…, hiddenPayload)` parks for the override lambdas to
/// pull during property dispatch (test pattern:
/// `^D_D(name = $o->cast(@D_A).a + $o->getHiddenPayload()->cast(@String)->toOne())`).
/// Returns `Value::Unit` when the receiver has no `elementOverride`
/// (i.e., wasn't constructed via the hook-bearing `dynamicNew`
/// overloads) or the payload is unset.
#[derive(Debug)]
pub struct GetHiddenPayload;

impl NativeFunction for GetHiddenPayload {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("getHiddenPayload", &values, 1)?;
        let Value::Object(obj_id) = &values[0] else {
            return Ok(Evaluated::new(Value::Unit));
        };
        let override_vals = ctx.heap().get_property_values(*obj_id, "elementOverride")?;
        let Some(Value::Object(override_id)) = override_vals.iter().next().cloned() else {
            return Ok(Evaluated::new(Value::Unit));
        };
        let payload = ctx
            .heap()
            .get_property_values(override_id, "hiddenPayload")?;
        let collected: Vec<Value> = payload.iter().cloned().collect();
        Ok(Evaluated::new(Value::from_vec(collected)))
    }

    fn signature(&self) -> &'static str {
        "getHiddenPayload(o:Any[1]):Any[0..1]"
    }
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
    registry.register("getHiddenPayload_Any_1__Any_1_", GetHiddenPayload);
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
