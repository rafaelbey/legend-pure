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
            && let Some(fn_val) = try_lambda_shortcut(&values[2..])
        {
            return Ok(Evaluated::new(fn_val));
        }

        let obj = ctx.heap_mut().alloc_dynamic(classifier);
        apply_key_value_pairs(ctx, obj, &values[2..])?;
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

        let obj = ctx.heap_mut().alloc_dynamic(classifier);
        for (name, prop_values) in original_props {
            ctx.heap_mut()
                .mutate_add(obj, name.as_str(), &prop_values)?;
        }

        apply_key_value_pairs(ctx, obj, &values[1..])?;
        Ok(Evaluated::new(Value::Object(obj)))
    }

    fn signature(&self) -> &'static str {
        "copy<T>(source:T[1], keyExpressions:KeyExpression[*]):T[1]"
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

/// Walk a flat `[key1, val1, key2, val2, ...]` slice and `mutate_add` each
/// pair onto `obj`. Collections are expanded so property storage stays flat.
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
        ctx.heap_mut().mutate_add(obj, key.as_str(), &values)?;
        i += 2;
    }
    Ok(())
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
