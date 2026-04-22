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

//! Meta-model native functions: `pathToElement`, `elementToPath`, `match`.
//!
//! These bridge the Pure-level metamodel (`PackageableElement`,
//! `ConcreteFunctionDefinition`, `Package`) with the compiled [`PureModel`].
//! They are the foundation the Pure-native test orchestrator
//! (`surveyor.pure`) uses to discover and run tests.

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::TypeExpr;
use smol_str::SmolStr;

use crate::error::PureRuntimeError;
use crate::heap::RuntimeHeap;
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
        let id = as_element_id(&args[0])?;
        let separator = args[1].as_string()?;
        let include_root = args[2].as_boolean()?;
        let path = build_element_path(ctx.model(), id, separator.as_str(), include_root);
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
// Helpers
// ---------------------------------------------------------------------------

/// Extract an `ElementId` from a [`Value::Element`].
fn as_element_id(v: &Value) -> Result<ElementId, PureRuntimeError> {
    match v {
        Value::Element(id) => Ok(*id),
        other => Err(PureRuntimeError::type_mismatch("PackageableElement", other)),
    }
}

/// Resolve a package-qualified path to an element or a package.
///
/// Walks all segments but the last as packages, then tries:
/// 1. A child element (matched on simple name — function simple name, not mangled)
/// 2. A child package
///
/// Returns `None` if any segment fails to resolve.
fn resolve_path(model: &PureModel, path: &str, separator: &str) -> Option<ElementId> {
    let segments: Vec<&str> = if separator.is_empty() {
        vec![path]
    } else {
        path.split(separator).collect()
    };
    if segments.is_empty() {
        return None;
    }

    // Walk packages for all but the last segment.
    let mut current = model.root_package;
    for segment in &segments[..segments.len() - 1] {
        let pkg = model.get_package(current);
        let next = pkg
            .children_packages
            .iter()
            .find(|&&child_id| model.get_package(child_id).name == *segment)?;
        current = *next;
    }

    let last = segments[segments.len() - 1];
    let pkg = model.get_package(current);

    // Prefer child-element match (uses function simple name for Function elements).
    for &eid in &pkg.children_elements {
        let name = element_simple_name(model, eid);
        if name == last {
            return Some(eid);
        }
    }

    // Fall back to a child package of that name.
    pkg.children_packages
        .iter()
        .find(|&&child_id| model.get_package(child_id).name == *last)
        .map(|&child_id| ElementId::Package(child_id))
}

/// Build the `separator`-joined qualified path for an element.
///
/// For functions, uses `function_name` (simple name). When `include_root`
/// is `true` the anonymous root package is rendered as the literal string
/// `"Root"` (matching the Java Pure runtime) — so a top-level element
/// like `Package` becomes `"Root::Package"` under `includeRoot=true`.
/// When `include_root` is `false`, the root segment is dropped entirely.
///
/// Special case: `elementToPath(::)` on the root package with
/// `include_root=false` must be the empty string, not `""` joined into
/// the output.
fn build_element_path(
    model: &PureModel,
    id: ElementId,
    separator: &str,
    include_root: bool,
) -> String {
    // Calling elementToPath on the root package itself.
    if let ElementId::Package(pkg_id) = id
        && model.get_package(pkg_id).parent.is_none()
    {
        return if include_root {
            "Root".to_string()
        } else {
            String::new()
        };
    }

    let mut segments: Vec<SmolStr> = Vec::new();
    segments.push(element_simple_name(model, id).clone());

    let mut parent_pkg = match id {
        ElementId::Package(pkg_id) => model.get_package(pkg_id).parent,
        ElementId::InstanceId { .. } => Some(model.get_node(id).parent_package),
    };

    while let Some(pkg_id) = parent_pkg {
        let pkg = model.get_package(pkg_id);
        let is_root = pkg.parent.is_none();
        if is_root {
            if include_root {
                segments.push(SmolStr::new("Root"));
            }
            break;
        }
        segments.push(pkg.name.clone());
        parent_pkg = pkg.parent;
    }

    segments.reverse();
    segments
        .iter()
        .map(SmolStr::as_str)
        .collect::<Vec<_>>()
        .join(separator)
}

/// Return the element's simple name.
///
/// For `Element::Function`, this is the unmangled `function_name`. For
/// packages it is the package's own `name`. For everything else it is the
/// [`ElementNode::name`](legend_pure_parser_pure::model::ElementNode::name).
fn element_simple_name(model: &PureModel, id: ElementId) -> &SmolStr {
    if let Element::Function(f) = model.get_element(id) {
        return &f.function_name;
    }
    model.element_name(id)
}

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
}
