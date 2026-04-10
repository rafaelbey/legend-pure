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

//! Post-freeze validation pass on the compiled `PureModel`.
//!
//! This module implements **Pass 3** of the compiler pipeline — a read-only
//! scan over the frozen model that detects semantic errors not caught during
//! resolution (Pass 2). Because the model is frozen, this pass is safe to
//! run in parallel (future optimization).
//!
//! # Checks Performed
//!
//! | Check | Error Kind |
//! |-------|------------|
//! | Association must have exactly 2 properties | `InvalidAssociation` |
//! | Association properties must reference a Class | `InvalidAssociation` |
//! | Supertype must be a Class (not Enum, Function, etc.) | `InvalidSuperType` |
//! | No self-inheritance | `InvalidSuperType` |
//! | Stereotype must exist in the referenced Profile | `InvalidAnnotation` |
//! | Tag must exist in the referenced Profile | `InvalidAnnotation` |
//! | Annotation target must be a Profile element | `InvalidAnnotation` |
//! | No duplicate property names within a class | `DuplicateProperty` |

use std::collections::HashSet;

use smol_str::SmolStr;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::nodes::profile::Profile;
use crate::types::TypeExpr;

/// Validates the frozen `PureModel` and returns any errors found.
///
/// This is Pass 3 of the compiler pipeline. It runs after
/// `rebuild_derived_indexes()` and is purely read-only.
pub(crate) fn validate(model: &PureModel) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    // Skip chunk 0 (bootstrap) — those are compiler-trusted
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let id = ElementId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let node = chunk.nodes.get(local_idx);

            match element {
                Element::Class(class) => {
                    validate_super_types(model, id, node.name.clone(), class, &mut errors);
                    validate_duplicate_properties(&node.name, &class.properties, &mut errors);
                    validate_stereotypes(
                        model,
                        &node.name,
                        &class.stereotypes,
                        &node.source_info,
                        &mut errors,
                    );
                    validate_tagged_values(
                        model,
                        &node.name,
                        &class.tagged_values,
                        &node.source_info,
                        &mut errors,
                    );
                }
                Element::Association(assoc) => {
                    validate_association(model, &node.name, assoc, &node.source_info, &mut errors);
                    validate_stereotypes(
                        model,
                        &node.name,
                        &assoc.stereotypes,
                        &node.source_info,
                        &mut errors,
                    );
                    validate_tagged_values(
                        model,
                        &node.name,
                        &assoc.tagged_values,
                        &node.source_info,
                        &mut errors,
                    );
                }
                Element::Enumeration(enum_def) => {
                    validate_stereotypes(
                        model,
                        &node.name,
                        &enum_def.stereotypes,
                        &node.source_info,
                        &mut errors,
                    );
                    validate_tagged_values(
                        model,
                        &node.name,
                        &enum_def.tagged_values,
                        &node.source_info,
                        &mut errors,
                    );
                }
                Element::Function(func) => {
                    validate_stereotypes(
                        model,
                        &node.name,
                        &func.stereotypes,
                        &node.source_info,
                        &mut errors,
                    );
                    validate_tagged_values(
                        model,
                        &node.name,
                        &func.tagged_values,
                        &node.source_info,
                        &mut errors,
                    );
                }
                Element::Measure(_)
                | Element::Unit(_)
                | Element::Profile(_)
                | Element::PrimitiveType(_) => {}
            }
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Association Validation
// ---------------------------------------------------------------------------

/// Validates an association:
/// - Must have exactly 2 properties
/// - Each property must reference a Class
#[allow(clippy::collapsible_if)]
fn validate_association(
    model: &PureModel,
    assoc_name: &SmolStr,
    assoc: &crate::nodes::association::Association,
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    let prop_count = assoc.properties.len();
    if prop_count != 2 {
        errors.push(CompilationError {
            message: format!(
                "Association '{assoc_name}' must have exactly 2 properties, found {prop_count}"
            ),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::InvalidAssociation {
                name: assoc_name.clone(),
                reason: SmolStr::new(format!("expected 2 properties, found {prop_count}")),
            },
        });
    }

    // Validate each property references a Class
    for prop in &assoc.properties {
        if let TypeExpr::Named {
            element: target_id, ..
        } = &prop.type_expr
        {
            if let Some(target_element) = model.try_get_element(*target_id) {
                if !matches!(target_element, Element::Class(_)) {
                    let target_name = model.get_node(*target_id).name.clone();
                    errors.push(CompilationError {
                        message: format!(
                            "Association '{assoc_name}' property '{}' must reference a Class, \
                             but '{}' is not a Class",
                            prop.name, target_name
                        ),
                        source_info: prop.source_info.clone(),
                        kind: CompilationErrorKind::InvalidAssociation {
                            name: assoc_name.clone(),
                            reason: SmolStr::new(format!(
                                "property '{}' references non-Class '{}'",
                                prop.name, target_name
                            )),
                        },
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Super-type Validation
// ---------------------------------------------------------------------------

/// Validates class super-types:
/// - Must reference a Class (not Enum, Function, etc.)
/// - Must not be self-referential
#[allow(clippy::collapsible_if)]
fn validate_super_types(
    model: &PureModel,
    class_id: ElementId,
    class_name: SmolStr,
    class: &crate::nodes::class::Class,
    errors: &mut Vec<CompilationError>,
) {
    let class_node = model.get_node(class_id);
    for super_type in &class.super_types {
        if let TypeExpr::Named {
            element: super_id, ..
        } = super_type
        {
            // Self-inheritance check
            if *super_id == class_id {
                errors.push(CompilationError {
                    message: format!("Class '{class_name}' cannot extend itself"),
                    source_info: class_node.source_info.clone(),
                    kind: CompilationErrorKind::InvalidSuperType {
                        class_name: class_name.clone(),
                        super_name: class_name.clone(),
                    },
                });
                continue;
            }

            // Kind check: super must be a Class
            if let Some(super_element) = model.try_get_element(*super_id) {
                if !matches!(super_element, Element::Class(_)) {
                    let super_name = model.get_node(*super_id).name.clone();
                    errors.push(CompilationError {
                        message: format!(
                            "Class '{class_name}' cannot extend '{super_name}': \
                             only Classes can be extended"
                        ),
                        source_info: class_node.source_info.clone(),
                        kind: CompilationErrorKind::InvalidSuperType {
                            class_name,
                            super_name,
                        },
                    });
                    return; // Only report the first invalid super-type
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Annotation Validation
// ---------------------------------------------------------------------------

/// Validates stereotype references:
/// - Target element must be a Profile
/// - The stereotype name must exist in the Profile
fn validate_stereotypes(
    model: &PureModel,
    element_name: &SmolStr,
    stereotypes: &[StereotypeRef],
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    for stereo in stereotypes {
        match model.try_get_element(stereo.profile) {
            Some(Element::Profile(profile)) => {
                validate_stereotype_exists(
                    element_name,
                    &stereo.value,
                    profile,
                    source_info,
                    errors,
                );
            }
            Some(_) => {
                let target_name = model.get_node(stereo.profile).name.clone();
                errors.push(CompilationError {
                    message: format!(
                        "Stereotype target '{target_name}' on '{element_name}' is not a Profile"
                    ),
                    source_info: source_info.clone(),
                    kind: CompilationErrorKind::InvalidAnnotation {
                        element_name: element_name.clone(),
                        reason: SmolStr::new(format!("'{target_name}' is not a Profile")),
                    },
                });
            }
            None => {} // Resolution error already reported
        }
    }
}

/// Checks that a stereotype name actually exists in the profile.
fn validate_stereotype_exists(
    element_name: &SmolStr,
    stereotype_name: &SmolStr,
    profile: &Profile,
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    if !profile.stereotypes.iter().any(|s| s == stereotype_name) {
        let profile_stereos: Vec<&str> = profile.stereotypes.iter().map(SmolStr::as_str).collect();
        errors.push(CompilationError {
            message: format!(
                "Stereotype '{stereotype_name}' does not exist in the Profile. \
                 Available stereotypes: [{}]",
                profile_stereos.join(", ")
            ),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::InvalidAnnotation {
                element_name: element_name.clone(),
                reason: SmolStr::new(format!("stereotype '{stereotype_name}' not found")),
            },
        });
    }
}

/// Validates tagged value references:
/// - Target element must be a Profile
/// - The tag name must exist in the Profile
fn validate_tagged_values(
    model: &PureModel,
    element_name: &SmolStr,
    tagged_values: &[TaggedValueRef],
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    for tv in tagged_values {
        match model.try_get_element(tv.profile) {
            Some(Element::Profile(profile)) => {
                validate_tag_exists(element_name, &tv.tag, profile, source_info, errors);
            }
            Some(_) => {
                let target_name = model.get_node(tv.profile).name.clone();
                errors.push(CompilationError {
                    message: format!(
                        "Tag target '{target_name}' on '{element_name}' is not a Profile"
                    ),
                    source_info: source_info.clone(),
                    kind: CompilationErrorKind::InvalidAnnotation {
                        element_name: element_name.clone(),
                        reason: SmolStr::new(format!("'{target_name}' is not a Profile")),
                    },
                });
            }
            None => {} // Resolution error already reported
        }
    }
}

/// Checks that a tag name actually exists in the profile.
fn validate_tag_exists(
    element_name: &SmolStr,
    tag_name: &SmolStr,
    profile: &Profile,
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    if !profile.tags.iter().any(|t| t == tag_name) {
        let profile_tags: Vec<&str> = profile.tags.iter().map(SmolStr::as_str).collect();
        errors.push(CompilationError {
            message: format!(
                "Tag '{tag_name}' does not exist in the Profile. \
                 Available tags: [{}]",
                profile_tags.join(", ")
            ),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::InvalidAnnotation {
                element_name: element_name.clone(),
                reason: SmolStr::new(format!("tag '{tag_name}' not found")),
            },
        });
    }
}

// ---------------------------------------------------------------------------
// Duplicate Property Validation
// ---------------------------------------------------------------------------

/// Checks that no two properties in a class share the same name.
fn validate_duplicate_properties(
    class_name: &SmolStr,
    properties: &[crate::nodes::class::Property],
    errors: &mut Vec<CompilationError>,
) {
    let mut seen = HashSet::new();
    for prop in properties {
        if !seen.insert(&prop.name) {
            errors.push(CompilationError {
                message: format!(
                    "Duplicate property '{prop_name}' in class '{class_name}'",
                    prop_name = prop.name
                ),
                source_info: prop.source_info.clone(),
                kind: CompilationErrorKind::DuplicateProperty {
                    class_name: class_name.clone(),
                    property_name: prop.name.clone(),
                },
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::class::Property;
    use crate::types::Multiplicity;

    #[test]
    fn duplicate_property_detection() {
        let class_name = SmolStr::new("TestClass");
        let src = legend_pure_parser_ast::SourceInfo::new("test.pure", 1, 1, 1, 10);
        let props = vec![
            Property {
                name: SmolStr::new("name"),
                source_info: src.clone(),
                type_expr: TypeExpr::Named {
                    element: ElementId {
                        chunk_id: 0,
                        local_idx: 0,
                    },
                    type_arguments: vec![],
                    value_arguments: vec![],
                },
                multiplicity: Multiplicity::PureOne,
                aggregation: None,
                default_value: None,
                stereotypes: vec![],
                tagged_values: vec![],
            },
            Property {
                name: SmolStr::new("name"), // duplicate!
                source_info: src.clone(),
                type_expr: TypeExpr::Named {
                    element: ElementId {
                        chunk_id: 0,
                        local_idx: 0,
                    },
                    type_arguments: vec![],
                    value_arguments: vec![],
                },
                multiplicity: Multiplicity::PureOne,
                aggregation: None,
                default_value: None,
                stereotypes: vec![],
                tagged_values: vec![],
            },
        ];

        let mut errors = Vec::new();
        validate_duplicate_properties(&class_name, &props, &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0].kind,
            CompilationErrorKind::DuplicateProperty { .. }
        ));
    }
}
