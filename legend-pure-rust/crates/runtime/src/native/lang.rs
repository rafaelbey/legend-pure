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
use smol_str::SmolStr;

use crate::error::PureRuntimeError;
use crate::heap::ObjectId;
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("letFunction", args, 2)?;
        let name = args[0].as_string()?;
        let value = args[1].clone();
        ctx.context_mut().set(name.clone(), value.clone());
        Ok(value)
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
/// Short-circuiting conditional. Uses `defer_execution()` so the evaluator
/// wraps both branches as zero-parameter lambdas. Only the taken branch
/// is evaluated.
#[derive(Debug)]
pub struct If;

impl NativeFunction for If {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("if", args, 3)?;

        // With defer_execution, arg[0] is a lambda wrapping the boolean condition.
        // Evaluate it to get the actual boolean.
        let condition_val = ctx.eval_lambda(&args[0], &[])?;
        let condition = condition_val.as_boolean()?;

        if condition {
            ctx.eval_lambda(&args[1], &[])
        } else {
            ctx.eval_lambda(&args[2], &[])
        }
    }

    fn signature(&self) -> &'static str {
        "if(Boolean[1], Function<{->T[m]}>[1], Function<{->T[m]}>[1]): T[m]"
    }

    fn defer_execution(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// eval — call a function value with 0-7 arguments
// ---------------------------------------------------------------------------

/// Pure `eval(func:Function<{...->V[m]}>[1], ...params):V[m]`
///
/// All 8 platform `eval` overloads (0–7 extra parameters) share this single
/// implementation: extract `args[0]` as the callable and pass `args[1..]` as
/// the arguments via `ctx.eval_lambda`, which handles both `Lambda` and `FunctionRef`.
#[derive(Debug)]
pub struct Eval;

impl NativeFunction for Eval {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        if args.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "eval: expected at least 1 argument (the function)".into(),
            ));
        }
        let func = &args[0];
        let params = &args[1..];
        ctx.eval_lambda(func, params)
    }

    fn signature(&self) -> &'static str {
        "eval(Function[1], ...): Any[*]"
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("print", args, 2)?;
        match &args[0] {
            Value::Collection(v) => {
                for item in v.iter() {
                    ctx.console_output(&format!("{item}"));
                }
            }
            Value::Unit => {}
            other => ctx.console_output(&format!("{other}")),
        }
        Ok(Value::Unit)
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        if args.len() < 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "new: expected at least 2 arguments (class, id), got {}",
                args.len()
            )));
        }
        let class_id = match &args[0] {
            Value::Element(id) => *id,
            other => {
                return Err(PureRuntimeError::type_mismatch("Class", other));
            }
        };
        let _id = &args[1]; // ignored — Pure's Java impl uses this only as a debug label
        let classifier = class_fqn(ctx.model(), class_id);

        let obj = ctx.heap_mut().alloc_dynamic(classifier);
        apply_key_value_pairs(ctx, obj, &args[2..])?;
        Ok(Value::Object(obj))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        if args.is_empty() {
            return Err(PureRuntimeError::EvaluationError(
                "copy: expected at least 1 argument (source)".into(),
            ));
        }
        let source_id = args[0].as_object()?;

        // Snapshot classifier + properties before we take a mutable heap borrow.
        let classifier: String = ctx.heap().classifier(source_id)?.to_owned();
        let names = ctx.heap().property_names(source_id)?;
        let mut original_props: Vec<(SmolStr, Vec<Value>)> = Vec::with_capacity(names.len());
        for name in names {
            let values: Vec<Value> = ctx
                .heap()
                .get_property_values(source_id, name.as_str())?
                .iter()
                .cloned()
                .collect();
            original_props.push((name, values));
        }

        let obj = ctx.heap_mut().alloc_dynamic(classifier);
        for (name, values) in original_props {
            ctx.heap_mut().mutate_add(obj, name.as_str(), &values)?;
        }

        apply_key_value_pairs(ctx, obj, &args[1..])?;
        Ok(Value::Object(obj))
    }

    fn signature(&self) -> &'static str {
        "copy<T>(source:T[1], keyExpressions:KeyExpression[*]):T[1]"
    }
}

/// Walk a flat `[key1, val1, key2, val2, ...]` slice and `mutate_add` each
/// pair onto `obj`. Collections are expanded so property storage stays flat.
fn apply_key_value_pairs(
    ctx: &mut dyn EvalContextTrait,
    obj: ObjectId,
    kvs: &[Value],
) -> Result<(), PureRuntimeError> {
    if kvs.len() % 2 != 0 {
        return Err(PureRuntimeError::EvaluationError(format!(
            "object construction: expected key/value pairs, got {} extra arguments",
            kvs.len()
        )));
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
}
