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
//! - **Infallible**: Every AST node can be represented in the protocol model.
//!   Conversions use `From` (not `TryFrom`) because the protocol is strictly
//!   more permissive than the AST.
//! - **No state**: Conversions are pure functions with no shared mutable state.
//! - **Recursive**: Complex types (expressions, elements) recurse into children.

use legend_pure_parser_ast as ast;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::HasMultiplicity;

use crate::v1;

// ---------------------------------------------------------------------------
// Leaf conversions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Type conversions
// ---------------------------------------------------------------------------

impl From<&ast::type_ref::TypeReference> for v1::generic_type::GenericType {
    fn from(tr: &ast::type_ref::TypeReference) -> Self {
        Self {
            raw_type: v1::generic_type::PackageableType {
                full_path: tr.path.to_string(),
                source_information: Some(tr.path.source_info().into()),
            },
            type_arguments: tr.type_arguments.iter().map(Into::into).collect(),
            multiplicity_arguments: vec![],
            type_variable_values: tr
                .type_variable_values
                .iter()
                .map(type_variable_value_to_json)
                .collect(),
            source_information: Some(tr.source_info.clone().into()),
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

// ---------------------------------------------------------------------------
// Annotation conversions
// ---------------------------------------------------------------------------

impl From<&ast::annotation::StereotypePtr> for v1::annotation::StereotypePtr {
    fn from(s: &ast::annotation::StereotypePtr) -> Self {
        Self {
            profile: format_element_ptr(&s.profile),
            value: s.value.to_string(),
            source_information: Some(s.source_info.clone().into()),
            profile_source_information: Some(s.profile.source_info.clone().into()),
        }
    }
}

impl From<&ast::annotation::TagPtr> for v1::annotation::TagPtr {
    fn from(t: &ast::annotation::TagPtr) -> Self {
        Self {
            profile: format_element_ptr(&t.profile),
            value: t.value.to_string(),
            source_information: Some(t.source_info.clone().into()),
            profile_source_information: Some(t.profile.source_info.clone().into()),
        }
    }
}

impl From<&ast::annotation::TaggedValue> for v1::annotation::TaggedValue {
    fn from(tv: &ast::annotation::TaggedValue) -> Self {
        Self {
            tag: (&tv.tag).into(),
            value: tv.value.clone(),
            source_information: Some(tv.source_info.clone().into()),
        }
    }
}

/// Formats a `PackageableElementPtr` (profile ref) as a fully qualified path
/// string: `"package::name"`.
fn format_element_ptr(ptr: &ast::annotation::PackageableElementPtr) -> String {
    match &ptr.package {
        Some(pkg) => format!("{pkg}::{}", ptr.name),
        None => ptr.name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Property conversions
// ---------------------------------------------------------------------------

impl From<&ast::element::Property> for v1::property::Property {
    fn from(p: &ast::element::Property) -> Self {
        Self {
            name: p.name.to_string(),
            generic_type: (&p.type_ref).into(),
            multiplicity: (&p.multiplicity).into(),
            default_value: p.default_value.as_ref().map(|dv| v1::property::DefaultValue {
                value: convert_expression(dv),
                source_information: Some(dv.source_info().clone().into()),
            }),
            stereotypes: p.stereotypes.iter().map(Into::into).collect(),
            tagged_values: p.tagged_values.iter().map(Into::into).collect(),
            aggregation: p.aggregation.map(std::convert::Into::into),
            source_information: Some(p.source_info.clone().into()),
        }
    }
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

impl From<&ast::element::QualifiedProperty> for v1::property::QualifiedProperty {
    fn from(qp: &ast::element::QualifiedProperty) -> Self {
        Self {
            name: qp.name.to_string(),
            parameters: qp.parameters.iter().map(convert_parameter).collect(),
            return_generic_type: (&qp.return_type).into(),
            return_multiplicity: (&qp.return_multiplicity).into(),
            stereotypes: qp.stereotypes.iter().map(Into::into).collect(),
            tagged_values: qp.tagged_values.iter().map(Into::into).collect(),
            body: qp.body.iter().map(convert_expression).collect(),
            source_information: Some(qp.source_info.clone().into()),
        }
    }
}

impl From<&ast::element::Constraint> for v1::property::Constraint {
    fn from(c: &ast::element::Constraint) -> Self {
        Self {
            name: c
                .name
                .as_ref()
                .map_or_else(|| "constraint".to_string(), ToString::to_string),
            owner: None,
            function_definition: convert_expression(&c.function_definition),
            source_information: Some(c.source_info.clone().into()),
            external_id: c.external_id.clone(),
            enforcement_level: c.enforcement_level.as_ref().map(ToString::to_string),
            message_function: c.message.as_ref().map(convert_expression),
        }
    }
}

/// Converts a `Parameter` to a serialized variable value specification.
fn convert_parameter(p: &ast::annotation::Parameter) -> serde_json::Value {
    let var = v1::value_spec::Variable {
        name: p.name.to_string(),
        generic_type: p.type_ref.as_ref().map(Into::into),
        multiplicity: p.multiplicity.as_ref().map(Into::into),
        supports_stream: None,
        source_information: Some(p.source_info.clone().into()),
    };
    let vs = v1::value_spec::ValueSpecification::Var(var);
    serde_json::to_value(&vs).expect("Variable serialization cannot fail")
}

// ---------------------------------------------------------------------------
// Expression → ValueSpecification
// ---------------------------------------------------------------------------

/// Converts an AST `Expression` into a protocol `serde_json::Value`.
///
/// Returns a `serde_json::Value` rather than a typed `ValueSpecification`
/// because expressions can appear in contexts that expect `serde_json::Value`
/// (e.g., constraint function definitions, qualified property bodies).
///
/// # Panics
///
/// Panics if the `ValueSpecification` cannot be serialized to JSON, which
/// should never happen for well-formed protocol types.
#[must_use]
pub fn convert_expression(expr: &ast::expression::Expression) -> serde_json::Value {
    let vs = convert_expression_typed(expr);
    serde_json::to_value(&vs).expect("ValueSpecification serialization cannot fail")
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
            source_information: Some(var.source_info.clone().into()),
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
        Expression::FunctionApplication(e) => {
            ValueSpecification::Func(AppliedFunction {
                function: format_element_ptr(&e.function),
                f_control: None,
                parameters: e
                    .arguments
                    .iter()
                    .map(convert_expression_typed)
                    .collect(),
                source_information: Some(e.source_info.clone().into()),
            })
        }

        // -- Arrow function: `expr->func(args)` desugars to func(expr, args) --
        Expression::ArrowFunction(e) => ValueSpecification::Func(AppliedFunction {
            function: e.function.to_string(),
            f_control: None,
            parameters: std::iter::once(convert_expression_typed(&e.target))
                .chain(e.arguments.iter().map(convert_expression_typed))
                .collect(),
            source_information: Some(e.source_info.clone().into()),
        }),

        // -- Member access --
        Expression::MemberAccess(ma) => convert_member_access(ma),

        // -- Type reference: `@MyType` → packageableElementPtr --
        Expression::TypeReferenceExpr(e) => {
            ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                full_path: e.type_ref.path.to_string(),
                source_information: Some(e.source_info.clone().into()),
            })
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
                    source_information: Some(p.source_info.clone().into()),
                })
                .collect(),
            source_information: Some(e.source_info.clone().into()),
        }),

        // -- Let: desugared to letFunction('name', expr) --
        Expression::Let(e) => {
            let name_val = ValueSpecification::String(CString {
                value: e.name.to_string(),
                source_information: Some(e.source_info.clone().into()),
            });
            let value_val = convert_expression_typed(&e.value);
            ValueSpecification::Func(AppliedFunction {
                function: "letFunction".to_string(),
                f_control: None,
                parameters: vec![name_val, value_val],
                source_information: Some(e.source_info.clone().into()),
            })
        }

        // -- Collection literal --
        Expression::Collection(e) => ValueSpecification::Collection(ProtocolCollection {
            multiplicity: collection_multiplicity(e.elements.len()),
            values: e.elements.iter().map(convert_expression_typed).collect(),
            source_information: Some(e.source_info.clone().into()),
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
                            source_information: Some(a.source_info.clone().into()),
                        })),
                        expression: Box::new(convert_expression_typed(&a.value)),
                        source_information: Some(a.source_info.clone().into()),
                    })
                })
                .collect();
            // Wrap as func call: new(Class, '', [key-expressions])
            let class_ref =
                ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                    full_path: format_element_ptr(&e.class),
                    source_information: Some(e.source_info.clone().into()),
                });
            let empty_name = ValueSpecification::String(CString {
                value: String::new(),
                source_information: None,
            });
            let keys_collection = ValueSpecification::Collection(ProtocolCollection {
                multiplicity: collection_multiplicity(key_expressions.len()),
                values: key_expressions,
                source_information: Some(e.source_info.clone().into()),
            });
            ValueSpecification::Func(AppliedFunction {
                function: "new".to_string(),
                f_control: None,
                parameters: vec![class_ref, empty_name, keys_collection],
                source_information: Some(e.source_info.clone().into()),
            })
        }

        // -- Column expressions → classInstance --
        Expression::Column(col) => convert_column(col),
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
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::Float(e) => ValueSpecification::Float(CFloat {
            value: e.value,
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::Decimal(e) => ValueSpecification::Decimal(CDecimal {
            value: e.value.parse::<f64>().unwrap_or(0.0),
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::String(e) => ValueSpecification::String(CString {
            value: e.value.clone(),
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::Boolean(e) => ValueSpecification::Boolean(CBoolean {
            value: e.value,
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::StrictDate(e) => ValueSpecification::StrictDate(CStrictDate {
            value: e.value.clone(),
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::DateTime(e) => ValueSpecification::DateTime(CDateTime {
            value: e.value.clone(),
            source_information: Some(e.source_info.clone().into()),
        }),
        Literal::StrictTime(e) => ValueSpecification::StrictTime(CStrictTime {
            value: e.value.clone(),
            source_information: Some(e.source_info.clone().into()),
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
        source_information: Some(source_info.clone().into()),
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
fn convert_member_access(
    ma: &ast::expression::MemberAccess,
) -> v1::value_spec::ValueSpecification {
    use ast::expression::MemberAccess;
    use v1::value_spec::AppliedProperty;
    use v1::value_spec::ValueSpecification;

    match ma {
        MemberAccess::Simple(e) => ValueSpecification::Property(AppliedProperty {
            class: None,
            property: e.member.to_string(),
            parameters: vec![convert_expression_typed(&e.target)],
            source_information: Some(e.source_info.clone().into()),
        }),
        MemberAccess::Qualified(e) => ValueSpecification::Property(AppliedProperty {
            class: None,
            property: e.member.to_string(),
            parameters: std::iter::once(convert_expression_typed(&e.target))
                .chain(e.arguments.iter().map(convert_expression_typed))
                .collect(),
            source_information: Some(e.source_info.clone().into()),
        }),
    }
}

/// Converts a `ColumnExpression` into a `classInstance` value specification.
fn convert_column(
    col: &ast::expression::ColumnExpression,
) -> v1::value_spec::ValueSpecification {
    use ast::expression::ColumnExpression;
    use v1::value_spec::ClassInstance;
    use v1::value_spec::ValueSpecification;

    match col {
        ColumnExpression::Name(e) => ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::json!({
                "name": e.name.to_string(),
            }),
            source_information: Some(e.source_info.clone().into()),
        }),
        ColumnExpression::Typed(e) => ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::json!({
                "name": e.name.to_string(),
                "type": e.type_ref.path.to_string(),
            }),
            source_information: Some(e.source_info.clone().into()),
        }),
        ColumnExpression::WithLambda(e) => {
            let lambda_json = convert_expression(
                &ast::expression::Expression::Lambda(*e.lambda.clone()),
            );
            ValueSpecification::ClassInstance(ClassInstance {
                type_name: "colSpec".to_string(),
                value: serde_json::json!({
                    "name": e.name.to_string(),
                    "function1": lambda_json,
                }),
                source_information: Some(e.source_info.clone().into()),
            })
        }
        ColumnExpression::WithFunction(e) => {
            ValueSpecification::ClassInstance(ClassInstance {
                type_name: "colSpec".to_string(),
                value: serde_json::json!({
                    "name": e.name.to_string(),
                    "function1": format_element_ptr(&e.function),
                }),
                source_information: Some(e.source_info.clone().into()),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Element conversions
// ---------------------------------------------------------------------------

/// Converts an AST `Element` into a protocol `PackageableElement`.
#[must_use]
pub fn convert_element(elem: &ast::element::Element) -> v1::element::PackageableElement {
    use ast::element::Element;
    use v1::element::PackageableElement;

    match elem {
        Element::Class(c) => PackageableElement::Class(convert_class(c)),
        Element::Enumeration(e) => PackageableElement::Enumeration(convert_enumeration(e)),
        Element::Function(f) => PackageableElement::Function(convert_function(f)),
        Element::Profile(p) => PackageableElement::Profile(convert_profile(p)),
        Element::Association(a) => PackageableElement::Association(convert_association(a)),
        Element::Measure(m) => PackageableElement::Measure(convert_measure(m)),
    }
}

fn convert_class(c: &ast::element::ClassDef) -> v1::element::ProtocolClass {
    v1::element::ProtocolClass {
        package_path: optional_package_to_path(c.package.as_ref()),
        name: c.name.to_string(),
        super_types: c.super_types.iter().map(|t| t.path.to_string()).collect(),
        properties: c.properties.iter().map(Into::into).collect(),
        qualified_properties: c.qualified_properties.iter().map(Into::into).collect(),
        constraints: c.constraints.iter().map(Into::into).collect(),
        original_milestoned_properties: vec![],
        stereotypes: c.stereotypes.iter().map(Into::into).collect(),
        tagged_values: c.tagged_values.iter().map(Into::into).collect(),
        source_information: Some(c.source_info.clone().into()),
    }
}

fn convert_enumeration(e: &ast::element::EnumDef) -> v1::element::ProtocolEnumeration {
    v1::element::ProtocolEnumeration {
        package_path: optional_package_to_path(e.package.as_ref()),
        name: e.name.to_string(),
        values: e.values.iter().map(convert_enum_value).collect(),
        stereotypes: e.stereotypes.iter().map(Into::into).collect(),
        tagged_values: e.tagged_values.iter().map(Into::into).collect(),
        source_information: Some(e.source_info.clone().into()),
    }
}

fn convert_enum_value(v: &ast::element::EnumValue) -> v1::element::ProtocolEnumMember {
    v1::element::ProtocolEnumMember {
        value: v.name.to_string(),
        stereotypes: v.stereotypes.iter().map(Into::into).collect(),
        tagged_values: v.tagged_values.iter().map(Into::into).collect(),
        source_information: Some(v.source_info.clone().into()),
    }
}

fn convert_function(f: &ast::element::FunctionDef) -> v1::element::ProtocolFunction {
    v1::element::ProtocolFunction {
        package_path: optional_package_to_path(f.package.as_ref()),
        name: f.name.to_string(),
        parameters: f
            .parameters
            .iter()
            .map(|p| v1::value_spec::Variable {
                name: p.name.to_string(),
                generic_type: p.type_ref.as_ref().map(Into::into),
                multiplicity: p.multiplicity.as_ref().map(Into::into),
                supports_stream: None,
                source_information: Some(p.source_info.clone().into()),
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
        source_information: Some(f.source_info.clone().into()),
    }
}

fn convert_profile(p: &ast::element::ProfileDef) -> v1::element::ProtocolProfile {
    v1::element::ProtocolProfile {
        package_path: optional_package_to_path(p.package.as_ref()),
        name: p.name.to_string(),
        stereotypes: p.stereotypes.iter().map(|s| s.value.to_string()).collect(),
        tags: p.tags.iter().map(|t| t.value.to_string()).collect(),
        source_information: Some(p.source_info.clone().into()),
    }
}

fn convert_association(a: &ast::element::AssociationDef) -> v1::element::ProtocolAssociation {
    v1::element::ProtocolAssociation {
        package_path: optional_package_to_path(a.package.as_ref()),
        name: a.name.to_string(),
        properties: a.properties.iter().map(Into::into).collect(),
        qualified_properties: a.qualified_properties.iter().map(Into::into).collect(),
        original_milestoned_properties: vec![],
        stereotypes: a.stereotypes.iter().map(Into::into).collect(),
        tagged_values: a.tagged_values.iter().map(Into::into).collect(),
        source_information: Some(a.source_info.clone().into()),
    }
}

fn convert_measure(m: &ast::element::MeasureDef) -> v1::element::ProtocolMeasure {
    v1::element::ProtocolMeasure {
        package_path: optional_package_to_path(m.package.as_ref()),
        name: m.name.to_string(),
        canonical_unit: m.canonical_unit.as_ref().map(|u| convert_unit(m, u)),
        non_canonical_units: m
            .non_canonical_units
            .iter()
            .map(|u| convert_unit(m, u))
            .collect(),
        source_information: Some(m.source_info.clone().into()),
    }
}

fn convert_unit(
    measure: &ast::element::MeasureDef,
    unit: &ast::element::UnitDef,
) -> v1::element::ProtocolUnit {
    // Unit package path = measure's fully qualified name (e.g., "pkg::Measure")
    let measure_fqn = match &measure.package {
        Some(pkg) => format!("{pkg}::{}", measure.name),
        None => measure.name.to_string(),
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
                source_information: Some(unit.source_info.clone().into()),
            }
        }),
        super_types: vec![measure_fqn],
        source_information: Some(unit.source_info.clone().into()),
    }
}

// ---------------------------------------------------------------------------
// Top-level: SourceFile → PureModelContextData
// ---------------------------------------------------------------------------

/// Converts a parsed `SourceFile` into a `PureModelContextData`.
///
/// This is the top-level entry point for AST → Protocol conversion.
pub fn convert_source_file(
    source_file: &ast::section::SourceFile,
) -> v1::context::PureModelContextData {
    use ast::element::PackageableElement as _;

    let mut elements: Vec<v1::element::PackageableElement> = source_file
        .all_elements()
        .map(convert_element)
        .collect();

    // Build a section index from the source file's sections
    let sections: Vec<v1::element::ProtocolSection> = source_file
        .sections
        .iter()
        .map(|section| {
            let element_paths: Vec<String> = section
                .elements
                .iter()
                .map(|e| {
                    match e.package() {
                        Some(pkg) => format!("{pkg}::{}", e.name()),
                        None => e.name().to_string(),
                    }
                })
                .collect();

            if section.imports.is_empty() {
                v1::element::ProtocolSection::Default(v1::element::DefaultCodeSection {
                    parser_name: section.kind.to_string(),
                    elements: element_paths,
                    source_information: Some(section.source_info.clone().into()),
                })
            } else {
                v1::element::ProtocolSection::ImportAware(
                    v1::element::ImportAwareCodeSection {
                        parser_name: section.kind.to_string(),
                        elements: element_paths,
                        imports: section
                            .imports
                            .iter()
                            .map(|i| i.path.to_string())
                            .collect(),
                        source_information: Some(section.source_info.clone().into()),
                    },
                )
            }
        })
        .collect();

    let source_id = source_file.source_info.source.to_string();
    let section_index = v1::element::PackageableElement::SectionIndex(
        v1::element::ProtocolSectionIndex {
            package_path: "__internal__".to_string(),
            name: source_id,
            sections,
            source_information: Some(source_file.source_info.clone().into()),
        },
    );
    elements.push(section_index);

    v1::context::PureModelContextData::new(elements)
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
        let pure_one: v1::multiplicity::Multiplicity =
            (&ast::Multiplicity::pure_one()).into();
        assert_eq!(pure_one, v1::multiplicity::Multiplicity::PURE_ONE);

        let zero_many: v1::multiplicity::Multiplicity =
            (&ast::Multiplicity::zero_or_many()).into();
        assert_eq!(zero_many, v1::multiplicity::Multiplicity::ZERO_MANY);

        let zero_one: v1::multiplicity::Multiplicity =
            (&ast::Multiplicity::one()).into();
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
            path: ast::type_ref::Package::root(Identifier::new("String"), src()),
            type_arguments: vec![],
            type_variable_values: vec![],
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
            name: Identifier::new("doc"),
            stereotypes: vec![ast::annotation::SpannedString {
                value: Identifier::new("deprecated"),
                source_info: src(),
            }],
            tags: vec![ast::annotation::SpannedString {
                value: Identifier::new("description"),
                source_info: src(),
            }],
            source_info: src(),
        });
        let pe = convert_element(&profile);
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
            name: Identifier::new("Person"),
            type_parameters: vec![],
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
            source_info: src(),
        });
        let pe = convert_element(&class);
        match pe {
            v1::element::PackageableElement::Class(c) => {
                assert_eq!(c.package_path, "model::domain");
                assert_eq!(c.name, "Person");
            }
            other => panic!("Expected Class, got {other:?}"),
        }
    }
}
