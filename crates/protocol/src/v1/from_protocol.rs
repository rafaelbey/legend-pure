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

//! Protocol v1 → AST conversion.
//!
//! Converts the protocol v1 JSON model back into the parser's AST types.
//! This is the reverse direction of [`super::convert`].
//!
//! ## Design Principles
//!
//! - **Fallible**: Protocol values may not always map cleanly to AST nodes
//!   (e.g., invalid path strings, missing source information). Conversions
//!   use `TryFrom` / `Result` with [`ConversionError`].
//! - **Synthetic source info**: Protocol types have `Option<SourceInformation>`.
//!   When missing, a synthetic zero-span is created so that every AST node
//!   satisfies the `Spanned` invariant.
//! - **Path parsing**: Flat `"a::b::c"` path strings are parsed back into
//!   recursive `Package` trees.

#![allow(clippy::missing_errors_doc)] // Conversion functions have obvious error semantics

use legend_pure_parser_ast as ast;
use smol_str::SmolStr;

use crate::v1;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during Protocol → AST conversion.
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    /// A required path string was empty.
    #[error("empty path: expected a '::'-separated path but got an empty string")]
    EmptyPath,
    /// An unknown function name was encountered that cannot be reverse-desugared.
    #[error("unknown operator function: {0}")]
    UnknownOperator(String),
    /// A value specification variant is not supported for reverse conversion.
    #[error("unsupported value specification type for reverse conversion")]
    UnsupportedValueSpec,
    /// A JSON value could not be deserialized into the expected type.
    #[error("JSON deserialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result alias for conversions in this module.
pub type Result<T> = std::result::Result<T, ConversionError>;

// ---------------------------------------------------------------------------
// Synthetic source info
// ---------------------------------------------------------------------------

/// Creates a synthetic `SourceInfo` when protocol data has no source location.
///
/// Every AST node must be `Spanned`, so we use a zero-span placeholder
/// with the source identifier `"<protocol>"`.
fn synthetic_source_info() -> ast::SourceInfo {
    ast::SourceInfo::new("<protocol>", 0, 0, 0, 0)
}

// ---------------------------------------------------------------------------
// Leaf conversions
// ---------------------------------------------------------------------------

/// Converts a protocol `SourceInformation` into an AST `SourceInfo`.
impl From<&v1::source_info::SourceInformation> for ast::SourceInfo {
    fn from(si: &v1::source_info::SourceInformation) -> Self {
        Self::new(
            SmolStr::new(&si.source_id),
            si.start_line,
            si.start_column,
            si.end_line,
            si.end_column,
        )
    }
}

/// Extracts source info from an `Option<SourceInformation>`, using a synthetic
/// zero-span when absent.
fn source_info_or_synthetic(si: Option<&v1::source_info::SourceInformation>) -> ast::SourceInfo {
    si.map_or_else(synthetic_source_info, Into::into)
}

/// Converts a protocol `Multiplicity` into an AST `Multiplicity`.
impl From<&v1::multiplicity::Multiplicity> for ast::Multiplicity {
    fn from(m: &v1::multiplicity::Multiplicity) -> Self {
        Self::range(m.lower_bound, m.upper_bound)
    }
}

// ---------------------------------------------------------------------------
// Path parsing
// ---------------------------------------------------------------------------

/// Parses a `"a::b::c"` path string into a recursive AST `Package` tree.
///
/// The source info for each segment is synthetic (zero-span) because
/// the protocol path string doesn't carry per-segment source locations.
fn parse_path(path: &str) -> Result<ast::type_ref::Package> {
    let segments: Vec<&str> = path.split("::").collect();
    if segments.is_empty() || segments[0].is_empty() {
        return Err(ConversionError::EmptyPath);
    }

    let si = synthetic_source_info();
    let mut pkg = ast::type_ref::Package::root(SmolStr::new(segments[0]), si.clone());
    for segment in &segments[1..] {
        pkg = pkg.child(SmolStr::new(segment), si.clone());
    }
    Ok(pkg)
}

/// Parses a path string into an `Option<Package>`, treating empty strings as `None`.
fn parse_optional_package(path: &str) -> Result<Option<ast::type_ref::Package>> {
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_path(path)?))
    }
}

/// Parses a fully qualified path like `"pkg::Name"` into `(Option<Package>, Identifier)`.
///
/// The last segment becomes the name, and everything before it becomes the package.
/// For a single segment like `"Name"`, the package is `None`.
fn parse_qualified_path(path: &str) -> Result<(Option<ast::type_ref::Package>, SmolStr)> {
    let segments: Vec<&str> = path.split("::").collect();
    if segments.is_empty() || segments[0].is_empty() {
        return Err(ConversionError::EmptyPath);
    }
    if segments.len() == 1 {
        return Ok((None, SmolStr::new(segments[0])));
    }

    let name = SmolStr::new(segments[segments.len() - 1]);
    let si = synthetic_source_info();
    let mut pkg = ast::type_ref::Package::root(SmolStr::new(segments[0]), si.clone());
    for segment in &segments[1..segments.len() - 1] {
        pkg = pkg.child(SmolStr::new(segment), si.clone());
    }
    Ok((Some(pkg), name))
}

// ---------------------------------------------------------------------------
// Type conversions
// ---------------------------------------------------------------------------

/// Converts a protocol `GenericType` into an AST `TypeReference`.
pub fn convert_generic_type(
    gt: &v1::generic_type::GenericType,
) -> Result<ast::type_ref::TypeReference> {
    let si = source_info_or_synthetic(gt.source_information.as_ref());
    let path = parse_path(&gt.raw_type.full_path)?;
    let type_arguments: std::result::Result<Vec<_>, _> = gt
        .type_arguments
        .iter()
        .map(convert_generic_type)
        .collect();

    Ok(ast::type_ref::TypeReference {
        path,
        type_arguments: type_arguments?,
        type_variable_values: vec![], // Simplified — type variable values rarely roundtrip
        source_info: si,
    })
}

// ---------------------------------------------------------------------------
// Annotation conversions
// ---------------------------------------------------------------------------

/// Converts a protocol `StereotypePtr` into an AST `StereotypePtr`.
pub fn convert_stereotype_ptr(
    sp: &v1::annotation::StereotypePtr,
) -> Result<ast::annotation::StereotypePtr> {
    let si = source_info_or_synthetic(sp.source_information.as_ref());
    let profile_si = source_info_or_synthetic(sp.profile_source_information.as_ref());
    let (package, name) = parse_qualified_path(&sp.profile)?;

    Ok(ast::annotation::StereotypePtr {
        profile: ast::annotation::PackageableElementPtr {
            package,
            name,
            source_info: profile_si,
        },
        value: SmolStr::new(&sp.value),
        source_info: si,
    })
}

/// Converts a protocol `TagPtr` into an AST `TagPtr`.
pub fn convert_tag_ptr(tp: &v1::annotation::TagPtr) -> Result<ast::annotation::TagPtr> {
    let si = source_info_or_synthetic(tp.source_information.as_ref());
    let profile_si = source_info_or_synthetic(tp.profile_source_information.as_ref());
    let (package, name) = parse_qualified_path(&tp.profile)?;

    Ok(ast::annotation::TagPtr {
        profile: ast::annotation::PackageableElementPtr {
            package,
            name,
            source_info: profile_si,
        },
        value: SmolStr::new(&tp.value),
        source_info: si,
    })
}

/// Converts a protocol `TaggedValue` into an AST `TaggedValue`.
pub fn convert_tagged_value(
    tv: &v1::annotation::TaggedValue,
) -> Result<ast::annotation::TaggedValue> {
    let si = source_info_or_synthetic(tv.source_information.as_ref());
    Ok(ast::annotation::TaggedValue {
        tag: convert_tag_ptr(&tv.tag)?,
        value: tv.value.clone(),
        source_info: si,
    })
}

// ---------------------------------------------------------------------------
// Property conversions
// ---------------------------------------------------------------------------

/// Converts a protocol `Property` into an AST `Property`.
pub fn convert_property(p: &v1::property::Property) -> Result<ast::element::Property> {
    let si = source_info_or_synthetic(p.source_information.as_ref());
    let stereotypes: std::result::Result<Vec<_>, _> =
        p.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        p.tagged_values.iter().map(convert_tagged_value).collect();

    Ok(ast::element::Property {
        name: SmolStr::new(&p.name),
        type_ref: convert_generic_type(&p.generic_type)?,
        multiplicity: (&p.multiplicity).into(),
        aggregation: p.aggregation.map(|ak| match ak {
            v1::property::AggregationKind::NONE => ast::element::AggregationKind::None,
            v1::property::AggregationKind::SHARED => ast::element::AggregationKind::Shared,
            v1::property::AggregationKind::COMPOSITE => ast::element::AggregationKind::Composite,
        }),
        default_value: None, // Default values require expression conversion
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        source_info: si,
    })
}

/// Converts a protocol `QualifiedProperty` into an AST `QualifiedProperty`.
pub fn convert_qualified_property(
    qp: &v1::property::QualifiedProperty,
) -> Result<ast::element::QualifiedProperty> {
    let si = source_info_or_synthetic(qp.source_information.as_ref());
    let stereotypes: std::result::Result<Vec<_>, _> =
        qp.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        qp.tagged_values.iter().map(convert_tagged_value).collect();

    // Convert parameters from serde_json::Value (Variable specs)
    let parameters: std::result::Result<Vec<_>, _> = qp
        .parameters
        .iter()
        .map(convert_json_to_parameter)
        .collect();

    // Convert body from serde_json::Value to Expression
    let body: std::result::Result<Vec<_>, _> = qp
        .body
        .iter()
        .map(convert_json_value_to_expression)
        .collect();

    Ok(ast::element::QualifiedProperty {
        name: SmolStr::new(&qp.name),
        parameters: parameters?,
        return_type: convert_generic_type(&qp.return_generic_type)?,
        return_multiplicity: (&qp.return_multiplicity).into(),
        body: body?,
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        source_info: si,
    })
}

/// Converts a protocol `Constraint` into an AST `Constraint`.
pub fn convert_constraint(c: &v1::property::Constraint) -> Result<ast::element::Constraint> {
    let si = source_info_or_synthetic(c.source_information.as_ref());
    let function_def = convert_json_value_to_expression(&c.function_definition)?;
    let message = c
        .message_function
        .as_ref()
        .map(convert_json_value_to_expression)
        .transpose()?;

    Ok(ast::element::Constraint {
        name: Some(SmolStr::new(&c.name)),
        function_definition: function_def,
        enforcement_level: c.enforcement_level.as_ref().map(SmolStr::new),
        external_id: c.external_id.clone(),
        message,
        source_info: si,
    })
}

/// Converts a JSON parameter value (expected to be a `Variable` `ValueSpec`) into a `Parameter`.
fn convert_json_to_parameter(
    json: &serde_json::Value,
) -> Result<ast::annotation::Parameter> {
    let vs: v1::value_spec::ValueSpecification = serde_json::from_value(json.clone())?;
    match vs {
        v1::value_spec::ValueSpecification::Var(var) => {
            let gt = var.generic_type.as_ref().ok_or(
                ConversionError::UnsupportedValueSpec
            )?;
            let mult = var.multiplicity.as_ref().ok_or(
                ConversionError::UnsupportedValueSpec
            )?;
            let si = source_info_or_synthetic(var.source_information.as_ref());
            Ok(ast::annotation::Parameter {
                name: SmolStr::new(&var.name),
                type_ref: Some(convert_generic_type(gt)?),
                multiplicity: Some(mult.into()),
                source_info: si,
            })
        }
        _ => Err(ConversionError::UnsupportedValueSpec),
    }
}

// ---------------------------------------------------------------------------
// ValueSpecification → Expression
// ---------------------------------------------------------------------------

/// Converts a protocol `ValueSpecification` into an AST `Expression`.
#[allow(clippy::too_many_lines)]
pub fn convert_value_spec_to_expression(
    vs: &v1::value_spec::ValueSpecification,
) -> Result<ast::expression::Expression> {
    use ast::expression::BooleanLiteral;
    use ast::expression::CollectionExpr;
    use ast::expression::DateTimeLiteral;
    use ast::expression::DecimalLiteral;
    use ast::expression::Expression;
    use ast::expression::FloatLiteral;
    use ast::expression::IntegerLiteral;
    use ast::expression::Lambda;
    use ast::expression::Literal;
    use ast::expression::StrictDateLiteral;
    use ast::expression::StrictTimeLiteral;
    use ast::expression::StringLiteral;
    use ast::expression::TypeReferenceExpr;
    use ast::expression::Variable;
    use v1::value_spec::ValueSpecification;

    match vs {
        ValueSpecification::Integer(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::Integer(IntegerLiteral {
                value: c.value,
                source_info: si,
            })))
        }
        ValueSpecification::Float(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::Float(FloatLiteral {
                value: c.value,
                source_info: si,
            })))
        }
        ValueSpecification::Decimal(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::Decimal(DecimalLiteral {
                value: c.value.to_string(),
                source_info: si,
            })))
        }
        ValueSpecification::String(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::String(StringLiteral {
                value: c.value.clone(),
                source_info: si,
            })))
        }
        ValueSpecification::Boolean(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::Boolean(BooleanLiteral {
                value: c.value,
                source_info: si,
            })))
        }
        ValueSpecification::DateTime(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::DateTime(DateTimeLiteral {
                value: c.value.clone(),
                source_info: si,
            })))
        }
        ValueSpecification::StrictDate(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::StrictDate(StrictDateLiteral {
                value: c.value.clone(),
                source_info: si,
            })))
        }
        ValueSpecification::StrictTime(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::StrictTime(StrictTimeLiteral {
                value: c.value.clone(),
                source_info: si,
            })))
        }
        ValueSpecification::LatestDate(c) => {
            // LatestDate maps to a StrictDate with the value "%latest"
            let si = source_info_or_synthetic(c.source_information.as_ref());
            Ok(Expression::Literal(Literal::StrictDate(StrictDateLiteral {
                value: "%latest".to_string(),
                source_info: si,
            })))
        }
        ValueSpecification::Var(v) => {
            let si = source_info_or_synthetic(v.source_information.as_ref());
            Ok(Expression::Variable(Variable {
                name: SmolStr::new(&v.name),
                source_info: si,
            }))
        }
        ValueSpecification::Func(f) => convert_applied_function(f),
        ValueSpecification::Property(p) => convert_applied_property(p),
        ValueSpecification::Collection(c) => {
            let si = source_info_or_synthetic(c.source_information.as_ref());
            let elements: std::result::Result<Vec<_>, _> = c
                .values
                .iter()
                .map(convert_value_spec_to_expression)
                .collect();
            Ok(Expression::Collection(CollectionExpr {
                elements: elements?,
                multiplicity: None,
                source_info: si,
            }))
        }
        ValueSpecification::Lambda(l) => {
            let si = source_info_or_synthetic(l.source_information.as_ref());
            let params: std::result::Result<Vec<_>, _> = l
                .parameters
                .iter()
                .map(|v| -> Result<ast::annotation::Parameter> {
                    let vsi = source_info_or_synthetic(v.source_information.as_ref());
                    let gt = v.generic_type.as_ref().ok_or(ConversionError::UnsupportedValueSpec)?;
                    let mult = v.multiplicity.as_ref().ok_or(ConversionError::UnsupportedValueSpec)?;
                    Ok(ast::annotation::Parameter {
                        name: SmolStr::new(&v.name),
                        type_ref: Some(convert_generic_type(gt)?),
                        multiplicity: Some(mult.into()),
                        source_info: vsi,
                    })
                })
                .collect();
            let body: std::result::Result<Vec<_>, _> = l
                .body
                .iter()
                .map(convert_value_spec_to_expression)
                .collect();
            Ok(Expression::Lambda(Lambda {
                parameters: params?,
                body: body?,
                source_info: si,
            }))
        }
        ValueSpecification::PackageableElementPtr(p) => {
            let si = source_info_or_synthetic(p.source_information.as_ref());
            let type_ref = ast::type_ref::TypeReference {
                path: parse_path(&p.full_path)?,
                type_arguments: vec![],
                type_variable_values: vec![],
                source_info: si.clone(),
            };
            Ok(Expression::TypeReferenceExpr(TypeReferenceExpr {
                type_ref,
                source_info: si,
            }))
        }
        ValueSpecification::EnumValue(_)
        | ValueSpecification::GenericTypeInstance(_)
        | ValueSpecification::KeyExpression(_)
        | ValueSpecification::ClassInstance(_) => {
            // These are complex types that don't have a direct AST equivalent
            // in the simple expression model. For now, return unsupported.
            Err(ConversionError::UnsupportedValueSpec)
        }
    }
}

/// Converts a `serde_json::Value` (expected to be a serialized `ValueSpecification`)
/// into an AST `Expression`.
pub fn convert_json_value_to_expression(
    json: &serde_json::Value,
) -> Result<ast::expression::Expression> {
    let vs: v1::value_spec::ValueSpecification = serde_json::from_value(json.clone())?;
    convert_value_spec_to_expression(&vs)
}

/// Converts an `AppliedFunction` to an AST `Expression`.
///
/// Attempts to reverse-desugar known operator functions (e.g., `plus` → `+`).
/// Unknown functions are kept as `FunctionApplication`.
#[allow(clippy::too_many_lines)]
fn convert_applied_function(
    f: &v1::value_spec::AppliedFunction,
) -> Result<ast::expression::Expression> {
    use ast::expression::ArithmeticExpr;
    use ast::expression::ArithmeticOp;
    use ast::expression::ComparisonExpr;
    use ast::expression::ComparisonOp;
    use ast::expression::Expression;
    use ast::expression::FunctionApplication;
    use ast::expression::LetExpr;
    use ast::expression::LogicalExpr;
    use ast::expression::LogicalOp;
    use ast::expression::NotExpr;
    use ast::expression::UnaryMinusExpr;

    let si = source_info_or_synthetic(f.source_information.as_ref());

    // Try to reverse-desugar binary operators
    if f.parameters.len() == 2 {
        if let Some(op) = match f.function.as_str() {
            "plus" => Some(ArithmeticOp::Plus),
            "minus" => Some(ArithmeticOp::Minus),
            "times" => Some(ArithmeticOp::Times),
            "divide" => Some(ArithmeticOp::Divide),
            _ => None,
        } {
            let left = convert_value_spec_to_expression(&f.parameters[0])?;
            let right = convert_value_spec_to_expression(&f.parameters[1])?;
            return Ok(Expression::Arithmetic(ArithmeticExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            }));
        }

        if let Some(op) = match f.function.as_str() {
            "equal" => Some(ComparisonOp::Equal),
            "notEqual" => Some(ComparisonOp::NotEqual),
            "lessThan" => Some(ComparisonOp::LessThan),
            "lessThanEqual" => Some(ComparisonOp::LessThanOrEqual),
            "greaterThan" => Some(ComparisonOp::GreaterThan),
            "greaterThanEqual" => Some(ComparisonOp::GreaterThanOrEqual),
            _ => None,
        } {
            let left = convert_value_spec_to_expression(&f.parameters[0])?;
            let right = convert_value_spec_to_expression(&f.parameters[1])?;
            return Ok(Expression::Comparison(ComparisonExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            }));
        }

        if let Some(op) = match f.function.as_str() {
            "and" => Some(LogicalOp::And),
            "or" => Some(LogicalOp::Or),
            _ => None,
        } {
            let left = convert_value_spec_to_expression(&f.parameters[0])?;
            let right = convert_value_spec_to_expression(&f.parameters[1])?;
            return Ok(Expression::Logical(LogicalExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            }));
        }
    }

    // Try to reverse-desugar unary operators
    if f.parameters.len() == 1 {
        if f.function == "not" {
            let operand = convert_value_spec_to_expression(&f.parameters[0])?;
            return Ok(Expression::Not(NotExpr {
                operand: Box::new(operand),
                source_info: si,
            }));
        }
        // Unary minus — only if the single arg is minus with 1 param
        if f.function == "minus" {
            let operand = convert_value_spec_to_expression(&f.parameters[0])?;
            return Ok(Expression::UnaryMinus(UnaryMinusExpr {
                operand: Box::new(operand),
                source_info: si,
            }));
        }
    }

    // letFunction('name', expr) → Let
    if f.function == "letFunction"
        && f.parameters.len() == 2
        && let v1::value_spec::ValueSpecification::String(name_cs) = &f.parameters[0]
    {
        let value = convert_value_spec_to_expression(&f.parameters[1])?;
        return Ok(Expression::Let(LetExpr {
            name: SmolStr::new(&name_cs.value),
            value: Box::new(value),
            source_info: si,
        }));
    }

    // General function application
    let (package, name) = parse_qualified_path(&f.function)?;
    let arguments: std::result::Result<Vec<_>, _> = f
        .parameters
        .iter()
        .map(convert_value_spec_to_expression)
        .collect();

    Ok(Expression::FunctionApplication(FunctionApplication {
        function: ast::annotation::PackageableElementPtr {
            package,
            name,
            source_info: si.clone(),
        },
        arguments: arguments?,
        source_info: si,
    }))
}

/// Converts an `AppliedProperty` to an AST `Expression`.
fn convert_applied_property(
    p: &v1::value_spec::AppliedProperty,
) -> Result<ast::expression::Expression> {
    use ast::expression::Expression;
    use ast::expression::MemberAccess;
    use ast::expression::QualifiedMemberAccess;
    use ast::expression::SimpleMemberAccess;

    let si = source_info_or_synthetic(p.source_information.as_ref());

    if p.parameters.is_empty() {
        return Err(ConversionError::UnsupportedValueSpec);
    }

    let target = convert_value_spec_to_expression(&p.parameters[0])?;

    if p.parameters.len() == 1 {
        // Simple member access: $x.name
        Ok(Expression::MemberAccess(MemberAccess::Simple(
            SimpleMemberAccess {
                target: Box::new(target),
                member: SmolStr::new(&p.property),
                source_info: si,
            },
        )))
    } else {
        // Qualified member access: $x.derived('arg')
        let arguments: std::result::Result<Vec<_>, _> = p.parameters[1..]
            .iter()
            .map(convert_value_spec_to_expression)
            .collect();
        Ok(Expression::MemberAccess(MemberAccess::Qualified(
            QualifiedMemberAccess {
                target: Box::new(target),
                member: SmolStr::new(&p.property),
                arguments: arguments?,
                source_info: si,
            },
        )))
    }
}

// ---------------------------------------------------------------------------
// Element conversions
// ---------------------------------------------------------------------------

/// Converts a protocol `PackageableElement` into an AST `Element`.
///
/// Note: `SectionIndex` elements are metadata and don't map to an AST `Element`,
/// so they return `None`.
pub fn convert_element(
    pe: &v1::element::PackageableElement,
) -> Result<Option<ast::element::Element>> {
    use v1::element::PackageableElement;

    match pe {
        PackageableElement::Class(c) => Ok(Some(ast::element::Element::Class(convert_class(c)?))),
        PackageableElement::Enumeration(e) => {
            Ok(Some(ast::element::Element::Enumeration(convert_enumeration(e)?)))
        }
        PackageableElement::Function(f) => {
            Ok(Some(ast::element::Element::Function(convert_function(f)?)))
        }
        PackageableElement::Profile(p) => {
            Ok(Some(ast::element::Element::Profile(convert_profile(p)?)))
        }
        PackageableElement::Association(a) => {
            Ok(Some(ast::element::Element::Association(convert_association(a)?)))
        }
        PackageableElement::Measure(m) => {
            Ok(Some(ast::element::Element::Measure(convert_measure(m)?)))
        }
        PackageableElement::SectionIndex(_) => Ok(None), // Metadata only
    }
}

fn convert_class(c: &v1::element::ProtocolClass) -> Result<ast::element::ClassDef> {
    let si = source_info_or_synthetic(c.source_information.as_ref());
    let package = parse_optional_package(&c.package_path)?;
    let properties: std::result::Result<Vec<_>, _> =
        c.properties.iter().map(convert_property).collect();
    let qualified_properties: std::result::Result<Vec<_>, _> = c
        .qualified_properties
        .iter()
        .map(convert_qualified_property)
        .collect();
    let constraints: std::result::Result<Vec<_>, _> =
        c.constraints.iter().map(convert_constraint).collect();
    let stereotypes: std::result::Result<Vec<_>, _> =
        c.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        c.tagged_values.iter().map(convert_tagged_value).collect();

    // Convert super_types from strings to TypeReferences
    let super_types: std::result::Result<Vec<_>, _> = c
        .super_types
        .iter()
        .map(|s| -> Result<ast::type_ref::TypeReference> {
            let path = parse_path(s)?;
            Ok(ast::type_ref::TypeReference {
                path,
                type_arguments: vec![],
                type_variable_values: vec![],
                source_info: si.clone(),
            })
        })
        .collect();

    Ok(ast::element::ClassDef {
        package,
        name: SmolStr::new(&c.name),
        type_parameters: vec![],
        super_types: super_types?,
        properties: properties?,
        qualified_properties: qualified_properties?,
        constraints: constraints?,
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        source_info: si,
    })
}

fn convert_enumeration(e: &v1::element::ProtocolEnumeration) -> Result<ast::element::EnumDef> {
    let si = source_info_or_synthetic(e.source_information.as_ref());
    let package = parse_optional_package(&e.package_path)?;
    let stereotypes: std::result::Result<Vec<_>, _> =
        e.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        e.tagged_values.iter().map(convert_tagged_value).collect();

    let values: std::result::Result<Vec<_>, _> = e
        .values
        .iter()
        .map(|v| -> Result<ast::element::EnumValue> {
            let vsi = source_info_or_synthetic(v.source_information.as_ref());
            let st: std::result::Result<Vec<_>, _> =
                v.stereotypes.iter().map(convert_stereotype_ptr).collect();
            let tv: std::result::Result<Vec<_>, _> =
                v.tagged_values.iter().map(convert_tagged_value).collect();
            Ok(ast::element::EnumValue {
                name: SmolStr::new(&v.value),
                stereotypes: st?,
                tagged_values: tv?,
                source_info: vsi,
            })
        })
        .collect();

    Ok(ast::element::EnumDef {
        package,
        name: SmolStr::new(&e.name),
        values: values?,
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        source_info: si,
    })
}

fn convert_function(f: &v1::element::ProtocolFunction) -> Result<ast::element::FunctionDef> {
    let si = source_info_or_synthetic(f.source_information.as_ref());
    let package = parse_optional_package(&f.package_path)?;
    let stereotypes: std::result::Result<Vec<_>, _> =
        f.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        f.tagged_values.iter().map(convert_tagged_value).collect();

    let parameters: std::result::Result<Vec<_>, _> = f
        .parameters
        .iter()
        .map(|v| -> Result<ast::annotation::Parameter> {
            let vsi = source_info_or_synthetic(v.source_information.as_ref());
            let gt = v.generic_type.as_ref().ok_or(ConversionError::UnsupportedValueSpec)?;
            let mult = v.multiplicity.as_ref().ok_or(ConversionError::UnsupportedValueSpec)?;
            Ok(ast::annotation::Parameter {
                name: SmolStr::new(&v.name),
                type_ref: Some(convert_generic_type(gt)?),
                multiplicity: Some(mult.into()),
                source_info: vsi,
            })
        })
        .collect();

    let body: std::result::Result<Vec<_>, _> = f
        .body
        .iter()
        .map(convert_value_spec_to_expression)
        .collect();

    Ok(ast::element::FunctionDef {
        package,
        name: SmolStr::new(&f.name),
        parameters: parameters?,
        return_type: convert_generic_type(&f.return_generic_type)?,
        return_multiplicity: (&f.return_multiplicity).into(),
        body: body?,
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        tests: vec![],
        source_info: si,
    })
}

fn convert_profile(p: &v1::element::ProtocolProfile) -> Result<ast::element::ProfileDef> {
    let si = source_info_or_synthetic(p.source_information.as_ref());
    let package = parse_optional_package(&p.package_path)?;

    Ok(ast::element::ProfileDef {
        package,
        name: SmolStr::new(&p.name),
        stereotypes: p
            .stereotypes
            .iter()
            .map(|s| ast::annotation::SpannedString {
                value: SmolStr::new(s),
                source_info: si.clone(),
            })
            .collect(),
        tags: p
            .tags
            .iter()
            .map(|t| ast::annotation::SpannedString {
                value: SmolStr::new(t),
                source_info: si.clone(),
            })
            .collect(),
        source_info: si,
    })
}

fn convert_association(
    a: &v1::element::ProtocolAssociation,
) -> Result<ast::element::AssociationDef> {
    let si = source_info_or_synthetic(a.source_information.as_ref());
    let package = parse_optional_package(&a.package_path)?;
    let properties: std::result::Result<Vec<_>, _> =
        a.properties.iter().map(convert_property).collect();
    let qualified_properties: std::result::Result<Vec<_>, _> = a
        .qualified_properties
        .iter()
        .map(convert_qualified_property)
        .collect();
    let stereotypes: std::result::Result<Vec<_>, _> =
        a.stereotypes.iter().map(convert_stereotype_ptr).collect();
    let tagged_values: std::result::Result<Vec<_>, _> =
        a.tagged_values.iter().map(convert_tagged_value).collect();

    Ok(ast::element::AssociationDef {
        package,
        name: SmolStr::new(&a.name),
        properties: properties?,
        qualified_properties: qualified_properties?,
        stereotypes: stereotypes?,
        tagged_values: tagged_values?,
        source_info: si,
    })
}

fn convert_measure(m: &v1::element::ProtocolMeasure) -> Result<ast::element::MeasureDef> {
    let si = source_info_or_synthetic(m.source_information.as_ref());
    let package = parse_optional_package(&m.package_path)?;

    let canonical_unit = m
        .canonical_unit
        .as_ref()
        .map(convert_unit_def)
        .transpose()?;
    let non_canonical: std::result::Result<Vec<_>, _> = m
        .non_canonical_units
        .iter()
        .map(convert_unit_def)
        .collect();

    Ok(ast::element::MeasureDef {
        package,
        name: SmolStr::new(&m.name),
        canonical_unit,
        non_canonical_units: non_canonical?,
        source_info: si,
    })
}

fn convert_unit_def(u: &v1::element::ProtocolUnit) -> Result<ast::element::UnitDef> {
    let si = source_info_or_synthetic(u.source_information.as_ref());
    // Unit name in protocol is "Measure~Unit", extract just the unit part
    let unit_name = u
        .name
        .rsplit_once('~')
        .map_or_else(|| u.name.as_str(), |(_, name)| name);

    let (conversion_param, conversion_body) = match &u.conversion_function {
        Some(lambda) => {
            let param = lambda.parameters.first().map(|v| SmolStr::new(&v.name));
            let body = lambda
                .body
                .first()
                .map(convert_value_spec_to_expression)
                .transpose()?;
            (param, body)
        }
        None => (None, None),
    };

    Ok(ast::element::UnitDef {
        name: SmolStr::new(unit_name),
        conversion_param,
        conversion_body,
        source_info: si,
    })
}

// ---------------------------------------------------------------------------
// Top-level: PureModelContextData → SourceFile
// ---------------------------------------------------------------------------

/// Converts a `PureModelContextData` into a parsed `SourceFile`.
///
/// This reconstructs the section structure from the `SectionIndex` element
/// and groups elements accordingly.
///
/// # Errors
///
/// Returns an error if any element conversion fails.
pub fn convert_context_to_source_file(
    ctx: &v1::context::PureModelContextData,
) -> Result<ast::section::SourceFile> {
    // Find the SectionIndex to reconstruct section structure
    let section_index = ctx.elements.iter().find_map(|e| {
        if let v1::element::PackageableElement::SectionIndex(si) = e {
            Some(si)
        } else {
            None
        }
    });

    // Convert all non-SectionIndex elements
    let element_map: std::collections::HashMap<String, ast::element::Element> = ctx
        .elements
        .iter()
        .filter_map(|e| {
            match convert_element(e) {
                Ok(Some(elem)) => {
                    use ast::element::PackageableElement as PE;
                    let fqn = match elem.package() {
                        Some(pkg) => format!("{pkg}::{}", elem.name()),
                        None => elem.name().to_string(),
                    };
                    Some((fqn, elem))
                }
                _ => None,
            }
        })
        .collect();

    let sections = if let Some(si) = section_index {
        si.sections
            .iter()
            .map(|proto_section| {
                let (parser_name, element_paths, imports, section_si) = match proto_section {
                    v1::element::ProtocolSection::Default(s) => {
                        let ssi = source_info_or_synthetic(s.source_information.as_ref());
                        (s.parser_name.as_str(), &s.elements, vec![], ssi)
                    }
                    v1::element::ProtocolSection::ImportAware(s) => {
                        let ssi = source_info_or_synthetic(s.source_information.as_ref());
                        let imports: Vec<ast::section::ImportStatement> = s
                            .imports
                            .iter()
                            .filter_map(|i| {
                                parse_path(i).ok().map(|path| ast::section::ImportStatement {
                                    path,
                                    source_info: ssi.clone(),
                                })
                            })
                            .collect();
                        (s.parser_name.as_str(), &s.elements, imports, ssi)
                    }
                };

                let elements: Vec<ast::element::Element> = element_paths
                    .iter()
                    .filter_map(|path| element_map.get(path).cloned())
                    .collect();

                ast::section::Section {
                    kind: SmolStr::new(parser_name),
                    imports,
                    elements,
                    source_info: section_si,
                }
            })
            .collect()
    } else {
        // No section index — put all elements in a single "Pure" section
        let all_elements: Vec<ast::element::Element> =
            element_map.into_values().collect();
        vec![ast::section::Section {
            kind: SmolStr::new("Pure"),
            imports: vec![],
            elements: all_elements,
            source_info: synthetic_source_info(),
        }]
    };

    let file_si = section_index
        .and_then(|si| si.source_information.as_ref())
        .map_or_else(synthetic_source_info, Into::into);

    Ok(ast::section::SourceFile {
        sections,
        source_info: file_si,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_parser_ast as ast;
    use legend_pure_parser_ast::type_ref::HasMultiplicity;

    #[test]
    fn test_source_info_roundtrip() {
        let ast_si = ast::SourceInfo::new("test.pure", 3, 5, 10, 20);
        let proto: v1::source_info::SourceInformation = (&ast_si).into();
        let back: ast::SourceInfo = (&proto).into();
        assert_eq!(back.source.as_str(), "test.pure");
        assert_eq!(back.start_line, 3);
        assert_eq!(back.start_column, 5);
        assert_eq!(back.end_line, 10);
        assert_eq!(back.end_column, 20);
    }

    #[test]
    fn test_multiplicity_roundtrip() {
        let ast_m = ast::Multiplicity::one();
        let proto: v1::multiplicity::Multiplicity = (&ast_m).into();
        let back: ast::Multiplicity = (&proto).into();
        assert_eq!(back.lower(), 1);
        assert_eq!(back.upper(), Some(1));
    }

    #[test]
    fn test_parse_path_simple() {
        let pkg = parse_path("meta::pure::profiles").unwrap();
        assert_eq!(pkg.to_string(), "meta::pure::profiles");
    }

    #[test]
    fn test_parse_path_single() {
        let pkg = parse_path("String").unwrap();
        assert_eq!(pkg.to_string(), "String");
    }

    #[test]
    fn test_parse_path_empty() {
        assert!(parse_path("").is_err());
    }

    #[test]
    fn test_parse_qualified_path() {
        let (pkg, name) = parse_qualified_path("model::domain::Person").unwrap();
        assert_eq!(pkg.unwrap().to_string(), "model::domain");
        assert_eq!(name.as_str(), "Person");
    }

    #[test]
    fn test_parse_qualified_path_no_package() {
        let (pkg, name) = parse_qualified_path("Person").unwrap();
        assert!(pkg.is_none());
        assert_eq!(name.as_str(), "Person");
    }

    #[test]
    fn test_integer_literal_roundtrip() {
        let proto_vs = v1::value_spec::ValueSpecification::Integer(v1::value_spec::CInteger {
            value: 42,
            source_information: None,
        });
        let expr = convert_value_spec_to_expression(&proto_vs).unwrap();
        match expr {
            ast::expression::Expression::Literal(ast::expression::Literal::Integer(i)) => {
                assert_eq!(i.value, 42);
            }
            other => panic!("Expected integer literal, got {other:?}"),
        }
    }

    #[test]
    fn test_variable_roundtrip() {
        let proto_vs = v1::value_spec::ValueSpecification::Var(v1::value_spec::Variable {
            name: "x".to_string(),
            generic_type: None,
            multiplicity: None,
            supports_stream: None,
            source_information: None,
        });
        let expr = convert_value_spec_to_expression(&proto_vs).unwrap();
        match expr {
            ast::expression::Expression::Variable(v) => {
                assert_eq!(v.name.as_str(), "x");
            }
            other => panic!("Expected variable, got {other:?}"),
        }
    }

    #[test]
    fn test_arithmetic_roundtrip() {
        // plus(1, 2) → 1 + 2
        let proto_vs = v1::value_spec::ValueSpecification::Func(v1::value_spec::AppliedFunction {
            function: "plus".to_string(),
            f_control: None,
            parameters: vec![
                v1::value_spec::ValueSpecification::Integer(v1::value_spec::CInteger {
                    value: 1,
                    source_information: None,
                }),
                v1::value_spec::ValueSpecification::Integer(v1::value_spec::CInteger {
                    value: 2,
                    source_information: None,
                }),
            ],
            source_information: None,
        });
        let expr = convert_value_spec_to_expression(&proto_vs).unwrap();
        match expr {
            ast::expression::Expression::Arithmetic(a) => {
                assert_eq!(a.op, ast::expression::ArithmeticOp::Plus);
            }
            other => panic!("Expected arithmetic, got {other:?}"),
        }
    }

    #[test]
    fn test_let_roundtrip() {
        // letFunction('x', 42) → let x = 42
        let proto_vs = v1::value_spec::ValueSpecification::Func(v1::value_spec::AppliedFunction {
            function: "letFunction".to_string(),
            f_control: None,
            parameters: vec![
                v1::value_spec::ValueSpecification::String(v1::value_spec::CString {
                    value: "x".to_string(),
                    source_information: None,
                }),
                v1::value_spec::ValueSpecification::Integer(v1::value_spec::CInteger {
                    value: 42,
                    source_information: None,
                }),
            ],
            source_information: None,
        });
        let expr = convert_value_spec_to_expression(&proto_vs).unwrap();
        match expr {
            ast::expression::Expression::Let(l) => {
                assert_eq!(l.name.as_str(), "x");
            }
            other => panic!("Expected let, got {other:?}"),
        }
    }

    #[test]
    fn test_profile_element_roundtrip() {
        let proto = v1::element::PackageableElement::Profile(v1::element::ProtocolProfile {
            package_path: "meta".to_string(),
            name: "doc".to_string(),
            stereotypes: vec!["deprecated".to_string()],
            tags: vec!["description".to_string()],
            source_information: None,
        });
        let elem = convert_element(&proto).unwrap().unwrap();
        match elem {
            ast::element::Element::Profile(p) => {
                assert_eq!(p.name.as_str(), "doc");
                assert_eq!(p.stereotypes.len(), 1);
                assert_eq!(p.tags.len(), 1);
            }
            other => panic!("Expected profile, got {other:?}"),
        }
    }

    #[test]
    fn test_class_element_roundtrip() {
        let proto = v1::element::PackageableElement::Class(v1::element::ProtocolClass {
            package_path: "model::domain".to_string(),
            name: "Person".to_string(),
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            original_milestoned_properties: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
            source_information: None,
        });
        let elem = convert_element(&proto).unwrap().unwrap();
        match elem {
            ast::element::Element::Class(c) => {
                assert_eq!(c.name.as_str(), "Person");
                assert_eq!(c.package.unwrap().to_string(), "model::domain");
            }
            other => panic!("Expected class, got {other:?}"),
        }
    }

    #[test]
    fn test_section_index_returns_none() {
        let proto =
            v1::element::PackageableElement::SectionIndex(v1::element::ProtocolSectionIndex {
                package_path: "__internal__".to_string(),
                name: "test.pure".to_string(),
                sections: vec![],
                source_information: None,
            });
        let result = convert_element(&proto).unwrap();
        assert!(result.is_none());
    }
}
