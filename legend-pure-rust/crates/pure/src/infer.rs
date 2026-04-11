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

//! Type inference engine for compiled Pure expressions.
//!
//! Performs bottom-up type inference over [`ValueSpec`] trees, setting
//! the [`type_info`](ValueSpec::type_info) field on every expression node.
//!
//! # Design
//!
//! - **Inline types** — types are set directly on `ValueSpec::type_info`,
//!   eliminating the need for a side map.
//! - **Bottom-up** — literals carry their own types, variables resolve from
//!   scope, property access looks up the class, function calls use the
//!   declared return type.
//! - **Scope chain** — `let` bindings and lambda parameters push entries
//!   into a scope stack. Variable references resolve by walking up.

use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::CompilationError;
use crate::model::{Element, PureModel};
use crate::types::{
    DateValue, ExprKind, Multiplicity, Parameter, ResolvedType, TypeExpr, ValueSpec,
};

// ---------------------------------------------------------------------------
// Scope — variable type tracking
// ---------------------------------------------------------------------------

/// A lexical scope for variable bindings during type inference.
///
/// Function parameters initialize the root scope. `let` bindings and
/// lambda parameters extend the current scope.
struct Scope {
    /// Variable name → inferred type.
    bindings: Vec<(SmolStr, ResolvedType)>,
}

#[allow(dead_code)]
impl Scope {
    /// Creates a scope pre-populated with function/lambda parameters.
    fn from_params(params: &[Parameter]) -> Self {
        let bindings = params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    ResolvedType {
                        type_expr: p.type_expr.clone(),
                        multiplicity: p.multiplicity.clone(),
                    },
                )
            })
            .collect();
        Scope { bindings }
    }

    /// Looks up a variable in this scope.
    fn lookup(&self, name: &str) -> Option<&ResolvedType> {
        self.bindings
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t)
    }

    /// Binds a new variable (e.g., from `let`).
    fn bind(&mut self, name: SmolStr, inferred: ResolvedType) {
        self.bindings.push((name, inferred));
    }
}

// ---------------------------------------------------------------------------
// Inference context
// ---------------------------------------------------------------------------

/// Accumulates inferred types and errors during a single function's inference.
struct InferCtx<'a> {
    /// The compiled model (read-only).
    model: &'a PureModel,
    /// Scope chain (outermost first).
    scopes: Vec<Scope>,
    /// Errors accumulator (used in Phase B for type mismatch errors).
    _errors: &'a mut Vec<CompilationError>,
}

impl InferCtx<'_> {
    /// Looks up a variable by walking the scope chain from innermost to outermost.
    fn lookup_var(&self, name: &str) -> Option<&ResolvedType> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.lookup(name) {
                return Some(t);
            }
        }
        None
    }

    /// Pushes a new child scope.
    fn push_scope(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    /// Pops the innermost scope.
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Infers types for all expressions in a function body.
///
/// Sets `type_info` on every expression node in `body` (in place).
/// Type errors are appended to `errors`.
pub(crate) fn infer_function_body(
    model: &PureModel,
    params: &[Parameter],
    body: &mut [ValueSpec],
    errors: &mut Vec<CompilationError>,
) {
    let root_scope = Scope::from_params(params);
    let mut ctx = InferCtx {
        model,
        scopes: vec![root_scope],
        _errors: errors,
    };

    for expr in body.iter_mut() {
        infer_expr(&mut ctx, expr);
    }
}

// ---------------------------------------------------------------------------
// Bottom-up inference
// ---------------------------------------------------------------------------

/// Infers the type of a single expression, setting its `type_info` and
/// returning a clone of the resolved type.
#[allow(clippy::too_many_lines)]
fn infer_expr(ctx: &mut InferCtx<'_>, expr: &mut ValueSpec) -> Option<ResolvedType> {
    let result = match &mut expr.kind {
        // -- Literals -------------------------------------------------------
        ExprKind::IntegerLiteral(_) => Some(primitive(bootstrap::INTEGER_ID)),
        ExprKind::FloatLiteral(_) => Some(primitive(bootstrap::FLOAT_ID)),
        ExprKind::DecimalLiteral(_) => Some(primitive(bootstrap::DECIMAL_ID)),
        ExprKind::StringLiteral(_) => Some(primitive(bootstrap::STRING_ID)),
        ExprKind::BooleanLiteral(_) => Some(primitive(bootstrap::BOOLEAN_ID)),
        ExprKind::DateLiteral(dv) => Some(date_literal_type(dv)),

        // -- Variable -------------------------------------------------------
        ExprKind::Variable { name } => ctx.lookup_var(name).cloned(),

        // -- Function call --------------------------------------------------
        ExprKind::FunctionCall {
            function,
            function_name,
            arguments,
        } => {
            // Infer argument types first (bottom-up)
            let arg_types: Vec<Option<ResolvedType>> =
                arguments.iter_mut().map(|a| infer_expr(ctx, a)).collect();

            // Extract let name from AST before calling inference
            let let_name = if function_name == "letFunction" && arguments.len() == 2 {
                if let ExprKind::StringLiteral(name) = &arguments[0].kind {
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };

            let result = infer_function_call(ctx, *function, function_name, let_name, &arg_types);
            return set_and_return(expr, result);
        }

        // -- Property access ------------------------------------------------
        ExprKind::PropertyAccess { target, property } => {
            let target_type = infer_expr(ctx, target);
            let result = infer_property_access(ctx, target_type.as_ref(), property);
            return set_and_return(expr, result);
        }

        // -- Qualified property access --------------------------------------
        ExprKind::QualifiedPropertyAccess {
            target,
            property,
            arguments,
        } => {
            let target_type = infer_expr(ctx, target);
            // Infer argument types (for completeness)
            for arg in arguments.iter_mut() {
                infer_expr(ctx, arg);
            }
            let result = infer_property_access(ctx, target_type.as_ref(), property);
            return set_and_return(expr, result);
        }

        // -- Enum value -----------------------------------------------------
        ExprKind::EnumValue { enum_element, .. } => Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: *enum_element,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Lambda ---------------------------------------------------------
        ExprKind::Lambda { parameters, body } => {
            // Push a child scope with lambda parameters
            ctx.push_scope(Scope::from_params(parameters));

            // Infer body types
            let mut last_type = None;
            for body_expr in body.iter_mut() {
                last_type = infer_expr(ctx, body_expr);
            }

            ctx.pop_scope();

            // The lambda's type is a FunctionType
            let param_types: Vec<(TypeExpr, Multiplicity)> = parameters
                .iter()
                .map(|p| (p.type_expr.clone(), p.multiplicity.clone()))
                .collect();

            let (return_type, return_mult) = if let Some(ref lt) = last_type {
                (lt.type_expr.clone(), lt.multiplicity.clone())
            } else {
                // No body or untyped → Any[*]
                (
                    TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: Vec::new(),
                        value_arguments: Vec::new(),
                    },
                    Multiplicity::ZeroOrMany,
                )
            };

            Some(ResolvedType {
                type_expr: TypeExpr::FunctionType {
                    parameters: param_types,
                    return_type: Box::new(return_type),
                    return_multiplicity: return_mult,
                },
                multiplicity: Multiplicity::PureOne,
            })
        }

        // -- Collection -----------------------------------------------------
        ExprKind::Collection { elements } => {
            let elem_types: Vec<Option<ResolvedType>> =
                elements.iter_mut().map(|e| infer_expr(ctx, e)).collect();

            let count = u32::try_from(elements.len()).unwrap_or(u32::MAX);
            let multiplicity = Multiplicity::Range {
                lower: count,
                upper: Some(count),
            };

            let type_expr = elem_types.iter().flatten().next().map_or_else(
                || TypeExpr::Named {
                    element: bootstrap::NIL_ID,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                |t| t.type_expr.clone(),
            );

            Some(ResolvedType {
                type_expr,
                multiplicity,
            })
        }

        // -- Type reference -------------------------------------------------
        ExprKind::TypeReference { type_expr: te } => Some(ResolvedType {
            type_expr: te.clone(),
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Element reference ----------------------------------------------
        ExprKind::PackageableElementRef { element } => Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: *element,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Column (TDS — deferred) ----------------------------------------
        ExprKind::Column => None,
    };

    set_and_return(expr, result)
}

/// Sets `expr.type_info` and returns the resolved type.
fn set_and_return(expr: &mut ValueSpec, result: Option<ResolvedType>) -> Option<ResolvedType> {
    expr.type_info.clone_from(&result);
    result
}

// ---------------------------------------------------------------------------
// Function call type inference
// ---------------------------------------------------------------------------

/// Infers the return type of a function call.
fn infer_function_call(
    ctx: &mut InferCtx<'_>,
    function: Option<crate::ids::ElementId>,
    function_name: &SmolStr,
    let_name: Option<&SmolStr>,
    arg_types: &[Option<ResolvedType>],
) -> Option<ResolvedType> {
    // Handle `letFunction` — side effect: bind the variable in scope
    if function_name == "letFunction"
        && arg_types.len() == 2
        && let (Some(val_type), Some(name)) = (&arg_types[1], let_name)
    {
        if let Some(scope) = ctx.scopes.last_mut() {
            scope.bind(name.clone(), val_type.clone());
        }
        // letFunction itself returns Nil[0] (it's a side-effect statement)
        return Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: bootstrap::NIL_ID,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::Range {
                lower: 0,
                upper: Some(0),
            },
        });
    }

    // Resolved user function — use its declared return type
    if let Some(Element::Function(f)) = function.and_then(|id| ctx.model.try_get_element(id)) {
        return Some(ResolvedType {
            type_expr: f.return_type.clone(),
            multiplicity: f.return_multiplicity.clone(),
        });
    }

    // Built-in operator return types
    infer_builtin_return_type(function_name, arg_types)
}

/// Infers return types for well-known built-in operators.
fn infer_builtin_return_type(
    name: &str,
    arg_types: &[Option<ResolvedType>],
) -> Option<ResolvedType> {
    match name {
        // Comparison operators → Boolean[1]
        "equal" | "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" => {
            Some(primitive(bootstrap::BOOLEAN_ID))
        }

        // Boolean operators → Boolean[1]
        "and" | "or" | "not" => Some(primitive(bootstrap::BOOLEAN_ID)),

        // Arithmetic operators — return widened type of arguments
        "plus" | "minus" | "times" | "divide" => arg_types.iter().flatten().next().cloned(),

        // String concatenation
        "joinStrings" => Some(primitive(bootstrap::STRING_ID)),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Property access type inference
// ---------------------------------------------------------------------------

/// Infers the type of a property access (`$target.property`).
fn infer_property_access(
    ctx: &InferCtx<'_>,
    target_type: Option<&ResolvedType>,
    property_name: &str,
) -> Option<ResolvedType> {
    let target = target_type?;

    // Get the element ID from the target type
    let element_id = match &target.type_expr {
        TypeExpr::Named { element, .. } => *element,
        _ => return None,
    };

    // Look up the element — it must be a Class
    let elem = ctx.model.try_get_element(element_id)?;
    if let Element::Class(class) = elem {
        // Search in own properties
        if let Some(prop) = class.properties.iter().find(|p| p.name == property_name) {
            return Some(ResolvedType {
                type_expr: prop.type_expr.clone(),
                multiplicity: prop.multiplicity.clone(),
            });
        }
        // Search in qualified properties
        if let Some(qp) = class
            .qualified_properties
            .iter()
            .find(|q| q.name == property_name)
        {
            return Some(ResolvedType {
                type_expr: qp.return_type.clone(),
                multiplicity: qp.return_multiplicity.clone(),
            });
        }
        // Walk supertypes for inherited properties
        for st in &class.super_types {
            if matches!(st, TypeExpr::Named { .. }) {
                let super_type = ResolvedType {
                    type_expr: st.clone(),
                    multiplicity: Multiplicity::PureOne,
                };
                if let Some(result) = infer_property_access(ctx, Some(&super_type), property_name) {
                    return Some(result);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a `ResolvedType` for a primitive type with multiplicity `[1]`.
fn primitive(element_id: crate::ids::ElementId) -> ResolvedType {
    ResolvedType {
        type_expr: TypeExpr::Named {
            element: element_id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        },
        multiplicity: Multiplicity::PureOne,
    }
}

/// Returns the resolved type for a date literal based on its variant.
fn date_literal_type(dv: &DateValue) -> ResolvedType {
    match dv {
        DateValue::StrictDate { .. } => primitive(bootstrap::STRICT_DATE_ID),
        DateValue::DateTime { .. } => primitive(bootstrap::DATE_TIME_ID),
        DateValue::StrictTime { .. } => primitive(bootstrap::STRICT_TIME_ID),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap;
    use crate::types::{ExprKind, Multiplicity, TypeExpr, ValueSpec};
    use legend_pure_parser_ast::SourceInfo;
    use smol_str::SmolStr;

    fn si() -> SourceInfo {
        SourceInfo::new("test.pure", 1, 1, 1, 10)
    }

    fn named_type(id: crate::ids::ElementId) -> TypeExpr {
        TypeExpr::Named {
            element: id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        }
    }

    fn model_with_bootstrap() -> PureModel {
        let mut model = PureModel::new();
        let bootstrap = bootstrap::create_bootstrap_chunk(model.root_package);
        model.chunks.push(bootstrap);
        model
    }

    /// Helper: create a `ValueSpec` with no type info.
    fn untyped(kind: ExprKind, source_info: SourceInfo) -> ValueSpec {
        ValueSpec {
            kind,
            source_info,
            type_info: None,
        }
    }

    #[test]
    fn infer_integer_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(ExprKind::IntegerLiteral(42), si())];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        assert!(errors.is_empty());
        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_string_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(
            ExprKind::StringLiteral(SmolStr::new("hello")),
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_boolean_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(ExprKind::BooleanLiteral(true), si())];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::BOOLEAN_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_variable_from_param() {
        let model = model_with_bootstrap();
        let params = vec![Parameter {
            name: SmolStr::new("x"),
            type_expr: named_type(bootstrap::STRING_ID),
            multiplicity: Multiplicity::PureOne,
            source_info: SourceInfo::new("test.pure", 1, 1, 1, 5),
        }];

        let mut body = vec![untyped(
            ExprKind::Variable {
                name: SmolStr::new("x"),
            },
            SourceInfo::new("test.pure", 2, 1, 2, 3),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &params, &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_collection() {
        let model = model_with_bootstrap();
        let si1 = SourceInfo::new("test.pure", 1, 2, 1, 3);
        let si2 = SourceInfo::new("test.pure", 1, 5, 1, 6);
        let si3 = SourceInfo::new("test.pure", 1, 8, 1, 9);
        let coll_si = SourceInfo::new("test.pure", 1, 1, 1, 10);

        let mut body = vec![untyped(
            ExprKind::Collection {
                elements: vec![
                    untyped(ExprKind::IntegerLiteral(1), si1),
                    untyped(ExprKind::IntegerLiteral(2), si2),
                    untyped(ExprKind::IntegerLiteral(3), si3),
                ],
            },
            coll_si,
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(
            ti.multiplicity,
            Multiplicity::Range {
                lower: 3,
                upper: Some(3)
            }
        );
    }

    #[test]
    fn infer_lambda() {
        let model = model_with_bootstrap();
        let lambda_si = SourceInfo::new("test.pure", 1, 1, 1, 30);

        let mut body = vec![untyped(
            ExprKind::Lambda {
                parameters: vec![Parameter {
                    name: SmolStr::new("x"),
                    type_expr: named_type(bootstrap::STRING_ID),
                    multiplicity: Multiplicity::PureOne,
                    source_info: SourceInfo::new("test.pure", 1, 2, 1, 15),
                }],
                body: vec![untyped(
                    ExprKind::Variable {
                        name: SmolStr::new("x"),
                    },
                    SourceInfo::new("test.pure", 1, 20, 1, 22),
                )],
            },
            lambda_si,
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        match &ti.type_expr {
            TypeExpr::FunctionType {
                parameters,
                return_type,
                return_multiplicity,
            } => {
                assert_eq!(parameters.len(), 1);
                assert_eq!(parameters[0].0, named_type(bootstrap::STRING_ID));
                assert_eq!(**return_type, named_type(bootstrap::STRING_ID));
                assert_eq!(*return_multiplicity, Multiplicity::PureOne);
            }
            other => panic!("Expected FunctionType, got {other:?}"),
        }
    }

    #[test]
    fn infer_date_literals() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(
            ExprKind::DateLiteral(DateValue::StrictDate {
                year: 2024,
                month: 1,
                day: 15,
            }),
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRICT_DATE_ID));
    }

    #[test]
    fn infer_enum_value() {
        let model = model_with_bootstrap();
        let enum_id = crate::ids::ElementId {
            chunk_id: 1,
            local_idx: 0,
        };

        let mut body = vec![untyped(
            ExprKind::EnumValue {
                enum_element: enum_id,
                value: SmolStr::new("VALUE_A"),
            },
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(enum_id));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }
}
