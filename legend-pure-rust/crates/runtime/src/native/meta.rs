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

use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::TypeExpr;
use smol_str::SmolStr;

use crate::date::DatePrecision;
use crate::error::PureRuntimeError;
use crate::heap::{ObjectId, RuntimeHeap};
use crate::native::{EvalContextTrait, NativeFunction, NativeRegistry, expect_args};
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("pathToElement", args, 2)?;
        let path = args[0].as_string()?;
        let separator = args[1].as_string()?;
        match resolve_path(ctx.model(), path.as_str(), separator.as_str()) {
            Some(id) => Ok(Value::Element(id)),
            None => Err(PureRuntimeError::EvaluationError(format!(
                "pathToElement: path not found: '{path}'"
            ))),
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("lenientPathToElement", args, 2)?;
        let path = args[0].as_string()?;
        let separator = args[1].as_string()?;
        match resolve_path(ctx.model(), path.as_str(), separator.as_str()) {
            Some(id) => Ok(Value::Element(id)),
            None => Ok(Value::Unit),
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("elementToPath", args, 3)?;
        let separator = args[1].as_string()?;
        let include_root = args[2].as_boolean()?;
        let path = match &args[0] {
            Value::Element(id) => {
                build_element_path(ctx.model(), *id, separator.as_str(), include_root)
            }
            Value::Object(obj_id) => {
                build_ephemeral_path(ctx.heap(), *obj_id, separator.as_str(), include_root)
            }
            other => return Err(PureRuntimeError::type_mismatch("PackageableElement", other)),
        };
        Ok(Value::String(SmolStr::new(path)))
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
/// Returns a SourceInformation heap object for a model-element reference
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("sourceInformation", args, 1)?;
        let Value::Element(id) = &args[0] else {
            return Ok(Value::Unit);
        };
        // Package elements have no ElementNode — surface that as "no info".
        let ElementId::InstanceId { .. } = id else {
            return Ok(Value::Unit);
        };
        let node = ctx.model().get_node(*id);
        let source = node.source_info.clone();
        let obj = ctx
            .heap_mut()
            .alloc_dynamic("meta::pure::functions::meta::SourceInformation");
        let heap = ctx.heap_mut();
        heap.mutate_add(obj, "source", &[Value::String(source.source.clone())])?;
        heap.mutate_add(
            obj,
            "startLine",
            &[Value::Integer(i64::from(source.start_line))],
        )?;
        heap.mutate_add(
            obj,
            "startColumn",
            &[Value::Integer(i64::from(source.start_column))],
        )?;
        heap.mutate_add(obj, "line", &[Value::Integer(i64::from(source.start_line))])?;
        heap.mutate_add(
            obj,
            "column",
            &[Value::Integer(i64::from(source.start_column))],
        )?;
        heap.mutate_add(
            obj,
            "endLine",
            &[Value::Integer(i64::from(source.end_line))],
        )?;
        heap.mutate_add(
            obj,
            "endColumn",
            &[Value::Integer(i64::from(source.end_column))],
        )?;
        Ok(Value::Object(obj))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("instanceOf", args, 2)?;
        let subject = &args[0];
        let type_id = as_element_id(&args[1])?;
        Ok(Value::Boolean(value_matches_type(
            ctx.model(),
            subject,
            type_id,
            ctx.heap(),
        )))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("cast", args, 2)?;
        let subject = args[0].clone();
        let type_id = as_element_id(&args[1])?;
        let type_name = ctx.model().element_name(type_id).to_string();
        let check = |v: &Value| value_matches_type(ctx.model(), v, type_id, ctx.heap());
        let mk_err = |v: &Value| {
            PureRuntimeError::EvaluationError(format!(
                "cast: value of type {} is not an instance of {type_name}",
                v.type_name()
            ))
        };
        match &subject {
            Value::Collection(coll) => {
                for v in coll.iter() {
                    if !check(v) {
                        return Err(mk_err(v));
                    }
                }
                Ok(subject)
            }
            Value::Unit => Ok(Value::Unit),
            other => {
                if check(other) {
                    Ok(subject)
                } else {
                    Err(mk_err(other))
                }
            }
        }
    }

    fn signature(&self) -> &'static str {
        "cast<V|m>(p:Any[m], typ:V[1]):V[m]"
    }
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("match", args, 2)?;
        let subject = args[0].clone();
        let functions = args[1].to_collection();

        for func_val in functions.iter() {
            let Value::Function(fv) = func_val else {
                continue;
            };
            let FunctionValue::Lambda(lambda) = fv.as_ref() else {
                continue;
            };

            // Zero-arg lambda is a catch-all branch.
            if lambda.parameters.is_empty() {
                return ctx.eval_lambda(func_val, &[]);
            }

            let param = &lambda.parameters[0];
            let is_match = match &param.type_expr {
                TypeExpr::Named {
                    element: type_class_id,
                    ..
                } => value_matches_type(ctx.model(), &subject, *type_class_id, ctx.heap()),
                // Unconstrained generic parameter → always matches.
                TypeExpr::Generic(_) => true,
                _ => false,
            };

            if is_match {
                return ctx.eval_lambda(func_val, &[subject]);
            }
        }

        Err(PureRuntimeError::EvaluationError(format!(
            "match: no branch matched for value: {subject}"
        )))
    }

    fn signature(&self) -> &'static str {
        "match<T|m,n>(var:Any[*], functions:Function<{Nil[n]->T[m]}>[1..*]):T[m]"
    }
}

// ---------------------------------------------------------------------------
// id
// ---------------------------------------------------------------------------

/// Pure `id(Any[1]):String[1]`
///
/// Returns an identity string for any value. Heap objects are identified by
/// `Anonymous_<ObjectId>` — matching the Java Pure runtime's convention for
/// anonymous instances. Primitives render their canonical textual form
/// (unquoted for strings). Model-element references render their fully
/// qualified path.
#[derive(Debug)]
pub struct Id;

impl NativeFunction for Id {
    fn execute(
        &self,
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("id", args, 1)?;
        let s = render_id(&args[0], ctx.model());
        Ok(Value::String(SmolStr::new(s)))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("type", args, 1)?;
        let type_id = resolve_value_type(&args[0], ctx.model(), ctx.heap())?;
        Ok(Value::Element(type_id))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("genericType", args, 1)?;
        let type_id = resolve_value_type(&args[0], ctx.model(), ctx.heap())?;
        let obj = ctx
            .heap_mut()
            .alloc_dynamic("meta::pure::metamodel::type::generics::GenericType");
        ctx.heap_mut()
            .mutate_add(obj, "rawType", &[Value::Element(type_id)])?;
        Ok(Value::Object(obj))
    }

    fn signature(&self) -> &'static str {
        "genericType(Any[1]):GenericType[1]"
    }
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("rawType", args, 1)?;
        let Value::Object(obj_id) = &args[0] else {
            return Err(PureRuntimeError::type_mismatch("GenericType", &args[0]));
        };
        let values = ctx.heap().get_property_values(*obj_id, "rawType")?;
        match values.head() {
            Some(v) => Ok(v.clone()),
            None => Ok(Value::Unit),
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("enumName", args, 1)?;
        let id = as_element_id(&args[0])?;
        match ctx.model().get_element(id) {
            Element::Enumeration(_) => Ok(Value::String(ctx.model().element_name(id).clone())),
            _ => Err(PureRuntimeError::type_mismatch("Enumeration", &args[0])),
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("enumValues", args, 1)?;
        let id = as_element_id(&args[0])?;
        let Element::Enumeration(enum_def) = ctx.model().get_element(id) else {
            return Err(PureRuntimeError::type_mismatch("Enumeration", &args[0]));
        };
        let simple = ctx.model().element_name(id).clone();
        let values: Vec<Value> = enum_def
            .values
            .iter()
            .map(|v| Value::String(SmolStr::new(format!("{simple}.{}", v.name))))
            .collect();
        Ok(Value::from_vec(values))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("toRepresentation", args, 1)?;
        let s = render_representation(&args[0], ctx.model(), ctx.heap());
        Ok(Value::String(SmolStr::new(s)))
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
        args: &[Value],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Value, PureRuntimeError> {
        expect_args("subTypeOf", args, 2)?;
        let child = as_element_id(&args[0])?;
        let parent = as_element_id(&args[1])?;
        Ok(Value::Boolean(is_sub_type_of(child, parent, ctx.model())))
    }

    fn signature(&self) -> &'static str {
        "subTypeOf(Type[1], Type[1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract an `ElementId` from a [`Value::Element`].
fn as_element_id(v: &Value) -> Result<ElementId, PureRuntimeError> {
    match v {
        Value::Element(id) => Ok(*id),
        other => Err(PureRuntimeError::type_mismatch("PackageableElement", other)),
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
    obj_id: ObjectId,
    separator: &str,
    include_root: bool,
) -> String {
    let name_of = |oid: ObjectId| -> Option<SmolStr> {
        let values = heap.get_property_values(oid, "name").ok()?;
        values.iter().next().and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    };
    let package_of = |oid: ObjectId| -> Option<ObjectId> {
        let values = heap.get_property_values(oid, "package").ok()?;
        values.iter().next().and_then(|v| match v {
            Value::Object(p) => Some(*p),
            _ => None,
        })
    };

    // Leaf name — empty string if the ephemeral has no name at all.
    let Some(leaf_name) = name_of(obj_id) else {
        return String::new();
    };

    // Walk upward collecting names. The outermost package (one with no
    // `package` property) is treated as the "root" marker.
    let mut chain: Vec<SmolStr> = vec![leaf_name];
    let mut cursor = package_of(obj_id);
    let mut outermost_root_name: Option<SmolStr> = None;
    while let Some(pkg_id) = cursor {
        let next = package_of(pkg_id);
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
            // Fallback: heap-object classifier prefix/suffix match against the
            // type class's simple name. Conservative — only covers exact name
            // match at the end of the classifier FQN.
            if let Value::Object(obj_id) = value
                && let Ok(classifier) = heap.classifier(*obj_id)
            {
                return classifier
                    .rsplit("::")
                    .next()
                    .is_some_and(|tail| tail == type_name.as_str());
            }
            false
        }
    }
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
) -> Result<ElementId, PureRuntimeError> {
    match value {
        Value::Boolean(_) => Ok(bootstrap::BOOLEAN_ID),
        Value::Integer(_) => Ok(bootstrap::INTEGER_ID),
        Value::Float(_) => Ok(bootstrap::FLOAT_ID),
        Value::Decimal(_) => Ok(bootstrap::DECIMAL_ID),
        Value::String(_) => Ok(bootstrap::STRING_ID),
        Value::Date(d) => Ok(match d.precision() {
            DatePrecision::Day => bootstrap::STRICT_DATE_ID,
            DatePrecision::Time(_) => bootstrap::DATE_TIME_ID,
            // Year / Month precision — no finer bootstrap ID, classify as
            // the abstract `Date`.
            _ => bootstrap::DATE_ID,
        }),
        Value::StrictTime(_) => Ok(bootstrap::STRICT_TIME_ID),
        Value::Element(id) => Ok(*id),
        Value::Object(obj_id) => {
            let classifier = heap.classifier(*obj_id)?;
            let segments: Vec<SmolStr> = if classifier.is_empty() {
                Vec::new()
            } else {
                classifier.split("::").map(SmolStr::new).collect()
            };
            model.resolve_by_path(&segments).ok_or_else(|| {
                PureRuntimeError::EvaluationError(format!(
                    "type: classifier '{classifier}' does not resolve to a known Type"
                ))
            })
        }
        // Collections / maps / functions / unit — no reified runtime type,
        // classify as `Any`.
        Value::Collection(_) | Value::Map(_) | Value::Function(_) | Value::Unit => {
            Ok(bootstrap::ANY_ID)
        }
    }
}

/// Render the identity string for a value — powers the `id` native.
fn render_id(value: &Value, model: &PureModel) -> String {
    match value {
        Value::Object(obj_id) => format!("Anonymous_{obj_id}"),
        Value::Element(id) => build_element_path(model, *id, "::", false),
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

/// Render a value as a Pure-source-like debug representation —
/// powers the `toRepresentation` native.
fn render_representation(value: &Value, model: &PureModel, heap: &RuntimeHeap) -> String {
    match value {
        Value::String(s) => {
            let escaped = s.replace('\'', "\\'");
            format!("'{escaped}'")
        }
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Date(d) => format!("%{d}"),
        Value::StrictTime(t) => format!("%{t}"),
        Value::Element(id) => build_element_path(model, *id, "::", false),
        Value::Object(obj_id) => {
            let classifier = heap.classifier(*obj_id).unwrap_or("Object");
            format!("<{classifier} Anonymous_{obj_id}>")
        }
        Value::Collection(v) => {
            let parts: Vec<String> = v
                .iter()
                .map(|item| render_representation(item, model, heap))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Unit => "[]".to_string(),
        Value::Map(m) => format!("<Map size={}>", m.len()),
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Lambda(_) => "<Lambda>".to_string(),
            FunctionValue::Compiled(id) => format!("<Function:{id}>"),
        },
    }
}

/// Whether `child` is a subtype of (or equal to) `parent`.
///
/// Mirrors `crate::pure::resolve::is_subtype` — duplicated here because the
/// pure-crate function is `pub(crate)` and we cannot edit that crate.
fn is_sub_type_of(child: ElementId, parent: ElementId, model: &PureModel) -> bool {
    if child == parent {
        return true;
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
    registry.register("instanceOf_Any_1__Type_1__Boolean_1_", InstanceOf);
    registry.register("cast_Any_m__V_1__V_m_", Cast);
    // `match` is generic — the compiler-produced mangled name depends on
    // `T`/`m`/`n` bindings at the call site. Register under a key that starts
    // with `"match_"` so the runtime's simple-name prefix fallback finds it
    // when the exact-FQN lookup misses. See [`NativeRegistry::find_by_prefix`].
    registry.register("match_Any_MANY__Function_$1_MANY$__T_m_", Match);
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
}
