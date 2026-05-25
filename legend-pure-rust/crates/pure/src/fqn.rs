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

//! Function FQN (Fully Qualified Name) computation.
//!
//! Mirrors Java's `ConcreteFunctionDefinitionNameProcessor`: the mangled
//! function name encodes parameter types, multiplicities, and the return
//! type/multiplicity. This is the key used to look up native functions
//! in the runtime's `NativeRegistry`.
//!
//! # Format
//!
//! ```text
//! funcName_ParamType_Mult__ParamType_Mult__ReturnType_Mult_
//! ```
//!
//! Each parameter contributes `Type + multSig + "_"`, producing a
//! double-underscore `__` boundary between consecutive parameters.
//!
//! # Java algorithm (from `ConcreteFunctionDefinitionNameProcessor.getSignatureAndResolveImports`)
//!
//! ```text
//! builder = funcName + "_"
//! for each param:
//!     builder += Type + multiplicityToSignatureString(mult) + "_"
//! if no params:
//!     builder += "_"     // so we get funcName__ before the return type
//! builder += ReturnType + multiplicityToSignatureString(returnMult)
//! ```
//!
//! ## Multiplicity encoding (from `Multiplicity.multiplicityToSignatureString`)
//!
//! | Pure | Signature |
//! |------|-----------|
//! | `[1]` | `_1_` |
//! | `[0..1]` | `_$0_1$_` |
//! | `[*]` | `_MANY_` |
//! | `[1..*]` | `_$1_MANY$_` |
//! | `[n..m]` | `_$n_m$_` |
//!
//! ## Examples
//!
//! - `plus(Integer[*]): Integer[1]`
//!   → `plus_Integer_MANY__Integer_1_`
//! - `first(T[*]): T[0..1]`
//!   → `first_T_MANY__T_$0_1$_`
//! - `now(): DateTime[1]`
//!   → `now__DateTime_1_`

use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::nodes::function::Function;
use crate::types::{Multiplicity, TypeExpr};
use smol_str::SmolStr;

/// Compute the mangled function name (FQN) for a function element.
///
/// Returns `None` if the element is not a `Function`.
#[must_use]
pub fn compute_function_fqn(model: &PureModel, element_id: ElementId) -> Option<SmolStr> {
    let node = model.get_node(element_id);
    let element = model.get_element(element_id);
    let Element::Function(func) = element else {
        return None;
    };
    Some(SmolStr::new(build_function_fqn(&node.name, func, model)))
}

/// Build the mangled name from the function's simple name, parameters,
/// and return type — matching Java's `ConcreteFunctionDefinitionNameProcessor`.
pub(crate) fn build_function_fqn(simple_name: &str, func: &Function, model: &PureModel) -> String {
    let mut builder = String::with_capacity(64);
    builder.push_str(simple_name);
    builder.push('_');

    // Parameters — each produces: Type + multSig + "_"
    for param in func.parameters.iter() {
        append_type_signature(&mut builder, &param.type_expr, model);
        append_multiplicity_signature(&mut builder, &param.multiplicity);
        builder.push('_');
    }

    // If no parameters, add an extra underscore (matching Java: funcName__)
    if func.parameters.is_empty() {
        builder.push('_');
    }

    // Return type — NO trailing "_" after multiplicity
    append_type_signature(&mut builder, &func.return_type, model);
    append_multiplicity_signature(&mut builder, &func.return_multiplicity);

    builder
}

/// Append the type name to the signature.
///
/// Java's `appendSignatureStringForType` cases:
/// - Generic (rawType == null): uses type parameter name (T, V, etc.)
/// - Unit: `Measure$UnitName`
/// - `PackageableElement`: `rawType.getName()` (String, Integer, Relation, etc.)
/// - `FunctionType`: literal `"FunctionTypeTODO"` (an unimplemented case
///   on the Java side — the string is what Java actually emits, not a
///   Rust-side forward TODO). The Rust port emits `"Function"` instead
///   for pragmatic compatibility with registered FQNs; the divergence is
///   intentional and stable.
fn append_type_signature(builder: &mut String, type_expr: &TypeExpr, model: &PureModel) {
    match type_expr {
        TypeExpr::Named { element, .. } => {
            // PackageableElement case — use the element's simple name from the model.
            let node = model.get_node(*element);
            builder.push_str(&node.name);
        }
        TypeExpr::Generic(name) => {
            // rawType == null → type parameter (T, V, etc.)
            builder.push_str(name);
        }
        TypeExpr::FunctionType { .. } => {
            // Java emits the literal string `"FunctionTypeTODO"` here
            // (its `appendSignatureStringForType` has an unimplemented
            // branch). In practice function-typed parameters use
            // `Function<>` which resolves as a Named type, so the
            // FunctionType path is rare. We deviate from Java by
            // emitting `"Function"` — the registered FQNs in the
            // function index use `Function` as the type token, so this
            // makes lookup work where Java's literal string wouldn't.
            // Intentional divergence, not a forward TODO.
            builder.push_str("Function");
        }
        TypeExpr::Relation(_) => {
            // Structural anonymous relation type (inline column bag).
            // In practice, relation-typed params resolve as Named.
            builder.push_str("Relation");
        }
        TypeExpr::GenericTypeOperation { .. } => {
            builder.push_str("Any");
        }
        TypeExpr::Unresolved => {
            // Type holes shouldn't reach FQN mangling — that's only
            // called on `Function`/property/QP signatures, not on
            // lambda-parameter types. If it does, mangle as `Any` so
            // the call doesn't blow up; the upstream
            // `CannotInferLambdaParameterTypes` diagnostic is the real
            // signal.
            builder.push_str("Any");
        }
    }
}

/// Append the multiplicity signature, matching Java's `multiplicityToSignatureString`.
///
/// Each multiplicity is wrapped in `_..._`, so the output includes the
/// leading and trailing underscores.
fn append_multiplicity_signature(builder: &mut String, mult: &Multiplicity) {
    match mult {
        Multiplicity::PureOne => builder.push_str("_1_"),
        Multiplicity::ZeroOrOne => builder.push_str("_$0_1$_"),
        Multiplicity::ZeroOrMany => builder.push_str("_MANY_"),
        Multiplicity::OneOrMany => builder.push_str("_$1_MANY$_"),
        Multiplicity::Range { lower, upper } => match upper {
            None => {
                if *lower == 0 {
                    builder.push_str("_MANY_");
                } else {
                    builder.push_str("_$");
                    builder.push_str(&lower.to_string());
                    builder.push_str("_MANY$_");
                }
            }
            Some(u) => {
                if lower == u {
                    builder.push('_');
                    builder.push_str(&u.to_string());
                    builder.push('_');
                } else {
                    builder.push_str("_$");
                    builder.push_str(&lower.to_string());
                    builder.push('_');
                    builder.push_str(&u.to_string());
                    builder.push_str("$_");
                }
            }
        },
        Multiplicity::Variable(name) => {
            builder.push('_');
            builder.push_str(name);
            builder.push('_');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap;
    use crate::nodes::function::Function;
    use crate::types::{Multiplicity, Parameter, TypeExpr};

    fn si() -> legend_pure_parser_ast::SourceInfo {
        legend_pure_parser_ast::SourceInfo::new("", 0, 0, 0, 0)
    }

    fn param(name: &str, type_id: ElementId, mult: Multiplicity) -> Parameter {
        Parameter {
            name: name.into(),
            type_expr: TypeExpr::Named {
                element: type_id,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            multiplicity: mult,
            source_info: si(),
        }
    }

    fn test_model() -> PureModel {
        crate::pipeline::compile(&[], &[]).unwrap()
    }

    #[test]
    fn plus_integer() {
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: vec![param(
                "values",
                bootstrap::INTEGER_ID,
                Multiplicity::ZeroOrMany,
            )]
            .into(),
            return_type: TypeExpr::Named {
                element: bootstrap::INTEGER_ID,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            return_multiplicity: Multiplicity::PureOne,
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        assert_eq!(
            build_function_fqn("plus", &func, &model),
            "plus_Integer_MANY__Integer_1_"
        );
    }

    #[test]
    fn if_function() {
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: vec![
                param("test", bootstrap::BOOLEAN_ID, Multiplicity::PureOne),
                Parameter {
                    name: "valid".into(),
                    type_expr: TypeExpr::FunctionType {
                        parameters: vec![],
                        return_type: Box::new(TypeExpr::Generic("T".into())),
                        return_multiplicity: Multiplicity::PureOne,
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: si(),
                },
                Parameter {
                    name: "invalid".into(),
                    type_expr: TypeExpr::FunctionType {
                        parameters: vec![],
                        return_type: Box::new(TypeExpr::Generic("T".into())),
                        return_multiplicity: Multiplicity::PureOne,
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: si(),
                },
            ]
            .into(),
            return_type: TypeExpr::Generic("T".into()),
            return_multiplicity: Multiplicity::Range {
                lower: 0,
                upper: None,
            },
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        // Note: Java's `if` uses multiplicity type parameter `m` for the return,
        // but our Multiplicity enum doesn't have type parameters yet.
        // Range { lower: 0, upper: None } = [*] which encodes as MANY.
        assert_eq!(
            build_function_fqn("if", &func, &model),
            "if_Boolean_1__Function_1__Function_1__T_MANY_"
        );
    }

    #[test]
    fn first_collection() {
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: vec![Parameter {
                name: "set".into(),
                type_expr: TypeExpr::Generic("T".into()),
                multiplicity: Multiplicity::ZeroOrMany,
                source_info: si(),
            }]
            .into(),
            return_type: TypeExpr::Generic("T".into()),
            return_multiplicity: Multiplicity::ZeroOrOne,
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        assert_eq!(
            build_function_fqn("first", &func, &model),
            "first_T_MANY__T_$0_1$_"
        );
    }

    #[test]
    fn equal_function() {
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: vec![
                param("left", bootstrap::ANY_ID, Multiplicity::ZeroOrMany),
                param("right", bootstrap::ANY_ID, Multiplicity::ZeroOrMany),
            ]
            .into(),
            return_type: TypeExpr::Named {
                element: bootstrap::BOOLEAN_ID,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            return_multiplicity: Multiplicity::PureOne,
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        assert_eq!(
            build_function_fqn("equal", &func, &model),
            "equal_Any_MANY__Any_MANY__Boolean_1_"
        );
    }

    #[test]
    fn variable_multiplicity_reverse() {
        // reverse<T|m>(values:T[m]):T[m] → reverse_T_m__T_m_
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: vec![Parameter {
                name: "values".into(),
                type_expr: TypeExpr::Generic("T".into()),
                multiplicity: Multiplicity::Variable("m".into()),
                source_info: si(),
            }]
            .into(),
            return_type: TypeExpr::Generic("T".into()),
            return_multiplicity: Multiplicity::Variable("m".into()),
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        assert_eq!(
            build_function_fqn("reverse", &func, &model),
            "reverse_T_m__T_m_"
        );
    }

    #[test]
    fn no_params_function() {
        let model = test_model();
        let func = Function {
            function_name: SmolStr::default(),
            is_native: false,
            parameters: Vec::new().into(),
            return_type: TypeExpr::Named {
                element: bootstrap::DATE_TIME_ID,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            return_multiplicity: Multiplicity::PureOne,
            body: Vec::new().into(),
            stereotypes: vec![],
            tagged_values: vec![],
        };
        assert_eq!(build_function_fqn("now", &func, &model), "now__DateTime_1_");
    }
}
