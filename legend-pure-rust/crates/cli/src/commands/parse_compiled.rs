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

//! Pure → Protocol patch step: enriches AST-shape protocol JSON with
//! compile-time metadata, focusing on milestoning synthesis.
//!
//! The standard `legend parse` path is AST→Protocol — it emits exactly
//! what the user wrote. Phase A's milestoning synthesis happens at
//! compile time on the `pure::PureModel` graph, so the AST never sees
//! the synthesized properties or the moved-aside originals. For
//! downstream consumers that need Java-parity wire output (where Java's
//! engine emits the compiled view), we re-run the compile pipeline and
//! patch the protocol elements with the milestoning data only the
//! compiled graph carries.
//!
//! Today the patch only populates `original_milestoned_properties` on
//! `Class` / `Association`. Full Pure→Protocol conversion (post-synthesis
//! `properties` + `qualified_properties`, lowered expression bodies) is
//! tracked separately — each piece is a substantial addition. The
//! current scope gives downstream tooling visibility into "which
//! properties did the compiler move aside?" without rewriting every
//! converter.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_protocol::v1;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::nodes::class::Property as PureProperty;
use legend_pure_parser_pure::types::{Multiplicity as PureMultiplicity, TypeExpr};

/// Compile `source_files` and patch every Class / Association protocol
/// element with milestoning data from the compiled model.
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
                patch_class(c, &model);
            }
            v1::element::PackageableElement::Association(a) => {
                patch_association(a, &model);
            }
            _ => {}
        }
    }

    Some(model)
}

fn patch_class(c: &mut v1::element::ProtocolClass, model: &PureModel) {
    let fqn = full_path(&c.package_path, &c.name);
    let Some(id) = model.resolve_fqn_str(&fqn) else {
        return;
    };
    let Element::Class(class) = model.get_element(id) else {
        return;
    };
    c.original_milestoned_properties = class
        .original_milestoned_properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
        .collect();
}

fn patch_association(a: &mut v1::element::ProtocolAssociation, model: &PureModel) {
    let fqn = full_path(&a.package_path, &a.name);
    let Some(id) = model.resolve_fqn_str(&fqn) else {
        return;
    };
    let Element::Association(assoc) = model.get_element(id) else {
        return;
    };
    a.original_milestoned_properties = assoc
        .original_milestoned_properties
        .iter()
        .map(|p| pure_property_to_protocol(p, model))
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
