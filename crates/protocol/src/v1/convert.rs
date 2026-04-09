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

// ---------------------------------------------------------------------------
// Type conversions
// ---------------------------------------------------------------------------

impl From<&ast::type_ref::TypeReference> for v1::generic_type::GenericType {
    fn from(tr: &ast::type_ref::TypeReference) -> Self {
        Self {
            raw_type: v1::generic_type::PackageableType {
                full_path: tr.full_path(),
                source_information: source_information(&tr.source_info),
            },
            type_arguments: tr.type_arguments.iter().map(Into::into).collect(),
            multiplicity_arguments: vec![],
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

// ---------------------------------------------------------------------------
// Property conversions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Expression → ValueSpecification
// ---------------------------------------------------------------------------

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
                value: String::new(),
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
            value: e.value.clone(),
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

/// Converts a `ColumnExpression` into a `classInstance` value specification.
fn convert_column(col: &ast::expression::ColumnExpression) -> v1::value_spec::ValueSpecification {
    use ast::expression::ColumnExpression;
    use v1::value_spec::ClassInstance;
    use v1::value_spec::ValueSpecification;

    match col {
        ColumnExpression::Name(e) => ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::json!({
                "name": e.name.to_string(),
            }),
            source_information: source_information(&e.source_info),
        }),
        ColumnExpression::Typed(e) => ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::json!({
                "name": e.name.to_string(),
                "type": e.type_ref.full_path(),
            }),
            source_information: source_information(&e.source_info),
        }),
        ColumnExpression::WithLambda(e) => {
            let lambda_spec =
                convert_expression_typed(&ast::expression::Expression::Lambda(*e.lambda.clone()));
            // Serializing a well-formed ValueSpecification cannot fail —
            // all fields are primitives or recursively serializable protocol types.
            let Ok(lambda_val) = serde_json::to_value(&lambda_spec) else {
                unreachable!("well-formed ValueSpecification serialization cannot fail");
            };
            ValueSpecification::ClassInstance(ClassInstance {
                type_name: "colSpec".to_string(),
                value: serde_json::json!({
                    "name": e.name.to_string(),
                    "function1": lambda_val,
                }),
                source_information: source_information(&e.source_info),
            })
        }
        ColumnExpression::WithFunction(e) => ValueSpecification::ClassInstance(ClassInstance {
            type_name: "colSpec".to_string(),
            value: serde_json::json!({
                "name": e.name.to_string(),
                "function1": e.function.to_string(),
            }),
            source_information: source_information(&e.source_info),
        }),
    }
}

// ---------------------------------------------------------------------------
// Island expression conversions
// ---------------------------------------------------------------------------

/// Converts an island grammar expression into a `ValueSpecification`.
///
/// Dispatches based on the content's concrete type via `downcast_ref`.
/// Graph fetch trees are converted to `ClassInstance { type: "rootGraphFetchTree" }`.
fn convert_island_expression(
    island: &ast::island::IslandExpression,
) -> v1::value_spec::ValueSpecification {
    if let Some(tree) = island
        .content
        .as_any()
        .downcast_ref::<ast::island::RootGraphFetchTree>()
    {
        convert_root_graph_fetch_tree(tree)
    } else {
        // Unknown island type — produce a placeholder.
        // This path is unreachable for well-formed ASTs produced by
        // registered island parsers.
        v1::value_spec::ValueSpecification::ClassInstance(v1::value_spec::ClassInstance {
            type_name: format!("unknownIsland_{}", island.tag()),
            value: serde_json::json!({}),
            source_information: source_information(&island.source_info),
        })
    }
}

/// Converts a `RootGraphFetchTree` into a `ClassInstance` value specification.
fn convert_root_graph_fetch_tree(
    tree: &ast::island::RootGraphFetchTree,
) -> v1::value_spec::ValueSpecification {
    use v1::value_spec::{ClassInstance, ValueSpecification};

    let sub_trees: Vec<serde_json::Value> = tree
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = tree
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    ValueSpecification::ClassInstance(ClassInstance {
        type_name: "rootGraphFetchTree".to_string(),
        value: serde_json::json!({
            "_type": "rootGraphFetchTree",
            "class": tree.class.to_string(),
            "subTrees": sub_trees,
            "subTypeTrees": sub_type_trees,
        }),
        source_information: source_information(&tree.source_info),
    })
}

/// Converts a `PropertyGraphFetchTree` into a JSON value.
fn convert_property_graph_fetch_tree(
    prop: &ast::island::PropertyGraphFetchTree,
) -> serde_json::Value {
    let sub_trees: Vec<serde_json::Value> = prop
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = prop
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    let mut obj = serde_json::json!({
        "_type": "propertyGraphFetchTree",
        "property": prop.property.to_string(),
        "subTrees": sub_trees,
        "subTypeTrees": sub_type_trees,
    });

    if !prop.parameters.is_empty() {
        let params: Vec<serde_json::Value> = prop
            .parameters
            .iter()
            .map(|e| {
                let vs = convert_expression_typed(e);
                serde_json::to_value(&vs).unwrap_or_default()
            })
            .collect();
        obj["parameters"] = serde_json::json!(params);
    }

    if let Some(alias) = &prop.alias {
        obj["alias"] = serde_json::json!(alias.to_string());
    }

    if let Some(sub_type) = &prop.sub_type {
        obj["subType"] = serde_json::json!(sub_type.to_string());
    }

    obj
}

/// Converts a `SubTypeGraphFetchTree` into a JSON value.
fn convert_sub_type_graph_fetch_tree(
    sub: &ast::island::SubTypeGraphFetchTree,
) -> serde_json::Value {
    let sub_trees: Vec<serde_json::Value> = sub
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = sub
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    serde_json::json!({
        "_type": "subTypeGraphFetchTree",
        "subTypeClass": sub.sub_type_class.to_string(),
        "subTrees": sub_trees,
        "subTypeTrees": sub_type_trees,
    })
}

// ---------------------------------------------------------------------------
// Element conversions
// ---------------------------------------------------------------------------

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
        name: c.name.to_string(),
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
        name: e.name.to_string(),
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
        name: f.name.to_string(),
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
        name: p.name.to_string(),
        stereotypes: p.stereotypes.iter().map(|s| s.value.to_string()).collect(),
        tags: p.tags.iter().map(|t| t.value.to_string()).collect(),
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
        name: a.name.to_string(),
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
        name: m.name.to_string(),
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
                source_information: source_information(&unit.source_info),
            }
        }),
        super_types: vec![measure_fqn],
        source_information: source_information(&unit.source_info),
    }
}

// ---------------------------------------------------------------------------
// Top-level: SourceFile → PureModelContextData
// ---------------------------------------------------------------------------

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
