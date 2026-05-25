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

//! AST → Protocol v1 conversion.
//!
//! Converts the parser's AST types into the protocol v1 JSON model.
//! Each conversion is implemented as a `From` trait impl or a free function.
//!
//! ## Design Principles
//!
//! - **Fallible**: Conversion functions return `Result` so callers can handle
//!   serialization errors rather than panicking.
//! - **No state**: Conversions are pure functions with no shared mutable state.
//! - **Recursive**: Complex types (expressions, elements) recurse into children.

use legend_pure_parser_ast as ast;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::HasMultiplicity;
use serde::ser::Error as _;

use crate::v1;

/// Result alias for AST → Protocol conversions.
pub type Result<T> = std::result::Result<T, serde_json::Error>;

impl From<&ast::SourceInfo> for v1::source_info::SourceInformation {
    fn from(si: &ast::SourceInfo) -> Self {
        Self {
            source_id: si.source.to_string(),
            start_line: si.start_line,
            start_column: si.start_column,
            end_line: si.end_line,
            end_column: si.end_column,
        }
    }
}

impl From<ast::SourceInfo> for v1::source_info::SourceInformation {
    fn from(si: ast::SourceInfo) -> Self {
        Self::from(&si)
    }
}

/// Converts a borrowed AST [`SourceInfo`](ast::SourceInfo) into a protocol
/// `Option<SourceInformation>` without cloning.
///
/// This is the standard way to populate `source_information` fields during
/// AST → Protocol conversion.
#[allow(clippy::unnecessary_wraps)]
fn source_information(source_info: &ast::SourceInfo) -> Option<v1::source_info::SourceInformation> {
    Some(source_info.into())
}

impl From<&ast::Multiplicity> for v1::multiplicity::Multiplicity {
    fn from(m: &ast::Multiplicity) -> Self {
        Self {
            lower_bound: m.lower(),
            upper_bound: m.upper(),
        }
    }
}

/// Converts a recursive AST `Package` into a flat `"a::b::c"` path string.
fn package_to_path(pkg: &ast::type_ref::Package) -> String {
    pkg.to_string()
}

/// Converts an optional AST `Package` into the flat path string used by protocol.
fn optional_package_to_path(pkg: Option<&ast::type_ref::Package>) -> String {
    match pkg {
        Some(p) => package_to_path(p),
        None => String::new(),
    }
}

impl From<&ast::type_ref::TypeReference> for v1::generic_type::GenericType {
    fn from(tr: &ast::type_ref::TypeReference) -> Self {
        Self {
            raw_type: v1::generic_type::PackageableType {
                full_path: tr.full_path(),
                source_information: source_information(&tr.source_info),
            },
            type_arguments: tr.type_arguments.iter().map(Into::into).collect(),
            multiplicity_arguments: tr
                .multiplicity_arguments
                .iter()
                .map(|ma| match ma {
                    ast::type_ref::MultiplicityArgument::Identifier(_, _) => {
                        // Named multiplicity variables produce a placeholder multiplicity.
                        // The Java protocol uses the same representation — the name is lost
                        // and must be recovered by the compiler from parameter position.
                        v1::multiplicity::Multiplicity::PURE_ONE
                    }
                    ast::type_ref::MultiplicityArgument::Concrete(m, _) => m.into(),
                })
                .collect(),
            type_variable_values: tr
                .type_variable_values
                .iter()
                .map(type_variable_value_to_json)
                .collect(),
            source_information: source_information(&tr.source_info),
        }
    }
}

impl From<&ast::type_ref::TypeSpec> for v1::generic_type::GenericType {
    fn from(ts: &ast::type_ref::TypeSpec) -> Self {
        match ts {
            ast::type_ref::TypeSpec::Type(tr) => tr.into(),
            ast::type_ref::TypeSpec::Unit(ur) => {
                let mut gt: Self = (&ur.measure).into();
                gt.raw_type.full_path = ts.full_path();
                gt
            }
            ast::type_ref::TypeSpec::Relation(rt) => {
                // Encode relation type as GenericType with column info as type arguments.
                // Each column becomes a type argument with name = column name.
                let si = source_information(&rt.source_info);
                let type_arguments: Vec<v1::generic_type::GenericType> = rt
                    .columns
                    .iter()
                    .map(|col| {
                        let col_type: v1::generic_type::GenericType = (&col.type_ref).into();
                        let col_si = source_information(&col.source_info);
                        v1::generic_type::GenericType {
                            raw_type: v1::generic_type::PackageableType {
                                full_path: col.name.to_string(),
                                source_information: col_si.clone(),
                            },
                            type_arguments: vec![col_type],
                            multiplicity_arguments: vec![],
                            type_variable_values: vec![],
                            source_information: col_si,
                        }
                    })
                    .collect();
                v1::generic_type::GenericType {
                    raw_type: v1::generic_type::PackageableType {
                        full_path: "meta::pure::metamodel::relation::RelationType".to_string(),
                        source_information: si.clone(),
                    },
                    type_arguments,
                    multiplicity_arguments: vec![],
                    type_variable_values: vec![],
                    source_information: si,
                }
            }
            ast::type_ref::TypeSpec::Function(ft) => {
                // Function types: encode as GenericType with FunctionType path.
                let si = source_information(&ft.source_info);
                v1::generic_type::GenericType {
                    raw_type: v1::generic_type::PackageableType {
                        full_path: "meta::pure::metamodel::type::FunctionType".to_string(),
                        source_information: si.clone(),
                    },
                    type_arguments: vec![],
                    multiplicity_arguments: vec![],
                    type_variable_values: vec![],
                    source_information: si,
                }
            }
        }
    }
}

/// Converts an AST `TypeVariableValue` to a `serde_json::Value`.
fn type_variable_value_to_json(tvv: &ast::type_ref::TypeVariableValue) -> serde_json::Value {
    match tvv {
        ast::type_ref::TypeVariableValue::Integer(v, _) => {
            serde_json::json!({"_type": "integer", "value": v})
        }
        ast::type_ref::TypeVariableValue::String(v, _) => {
            serde_json::json!({"_type": "string", "value": v})
        }
    }
}

impl From<&ast::annotation::StereotypePtr> for v1::annotation::StereotypePtr {
    fn from(s: &ast::annotation::StereotypePtr) -> Self {
        Self {
            profile: s.profile.to_string(),
            value: s.value.to_string(),
            source_information: source_information(&s.source_info),
            profile_source_information: source_information(&s.profile.source_info),
        }
    }
}

impl From<&ast::annotation::TagPtr> for v1::annotation::TagPtr {
    fn from(t: &ast::annotation::TagPtr) -> Self {
        Self {
            profile: t.profile.to_string(),
            value: t.value.to_string(),
            source_information: source_information(&t.source_info),
            profile_source_information: source_information(&t.profile.source_info),
        }
    }
}

impl From<&ast::annotation::TaggedValue> for v1::annotation::TaggedValue {
    fn from(tv: &ast::annotation::TaggedValue) -> Self {
        Self {
            tag: (&tv.tag).into(),
            value: tv.value.clone(),
            source_information: source_information(&tv.source_info),
        }
    }
}

/// Converts an AST `Property` into a protocol `Property`.
///
/// # Errors
///
/// Returns an error if expression serialization within default values fails.
fn convert_property(p: &ast::element::Property) -> Result<v1::property::Property> {
    let default_value = match &p.default_value {
        Some(dv) => Some(v1::property::DefaultValue {
            value: convert_expression(dv)?,
            source_information: source_information(dv.source_info()),
        }),
        None => None,
    };
    Ok(v1::property::Property {
        name: p.name.to_string(),
        generic_type: (&p.type_ref).into(),
        multiplicity: (&p.multiplicity).into(),
        default_value,
        stereotypes: p.stereotypes.iter().map(Into::into).collect(),
        tagged_values: p.tagged_values.iter().map(Into::into).collect(),
        aggregation: p.aggregation.map(std::convert::Into::into),
        source_information: source_information(&p.source_info),
    })
}

impl From<ast::element::AggregationKind> for v1::property::AggregationKind {
    fn from(ak: ast::element::AggregationKind) -> Self {
        match ak {
            ast::element::AggregationKind::None => Self::NONE,
            ast::element::AggregationKind::Shared => Self::SHARED,
            ast::element::AggregationKind::Composite => Self::COMPOSITE,
        }
    }
}

/// Converts an AST `QualifiedProperty` into a protocol `QualifiedProperty`.
///
/// # Errors
///
/// Returns an error if parameter or body expression serialization fails.
fn convert_qualified_property(
    qp: &ast::element::QualifiedProperty,
) -> Result<v1::property::QualifiedProperty> {
    let parameters: std::result::Result<Vec<_>, _> =
        qp.parameters.iter().map(convert_parameter).collect();
    let body: std::result::Result<Vec<_>, _> = qp.body.iter().map(convert_expression).collect();
    Ok(v1::property::QualifiedProperty {
        name: qp.name.to_string(),
        parameters: parameters?,
        return_generic_type: (&qp.return_type).into(),
        return_multiplicity: (&qp.return_multiplicity).into(),
        stereotypes: qp.stereotypes.iter().map(Into::into).collect(),
        tagged_values: qp.tagged_values.iter().map(Into::into).collect(),
        body: body?,
        source_information: source_information(&qp.source_info),
    })
}

/// Converts an AST `Constraint` into a protocol `Constraint`.
///
/// # Errors
///
/// Returns an error if constraint expression serialization fails.
fn convert_constraint(c: &ast::element::Constraint) -> Result<v1::property::Constraint> {
    Ok(v1::property::Constraint {
        name: c
            .name
            .as_ref()
            .map_or_else(|| "constraint".to_string(), ToString::to_string),
        owner: None,
        function_definition: convert_expression(&c.function_definition)?,
        source_information: source_information(&c.source_info),
        external_id: c.external_id.clone(),
        enforcement_level: c.enforcement_level.as_ref().map(ToString::to_string),
        message_function: c.message.as_ref().map(convert_expression).transpose()?,
    })
}

/// Converts a `Parameter` to a serialized variable value specification.
///
/// # Errors
///
/// Returns an error if the variable serialization fails.
fn convert_parameter(p: &ast::annotation::Parameter) -> Result<serde_json::Value> {
    let var = v1::value_spec::Variable {
        name: p.name.to_string(),
        generic_type: p.type_ref.as_ref().map(Into::into),
        multiplicity: p.multiplicity.as_ref().map(Into::into),
        supports_stream: None,
        source_information: source_information(&p.source_info),
    };
    let vs = v1::value_spec::ValueSpecification::Var(var);
    serde_json::to_value(&vs)
}

/// Converts an AST `Expression` into a protocol `serde_json::Value`.
///
/// Returns a `serde_json::Value` rather than a typed `ValueSpecification`
/// because expressions can appear in contexts that expect `serde_json::Value`
/// (e.g., constraint function definitions, qualified property bodies).
///
/// # Errors
///
/// Returns an error if the `ValueSpecification` cannot be serialized to JSON.
pub fn convert_expression(expr: &ast::expression::Expression) -> Result<serde_json::Value> {
    let vs = convert_expression_typed(expr);
    serde_json::to_value(&vs)
}

/// Converts an AST `Expression` into a typed `ValueSpecification`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn convert_expression_typed(
    expr: &ast::expression::Expression,
) -> v1::value_spec::ValueSpecification {
    use ast::expression::Expression;
    use v1::value_spec::AppliedFunction;
    use v1::value_spec::CString;
    use v1::value_spec::LambdaFunction;
    use v1::value_spec::ProtocolCollection;
    use v1::value_spec::ProtocolKeyExpression;
    use v1::value_spec::ProtocolPackageableElementPtr;
    use v1::value_spec::ValueSpecification;
    use v1::value_spec::Variable;

    match expr {
        // -- Literals --
        Expression::Literal(lit) => convert_literal(lit),

        // -- Variables --
        Expression::Variable(var) => ValueSpecification::Var(Variable {
            name: var.name.to_string(),
            generic_type: None,
            multiplicity: None,
            supports_stream: None,
            source_information: source_information(&var.source_info),
        }),

        // -- Operators (desugared to function applications) --
        Expression::Arithmetic(e) => {
            let func_name = match e.op {
                ast::expression::ArithmeticOp::Plus => "plus",
                ast::expression::ArithmeticOp::Minus => "minus",
                ast::expression::ArithmeticOp::Times => "times",
                ast::expression::ArithmeticOp::Divide => "divide",
            };
            make_func(func_name, &[&e.left, &e.right], &e.source_info)
        }
        Expression::Comparison(e) => {
            let func_name = match e.op {
                ast::expression::ComparisonOp::Equal => "equal",
                ast::expression::ComparisonOp::NotEqual => "notEqual",
                ast::expression::ComparisonOp::LessThan => "lessThan",
                ast::expression::ComparisonOp::LessThanOrEqual => "lessThanEqual",
                ast::expression::ComparisonOp::GreaterThan => "greaterThan",
                ast::expression::ComparisonOp::GreaterThanOrEqual => "greaterThanEqual",
            };
            make_func(func_name, &[&e.left, &e.right], &e.source_info)
        }
        Expression::Logical(e) => {
            let func_name = match e.op {
                ast::expression::LogicalOp::And => "and",
                ast::expression::LogicalOp::Or => "or",
            };
            make_func(func_name, &[&e.left, &e.right], &e.source_info)
        }
        Expression::Bitwise(e) => {
            let func_name = match e.op {
                ast::expression::BitwiseOp::And => "bitwiseAnd",
                ast::expression::BitwiseOp::Or => "bitwiseOr",
                ast::expression::BitwiseOp::Xor => "bitwiseXor",
                ast::expression::BitwiseOp::ShiftLeft => "shiftLeft",
                ast::expression::BitwiseOp::ShiftRight => "shiftRight",
            };
            make_func(func_name, &[&e.left, &e.right], &e.source_info)
        }
        Expression::Not(e) => make_func("not", &[&e.operand], &e.source_info),
        Expression::UnaryMinus(e) => make_func("minus", &[&e.operand], &e.source_info),
        Expression::BitwiseNot(e) => make_func("bitwiseNot", &[&e.operand], &e.source_info),

        // -- Function application --
        Expression::FunctionApplication(e) => ValueSpecification::Func(AppliedFunction {
            function: e.function.to_string(),
            f_control: None,
            parameters: e.arguments.iter().map(convert_expression_typed).collect(),
            source_information: source_information(&e.source_info),
        }),

        // -- Arrow function: `expr->func(args)` desugars to func(expr, args) --
        Expression::ArrowFunction(e) => ValueSpecification::Func(AppliedFunction {
            function: e.function.to_string(),
            f_control: None,
            parameters: std::iter::once(convert_expression_typed(&e.target))
                .chain(e.arguments.iter().map(convert_expression_typed))
                .collect(),
            source_information: source_information(&e.source_info),
        }),

        // -- Member access --
        Expression::MemberAccess(ma) => convert_member_access(ma),

        // -- Type reference: `@MyType` → packageableElementPtr --
        Expression::TypeReferenceExpr(e) => {
            ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                full_path: e.type_ref.full_path(),
                source_information: source_information(&e.source_info),
            })
        }

        //
        // Settled wire form (mirrors what Java's `MultiplicityInstance`
        // round-trips to at the metamodel level):
        //
        // - Well-known multiplicities (`@[1]`, `@[0..1]`, `@[*]`,
        //   `@[1..*]`) → `packageableElementPtr` pointing at the
        //   corresponding `meta::pure::metamodel::multiplicity::{PureOne,
        //   ZeroOne, ZeroMany, OneMany}` `PackageableMultiplicity`
        //   instance.
        // - Arbitrary ranges (`@[2..5]`) → `classInstance("multiplicity",
        //   { lowerBound, upperBound? })`.
        // - Parameter variables (`@[m]`) → `classInstance("multiplicity",
        //   { multiplicityParameter })`.
        Expression::MultiplicityReferenceExpr(e) => {
            multiplicity_arg_to_value_spec(&e.multiplicity, source_information(&e.source_info))
        }

        // -- Lambda --
        Expression::Lambda(e) => ValueSpecification::Lambda(LambdaFunction {
            body: e.body.iter().map(convert_expression_typed).collect(),
            parameters: e
                .parameters
                .iter()
                .map(|p| Variable {
                    name: p.name.to_string(),
                    generic_type: p.type_ref.as_ref().map(Into::into),
                    multiplicity: p.multiplicity.as_ref().map(Into::into),
                    supports_stream: None,
                    source_information: source_information(&p.source_info),
                })
                .collect(),
            source_information: source_information(&e.source_info),
        }),

        // -- Let: desugared to letFunction('name', expr) --
        Expression::Let(e) => {
            let name_val = ValueSpecification::String(CString {
                value: e.name.to_string(),
                source_information: source_information(&e.source_info),
            });
            let value_val = convert_expression_typed(&e.value);
            ValueSpecification::Func(AppliedFunction {
                function: "letFunction".to_string(),
                f_control: None,
                parameters: vec![name_val, value_val],
                source_information: source_information(&e.source_info),
            })
        }

        // -- Collection literal --
        Expression::Collection(e) => ValueSpecification::Collection(ProtocolCollection {
            multiplicity: collection_multiplicity(e.elements.len()),
            values: e.elements.iter().map(convert_expression_typed).collect(),
            source_information: source_information(&e.source_info),
        }),

        // Slice expressions are desugared by the compiler to range() calls;
        // emit an empty collection as placeholder for protocol serialization.
        Expression::Slice(e) => ValueSpecification::Collection(ProtocolCollection {
            multiplicity: collection_multiplicity(0),
            values: vec![],
            source_information: source_information(&e.source_info),
        }),

        // -- New instance: `^MyClass(name='John')` → classInstance --
        Expression::NewInstance(e) => {
            let key_expressions: Vec<ValueSpecification> = e
                .assignments
                .iter()
                .map(|a| {
                    ValueSpecification::KeyExpression(ProtocolKeyExpression {
                        add: false,
                        key: Box::new(ValueSpecification::String(CString {
                            value: a.key.to_string(),
                            source_information: source_information(&a.source_info),
                        })),
                        expression: Box::new(convert_expression_typed(&a.value)),
                        source_information: source_information(&a.source_info),
                    })
                })
                .collect();
            // Wrap as func call: new(Class, '', [key-expressions])
            let class_ref =
                ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                    full_path: e.class.to_string(),
                    source_information: source_information(&e.source_info),
                });
            let empty_name = ValueSpecification::String(CString {
                value: e
                    .instance_name
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                source_information: None,
            });
            let keys_collection = ValueSpecification::Collection(ProtocolCollection {
                multiplicity: collection_multiplicity(key_expressions.len()),
                values: key_expressions,
                source_information: source_information(&e.source_info),
            });
            ValueSpecification::Func(AppliedFunction {
                function: "new".to_string(),
                f_control: None,
                parameters: vec![class_ref, empty_name, keys_collection],
                source_information: source_information(&e.source_info),
            })
        }

        // -- Copy from variable: `^$var(props)` → copy(var, key-expressions) --
        Expression::Copy(e) => {
            let key_expressions: Vec<ValueSpecification> = e
                .assignments
                .iter()
                .map(|a| {
                    ValueSpecification::KeyExpression(ProtocolKeyExpression {
                        add: false,
                        key: Box::new(ValueSpecification::String(CString {
                            value: a.key.to_string(),
                            source_information: source_information(&a.source_info),
                        })),
                        expression: Box::new(convert_expression_typed(&a.value)),
                        source_information: source_information(&a.source_info),
                    })
                })
                .collect();
            let source_var = ValueSpecification::Var(Variable {
                name: e.source.to_string(),
                generic_type: None,
                multiplicity: None,
                supports_stream: None,
                source_information: source_information(&e.source_info),
            });
            let keys_collection = ValueSpecification::Collection(ProtocolCollection {
                multiplicity: collection_multiplicity(key_expressions.len()),
                values: key_expressions,
                source_information: source_information(&e.source_info),
            });
            ValueSpecification::Func(AppliedFunction {
                function: "copy".to_string(),
                f_control: None,
                parameters: vec![source_var, keys_collection],
                source_information: source_information(&e.source_info),
            })
        }

        // -- Column expressions → classInstance --
        Expression::Column(col) => convert_column(col),

        // -- Bare element reference (no args): same as zero-arg function in protocol --
        Expression::PackageableElementRef(e) => ValueSpecification::Func(AppliedFunction {
            function: e.element.to_string(),
            f_control: None,
            parameters: vec![],
            source_information: source_information(&e.source_info),
        }),

        // -- Island grammar: graph fetch → classInstance --
        Expression::Island(island) => convert_island_expression(island),

        // -- Navigation path: `#/Type/p1/p2!alias#` → classInstance("path", { startType, path, name }) --
        Expression::NavigationPath(nav) => convert_navigation_path(nav),

        // -- Unit instance: `5 RomanLength~Pes` → newUnit(unit, value) --
        Expression::UnitInstance(e) => {
            let unit_ref =
                ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                    full_path: e.unit.to_string(),
                    source_information: source_information(&e.source_info),
                });
            let value = convert_expression_typed(&e.value);
            ValueSpecification::Func(AppliedFunction {
                function: "newUnit".to_string(),
                f_control: None,
                parameters: vec![unit_ref, value],
                source_information: source_information(&e.source_info),
            })
        }

        // -- Grouping (transparent) --
        Expression::Group(inner) => convert_expression_typed(inner),
    }
}

/// Converts a literal AST node to a `ValueSpecification`.
fn convert_literal(lit: &ast::expression::Literal) -> v1::value_spec::ValueSpecification {
    use ast::expression::Literal;
    use v1::value_spec::CBoolean;
    use v1::value_spec::CDateTime;
    use v1::value_spec::CDecimal;
    use v1::value_spec::CFloat;
    use v1::value_spec::CInteger;
    use v1::value_spec::CStrictDate;
    use v1::value_spec::CStrictTime;
    use v1::value_spec::CString;
    use v1::value_spec::ValueSpecification;

    match lit {
        Literal::Integer(e) => ValueSpecification::Integer(CInteger {
            value: e.value,
            source_information: source_information(&e.source_info),
        }),
        Literal::Float(e) => ValueSpecification::Float(CFloat {
            value: e.value,
            source_information: source_information(&e.source_info),
        }),
        Literal::Decimal(e) => ValueSpecification::Decimal(CDecimal {
            value: e.value.parse::<f64>().unwrap_or(0.0),
            source_information: source_information(&e.source_info),
        }),
        Literal::String(e) => ValueSpecification::String(CString {
            value: e.value.to_string(),
            source_information: source_information(&e.source_info),
        }),
        Literal::Boolean(e) => ValueSpecification::Boolean(CBoolean {
            value: e.value,
            source_information: source_information(&e.source_info),
        }),
        Literal::StrictDate(e) => ValueSpecification::StrictDate(CStrictDate {
            value: e.value.clone(),
            source_information: source_information(&e.source_info),
        }),
        Literal::DateTime(e) => ValueSpecification::DateTime(CDateTime {
            value: e.value.clone(),
            source_information: source_information(&e.source_info),
        }),
        Literal::StrictTime(e) => ValueSpecification::StrictTime(CStrictTime {
            value: e.value.clone(),
            source_information: source_information(&e.source_info),
        }),
    }
}

/// Helper to create an `AppliedFunction` from a function name and arguments.
fn make_func(
    name: &str,
    args: &[&ast::expression::Expression],
    source_info: &ast::SourceInfo,
) -> v1::value_spec::ValueSpecification {
    v1::value_spec::ValueSpecification::Func(v1::value_spec::AppliedFunction {
        function: name.to_string(),
        f_control: None,
        parameters: args.iter().copied().map(convert_expression_typed).collect(),
        source_information: source_information(source_info),
    })
}

/// Creates a multiplicity with both bounds set to `len` (exact collection size).
#[allow(clippy::cast_possible_truncation)]
fn collection_multiplicity(len: usize) -> v1::multiplicity::Multiplicity {
    let bound = len as u32;
    v1::multiplicity::Multiplicity {
        lower_bound: bound,
        upper_bound: Some(bound),
    }
}

/// Converts a `MemberAccess` into a property value specification.
fn convert_member_access(ma: &ast::expression::MemberAccess) -> v1::value_spec::ValueSpecification {
    use ast::expression::MemberAccess;
    use v1::value_spec::AppliedProperty;
    use v1::value_spec::ValueSpecification;

    match ma {
        MemberAccess::Simple(e) => ValueSpecification::Property(AppliedProperty {
            class: None,
            property: e.member.to_string(),
            parameters: vec![convert_expression_typed(&e.target)],
            source_information: source_information(&e.source_info),
        }),
        MemberAccess::Qualified(e) => ValueSpecification::Property(AppliedProperty {
            class: None,
            property: e.member.to_string(),
            parameters: std::iter::once(convert_expression_typed(&e.target))
                .chain(e.arguments.iter().map(convert_expression_typed))
                .collect(),
            source_information: source_information(&e.source_info),
        }),
    }
}

/// Converts a `ColumnBuilderExpr` into a `colSpec` value specification.
/// Note: Currently only converts the first column spec into a `colSpec`.
/// Full `colSpecArray` support pending.
fn convert_column(e: &ast::expression::ColumnBuilderExpr) -> v1::value_spec::ValueSpecification {
    use ast::expression::ColumnTypeSpec;
    use v1::value_spec::ClassInstance;
    use v1::value_spec::ValueSpecification;

    let mut value_map = serde_json::Map::new();
    let Some(col) = e.columns.first() else {
        return ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::Value::Object(value_map),
            source_information: source_information(&e.source_info),
        });
    };
    value_map.insert("name".to_string(), serde_json::json!(col.name.to_string()));

    // In a real implementation we would convert the annotations, tagged values, etc.
    if let Some(type_spec) = &col.type_spec {
        match type_spec {
            ColumnTypeSpec::Typed(tr, _) => {
                value_map.insert("type".to_string(), serde_json::json!(tr.full_path()));
            }
            ColumnTypeSpec::Lambda(lambda) => {
                let lambda_spec =
                    convert_expression_typed(&ast::expression::Expression::Lambda(lambda.clone()));
                if let Ok(lambda_val) = serde_json::to_value(&lambda_spec) {
                    value_map.insert("function1".to_string(), lambda_val);
                }
            }
        }
    }

    #[allow(clippy::collapsible_if)]
    if let Some(func) = &col.extra_function {
        if let ast::expression::Expression::PackageableElementRef(r) = &**func {
            value_map.insert(
                "function1".to_string(),
                serde_json::json!(r.element.name.to_string()),
            );
        }
    }

    ValueSpecification::ClassInstance(ClassInstance {
        type_name: "colSpec".to_string(),
        value: serde_json::Value::Object(value_map),
        source_information: source_information(&e.source_info),
    })
}

/// Converts a `NavigationPath` into a `classInstance("path", ...)`.
///
/// Wire format (mirrors Legend Engine's `NavigationPathComposer`):
/// ```json
/// {
///   "_type": "classInstance",
///   "type": "path",
///   "value": {
///     "startType": "model::Person",
///     "path": [
///       { "_type": "propertyPathElement", "property": "name", "parameters": [...] },
///       ...
///     ],
///     "name": "alias"   // optional
///   }
/// }
/// ```
fn convert_navigation_path(
    nav: &ast::expression::NavigationPath,
) -> v1::value_spec::ValueSpecification {
    let mut value_map = serde_json::Map::new();
    value_map.insert(
        "startType".to_string(),
        serde_json::Value::String(nav.start_type.full_path()),
    );

    let path_steps: Vec<serde_json::Value> = nav
        .path
        .iter()
        .map(|step| {
            let params: Vec<serde_json::Value> = step
                .parameters
                .iter()
                .map(|p| {
                    serde_json::to_value(convert_expression_typed(p))
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect();
            serde_json::json!({
                "_type": "propertyPathElement",
                "property": step.property.to_string(),
                "parameters": params,
            })
        })
        .collect();
    value_map.insert("path".to_string(), serde_json::Value::Array(path_steps));

    if let Some(alias) = &nav.name {
        value_map.insert(
            "name".to_string(),
            serde_json::Value::String(alias.to_string()),
        );
    }

    v1::value_spec::ValueSpecification::ClassInstance(v1::value_spec::ClassInstance {
        type_name: "path".to_string(),
        value: serde_json::Value::Object(value_map),
        source_information: source_information(&nav.source_info),
    })
}

/// Converts an island grammar expression into a `ValueSpecification`.
///
/// Without a registered [`IslandProtocol`](crate::IslandProtocol)
/// converter for the island's tag, falls back to a placeholder JSON
/// shape. Callers that need concrete graph-fetch / TDS / store JSON
/// register the matching DSL crate's protocol converter and call
/// [`crate::dispatch_island_convert`] directly.
fn convert_island_expression(
    island: &ast::island::IslandExpression,
) -> v1::value_spec::ValueSpecification {
    v1::value_spec::ValueSpecification::ClassInstance(v1::value_spec::ClassInstance {
        type_name: format!("unknownIsland_{}", island.tag()),
        value: serde_json::json!({}),
        source_information: source_information(&island.source_info),
    })
}

/// Converts an AST `Element` into a protocol `PackageableElement`.
///
/// # Errors
///
/// Returns an error if any expression serialization within the element fails.
pub fn convert_element(elem: &ast::element::Element) -> Result<v1::element::PackageableElement> {
    use ast::element::Element;
    use v1::element::PackageableElement;

    match elem {
        Element::Class(c) => Ok(PackageableElement::Class(convert_class(c)?)),
        Element::Enumeration(e) => Ok(PackageableElement::Enumeration(convert_enumeration(e))),
        Element::Function(f) => Ok(PackageableElement::Function(convert_function(f))),
        Element::NativeFunction(_) => Err(serde_json::Error::custom(
            "native functions cannot be converted to Engine protocol",
        )),
        Element::Profile(p) => Ok(PackageableElement::Profile(convert_profile(p))),
        Element::Association(a) => Ok(PackageableElement::Association(convert_association(a)?)),
        Element::Measure(m) => Ok(PackageableElement::Measure(convert_measure(m))),
        Element::Primitive(_) => Err(serde_json::Error::custom(
            "primitive type definitions cannot be converted to Engine protocol",
        )),
        // DSL elements are converted by their owning DSL crate via a
        // future SectionProtocol plug-in. Core only knows how to
        // emit M3 elements.
        Element::DSLElement(e) => Err(serde_json::Error::custom(format!(
            "DSL element of kind '{}' must be converted by its owning DSL crate",
            e.kind()
        ))),
    }
}

fn convert_class(c: &ast::element::ClassDef) -> Result<v1::element::ProtocolClass> {
    let properties: std::result::Result<Vec<_>, _> =
        c.properties.iter().map(convert_property).collect();
    let qualified_properties: std::result::Result<Vec<_>, _> = c
        .qualified_properties
        .iter()
        .map(convert_qualified_property)
        .collect();
    let constraints: std::result::Result<Vec<_>, _> =
        c.constraints.iter().map(convert_constraint).collect();
    Ok(v1::element::ProtocolClass {
        package_path: optional_package_to_path(c.package.as_ref()),
        name: c.name.value.to_string(),
        super_types: c
            .super_types
            .iter()
            .map(ast::type_ref::TypeReference::full_path)
            .collect(),
        properties: properties?,
        qualified_properties: qualified_properties?,
        constraints: constraints?,
        original_milestoned_properties: vec![],
        stereotypes: c.stereotypes.iter().map(Into::into).collect(),
        tagged_values: c.tagged_values.iter().map(Into::into).collect(),
        source_information: source_information(&c.source_info),
    })
}

fn convert_enumeration(e: &ast::element::EnumDef) -> v1::element::ProtocolEnumeration {
    v1::element::ProtocolEnumeration {
        package_path: optional_package_to_path(e.package.as_ref()),
        name: e.name.value.to_string(),
        values: e.values.iter().map(convert_enum_value).collect(),
        stereotypes: e.stereotypes.iter().map(Into::into).collect(),
        tagged_values: e.tagged_values.iter().map(Into::into).collect(),
        source_information: source_information(&e.source_info),
    }
}

fn convert_enum_value(v: &ast::element::EnumValue) -> v1::element::ProtocolEnumMember {
    v1::element::ProtocolEnumMember {
        value: v.name.to_string(),
        stereotypes: v.stereotypes.iter().map(Into::into).collect(),
        tagged_values: v.tagged_values.iter().map(Into::into).collect(),
        source_information: source_information(&v.source_info),
    }
}

fn convert_function(f: &ast::element::FunctionDef) -> v1::element::ProtocolFunction {
    v1::element::ProtocolFunction {
        package_path: optional_package_to_path(f.package.as_ref()),
        name: f.name.value.to_string(),
        parameters: f
            .parameters
            .iter()
            .map(|p| v1::value_spec::Variable {
                name: p.name.to_string(),
                generic_type: p.type_ref.as_ref().map(Into::into),
                multiplicity: p.multiplicity.as_ref().map(Into::into),
                supports_stream: None,
                source_information: source_information(&p.source_info),
            })
            .collect(),
        return_generic_type: (&f.return_type).into(),
        return_multiplicity: (&f.return_multiplicity).into(),
        body: f.body.iter().map(convert_expression_typed).collect(),
        stereotypes: f.stereotypes.iter().map(Into::into).collect(),
        tagged_values: f.tagged_values.iter().map(Into::into).collect(),
        tests: vec![], // Function tests are not in scope for v1
        pre_constraints: vec![],
        post_constraints: vec![],
        source_information: source_information(&f.source_info),
    }
}

fn convert_profile(p: &ast::element::ProfileDef) -> v1::element::ProtocolProfile {
    v1::element::ProtocolProfile {
        package_path: optional_package_to_path(p.package.as_ref()),
        name: p.name.value.to_string(),
        stereotypes: p
            .stereotype_names
            .iter()
            .map(|s| s.value.to_string())
            .collect(),
        tags: p.tag_names.iter().map(|t| t.value.to_string()).collect(),
        source_information: source_information(&p.source_info),
    }
}

fn convert_association(
    a: &ast::element::AssociationDef,
) -> Result<v1::element::ProtocolAssociation> {
    let properties: std::result::Result<Vec<_>, _> =
        a.properties.iter().map(convert_property).collect();
    let qualified_properties: std::result::Result<Vec<_>, _> = a
        .qualified_properties
        .iter()
        .map(convert_qualified_property)
        .collect();
    Ok(v1::element::ProtocolAssociation {
        package_path: optional_package_to_path(a.package.as_ref()),
        name: a.name.value.to_string(),
        properties: properties?,
        qualified_properties: qualified_properties?,
        original_milestoned_properties: vec![],
        stereotypes: a.stereotypes.iter().map(Into::into).collect(),
        tagged_values: a.tagged_values.iter().map(Into::into).collect(),
        source_information: source_information(&a.source_info),
    })
}

fn convert_measure(m: &ast::element::MeasureDef) -> v1::element::ProtocolMeasure {
    v1::element::ProtocolMeasure {
        package_path: optional_package_to_path(m.package.as_ref()),
        name: m.name.value.to_string(),
        canonical_unit: m.canonical_unit.as_ref().map(|u| convert_unit(m, u)),
        non_canonical_units: m
            .non_canonical_units
            .iter()
            .map(|u| convert_unit(m, u))
            .collect(),
        source_information: source_information(&m.source_info),
    }
}

fn convert_unit(
    measure: &ast::element::MeasureDef,
    unit: &ast::element::UnitDef,
) -> v1::element::ProtocolUnit {
    // Unit package path = measure's fully qualified name (e.g., "pkg::Measure")
    let measure_fqn = match &measure.package {
        Some(pkg) => format!("{pkg}::{}", measure.name.value),
        None => measure.name.value.to_string(),
    };
    v1::element::ProtocolUnit {
        package_path: optional_package_to_path(measure.package.as_ref()),
        name: format!("{measure_fqn}~{}", unit.name),
        conversion_function: unit.conversion_body.as_ref().map(|body| {
            v1::value_spec::LambdaFunction {
                body: vec![convert_expression_typed(body)],
                parameters: unit
                    .conversion_param
                    .as_ref()
                    .map(|p| {
                        vec![v1::value_spec::Variable {
                            name: p.to_string(),
                            generic_type: None,
                            multiplicity: None,
                            supports_stream: None,
                            source_information: None,
                        }]
                    })
                    .unwrap_or_default(),
                source_information: source_information(&unit.source_info),
            }
        }),
        super_types: vec![measure_fqn],
        source_information: source_information(&unit.source_info),
    }
}

/// Convert an AST `MultiplicityArgument` (the `@[m]` / `@[1..*]` /
/// `@[*]` form in expression position) to a protocol
/// `ValueSpecification`. Settled wire form:
///
/// - Well-known multiplicities (`@[1]`, `@[0..1]`, `@[*]`, `@[1..*]`)
///   → `packageableElementPtr` pointing at
///   `meta::pure::metamodel::multiplicity::{PureOne, ZeroOne,
///   ZeroMany, OneMany}`.
/// - Arbitrary ranges (`@[2..5]`) → `classInstance("multiplicity",
///   { lowerBound, upperBound? })`.
/// - Parameter variables (`@[m]`) → `classInstance("multiplicity",
///   { multiplicityParameter })`.
///
/// Mirrors what Java's `MultiplicityInstance.createPersistent` builds
/// at the metamodel level — the engine then serialises that instance
/// to the same JSON shape.
#[must_use]
pub fn multiplicity_arg_to_value_spec(
    arg: &ast::type_ref::MultiplicityArgument,
    source_information: Option<v1::source_info::SourceInformation>,
) -> v1::value_spec::ValueSpecification {
    use ast::type_ref::MultiplicityArgument;
    match arg {
        MultiplicityArgument::Identifier(name, _) => {
            multiplicity_variable_value_spec(name.as_str(), source_information)
        }
        MultiplicityArgument::Concrete(m, _) => multiplicity_to_value_spec(m, source_information),
    }
}

/// Convert an AST `Multiplicity` (concrete bounds, no variable case)
/// to a protocol `ValueSpecification` using the same wire form as
/// [`multiplicity_arg_to_value_spec`].
#[must_use]
pub fn multiplicity_to_value_spec(
    m: &ast::type_ref::Multiplicity,
    source_information: Option<v1::source_info::SourceInformation>,
) -> v1::value_spec::ValueSpecification {
    use ast::type_ref::Multiplicity as AM;
    use v1::multiplicity::Multiplicity as PM;
    let pm = match m {
        AM::PureOne => PM::PURE_ONE,
        AM::ZeroOrOne => PM::ZERO_ONE,
        AM::ZeroOrMany => PM::ZERO_MANY,
        AM::OneOrMany => PM::ONE_MANY,
        AM::Range {
            lower,
            upper: Some(u),
        } => PM {
            lower_bound: *lower,
            upper_bound: Some(*u),
        },
        AM::Range { lower, upper: None } => PM {
            lower_bound: *lower,
            upper_bound: None,
        },
        AM::Variable(name) => return multiplicity_variable_value_spec(name, source_information),
    };
    protocol_multiplicity_to_value_spec(&pm, source_information)
}

/// Convert a protocol `Multiplicity` directly to its `ValueSpecification`
/// representation. Used by callers that already have the resolved
/// bounds in protocol form (e.g. the Pure→Protocol path in the CLI
/// crate, which renders lowered `Multiplicity` through
/// `crate::commands::parse_compiled::render_multiplicity`).
#[must_use]
pub fn protocol_multiplicity_to_value_spec(
    m: &v1::multiplicity::Multiplicity,
    source_information: Option<v1::source_info::SourceInformation>,
) -> v1::value_spec::ValueSpecification {
    use v1::value_spec::{ClassInstance, ProtocolPackageableElementPtr, ValueSpecification};
    if let Some(path) = m.well_known_path() {
        return ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
            full_path: path.to_string(),
            source_information,
        });
    }
    let mut value_map = serde_json::Map::new();
    value_map.insert(
        "lowerBound".to_string(),
        serde_json::Value::Number(m.lower_bound.into()),
    );
    if let Some(u) = m.upper_bound {
        value_map.insert(
            "upperBound".to_string(),
            serde_json::Value::Number(u.into()),
        );
    }
    ValueSpecification::ClassInstance(ClassInstance {
        type_name: "multiplicity".to_string(),
        value: serde_json::Value::Object(value_map),
        source_information,
    })
}

fn multiplicity_variable_value_spec(
    name: &str,
    source_information: Option<v1::source_info::SourceInformation>,
) -> v1::value_spec::ValueSpecification {
    use v1::value_spec::{ClassInstance, ValueSpecification};
    let mut value_map = serde_json::Map::new();
    value_map.insert(
        "multiplicityParameter".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    ValueSpecification::ClassInstance(ClassInstance {
        type_name: "multiplicity".to_string(),
        value: serde_json::Value::Object(value_map),
        source_information,
    })
}

/// Converts a parsed `SourceFile` into a `PureModelContextData`.
///
/// This is the top-level entry point for AST → Protocol conversion.
///
/// # Errors
///
/// Returns an error if any expression serialization within the source file fails.
pub fn convert_source_file(
    source_file: &ast::section::SourceFile,
) -> Result<v1::context::PureModelContextData> {
    use ast::element::PackageableElement as _;

    let mut elements: Vec<v1::element::PackageableElement> = source_file
        .all_elements()
        .map(convert_element)
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Build a section index from the source file's sections
    let sections: Vec<v1::element::ProtocolSection> = source_file
        .sections
        .iter()
        .map(|section| {
            let element_paths: Vec<String> = section
                .elements
                .iter()
                .map(|e| match e.package() {
                    Some(pkg) => format!("{pkg}::{}", e.name()),
                    None => e.name().to_string(),
                })
                .collect();

            if section.imports.is_empty() {
                v1::element::ProtocolSection::Default(v1::element::DefaultCodeSection {
                    parser_name: section.kind.to_string(),
                    elements: element_paths,
                    source_information: source_information(&section.source_info),
                })
            } else {
                v1::element::ProtocolSection::ImportAware(v1::element::ImportAwareCodeSection {
                    parser_name: section.kind.to_string(),
                    elements: element_paths,
                    imports: section.imports.iter().map(|i| i.path.to_string()).collect(),
                    source_information: source_information(&section.source_info),
                })
            }
        })
        .collect();

    let source_id = source_file.source_info.source.to_string();
    let section_index =
        v1::element::PackageableElement::SectionIndex(v1::element::ProtocolSectionIndex {
            package_path: "__internal__".to_string(),
            name: source_id,
            sections,
            source_information: source_information(&source_file.source_info),
        });
    elements.push(section_index);

    Ok(v1::context::PureModelContextData::new(elements))
}

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_parser_ast as ast;
    use legend_pure_parser_ast::type_ref::Identifier;

    fn src() -> ast::SourceInfo {
        ast::SourceInfo::new("test.pure", 1, 1, 1, 10)
    }

    #[test]
    fn test_source_info_conversion() {
        let ast_si = ast::SourceInfo::new("test.pure", 3, 5, 10, 20);
        let proto_si: v1::source_info::SourceInformation = (&ast_si).into();
        assert_eq!(proto_si.source_id, "test.pure");
        assert_eq!(proto_si.start_line, 3);
        assert_eq!(proto_si.start_column, 5);
        assert_eq!(proto_si.end_line, 10);
        assert_eq!(proto_si.end_column, 20);
    }

    #[test]
    fn test_multiplicity_conversion() {
        let pure_one: v1::multiplicity::Multiplicity = (&ast::Multiplicity::one()).into();
        assert_eq!(pure_one, v1::multiplicity::Multiplicity::PURE_ONE);

        let zero_many: v1::multiplicity::Multiplicity = (&ast::Multiplicity::zero_or_many()).into();
        assert_eq!(zero_many, v1::multiplicity::Multiplicity::ZERO_MANY);

        let zero_one: v1::multiplicity::Multiplicity = (&ast::Multiplicity::zero_or_one()).into();
        assert_eq!(zero_one, v1::multiplicity::Multiplicity::ZERO_ONE);
    }

    #[test]
    fn test_package_to_path() {
        let pkg = ast::type_ref::Package::root(Identifier::new("meta"), src())
            .child(Identifier::new("pure"), src())
            .child(Identifier::new("profiles"), src());
        assert_eq!(package_to_path(&pkg), "meta::pure::profiles");
    }

    #[test]
    fn test_type_reference_conversion() {
        let tr = ast::type_ref::TypeReference {
            package: None,
            name: Identifier::new("String"),
            type_arguments: vec![],
            multiplicity_arguments: vec![],
            type_variable_values: vec![],
            algebra_ops: vec![],
            subset_bound: None,
            equal_binding: None,
            source_info: src(),
        };
        let gt: v1::generic_type::GenericType = (&tr).into();
        assert_eq!(gt.raw_type.full_path, "String");
        assert!(gt.type_arguments.is_empty());
    }

    #[test]
    fn test_stereotype_ptr_conversion() {
        let ast_sp = ast::annotation::StereotypePtr {
            profile: ast::annotation::PackageableElementPtr {
                package: Some(
                    ast::type_ref::Package::root(Identifier::new("meta"), src())
                        .child(Identifier::new("pure"), src())
                        .child(Identifier::new("profiles"), src()),
                ),
                name: Identifier::new("temporal"),
                source_info: src(),
            },
            value: Identifier::new("businesstemporal"),
            source_info: src(),
        };
        let proto_sp: v1::annotation::StereotypePtr = (&ast_sp).into();
        assert_eq!(proto_sp.profile, "meta::pure::profiles::temporal");
        assert_eq!(proto_sp.value, "businesstemporal");
    }

    #[test]
    fn test_integer_literal_conversion() {
        let expr = ast::expression::Expression::Literal(ast::expression::Literal::Integer(
            ast::expression::IntegerLiteral {
                value: 42,
                source_info: src(),
            },
        ));
        let vs = convert_expression_typed(&expr);
        match vs {
            v1::value_spec::ValueSpecification::Integer(ci) => {
                assert_eq!(ci.value, 42);
            }
            other => panic!("Expected Integer, got {other:?}"),
        }
    }

    #[test]
    fn test_variable_conversion() {
        let expr = ast::expression::Expression::Variable(ast::expression::Variable {
            name: Identifier::new("name"),
            source_info: src(),
        });
        let vs = convert_expression_typed(&expr);
        match vs {
            v1::value_spec::ValueSpecification::Var(v) => {
                assert_eq!(v.name, "name");
            }
            other => panic!("Expected Var, got {other:?}"),
        }
    }

    #[test]
    fn test_arithmetic_desugars_to_func() {
        let expr = ast::expression::Expression::Arithmetic(ast::expression::ArithmeticExpr {
            left: Box::new(ast::expression::Expression::Literal(
                ast::expression::Literal::Integer(ast::expression::IntegerLiteral {
                    value: 1,
                    source_info: src(),
                }),
            )),
            op: ast::expression::ArithmeticOp::Plus,
            right: Box::new(ast::expression::Expression::Literal(
                ast::expression::Literal::Integer(ast::expression::IntegerLiteral {
                    value: 2,
                    source_info: src(),
                }),
            )),
            source_info: src(),
        });
        let vs = convert_expression_typed(&expr);
        match vs {
            v1::value_spec::ValueSpecification::Func(f) => {
                assert_eq!(f.function, "plus");
                assert_eq!(f.parameters.len(), 2);
            }
            other => panic!("Expected Func, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_profile_element() {
        let profile = ast::element::Element::Profile(ast::element::ProfileDef {
            package: Some(ast::type_ref::Package::root(Identifier::new("meta"), src())),
            name: ast::annotation::SpannedString {
                value: Identifier::new("doc"),
                source_info: src(),
            },
            stereotype_names: vec![ast::annotation::SpannedString {
                value: Identifier::new("deprecated"),
                source_info: src(),
            }],
            tag_names: vec![ast::annotation::SpannedString {
                value: Identifier::new("description"),
                source_info: src(),
            }],
            stereotypes: vec![],
            tagged_values: vec![],
            source_info: src(),
        });
        let pe = convert_element(&profile).unwrap();
        match pe {
            v1::element::PackageableElement::Profile(p) => {
                assert_eq!(p.package_path, "meta");
                assert_eq!(p.name, "doc");
                assert_eq!(p.stereotypes, vec!["deprecated"]);
                assert_eq!(p.tags, vec!["description"]);
            }
            other => panic!("Expected Profile, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_class_element() {
        let class = ast::element::Element::Class(ast::element::ClassDef {
            package: Some(
                ast::type_ref::Package::root(Identifier::new("model"), src())
                    .child(Identifier::new("domain"), src()),
            ),
            name: ast::annotation::SpannedString {
                value: Identifier::new("Person"),
                source_info: src(),
            },
            type_variable_parameters: vec![],
            type_parameters: vec![],
            multiplicity_parameters: vec![],
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
            source_info: src(),
        });
        let pe = convert_element(&class).unwrap();
        match pe {
            v1::element::PackageableElement::Class(c) => {
                assert_eq!(c.package_path, "model::domain");
                assert_eq!(c.name, "Person");
            }
            other => panic!("Expected Class, got {other:?}"),
        }
    }
}
