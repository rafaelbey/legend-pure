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
use legend_pure_parser_pure::types::{TypeExpr, ValueSpec};
use smol_str::SmolStr;

use crate::date::DatePrecision;
use crate::error::{PureException, PureRuntimeError};
use crate::heap::{ObjectId, RuntimeHeap};
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
                build_ephemeral_path(ctx.heap(), *obj_id, separator.as_str(), include_root)
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("sourceInformation", &values, 1)?;
        let Value::Element(id) = &values[0] else {
            return Ok(Evaluated::new(Value::Unit));
        };
        // Package elements have no ElementNode — surface that as "no info".
        let ElementId::InstanceId { .. } = id else {
            return Ok(Evaluated::new(Value::Unit));
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
        let check = |v: &Value| cast_compatible(v, type_id, ctx.model(), ctx.heap());
        match &subject {
            Value::Collection(coll) => {
                for v in coll.iter() {
                    if !check(v) {
                        return Err(mk_err(v));
                    }
                }
                Ok(Evaluated::new(subject))
            }
            Value::Unit => Ok(Evaluated::new(Value::Unit)),
            other => {
                if check(other) {
                    Ok(Evaluated::new(subject))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("match", &values, 2)?;
        let subject = values[0].clone();
        let functions = values[1].to_collection();

        for func_val in functions.iter() {
            let Value::Function(fv) = func_val else {
                continue;
            };
            let FunctionValue::Lambda(lambda) = fv.as_ref() else {
                continue;
            };

            // Zero-arg lambda is a catch-all branch.
            if lambda.parameters.is_empty() {
                return ctx.call_function(func_val, &[]).map(Evaluated::new);
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
                return ctx.call_function(func_val, &[subject]).map(Evaluated::new);
            }
        }

        Err(PureRuntimeError::EvaluationError(format!(
            "match: no branch matched for value: {subject}"
        ))
        .into())
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
        let type_id = resolve_value_type(&values[0], ctx.model(), ctx.heap())?;
        let obj = ctx
            .heap_mut()
            .alloc_dynamic("meta::pure::metamodel::type::generics::GenericType");
        ctx.heap_mut()
            .mutate_add(obj, "rawType", &[Value::Element(type_id)])?;

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
                let arg_gt = ctx
                    .heap_mut()
                    .alloc_dynamic("meta::pure::metamodel::type::generics::GenericType");
                ctx.heap_mut()
                    .mutate_add(arg_gt, "rawType", &[Value::Element(*elem_id)])?;
                ctx.heap_mut()
                    .mutate_add(obj, "typeArguments", &[Value::Object(arg_gt)])?;
            }
        }
        Ok(Evaluated::new(Value::Object(obj)))
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
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("rawType", &values, 1)?;
        let Value::Object(obj_id) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("GenericType", &values[0]).into());
        };
        let heap_values = ctx.heap().get_property_values(*obj_id, "rawType")?;
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
        let simple = ctx.model().element_name(id).clone();
        let enum_values: Vec<Value> = enum_def
            .values
            .iter()
            .map(|v| Value::String(SmolStr::new(format!("{simple}.{}", v.name))))
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
fn as_element_id(v: &Value) -> Result<ElementId, PureException> {
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
            // Fallback 1: heap-object classifier exact-name match.
            if let Value::Object(obj_id) = value
                && let Ok(classifier) = heap.classifier(*obj_id)
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
/// `Any` when a Class / PrimitiveType declares none.
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
            for list in lists.iter_mut() {
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
fn type_extends(model: &PureModel, descendant: ElementId, ancestor: ElementId) -> bool {
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
                .into()
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
///
/// For model-element references the id is the element's **simple name**
/// (`CC_Person->id() == 'CC_Person'`), not its qualified path — that
/// matches Java Pure and the `id.pure` / `reactivate.pure` assertions.
/// Qualified-path rendering is the job of `elementToPath`.
fn render_id(value: &Value, model: &PureModel) -> String {
    match value {
        Value::Object(obj_id) => format!("Anonymous_{obj_id}"),
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
        Value::Element(id) => build_element_path(model, *id, "::", false),
        Value::Object(obj_id) => {
            // Java Pure renders object instances as `<Anonymous_{id}>` with
            // no classifier prefix. `testClassInstanceToRepresentation`
            // checks `startsWith('<Anonymous_')`; the classifier belongs
            // on `type()`, not on the instance's textual identity.
            let _ = heap;
            format!("<Anonymous_{obj_id}>")
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
        Value::EnumValue { enum_id, member } => {
            format!("{}.{member}", model.element_name(*enum_id))
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
/// - Every Class / PrimitiveType implicitly extends `Any`, so an empty
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
/// Returns the enum-value form `"SimpleName.NAME"` for the `name`d member of
/// `enumeration`, matching the string shape [`Evaluator::eval_enum_value`]
/// emits so `==` comparisons agree both directions.
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
        let simple = ctx.model().element_name(enum_id);
        Ok(Evaluated::new(Value::String(SmolStr::new(format!(
            "{simple}.{name}"
        )))))
    }

    fn signature(&self) -> &'static str {
        "extractEnumValue(Enumeration[1], String[1]):T[1]"
    }
}

// ---------------------------------------------------------------------------
// evaluateAndDeactivate / deactivate — round-trip identities
// ---------------------------------------------------------------------------

/// Pure `evaluateAndDeactivate<T|m>(var:T[m]):T[m]`
///
/// Java Pure evaluates the ValueSpec then strips activation metadata. The
/// Rust runtime has already forced the argument (every ValueSpec reaches a
/// native as a `Value`), so this is an identity — the platform-level chains
/// that call it (`{|expr}.expressionSequence->evaluateAndDeactivate()`)
/// just want the resulting value back out.
#[derive(Debug)]
pub struct EvaluateAndDeactivate;

impl NativeFunction for EvaluateAndDeactivate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let mut values = force_all(args, ctx)?;
        expect_args("evaluateAndDeactivate", &values, 1)?;
        Ok(Evaluated::new(values.pop().unwrap_or(Value::Unit)))
    }

    fn signature(&self) -> &'static str {
        "evaluateAndDeactivate<T|m>(var:T[m]):T[m]"
    }
}

/// Pure `deactivate(var:Any[*]):ValueSpecification[1]`
///
/// Java Pure wraps the input in a `ValueSpecification` so later `reactivate`
/// calls can re-evaluate. We return the value directly — our runtime uses
/// the value itself as the spec proxy (the `expressionSequence` round-trip
/// path already treats the materialised closure as the spec).
#[derive(Debug)]
pub struct Deactivate;

impl NativeFunction for Deactivate {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let mut values = force_all(args, ctx)?;
        expect_args("deactivate", &values, 1)?;
        Ok(Evaluated::new(values.pop().unwrap_or(Value::Unit)))
    }

    fn signature(&self) -> &'static str {
        "deactivate(var:Any[*]):ValueSpecification[1]"
    }
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
        let Value::Function(fv) = &values[0] else {
            return Err(PureRuntimeError::type_mismatch("Function", &values[0]).into());
        };
        // Snapshot captures before the mutable heap borrow.
        let captures: Vec<(SmolStr, Value)> = match fv.as_ref() {
            FunctionValue::Lambda(closure) => closure
                .captures
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            FunctionValue::Compiled(_) => Vec::new(),
        };
        let mut map = im_rc::HashMap::new();
        for (name, value) in captures {
            // Wrap each captured binding in a `List<Any>(values=…)` heap
            // object so `$map->get(name).values` round-trips through the
            // Java-shaped List container the platform tests expect.
            let list_id = ctx
                .heap_mut()
                .alloc_dynamic("meta::pure::functions::collection::List");
            ctx.heap_mut().mutate_add(list_id, "values", &[value])?;
            map.insert(crate::value::ValueKey::String(name), Value::Object(list_id));
        }
        Ok(Evaluated::new(Value::Map(Box::new(map))))
    }

    fn signature(&self) -> &'static str {
        "openVariableValues(f:Function<Any>[1]):Map<String, List<Any>>[1]"
    }
}

/// Pure `genericTypeClass(g:GenericType[1]):Class<Any>[0..1]`
///
/// Reads the GenericType's `rawType` property and returns it only when it
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
        let raw_type_vals = ctx.heap().get_property_values(*obj_id, "rawType")?;
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
/// Returns the chain of elements from the root package down to `element`:
/// `[Root, pkg1, pkg1::pkg2, …, element]`. Each intermediate entry is the
/// `Value::Element` of a package on the path; the final entry is the input
/// element itself. Mirrors Java Pure's `.elementPath` accessor used for
/// reflection-heavy code generation (`elementToPath.pure::testElementPath`).
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
        let id = as_element_id(&values[0])?;

        // Walk the parent chain up to the root. For instance elements, the
        // chain starts with the element's own parent package; for a package
        // it starts with the package's own parent. The element / package
        // itself is appended last before we reverse.
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
                _ => None,
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
/// The Rust runtime treats `Value::Function` closures as the ValueSpec
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
        let spec = &values[0];
        match spec {
            Value::Function(_) => Ok(Evaluated::new(ctx.call_function(spec, &[])?)),
            other => Ok(Evaluated::new(other.clone())),
        }
    }

    fn signature(&self) -> &'static str {
        "reactivate(vs:ValueSpecification[1], vars:Map<String, List<Any>>[1]):Any[*]"
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
    registry.register("generalizations_Type_1__Type_$1_MANY$_", Generalizations);
    registry.register(
        "extractEnumValue_Enumeration_1__String_1__T_1_",
        ExtractEnumValue,
    );
    registry.register("evaluateAndDeactivate_T_m__T_m_", EvaluateAndDeactivate);
    registry.register("deactivate_Any_MANY__ValueSpecification_1_", Deactivate);
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
