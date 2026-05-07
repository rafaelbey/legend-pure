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

//! Pure type → Java type mapping.

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

use crate::model::{CodegenError, Options};
use crate::naming::{
    join_java_package, pure_fqn_segments, pure_package_segments_of, safe_java_identifier,
};

/// Where in a function signature a type appears — for diagnostics.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TypePosition<'a> {
    Parameter(&'a str),
    Return,
}

impl<'a> TypePosition<'a> {
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Parameter(name) => format!("parameter '{name}'"),
            Self::Return => "return type".to_owned(),
        }
    }
}

/// A Java type rendered as a source string. Carries flags so the emitter
/// can decide between `Long` vs `long`, generate the proper `Optional`
/// wrapper, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JavaType {
    /// Source-level rendering, e.g. `"Iterable<Long>"`, `"Optional<Person>"`.
    pub(crate) source: String,
    /// True if this type denotes a generated user interface (or an
    /// `Iterable`/`Optional` of one). Drives whether the emitter wraps
    /// returns through the proxy factory.
    pub(crate) carries_user_type: bool,
}

/// Render the Java type for a Pure (TypeExpr, Multiplicity) pair.
///
/// `function_fqn` is included only so error variants can attribute the
/// failure to a specific Pure function.
pub(crate) fn render_java_type(
    model: &PureModel,
    type_expr: &TypeExpr,
    mult: &Multiplicity,
    function_fqn: &str,
    position: TypePosition<'_>,
    opts: &Options,
) -> Result<JavaType, CodegenError> {
    let inner = render_inner_type(model, type_expr, function_fqn, position, opts)?;
    Ok(wrap_multiplicity(inner, mult))
}

fn wrap_multiplicity(inner: JavaType, mult: &Multiplicity) -> JavaType {
    match mult {
        Multiplicity::PureOne => inner,
        Multiplicity::ZeroOrOne => JavaType {
            source: format!("java.util.Optional<{}>", inner.source),
            carries_user_type: inner.carries_user_type,
        },
        Multiplicity::ZeroOrMany | Multiplicity::OneOrMany => JavaType {
            source: format!("Iterable<{}>", inner.source),
            carries_user_type: inner.carries_user_type,
        },
        Multiplicity::Range { lower, upper } => {
            // [n..n] → singleton when both are 1; treat anything else as
            // many. The marshaller still reads the runtime list element-wise.
            if *lower == 1 && *upper == Some(1) {
                inner
            } else {
                JavaType {
                    source: format!("Iterable<{}>", inner.source),
                    carries_user_type: inner.carries_user_type,
                }
            }
        }
        Multiplicity::Variable(_) => JavaType {
            source: format!("Iterable<{}>", inner.source),
            carries_user_type: inner.carries_user_type,
        },
    }
}

fn render_inner_type(
    model: &PureModel,
    type_expr: &TypeExpr,
    function_fqn: &str,
    position: TypePosition<'_>,
    opts: &Options,
) -> Result<JavaType, CodegenError> {
    // Reject any nested function-type (e.g. `Function<{T[1]->V[1]}>`) before
    // we even try to render the outer Named ref.
    reject_nested_function_types(type_expr, function_fqn, position)?;
    match type_expr {
        TypeExpr::Named { element, .. } => render_named(model, *element, opts),
        TypeExpr::FunctionType { .. } => Err(CodegenError::FunctionTypedParameter {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
        }),
        TypeExpr::Relation(_) => Err(CodegenError::RelationTyped {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
        }),
        TypeExpr::Generic(name) => Err(CodegenError::GenericTyped {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
            ty: name.to_string(),
        }),
        TypeExpr::AlgebraUnion(_, _) => Err(CodegenError::RelationTyped {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
        }),
        TypeExpr::Unresolved => Err(CodegenError::GenericTyped {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
            ty: "<unresolved>".to_owned(),
        }),
    }
}

fn render_named(
    model: &PureModel,
    id: ElementId,
    opts: &Options,
) -> Result<JavaType, CodegenError> {
    let element = model.get_element(id);
    let segments = pure_fqn_segments(model, id);
    let leaf_name = segments.last().map(smol_str::SmolStr::as_str).unwrap_or("");

    if let Some(java) = primitive_java_type(element, leaf_name, &segments) {
        return Ok(JavaType {
            source: java.to_owned(),
            carries_user_type: false,
        });
    }

    // Platform classes (anything under `meta::pure::*`) are out of scope
    // for v1 — render them as opaque `Object` and skip auto-generating
    // interfaces for them. This keeps the generated surface focused on
    // user code; users who need typed access to M3 metamodel objects
    // can still reach them via PureRustInstance.
    if is_platform_class(&segments) {
        return Ok(JavaType {
            source: "Object".to_owned(),
            carries_user_type: false,
        });
    }

    match element {
        Element::Class(_) | Element::Enumeration(_) => {
            let pkg_segments = pure_package_segments_of(model, id);
            let java_pkg = join_java_package(&opts.java_root_package, &pkg_segments);
            let java_simple = safe_java_identifier(leaf_name);
            Ok(JavaType {
                source: format!("{java_pkg}.{java_simple}"),
                carries_user_type: true,
            })
        }
        Element::PrimitiveType(_) => {
            // A primitive whose simple name we don't have a Java mapping
            // for — fall back to Object so the call still compiles.
            Ok(JavaType {
                source: "Object".to_owned(),
                carries_user_type: false,
            })
        }
        Element::Measure(_) | Element::Unit(_) => {
            // Treat measures/units as opaque; they round-trip via
            // PureRustInstance.
            Ok(JavaType {
                source: "Object".to_owned(),
                carries_user_type: false,
            })
        }
        _ => Ok(JavaType {
            source: "Object".to_owned(),
            carries_user_type: false,
        }),
    }
}

/// True for elements whose FQN starts with `meta::pure::*`. v1 declines
/// to auto-generate interfaces for the M3 metamodel; user-defined Pure
/// classes do not live under that path, so this acts as a pragmatic
/// "platform vs user code" boundary.
pub(crate) fn is_platform_class(segments: &[smol_str::SmolStr]) -> bool {
    segments.first().is_some_and(|s| s.as_str() == "meta")
        && segments.get(1).is_some_and(|s| s.as_str() == "pure")
}

fn reject_nested_function_types(
    ty: &TypeExpr,
    function_fqn: &str,
    position: TypePosition<'_>,
) -> Result<(), CodegenError> {
    match ty {
        TypeExpr::FunctionType { .. } => Err(CodegenError::FunctionTypedParameter {
            fqn: function_fqn.to_owned(),
            position: position.describe(),
        }),
        TypeExpr::Named { type_arguments, .. } => {
            for nested in type_arguments {
                reject_nested_function_types(nested, function_fqn, position)?;
            }
            Ok(())
        }
        TypeExpr::Relation(cols) => {
            for col in cols {
                reject_nested_function_types(&col.type_expr, function_fqn, position)?;
            }
            Ok(())
        }
        TypeExpr::AlgebraUnion(a, b) => {
            reject_nested_function_types(a, function_fqn, position)?;
            reject_nested_function_types(b, function_fqn, position)
        }
        TypeExpr::Generic(_) | TypeExpr::Unresolved => Ok(()),
    }
}

fn primitive_java_type(
    element: &Element,
    leaf_name: &str,
    segments: &[smol_str::SmolStr],
) -> Option<&'static str> {
    if matches!(element, Element::PrimitiveType(_)) {
        return Some(match leaf_name {
            "Boolean" => "Boolean",
            "Integer" => "Long",
            "Float" => "Double",
            "Decimal" => "java.math.BigDecimal",
            "Number" => "Number",
            "String" => "String",
            "Byte" => "Byte",
            "StrictDate" | "LatestDate" => "java.time.LocalDate",
            "DateTime" => "java.time.OffsetDateTime",
            "Date" => "java.time.temporal.Temporal",
            "StrictTime" => "java.time.OffsetTime",
            _ => return None,
        });
    }
    // `Any`/`Nil` are M3 classes, not primitives.
    if let Element::Class(_) = element {
        let fqn_matches = |suffix: &[&str]| -> bool {
            segments.len() == suffix.len()
                && segments
                    .iter()
                    .zip(suffix.iter())
                    .all(|(a, b)| a.as_str() == *b)
        };
        if fqn_matches(&["meta", "pure", "metamodel", "type", "Any"])
            || fqn_matches(&["meta", "pure", "metamodel", "type", "Nil"])
        {
            return Some("Object");
        }
    }
    None
}
