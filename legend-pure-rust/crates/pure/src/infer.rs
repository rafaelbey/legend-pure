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
//! Performs bottom-up type inference over [`ValueSpec`] trees, producing
//! an [`InferredType`] (type + multiplicity) for each expression. Results
//! are stored in a side map on [`PureModel`], keyed by [`SourceInfo`].
//!
//! # Design
//!
//! - **Side map** — types are stored externally (`HashMap<SourceInfo, InferredType>`)
//!   rather than embedded in `ValueSpec`. This keeps the IR unchanged.
//! - **Bottom-up** — literals carry their own types, variables resolve from
//!   scope, property access looks up the class, function calls use the
//!   declared return type.
//! - **Scope chain** — `let` bindings and lambda parameters push entries
//!   into a scope stack. Variable references resolve by walking up.

use std::collections::HashMap;

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::CompilationError;
use crate::model::{Element, InferredType, PureModel};
use crate::types::{DateValue, Multiplicity, Parameter, TypeExpr, ValueSpec};

// ---------------------------------------------------------------------------
// Scope — variable type tracking
// ---------------------------------------------------------------------------

/// A lexical scope for variable bindings during type inference.
///
/// Function parameters initialize the root scope. `let` bindings and
/// lambda parameters extend the current scope.
struct Scope {
    /// Variable name → inferred type.
    bindings: HashMap<SmolStr, InferredType>,
}

impl Scope {
    /// Creates a new scope from function/lambda parameters.
    fn from_params(params: &[Parameter]) -> Self {
        let mut bindings = HashMap::new();
        for p in params {
            bindings.insert(
                p.name.clone(),
                InferredType {
                    type_expr: p.type_expr.clone(),
                    multiplicity: p.multiplicity.clone(),
                },
            );
        }
        Self { bindings }
    }

    /// Adds a variable binding (e.g., from a `let`).
    fn bind(&mut self, name: SmolStr, inferred: InferredType) {
        self.bindings.insert(name, inferred);
    }

    /// Looks up a variable in this scope.
    fn lookup(&self, name: &str) -> Option<&InferredType> {
        self.bindings.get(name)
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
    /// Results accumulator.
    type_map: &'a mut HashMap<SourceInfo, InferredType>,
    /// Errors accumulator (used in Phase B for type mismatch errors).
    _errors: &'a mut Vec<CompilationError>,
}

impl InferCtx<'_> {
    /// Looks up a variable by walking the scope chain from innermost to outermost.
    fn lookup_var(&self, name: &str) -> Option<&InferredType> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.lookup(name) {
                return Some(t);
            }
        }
        None
    }

    /// Records an inferred type for an expression at the given source position.
    fn record(&mut self, source_info: &SourceInfo, inferred: InferredType) {
        self.type_map.insert(source_info.clone(), inferred);
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
/// Results are stored in `type_map`, keyed by expression source positions.
/// Type errors are appended to `errors`.
pub(crate) fn infer_function_body(
    model: &PureModel,
    params: &[Parameter],
    body: &[ValueSpec],
    type_map: &mut HashMap<SourceInfo, InferredType>,
    errors: &mut Vec<CompilationError>,
) {
    let root_scope = Scope::from_params(params);
    let mut ctx = InferCtx {
        model,
        scopes: vec![root_scope],
        type_map,
        _errors: errors,
    };

    for expr in body {
        infer_expr(&mut ctx, expr);
    }
}

// ---------------------------------------------------------------------------
// Bottom-up inference
// ---------------------------------------------------------------------------

/// Infers the type of a single expression, recording it and returning it.
#[allow(clippy::too_many_lines)]
fn infer_expr(ctx: &mut InferCtx<'_>, expr: &ValueSpec) -> Option<InferredType> {
    let result = match expr {
        // -- Literals -------------------------------------------------------
        ValueSpec::IntegerLiteral(_, si) => Some(primitive_type(bootstrap::INTEGER_ID, si)),
        ValueSpec::FloatLiteral(_, si) => Some(primitive_type(bootstrap::FLOAT_ID, si)),
        ValueSpec::DecimalLiteral(_, si) => Some(primitive_type(bootstrap::DECIMAL_ID, si)),
        ValueSpec::StringLiteral(_, si) => Some(primitive_type(bootstrap::STRING_ID, si)),
        ValueSpec::BooleanLiteral(_, si) => Some(primitive_type(bootstrap::BOOLEAN_ID, si)),
        ValueSpec::DateLiteral(dv, si) => Some(date_literal_type(dv, si)),

        // -- Variable -------------------------------------------------------
        ValueSpec::Variable { name, source_info } => {
            if let Some(inferred) = ctx.lookup_var(name) {
                let result = inferred.clone();
                ctx.record(source_info, result.clone());
                Some(result)
            } else {
                // Variable not in scope — type unknown (might be unresolved)
                None
            }
        }

        // -- Function call --------------------------------------------------
        ValueSpec::FunctionCall {
            function,
            function_name,
            arguments,
            source_info,
        } => {
            // Infer argument types first (bottom-up)
            let arg_types: Vec<Option<InferredType>> =
                arguments.iter().map(|a| infer_expr(ctx, a)).collect();

            let result = infer_function_call(
                ctx,
                *function,
                function_name,
                arguments,
                &arg_types,
                source_info,
            );
            if let Some(ref r) = result {
                ctx.record(source_info, r.clone());
            }
            return result;
        }

        // -- Property access ------------------------------------------------
        ValueSpec::PropertyAccess {
            target,
            property,
            source_info,
        } => {
            let target_type = infer_expr(ctx, target);
            let result = infer_property_access(ctx, target_type.as_ref(), property, source_info);
            if let Some(ref r) = result {
                ctx.record(source_info, r.clone());
            }
            return result;
        }

        // -- Qualified property access --------------------------------------
        ValueSpec::QualifiedPropertyAccess {
            target,
            property,
            arguments,
            source_info,
        } => {
            let target_type = infer_expr(ctx, target);
            // Infer argument types (for completeness)
            for arg in arguments {
                infer_expr(ctx, arg);
            }
            // Qualified properties are resolved the same as simple properties
            let result = infer_property_access(ctx, target_type.as_ref(), property, source_info);
            if let Some(ref r) = result {
                ctx.record(source_info, r.clone());
            }
            return result;
        }

        // -- Enum value -----------------------------------------------------
        ValueSpec::EnumValue {
            enum_element,
            source_info,
            ..
        } => {
            let result = InferredType {
                type_expr: TypeExpr::Named {
                    element: *enum_element,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                multiplicity: Multiplicity::PureOne,
            };
            ctx.record(source_info, result.clone());
            Some(result)
        }

        // -- Lambda ---------------------------------------------------------
        ValueSpec::Lambda {
            parameters,
            body,
            source_info,
        } => {
            // Push a child scope with lambda parameters
            ctx.push_scope(Scope::from_params(parameters));

            // Infer body types
            let mut last_type = None;
            for body_expr in body {
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

            let result = InferredType {
                type_expr: TypeExpr::FunctionType {
                    parameters: param_types,
                    return_type: Box::new(return_type),
                    return_multiplicity: return_mult,
                },
                multiplicity: Multiplicity::PureOne,
            };
            ctx.record(source_info, result.clone());
            Some(result)
        }

        // -- Collection -----------------------------------------------------
        ValueSpec::Collection {
            elements,
            source_info,
        } => {
            let elem_types: Vec<Option<InferredType>> =
                elements.iter().map(|e| infer_expr(ctx, e)).collect();

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

            let result = InferredType {
                type_expr,
                multiplicity,
            };
            ctx.record(source_info, result.clone());
            Some(result)
        }

        // -- Type reference -------------------------------------------------
        ValueSpec::TypeReference {
            type_expr: te,
            source_info,
        } => {
            // @Type evaluates to a Class<T> meta-type; for now store the type itself
            let result = InferredType {
                type_expr: te.clone(),
                multiplicity: Multiplicity::PureOne,
            };
            ctx.record(source_info, result.clone());
            Some(result)
        }

        // -- Element reference ----------------------------------------------
        ValueSpec::PackageableElementRef {
            element,
            source_info,
        } => {
            let result = InferredType {
                type_expr: TypeExpr::Named {
                    element: *element,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                multiplicity: Multiplicity::PureOne,
            };
            ctx.record(source_info, result.clone());
            Some(result)
        }

        // -- Column (TDS — deferred) ----------------------------------------
        ValueSpec::Column { .. } => None,
    };

    if let Some(ref r) = result {
        let si = expr_source_info(expr);
        ctx.record(si, r.clone());
    }
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
    arguments: &[ValueSpec],
    arg_types: &[Option<InferredType>],
    _source_info: &SourceInfo,
) -> Option<InferredType> {
    // Handle `letFunction` — side effect: bind the variable in scope
    if function_name == "letFunction"
        && arg_types.len() == 2
        && let (Some(val_type), Some(ValueSpec::StringLiteral(name, _))) =
            (&arg_types[1], arguments.first())
    {
        if let Some(scope) = ctx.scopes.last_mut() {
            scope.bind(SmolStr::new(name.as_str()), val_type.clone());
        }
        // letFunction itself returns Nil[0] (it's a side-effect statement)
        return Some(InferredType {
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
        return Some(InferredType {
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
    arg_types: &[Option<InferredType>],
) -> Option<InferredType> {
    match name {
        // Comparison operators → Boolean[1]
        "equal" | "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" => {
            Some(InferredType {
                type_expr: TypeExpr::Named {
                    element: bootstrap::BOOLEAN_ID,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                multiplicity: Multiplicity::PureOne,
            })
        }

        // Boolean operators → Boolean[1]
        "and" | "or" | "not" => Some(InferredType {
            type_expr: TypeExpr::Named {
                element: bootstrap::BOOLEAN_ID,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        // Arithmetic operators — return widened type of arguments
        "plus" | "minus" | "times" | "divide" => {
            // Use first argument's type as the result type (simplified)
            arg_types.iter().flatten().next().cloned()
        }

        // String concatenation
        "joinStrings" => Some(InferredType {
            type_expr: TypeExpr::Named {
                element: bootstrap::STRING_ID,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Property access type inference
// ---------------------------------------------------------------------------

/// Infers the type of a property access (`$target.property`).
#[allow(clippy::only_used_in_recursion)]
fn infer_property_access(
    ctx: &InferCtx<'_>,
    target_type: Option<&InferredType>,
    property_name: &str,
    source_info: &SourceInfo,
) -> Option<InferredType> {
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
            return Some(InferredType {
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
            return Some(InferredType {
                type_expr: qp.return_type.clone(),
                multiplicity: qp.return_multiplicity.clone(),
            });
        }
        // Walk supertypes for inherited properties
        for st in &class.super_types {
            if let TypeExpr::Named { element: _, .. } = st {
                let super_type = Some(InferredType {
                    type_expr: st.clone(),
                    multiplicity: Multiplicity::PureOne,
                });
                if let Some(result) =
                    infer_property_access(ctx, super_type.as_ref(), property_name, source_info)
                {
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

/// Creates an `InferredType` for a primitive type with multiplicity `[1]`.
fn primitive_type(element_id: crate::ids::ElementId, _source_info: &SourceInfo) -> InferredType {
    InferredType {
        type_expr: TypeExpr::Named {
            element: element_id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        },
        multiplicity: Multiplicity::PureOne,
    }
}

/// Returns the inferred type for a date literal based on its variant.
fn date_literal_type(dv: &DateValue, source_info: &SourceInfo) -> InferredType {
    match dv {
        DateValue::StrictDate { .. } => primitive_type(bootstrap::STRICT_DATE_ID, source_info),
        DateValue::DateTime { .. } => primitive_type(bootstrap::DATE_TIME_ID, source_info),
        DateValue::StrictTime { .. } => primitive_type(bootstrap::STRICT_TIME_ID, source_info),
    }
}

/// Returns the source info of a `ValueSpec`.
fn expr_source_info(expr: &ValueSpec) -> &SourceInfo {
    match expr {
        ValueSpec::IntegerLiteral(_, si)
        | ValueSpec::FloatLiteral(_, si)
        | ValueSpec::DecimalLiteral(_, si)
        | ValueSpec::StringLiteral(_, si)
        | ValueSpec::BooleanLiteral(_, si)
        | ValueSpec::DateLiteral(_, si) => si,
        ValueSpec::Variable { source_info, .. }
        | ValueSpec::FunctionCall { source_info, .. }
        | ValueSpec::PropertyAccess { source_info, .. }
        | ValueSpec::QualifiedPropertyAccess { source_info, .. }
        | ValueSpec::EnumValue { source_info, .. }
        | ValueSpec::Lambda { source_info, .. }
        | ValueSpec::Collection { source_info, .. }
        | ValueSpec::TypeReference { source_info, .. }
        | ValueSpec::PackageableElementRef { source_info, .. }
        | ValueSpec::Column { source_info, .. } => source_info,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap;
    use crate::types::{Multiplicity, TypeExpr};
    use legend_pure_parser_ast::SourceInfo;
    use smol_str::SmolStr;

    fn test_source_info() -> SourceInfo {
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

    #[test]
    fn infer_integer_literal() {
        let model = model_with_bootstrap();
        let body = vec![ValueSpec::IntegerLiteral(42, test_source_info())];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        assert!(errors.is_empty());
        let inferred = type_map.get(&test_source_info()).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(inferred.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_string_literal() {
        let model = model_with_bootstrap();
        let body = vec![ValueSpec::StringLiteral(
            SmolStr::new("hello"),
            test_source_info(),
        )];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&test_source_info()).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(inferred.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_boolean_literal() {
        let model = model_with_bootstrap();
        let body = vec![ValueSpec::BooleanLiteral(true, test_source_info())];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&test_source_info()).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::BOOLEAN_ID));
        assert_eq!(inferred.multiplicity, Multiplicity::PureOne);
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

        let var_si = SourceInfo::new("test.pure", 2, 1, 2, 3);
        let body = vec![ValueSpec::Variable {
            name: SmolStr::new("x"),
            source_info: var_si.clone(),
        }];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &params, &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&var_si).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(inferred.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_collection() {
        let model = model_with_bootstrap();
        let si1 = SourceInfo::new("test.pure", 1, 2, 1, 3);
        let si2 = SourceInfo::new("test.pure", 1, 5, 1, 6);
        let si3 = SourceInfo::new("test.pure", 1, 8, 1, 9);
        let coll_si = SourceInfo::new("test.pure", 1, 1, 1, 10);

        let body = vec![ValueSpec::Collection {
            elements: vec![
                ValueSpec::IntegerLiteral(1, si1),
                ValueSpec::IntegerLiteral(2, si2),
                ValueSpec::IntegerLiteral(3, si3),
            ],
            source_info: coll_si.clone(),
        }];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&coll_si).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(
            inferred.multiplicity,
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
        let var_si = SourceInfo::new("test.pure", 1, 20, 1, 22);

        let body = vec![ValueSpec::Lambda {
            parameters: vec![Parameter {
                name: SmolStr::new("x"),
                type_expr: named_type(bootstrap::STRING_ID),
                multiplicity: Multiplicity::PureOne,
                source_info: SourceInfo::new("test.pure", 1, 2, 1, 15),
            }],
            body: vec![ValueSpec::Variable {
                name: SmolStr::new("x"),
                source_info: var_si.clone(),
            }],
            source_info: lambda_si.clone(),
        }];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&lambda_si).unwrap();
        match &inferred.type_expr {
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
        let si = test_source_info();
        let body = vec![ValueSpec::DateLiteral(
            DateValue::StrictDate {
                year: 2024,
                month: 1,
                day: 15,
            },
            si.clone(),
        )];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&si).unwrap();
        assert_eq!(inferred.type_expr, named_type(bootstrap::STRICT_DATE_ID));
    }

    #[test]
    fn infer_enum_value() {
        let model = model_with_bootstrap();
        let enum_id = crate::ids::ElementId {
            chunk_id: 1,
            local_idx: 0,
        };
        let si = test_source_info();

        let body = vec![ValueSpec::EnumValue {
            enum_element: enum_id,
            value: SmolStr::new("VALUE_A"),
            source_info: si.clone(),
        }];
        let mut type_map = HashMap::new();
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &body, &mut type_map, &mut errors);

        let inferred = type_map.get(&si).unwrap();
        assert_eq!(inferred.type_expr, named_type(enum_id));
        assert_eq!(inferred.multiplicity, Multiplicity::PureOne);
    }
}
