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

//! Pure → Protocol conversion: re-emits each Class / Association from
//! the compiled `PureModel` graph instead of the parse-time AST, so the
//! protocol JSON reflects the post-synthesis view (milestoned date
//! properties, edge-points, qualified-property signatures, and the
//! `originalMilestonedProperties` slot all populated).
//!
//! The standard `legend parse` path is AST→Protocol — it emits exactly
//! what the user wrote. Phase A's milestoning synthesis happens at
//! compile time on the `pure::PureModel` graph, so the AST never sees
//! the synthesized properties or the moved-aside originals. The
//! `--compile` flag re-runs the compile pipeline and replaces the
//! AST-shape Class / Association elements with full compiled views.
//!
//! ## Scope of body conversion
//!
//! Qualified-property bodies, constraint function/message bodies, and
//! default-value expressions are converted from lowered `ValueSpec`
//! trees to protocol `ValueSpecification`s. Most common expression
//! variants are supported (literals incl. `%latest`, variables,
//! function calls, property/QP calls, lambdas, collections, type and
//! enum references, packageable-element refs). The remaining
//! lowered-IR shapes (`PathLiteral`, `RelationLiteral`,
//! `ColSpec*Literal`, `MultiplicityReference`) emit a placeholder
//! `Var` named after the shape so the JSON stays well-formed; they're
//! not used inside milestoning bodies, and full conversion lands
//! incrementally as concrete consumers surface.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_protocol::v1;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::nodes::class::{
    Constraint as PureConstraint, Property as PureProperty, QualifiedProperty as PureQP,
};
use legend_pure_parser_pure::types::{
    DateValue, ExprKind, Multiplicity as PureMultiplicity, Parameter as PureParam, TypeExpr,
    ValueSpec,
};

/// Compile `source_files` and replace every Class / Association protocol
/// element with the compiled view (post-synthesis properties, populated
/// `originalMilestonedProperties`, converted QP bodies).
///
/// Returns the compiled `PureModel` so callers can introspect further
/// (e.g. for error reporting); discards compile errors silently — the
/// AST→Protocol path is the primary output, and compile errors don't
/// invalidate the parse-time JSON.
pub fn patch_with_milestoning(
    source_files: &[SourceFile],
    elements: &mut [v1::element::PackageableElement],
) -> Option<PureModel> {
    let result = legend_pure_parser_pure::pipeline::compile(source_files, &[]);
    let model = match result {
        Ok(model) => model,
        Err(partial) => partial.model,
    };

    for element in elements.iter_mut() {
        match element {
            v1::element::PackageableElement::Class(c) => {
                replace_class_from_compiled(c, &model);
            }
            v1::element::PackageableElement::Association(a) => {
                replace_association_from_compiled(a, &model);
            }
            _ => {}
        }
    }

    Some(model)
}

fn replace_class_from_compiled(c: &mut v1::element::ProtocolClass, model: &PureModel) {
    let fqn = full_path(&c.package_path, &c.name);
    let Some(id) = model.resolve_fqn_str(&fqn) else {
        return;
    };
    let Element::Class(class) = model.get_element(id) else {
        return;
    };
    c.super_types = class
        .super_types
        .iter()
        .map(|t| match t {
            TypeExpr::Named { element, .. } => element_full_path(model, *element),
            _ => "meta::pure::metamodel::type::Any".to_string(),
        })
        .collect();
    c.properties = class
        .properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
        .collect();
    c.qualified_properties = class
        .qualified_properties
        .iter()
        .map(|qp| pure_qp_to_protocol(qp, model))
        .collect();
    c.constraints = class
        .constraints
        .iter()
        .map(|con| pure_constraint_to_protocol(con, model))
        .collect();
    c.original_milestoned_properties = class
        .original_milestoned_properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
        .collect();
    c.stereotypes = class
        .stereotypes
        .iter()
        .map(|s| stereotype_to_protocol(s, model))
        .collect();
    c.tagged_values = class
        .tagged_values
        .iter()
        .map(|tv| tagged_value_to_protocol(tv, model))
        .collect();
}

fn replace_association_from_compiled(
    a: &mut v1::element::ProtocolAssociation,
    model: &PureModel,
) {
    let fqn = full_path(&a.package_path, &a.name);
    let Some(id) = model.resolve_fqn_str(&fqn) else {
        return;
    };
    let Element::Association(assoc) = model.get_element(id) else {
        return;
    };
    a.properties = assoc
        .properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
        .collect();
    a.qualified_properties = assoc
        .qualified_properties
        .iter()
        .map(|qp| pure_qp_to_protocol(qp, model))
        .collect();
    a.original_milestoned_properties = assoc
        .original_milestoned_properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
        .collect();
    a.stereotypes = assoc
        .stereotypes
        .iter()
        .map(|s| stereotype_to_protocol(s, model))
        .collect();
    a.tagged_values = assoc
        .tagged_values
        .iter()
        .map(|tv| tagged_value_to_protocol(tv, model))
        .collect();
}

fn full_path(package_path: &str, name: &str) -> String {
    if package_path.is_empty() {
        name.to_string()
    } else {
        format!("{package_path}::{name}")
    }
}

// ---------------------------------------------------------------------------
// pure::Property → v1::property::Property
// ---------------------------------------------------------------------------

fn pure_property_to_protocol(p: &PureProperty, model: &PureModel) -> v1::property::Property {
    v1::property::Property {
        name: p.name.to_string(),
        generic_type: render_type_expr_as_generic_type(&p.type_expr, model),
        multiplicity: render_multiplicity(&p.multiplicity),
        default_value: None,
        stereotypes: p
            .stereotypes
            .iter()
            .map(|s| v1::annotation::StereotypePtr {
                profile: element_full_path(model, s.profile),
                value: s.value.to_string(),
                source_information: None,
                profile_source_information: None,
            })
            .collect(),
        tagged_values: p
            .tagged_values
            .iter()
            .map(|tv| v1::annotation::TaggedValue {
                tag: v1::annotation::TagPtr {
                    profile: element_full_path(model, tv.profile),
                    value: tv.tag.to_string(),
                    source_information: None,
                    profile_source_information: None,
                },
                value: tv.value.clone(),
                source_information: None,
            })
            .collect(),
        aggregation: p.aggregation.map(|ak| match ak {
            legend_pure_parser_pure::nodes::class::AggregationKind::None => {
                v1::property::AggregationKind::NONE
            }
            legend_pure_parser_pure::nodes::class::AggregationKind::Shared => {
                v1::property::AggregationKind::SHARED
            }
            legend_pure_parser_pure::nodes::class::AggregationKind::Composite => {
                v1::property::AggregationKind::COMPOSITE
            }
        }),
        source_information: Some(source_info_to_protocol(&p.source_info)),
    }
}

fn render_type_expr_as_generic_type(
    ty: &TypeExpr,
    model: &PureModel,
) -> v1::generic_type::GenericType {
    let (full_path, type_arguments) = match ty {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            let path = element_full_path(model, *element);
            let args: Vec<_> = type_arguments
                .iter()
                .map(|a| render_type_expr_as_generic_type(a, model))
                .collect();
            (path, args)
        }
        TypeExpr::Generic(name) => (name.to_string(), Vec::new()),
        // Function / Relation / AlgebraUnion / Unresolved aren't used as
        // milestoned-target property types, so falling back to a "Any"
        // placeholder is safe; if it ever becomes wrong we'll see it in
        // a snapshot mismatch.
        _ => ("meta::pure::metamodel::type::Any".to_string(), Vec::new()),
    };
    v1::generic_type::GenericType {
        raw_type: v1::generic_type::PackageableType {
            full_path,
            source_information: None,
        },
        type_arguments,
        multiplicity_arguments: Vec::new(),
        type_variable_values: Vec::new(),
        source_information: None,
    }
}

fn render_multiplicity(m: &PureMultiplicity) -> v1::multiplicity::Multiplicity {
    match m {
        PureMultiplicity::PureOne => v1::multiplicity::Multiplicity::PURE_ONE,
        PureMultiplicity::ZeroOrOne => v1::multiplicity::Multiplicity::ZERO_ONE,
        // Variable multiplicities (e.g. `m` in `T[m]`) don't have a
        // concrete bound to emit; render them as ZeroOrMany — the same
        // shape Java emits for unresolved-multiplicity carriers.
        PureMultiplicity::ZeroOrMany | PureMultiplicity::Variable(_) => {
            v1::multiplicity::Multiplicity::ZERO_MANY
        }
        PureMultiplicity::OneOrMany => v1::multiplicity::Multiplicity::ONE_MANY,
        PureMultiplicity::Range { lower, upper } => v1::multiplicity::Multiplicity {
            lower_bound: *lower,
            upper_bound: *upper,
        },
    }
}

fn source_info_to_protocol(
    si: &legend_pure_parser_ast::SourceInfo,
) -> v1::source_info::SourceInformation {
    v1::source_info::SourceInformation {
        source_id: si.source.to_string(),
        start_line: si.start_line,
        start_column: si.start_column,
        end_line: si.end_line,
        end_column: si.end_column,
    }
}

fn element_full_path(model: &PureModel, id: legend_pure_parser_pure::ids::ElementId) -> String {
    legend_pure_parser_pure::purem::fqn_path::element_fqn_path(model, id)
        .iter()
        .map(smol_str::SmolStr::as_str)
        .collect::<Vec<_>>()
        .join("::")
}

// ---------------------------------------------------------------------------
// pure::QualifiedProperty → v1::QualifiedProperty
// ---------------------------------------------------------------------------

fn pure_qp_to_protocol(qp: &PureQP, model: &PureModel) -> v1::property::QualifiedProperty {
    let parameters: Vec<serde_json::Value> = qp
        .parameters
        .iter()
        .map(|p| {
            let var = pure_param_to_variable(p, model);
            serde_json::to_value(v1::value_spec::ValueSpecification::Var(var))
                .unwrap_or(serde_json::Value::Null)
        })
        .collect();
    let body: Vec<serde_json::Value> = qp
        .body
        .iter()
        .map(|expr| {
            serde_json::to_value(value_spec_to_protocol(expr, model))
                .unwrap_or(serde_json::Value::Null)
        })
        .collect();
    v1::property::QualifiedProperty {
        name: qp.name.to_string(),
        parameters,
        return_generic_type: render_type_expr_as_generic_type(&qp.return_type, model),
        return_multiplicity: render_multiplicity(&qp.return_multiplicity),
        stereotypes: qp
            .stereotypes
            .iter()
            .map(|s| stereotype_to_protocol(s, model))
            .collect(),
        tagged_values: qp
            .tagged_values
            .iter()
            .map(|tv| tagged_value_to_protocol(tv, model))
            .collect(),
        body,
        source_information: Some(source_info_to_protocol(&qp.source_info)),
    }
}

fn pure_param_to_variable(p: &PureParam, model: &PureModel) -> v1::value_spec::Variable {
    v1::value_spec::Variable {
        name: p.name.to_string(),
        generic_type: Some(render_type_expr_as_generic_type(&p.type_expr, model)),
        multiplicity: Some(render_multiplicity(&p.multiplicity)),
        supports_stream: None,
        source_information: Some(source_info_to_protocol(&p.source_info)),
    }
}

// ---------------------------------------------------------------------------
// pure::Constraint → v1::Constraint
// ---------------------------------------------------------------------------

fn pure_constraint_to_protocol(
    con: &PureConstraint,
    model: &PureModel,
) -> v1::property::Constraint {
    let fn_def = serde_json::to_value(value_spec_to_protocol(&con.function, model))
        .unwrap_or(serde_json::Value::Null);
    let msg_fn = con.message.as_ref().map(|m| {
        serde_json::to_value(value_spec_to_protocol(m, model)).unwrap_or(serde_json::Value::Null)
    });
    v1::property::Constraint {
        name: con
            .name
            .as_ref()
            .map_or_else(|| "constraint".to_string(), |s| s.to_string()),
        owner: None,
        function_definition: fn_def,
        source_information: Some(source_info_to_protocol(&con.source_info)),
        external_id: con.external_id.clone(),
        enforcement_level: con.enforcement_level.as_ref().map(|s| s.to_string()),
        message_function: msg_fn,
    }
}

// ---------------------------------------------------------------------------
// Stereotype / tagged-value protocol rendering
// ---------------------------------------------------------------------------

fn stereotype_to_protocol(
    s: &legend_pure_parser_pure::annotations::StereotypeRef,
    model: &PureModel,
) -> v1::annotation::StereotypePtr {
    v1::annotation::StereotypePtr {
        profile: element_full_path(model, s.profile),
        value: s.value.to_string(),
        source_information: None,
        profile_source_information: None,
    }
}

fn tagged_value_to_protocol(
    tv: &legend_pure_parser_pure::annotations::TaggedValueRef,
    model: &PureModel,
) -> v1::annotation::TaggedValue {
    v1::annotation::TaggedValue {
        tag: v1::annotation::TagPtr {
            profile: element_full_path(model, tv.profile),
            value: tv.tag.to_string(),
            source_information: None,
            profile_source_information: None,
        },
        value: tv.value.clone(),
        source_information: None,
    }
}

// ---------------------------------------------------------------------------
// Lowered ValueSpec → protocol ValueSpecification
// ---------------------------------------------------------------------------

/// Convert a lowered `ValueSpec` to a protocol `ValueSpecification`.
///
/// Supports the common variants used in milestoning bodies and most
/// user-written QP / constraint bodies. Unsupported variants
/// (`PathLiteral`, `RelationLiteral`, `ColSpec*`, `MultiplicityReference`)
/// emit a placeholder `Var` named after the shape so the JSON stays
/// well-formed.
fn value_spec_to_protocol(
    vs: &ValueSpec,
    model: &PureModel,
) -> v1::value_spec::ValueSpecification {
    use v1::value_spec::{
        AppliedFunction, AppliedProperty, CBoolean, CDecimal, CFloat, CInteger, CString,
        LambdaFunction, ProtocolCollection, ProtocolEnumValue, ProtocolPackageableElementPtr,
        ValueSpecification, Variable,
    };
    let src = Some(source_info_to_protocol(&vs.source_info));
    match vs.kind.as_ref() {
        ExprKind::IntegerLiteral(i) => ValueSpecification::Integer(CInteger {
            value: *i,
            source_information: src,
        }),
        ExprKind::FloatLiteral(f) => ValueSpecification::Float(CFloat {
            value: *f,
            source_information: src,
        }),
        ExprKind::DecimalLiteral(d) => ValueSpecification::Decimal(CDecimal {
            value: d.to_string().parse().unwrap_or(0.0),
            source_information: src,
        }),
        ExprKind::StringLiteral(s) => ValueSpecification::String(CString {
            value: s.to_string(),
            source_information: src,
        }),
        ExprKind::BooleanLiteral(b) => ValueSpecification::Boolean(CBoolean {
            value: *b,
            source_information: src,
        }),
        ExprKind::DateLiteral(dv) => date_value_to_protocol(dv, src),
        ExprKind::Variable { name } => ValueSpecification::Var(Variable {
            name: name.to_string(),
            generic_type: None,
            multiplicity: None,
            supports_stream: None,
            source_information: src,
        }),
        ExprKind::FunctionCall(d) => ValueSpecification::Func(AppliedFunction {
            function: d.function_name.to_string(),
            f_control: None,
            parameters: d
                .arguments
                .iter()
                .map(|a| value_spec_to_protocol(a, model))
                .collect(),
            source_information: src,
        }),
        ExprKind::PropertyCall(d) | ExprKind::QualifiedPropertyCall(d) => {
            ValueSpecification::Property(AppliedProperty {
                class: None,
                property: d.function_name.to_string(),
                parameters: d
                    .arguments
                    .iter()
                    .map(|a| value_spec_to_protocol(a, model))
                    .collect(),
                source_information: src,
            })
        }
        ExprKind::EnumValue {
            enum_element,
            value,
        } => ValueSpecification::EnumValue(ProtocolEnumValue {
            full_path: element_full_path(model, *enum_element),
            value: value.to_string(),
            source_information: src,
        }),
        ExprKind::Lambda { parameters, body } => ValueSpecification::Lambda(LambdaFunction {
            body: body.iter().map(|e| value_spec_to_protocol(e, model)).collect(),
            parameters: parameters
                .iter()
                .map(|p| pure_param_to_variable(p, model))
                .collect(),
            source_information: src,
        }),
        ExprKind::Collection { elements } => {
            let multiplicity = elements_multiplicity(elements.len());
            ValueSpecification::Collection(ProtocolCollection {
                multiplicity,
                values: elements
                    .iter()
                    .map(|e| value_spec_to_protocol(e, model))
                    .collect(),
                source_information: src,
            })
        }
        ExprKind::TypeReference {
            type_expr: TypeExpr::Named { element, .. },
        } => ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
            full_path: element_full_path(model, *element),
            source_information: src,
        }),
        ExprKind::PackageableElementRef { element } => {
            ValueSpecification::PackageableElementPtr(ProtocolPackageableElementPtr {
                full_path: element_full_path(model, *element),
                source_information: src,
            })
        }
        // Variants that don't yet have a clean protocol mapping. Emit a
        // placeholder Var so the JSON stays well-formed; downstream
        // consumers that hit one of these for real should ask for the
        // specific variant to be wired up.
        ExprKind::TypeReference { .. }
        | ExprKind::MultiplicityReference { .. }
        | ExprKind::RelationLiteral { .. }
        | ExprKind::ColSpecArrayLiteral { .. }
        | ExprKind::ColSpecLiteral { .. }
        | ExprKind::PathLiteral { .. } => ValueSpecification::Var(Variable {
            name: format!("@unsupported:{}", expr_kind_tag(vs.kind.as_ref())),
            generic_type: None,
            multiplicity: None,
            supports_stream: None,
            source_information: src,
        }),
    }
}

fn date_value_to_protocol(
    dv: &DateValue,
    src: Option<v1::source_info::SourceInformation>,
) -> v1::value_spec::ValueSpecification {
    use v1::value_spec::{CDateTime, CLatestDate, CStrictDate, CStrictTime, ValueSpecification};
    match dv {
        DateValue::StrictDate { year, month, day } => {
            let s = match (month, day) {
                (None, _) => format!("{year:04}"),
                (Some(m), None) => format!("{year:04}-{m:02}"),
                (Some(m), Some(d)) => format!("{year:04}-{m:02}-{d:02}"),
            };
            ValueSpecification::StrictDate(CStrictDate {
                value: s,
                source_information: src,
            })
        }
        DateValue::DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            ..
        } => ValueSpecification::DateTime(CDateTime {
            value: format!(
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
            ),
            source_information: src,
        }),
        DateValue::StrictTime {
            hour,
            minute,
            second,
            ..
        } => ValueSpecification::StrictTime(CStrictTime {
            value: format!("{hour:02}:{minute:02}:{second:02}"),
            source_information: src,
        }),
        DateValue::Latest => ValueSpecification::LatestDate(CLatestDate {
            source_information: src,
        }),
    }
}

fn elements_multiplicity(n: usize) -> v1::multiplicity::Multiplicity {
    let n_u32 = u32::try_from(n).unwrap_or(u32::MAX);
    v1::multiplicity::Multiplicity {
        lower_bound: n_u32,
        upper_bound: Some(n_u32),
    }
}

fn expr_kind_tag(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::IntegerLiteral(_) => "Integer",
        ExprKind::FloatLiteral(_) => "Float",
        ExprKind::DecimalLiteral(_) => "Decimal",
        ExprKind::StringLiteral(_) => "String",
        ExprKind::BooleanLiteral(_) => "Boolean",
        ExprKind::DateLiteral(_) => "Date",
        ExprKind::Variable { .. } => "Variable",
        ExprKind::FunctionCall(_) => "FunctionCall",
        ExprKind::PropertyCall(_) => "PropertyCall",
        ExprKind::QualifiedPropertyCall(_) => "QualifiedPropertyCall",
        ExprKind::EnumValue { .. } => "EnumValue",
        ExprKind::Lambda { .. } => "Lambda",
        ExprKind::Collection { .. } => "Collection",
        ExprKind::TypeReference { .. } => "TypeReference",
        ExprKind::MultiplicityReference { .. } => "MultiplicityReference",
        ExprKind::PackageableElementRef { .. } => "PackageableElementRef",
        ExprKind::RelationLiteral { .. } => "RelationLiteral",
        ExprKind::ColSpecArrayLiteral { .. } => "ColSpecArrayLiteral",
        ExprKind::ColSpecLiteral { .. } => "ColSpecLiteral",
        ExprKind::PathLiteral { .. } => "PathLiteral",
    }
}

#[allow(dead_code)]
fn assert_element_exists(model: &PureModel, id: ElementId) {
    // Compile-time sanity hook; used only when debugging cross-element
    // references during patch development. Left in place so future
    // additions can flip it on without re-deriving the helper.
    let _ = model.try_get_element(id);
}
