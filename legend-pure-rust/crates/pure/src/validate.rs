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
//! | `<<access.private/protected>>` respected across packages | `NotAccessible` |
//! | At most one `<<access.X>>` stereotype per element | `MultipleAccessLevels` |
//! | Access stereotypes only on classes/functions | `AccessLevelNotAllowed` |

use std::collections::{HashMap, HashSet};

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::access::{self, AccessLevel, render_package_fqn, render_target_descriptor};
use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::{ElementId, PackageId};
use crate::model::{Element, PureModel};
use crate::nodes::association::Association;
use crate::nodes::class::{Class, Constraint, Property, QualifiedProperty};
use crate::nodes::function::Function;
use crate::nodes::profile::Profile;
use crate::types::{ExprKind, FunctionCallData, TypeExpr, ValueSpec};

/// Validates the frozen `PureModel` and returns any errors found.
///
/// This is Pass 3 of the compiler pipeline. It runs after
/// `rebuild_derived_indexes()` and is purely read-only.
#[tracing::instrument(level = "info", name = "validate", skip_all)]
// todo why we need most of these - should not the resolve do these like resolve_tagged_values and resolve_stereotypes rather than doing it here?
pub(crate) fn validate(model: &PureModel) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    // Repo-boundary visibility runs across every non-bootstrap chunk —
    // cross-repo refs are inherently a multi-chunk concern.
    errors.extend(validate_repo_visibility(model));

    // Access-level (`<<access.private/protected>>`) enforcement —
    // package-scoped, also a multi-chunk concern.
    errors.extend(validate_access_levels(model));

    // Validate every non-bootstrap chunk. Bootstrap (0) is
    // compiler-trusted. Earlier this routine validated only
    // `chunks.last()` on the assumption that prior chunks had been
    // validated when they were "last" — but `repo::load` runs
    // `finalize_model` only once, after every repo has been merged,
    // so anything but the final repo's elements would silently skip
    // per-element checks (stereotype-name correctness, duplicates,
    // bad supertypes, etc.). The LSP, which loads many repos and
    // surfaces diagnostics interactively, exposed the gap.
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let node = chunk.nodes.get(local_idx);

            match element {
                Element::Class(class) => {
                    validate_super_types(model, id, node.name.clone(), class, &mut errors);
                    validate_duplicate_properties(&node.name, &class.properties, &mut errors);
                    validate_class_type_args(model, &node.name, class, &mut errors);
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
                    validate_association_type_args(model, &node.name, assoc, &mut errors);
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
                    validate_function_type_args(model, &node.name, func, &node.source_info, &mut errors);
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
                | Element::PrimitiveType(_)
                | Element::PackageableMultiplicity(_)
                | Element::Package(_) => {}
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
                    // `element_name` (not `get_node`) — type-position
                    // targets shouldn't be Packages in well-formed
                    // input, but use the total accessor as defence.
                    let target_name = model.element_name(*target_id).clone();
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
// Type-Argument Completeness Validation
// ---------------------------------------------------------------------------

/// Walks a `TypeExpr`, recursing through nested type arguments, and
/// pushes an `InvalidAnnotation` error for every reference to a
/// generic class that doesn't supply its required type-arguments.
///
/// `Pair[1]` (where `Pair<U, V>` declares two type parameters) and
/// any other `Class<T1, …>` reference written without `<…>` is
/// malformed: with no `T` bindings, every downstream check
/// (dispatch, body-return, generic substitution) silently degrades
/// — failing here surfaces the upstream cause instead of the
/// noisy downstream symptoms.
///
/// `position_label` describes where the type appears (parameter,
/// return, property, etc.) so the error message points the user
/// at the right declaration site.
fn check_type_args_complete(
    model: &PureModel,
    type_expr: &TypeExpr,
    position_label: &str,
    owner_name: &SmolStr,
    source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    match type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            if let Some(Element::Class(class)) = model.try_get_element(*element) {
                let declared = class.type_parameters.len();
                let supplied = type_arguments.len();
                if declared > 0 && supplied != declared {
                    let class_fqn = SmolStr::new(
                        crate::purem::fqn_path::element_fqn_path(model, *element).join("::"),
                    );
                    errors.push(CompilationError {
                        message: format!(
                            "{position_label} of '{owner_name}' references generic class \
                             '{class_fqn}' without its required type arguments \
                             (expected {declared}, got {supplied})"
                        ),
                        source_info: source_info.clone(),
                        kind: CompilationErrorKind::InvalidAnnotation {
                            element_name: owner_name.clone(),
                            reason: SmolStr::new(format!(
                                "missing type arguments on '{class_fqn}': expected \
                                 {declared}, got {supplied}"
                            )),
                        },
                    });
                }
            }
            // Recurse into type-argument expressions — `List<Pair>`
            // (without args on `Pair`) must also error.
            for ta in type_arguments {
                check_type_args_complete(model, ta, position_label, owner_name, source_info, errors);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (pty, _) in parameters {
                check_type_args_complete(model, pty, position_label, owner_name, source_info, errors);
            }
            check_type_args_complete(model, return_type, position_label, owner_name, source_info, errors);
        }
        TypeExpr::AlgebraUnion(a, b) => {
            check_type_args_complete(model, a, position_label, owner_name, source_info, errors);
            check_type_args_complete(model, b, position_label, owner_name, source_info, errors);
        }
        TypeExpr::Generic(_) | TypeExpr::Relation(_) | TypeExpr::Unresolved => {}
    }
}

/// Walks every type-position reference on a `Class` (super-types,
/// property types, qualified-property param/return types) and
/// flags missing type arguments on generic class references.
fn validate_class_type_args(
    model: &PureModel,
    class_name: &SmolStr,
    class: &Class,
    errors: &mut Vec<CompilationError>,
) {
    for st in &class.super_types {
        // Super-types lack their own span; use the class's element
        // header span via the property's first member or fall back
        // to a synthetic span.
        let si = class
            .properties
            .first()
            .map(|p| p.source_info.clone())
            .unwrap_or_else(|| {
                legend_pure_parser_ast::SourceInfo::new("<unknown>", 0, 0, 0, 0)
            });
        check_type_args_complete(model, st, "super-type", class_name, &si, errors);
    }
    for prop in &class.properties {
        check_type_args_complete(
            model,
            &prop.type_expr,
            &format!("property '{}'", prop.name),
            class_name,
            &prop.source_info,
            errors,
        );
    }
    for qp in &class.qualified_properties {
        for param in qp.parameters.iter() {
            check_type_args_complete(
                model,
                &param.type_expr,
                &format!("qualified-property '{}' parameter '{}'", qp.name, param.name),
                class_name,
                &param.source_info,
                errors,
            );
        }
        check_type_args_complete(
            model,
            &qp.return_type,
            &format!("qualified-property '{}' return type", qp.name),
            class_name,
            &qp.source_info,
            errors,
        );
    }
}

/// Walks property types on an Association.
fn validate_association_type_args(
    model: &PureModel,
    assoc_name: &SmolStr,
    assoc: &crate::nodes::association::Association,
    errors: &mut Vec<CompilationError>,
) {
    for prop in &assoc.properties {
        check_type_args_complete(
            model,
            &prop.type_expr,
            &format!("property '{}'", prop.name),
            assoc_name,
            &prop.source_info,
            errors,
        );
    }
    for qp in &assoc.qualified_properties {
        for param in qp.parameters.iter() {
            check_type_args_complete(
                model,
                &param.type_expr,
                &format!("qualified-property '{}' parameter '{}'", qp.name, param.name),
                assoc_name,
                &param.source_info,
                errors,
            );
        }
        check_type_args_complete(
            model,
            &qp.return_type,
            &format!("qualified-property '{}' return type", qp.name),
            assoc_name,
            &qp.source_info,
            errors,
        );
    }
}

/// Walks parameter and return types on a Function.
fn validate_function_type_args(
    model: &PureModel,
    fn_name: &SmolStr,
    func: &Function,
    fn_source_info: &legend_pure_parser_ast::SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    for param in func.parameters.iter() {
        check_type_args_complete(
            model,
            &param.type_expr,
            &format!("parameter '{}'", param.name),
            fn_name,
            &param.source_info,
            errors,
        );
    }
    check_type_args_complete(
        model,
        &func.return_type,
        "return type",
        fn_name,
        fn_source_info,
        errors,
    );
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
                // `element_name` is total over both ElementId kinds —
                // the resolver only feeds Profile or genuine non-Profile
                // elements through to here (Package fallbacks are
                // intercepted in `resolve_stereotypes`), but
                // `element_name` is the safer accessor either way.
                let target_name = model.element_name(stereo.profile).clone();
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
                // `element_name` (not `get_node`) — same Package-fallback
                // safety as `validate_stereotypes`; the resolver
                // intercepts Package targets, but the safer accessor
                // costs nothing.
                let target_name = model.element_name(tv.profile).clone();
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
// Repo-boundary Visibility Validation
// ---------------------------------------------------------------------------

/// Walks every cross-repo `ElementId` reference and emits `NotVisible`
/// errors for each target whose home repo is not in the use-site repo's
/// declared dependencies. No-op when `model.repo_visibility` is empty,
/// so existing tests that build a model without going through a real
/// loader stay green.
fn validate_repo_visibility(model: &PureModel) -> Vec<CompilationError> {
    use crate::purem::walk::walk_element_ids;
    use crate::visibility::{check_element_visible, source_repo_name};

    let mut errors = Vec::new();
    if model.repo_visibility.is_empty() {
        return errors;
    }

    // Skip chunk 0 (bootstrap) — its source has no repo prefix.
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, _element) in chunk.elements.iter() {
            let node = chunk.nodes.get(local_idx);
            let use_site = &node.source_info.source;
            if source_repo_name(use_site).is_none() {
                continue;
            }

            // Clone the element so we can drive the mutable visitor
            // read-only. Element clone is O(its refs) so this is fine
            // at validate time.
            let mut owned = chunk.elements.get(local_idx).clone();
            walk_element_ids(&mut owned, |target| {
                if let Some(violation) = check_element_visible(model, use_site, *target) {
                    errors.push(CompilationError {
                        message: violation.message(),
                        source_info: node.source_info.clone(),
                        kind: CompilationErrorKind::NotVisible {
                            target_fqn: violation.target_fqn,
                            source_id: violation.use_site_source,
                        },
                    });
                }
            });
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Access-Level Validation (private / protected)
// ---------------------------------------------------------------------------

/// Walks every element in every non-bootstrap chunk and emits Java-parity
/// `NotAccessible` / `MultipleAccessLevels` / `AccessLevelNotAllowed`
/// diagnostics for `<<access.private>>` / `<<access.protected>>`
/// stereotypes that are misused or violated.
///
/// No-op when `meta::pure::profiles::access` isn't registered (so the
/// existing tests that build a model from raw sources without bootstrap
/// stay green).
fn validate_access_levels(model: &PureModel) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    let Some(access_profile) = access::access_profile_id(model) else {
        return errors;
    };

    // Memoize per-element access level — callees are referenced many
    // times across a chunk; recomputing per ref is wasteful.
    let mut level_cache: HashMap<ElementId, AccessLevel> = HashMap::new();

    // Skip chunk 0 (M3 bootstrap is compiler-trusted).
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let node = chunk.nodes.get(local_idx);

            // Step A — declaration-time checks.
            check_declaration_access(model, id, element, node, access_profile, &mut errors);

            // Step B — usage checks. Use-site package = this top-level
            // element's parent package.
            let use_site_pkg = node.parent_package;
            walk_element_for_access(
                model,
                element,
                node,
                use_site_pkg,
                &mut level_cache,
                &mut errors,
            );
        }
    }

    errors
}

/// Step A. Reject elements whose access stereotypes are invalid in
/// shape: more than one access stereotype, or an access stereotype on
/// something that isn't a class or function (the property cases live on
/// `Class` / `Association` / `QualifiedProperty`).
fn check_declaration_access(
    model: &PureModel,
    id: ElementId,
    element: &Element,
    node: &crate::model::ElementNode,
    access_profile: ElementId,
    errors: &mut Vec<CompilationError>,
) {
    // Multiple-access-level check — applies to any element kind that
    // carries stereotypes (the `access_level_stereotypes` helper returns
    // an empty iterator for the rest, so this is a no-op for them).
    let count = access::access_level_stereotypes(element, access_profile).count();
    if count > 1 {
        let descriptor = render_target_descriptor(model, id);
        errors.push(CompilationError {
            message: format!("{descriptor} has multiple access level stereotypes"),
            source_info: node.source_info.clone(),
            kind: CompilationErrorKind::MultipleAccessLevels {
                element_fqn: descriptor,
            },
        });
    }

    // "Only classes and functions may have an access level" — properties
    // (declared on Class or Association) and qualified properties carry
    // their own stereotype lists.
    match element {
        Element::Class(class) => {
            check_no_access_on_properties(
                model,
                id,
                &class.properties,
                &class.qualified_properties,
                access_profile,
                errors,
            );
        }
        Element::Association(assoc) => {
            check_no_access_on_properties(
                model,
                id,
                &assoc.properties,
                &assoc.qualified_properties,
                access_profile,
                errors,
            );
        }
        // Functions, classes (themselves), enums, and the rest are
        // either allowed (function/class) or never carry access stereos
        // (validated by the parser front-end).
        _ => {}
    }
}

fn check_no_access_on_properties(
    model: &PureModel,
    owner_id: ElementId,
    properties: &[Property],
    qualified_properties: &[QualifiedProperty],
    access_profile: ElementId,
    errors: &mut Vec<CompilationError>,
) {
    let owner_fqn = render_target_descriptor(model, owner_id);
    for prop in properties {
        if prop.stereotypes.iter().any(|s| s.profile == access_profile) {
            errors.push(CompilationError {
                message: "Only classes and functions may have an access level".to_string(),
                source_info: prop.source_info.clone(),
                kind: CompilationErrorKind::AccessLevelNotAllowed {
                    element_fqn: SmolStr::new(format!("{owner_fqn}::{}", prop.name)),
                    reason: SmolStr::new_static(
                        "Only classes and functions may have an access level",
                    ),
                },
            });
        }
    }
    for qp in qualified_properties {
        if qp.stereotypes.iter().any(|s| s.profile == access_profile) {
            errors.push(CompilationError {
                message: "Only classes and functions may have an access level".to_string(),
                source_info: qp.source_info.clone(),
                kind: CompilationErrorKind::AccessLevelNotAllowed {
                    element_fqn: SmolStr::new(format!("{owner_fqn}::{}", qp.name)),
                    reason: SmolStr::new_static(
                        "Only classes and functions may have an access level",
                    ),
                },
            });
        }
    }
}

/// Step B. Walk every reference inside `element` and check each one
/// against the use-site package's access window.
fn walk_element_for_access(
    model: &PureModel,
    element: &Element,
    node: &crate::model::ElementNode,
    use_site_pkg: PackageId,
    cache: &mut HashMap<ElementId, AccessLevel>,
    errors: &mut Vec<CompilationError>,
) {
    let mut emit = |target: ElementId, ref_si: &SourceInfo| {
        check_access_at(model, target, ref_si, use_site_pkg, cache, errors);
    };
    match element {
        Element::Class(class) => walk_class_refs(class, &node.source_info, &mut emit),
        Element::Function(func) => walk_function_refs(func, &node.source_info, &mut emit),
        Element::Association(assoc) => walk_association_refs(assoc, &mut emit),
        // Enumerations carry no element refs in their bodies; their
        // stereotypes are validated by `validate_stereotypes`. Profile,
        // Measure, Unit, PrimitiveType, PackageableMultiplicity, Package
        // either have no access-level surface or are owned by chunk 0.
        _ => {}
    }
}

fn walk_class_refs(
    class: &Class,
    class_si: &SourceInfo,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    for st in &class.super_types {
        // No per-supertype source location is preserved at lower time;
        // attribute to the class declaration's source info. This is a
        // known precision gap (parser doesn't emit per-ref spans here).
        walk_type_refs(st, class_si, emit);
    }
    for tp in &class.type_variable_parameters {
        walk_type_refs(&tp.type_expr, &tp.source_info, emit);
    }
    for prop in &class.properties {
        walk_property_refs(prop, emit);
    }
    for qp in &class.qualified_properties {
        walk_qualified_property_refs(qp, emit);
    }
    for con in &class.constraints {
        walk_constraint_refs(con, emit);
    }
}

fn walk_association_refs(assoc: &Association, emit: &mut impl FnMut(ElementId, &SourceInfo)) {
    for prop in &assoc.properties {
        walk_property_refs(prop, emit);
    }
    for qp in &assoc.qualified_properties {
        walk_qualified_property_refs(qp, emit);
    }
}

fn walk_function_refs(
    func: &Function,
    func_si: &SourceInfo,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    for param in func.parameters.iter() {
        walk_type_refs(&param.type_expr, &param.source_info, emit);
    }
    walk_type_refs(&func.return_type, func_si, emit);
    for expr in func.body.iter() {
        walk_value_spec_refs(expr, emit);
    }
}

fn walk_property_refs(prop: &Property, emit: &mut impl FnMut(ElementId, &SourceInfo)) {
    walk_type_refs(&prop.type_expr, &prop.source_info, emit);
    if let Some(dv) = &prop.default_value {
        walk_value_spec_refs(dv, emit);
    }
}

fn walk_qualified_property_refs(
    qp: &QualifiedProperty,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    for param in qp.parameters.iter() {
        walk_type_refs(&param.type_expr, &param.source_info, emit);
    }
    walk_type_refs(&qp.return_type, &qp.source_info, emit);
    for expr in qp.body.iter() {
        walk_value_spec_refs(expr, emit);
    }
}

fn walk_constraint_refs(con: &Constraint, emit: &mut impl FnMut(ElementId, &SourceInfo)) {
    walk_value_spec_refs(&con.function, emit);
    if let Some(m) = &con.message {
        walk_value_spec_refs(m, emit);
    }
}

fn walk_type_refs(
    ty: &TypeExpr,
    fallback_si: &SourceInfo,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    match ty {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            emit(*element, fallback_si);
            for arg in type_arguments {
                walk_type_refs(arg, fallback_si, emit);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (pty, _) in parameters {
                walk_type_refs(pty, fallback_si, emit);
            }
            walk_type_refs(return_type, fallback_si, emit);
        }
        TypeExpr::AlgebraUnion(a, b) => {
            walk_type_refs(a, fallback_si, emit);
            walk_type_refs(b, fallback_si, emit);
        }
        TypeExpr::Generic(_) | TypeExpr::Relation(_) | TypeExpr::Unresolved => {}
    }
}

fn walk_value_spec_refs(vs: &ValueSpec, emit: &mut impl FnMut(ElementId, &SourceInfo)) {
    walk_expr_kind_refs(&vs.kind, &vs.source_info, emit);
}

fn walk_expr_kind_refs(
    kind: &ExprKind,
    si: &SourceInfo,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    match kind {
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::Variable { .. }
        | ExprKind::Column => {}
        ExprKind::FunctionCall(d)
        | ExprKind::PropertyCall(d)
        | ExprKind::QualifiedPropertyCall(d) => {
            walk_function_call_data_refs(d, si, emit);
        }
        ExprKind::EnumValue { enum_element, .. } => {
            emit(*enum_element, si);
        }
        ExprKind::Lambda { parameters, body } => {
            for p in parameters {
                walk_type_refs(&p.type_expr, &p.source_info, emit);
            }
            for e in body {
                walk_value_spec_refs(e, emit);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                walk_value_spec_refs(e, emit);
            }
        }
        ExprKind::TypeReference { type_expr } => {
            walk_type_refs(type_expr, si, emit);
        }
        ExprKind::PackageableElementRef { element } => {
            emit(*element, si);
        }
        ExprKind::RelationLiteral { columns } => {
            for col in columns {
                emit(col.type_element, si);
            }
        }
        ExprKind::ColSpecArrayLiteral { columns, .. } => {
            for col in columns {
                emit(col.type_element, si);
            }
        }
        ExprKind::ColSpecLiteral { column, .. } => {
            emit(column.type_element, si);
        }
        ExprKind::PathLiteral {
            start_type, steps, ..
        } => {
            walk_type_refs(start_type, si, emit);
            for step in steps {
                for param in &step.parameters {
                    walk_value_spec_refs(param, emit);
                }
            }
        }
    }
}

fn walk_function_call_data_refs(
    d: &FunctionCallData,
    si: &SourceInfo,
    emit: &mut impl FnMut(ElementId, &SourceInfo),
) {
    if let Some(f) = &d.function {
        emit(*f, si);
    }
    for a in &d.arguments {
        walk_value_spec_refs(a, emit);
    }
}

fn check_access_at(
    model: &PureModel,
    target: ElementId,
    ref_si: &SourceInfo,
    use_site_pkg: PackageId,
    cache: &mut HashMap<ElementId, AccessLevel>,
    errors: &mut Vec<CompilationError>,
) {
    // Skip package refs, bootstrap-resident targets, and unknown ids.
    if matches!(target, ElementId::Package(_)) {
        return;
    }
    let ElementId::InstanceId { chunk_id, .. } = target else {
        return;
    };
    if chunk_id == 0 {
        return;
    }

    let level = *cache
        .entry(target)
        .or_insert_with(|| access::access_level_of(model, target));
    if matches!(level, AccessLevel::Public | AccessLevel::Externalizable) {
        return;
    }

    let target_pkg = model.get_node(target).parent_package;
    let visible = match level {
        AccessLevel::Private => use_site_pkg == target_pkg,
        AccessLevel::Protected => is_same_or_sub_package(model, use_site_pkg, target_pkg),
        AccessLevel::Public | AccessLevel::Externalizable => true,
    };
    if visible {
        return;
    }

    let descriptor = render_target_descriptor(model, target);
    let use_site_fqn = render_package_fqn(model, use_site_pkg);
    let message = format!("{descriptor} is not accessible in {use_site_fqn}");
    errors.push(CompilationError {
        message,
        source_info: ref_si.clone(),
        kind: CompilationErrorKind::NotAccessible {
            target_fqn: descriptor,
            use_site_package: use_site_fqn,
        },
    });
}

/// True if `use_site` is `target` itself or any sub-package of it. Java
/// `Visibility.isVisibleInPackage` walks the parent chain via
/// `_package`; mirroring it here ensures `pkg::sub` sees `pkg`'s
/// `<<access.protected>>` declarations but `other` does not.
fn is_same_or_sub_package(model: &PureModel, use_site: PackageId, target: PackageId) -> bool {
    let mut cur = Some(use_site);
    while let Some(p) = cur {
        if p == target {
            return true;
        }
        cur = model.get_package(p).parent;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::StereotypeRef;
    use crate::ids::PackageId;
    use crate::model::PureModel;
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
                    element: ElementId::InstanceId {
                        chunk_id: 0,
                        local_idx: 0,
                    },
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
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
                    element: ElementId::InstanceId {
                        chunk_id: 0,
                        local_idx: 0,
                    },
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
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

    /// `validate_stereotypes` used to call `model.get_node(profile)`
    /// without checking the ID kind, which panics for `Package` IDs
    /// (`get_node` rejects them by design — packages live in a
    /// separate arena). The fix is a switch to `model.element_name`,
    /// which is total over both kinds.
    ///
    /// In production the resolver intercepts Package fallbacks for
    /// stereotype profiles before they reach this validator (see
    /// `resolve::resolve_stereotypes`), but feeding one in directly
    /// still has to be panic-free.
    #[test]
    fn validate_stereotypes_handles_package_profile_without_panic() {
        let mut model = PureModel::new();
        // Create `test` as a child package of root. This is exactly
        // the shape the resolver used to produce when falling back
        // from `<<test.Foo>>` to package `test`.
        let pkg_id: PackageId =
            model.get_or_create_package(&[SmolStr::new("test")]);

        let stereos = vec![StereotypeRef {
            profile: ElementId::Package(pkg_id),
            value: SmolStr::new("Foo"),
        }];
        let element_name = SmolStr::new("someElement");
        let src = legend_pure_parser_ast::SourceInfo::new("t.pure", 1, 1, 1, 1);

        let mut errors = Vec::new();
        // Must not panic.
        validate_stereotypes(&model, &element_name, &stereos, &src, &mut errors);

        // The validator's own diagnostic is the "not a Profile" form —
        // the resolver-level filter is what suppresses double-reporting
        // in the full pipeline; this unit test exercises the validator
        // in isolation.
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors[0].kind,
            CompilationErrorKind::InvalidAnnotation { .. }
        ));
        // Error message must reference the package's name (not panic
        // by trying to reach into the chunk arena).
        assert!(
            errors[0].message.contains("test"),
            "expected error message to mention the package name, got: {}",
            errors[0].message
        );
    }

    /// A reference to a generic class without supplying its required
    /// type arguments must fail compilation. `function f(p: Pair[1])`
    /// where `Pair<U,V>` declares two type parameters must error —
    /// otherwise the type carries no anchoring for `U` / `V` and
    /// every downstream check (dispatch, body-return, generic
    /// substitution) silently degrades.
    #[test]
    fn validate_missing_type_arguments_on_generic_class_emits_error() {
        use crate::ids::PackageId;
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk, PureModel};
        use crate::nodes::class::Class;
        use crate::nodes::function::Function;
        use crate::types::{Multiplicity, Parameter, TypeExpr};

        let mut model = PureModel::new();
        let pkg: PackageId = model.get_or_create_package(&[SmolStr::new("test")]);
        // chunk 0 is the bootstrap chunk skipped by `validate()`; push
        // an empty placeholder so the user-content chunk lives at id 1.
        model.chunks.push(ModelChunk::new(0));
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let si = legend_pure_parser_ast::SourceInfo::new("t.pure", 1, 1, 1, 1);

        // Generic class `Pair<U, V>`.
        let pair_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("Pair"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg,
            },
            ModelElement::Class(Class {
                type_parameters: vec![SmolStr::new("U"), SmolStr::new("V")],
                multiplicity_parameters: Vec::new(),
                type_variable_parameters: Vec::new(),
                super_types: Vec::new(),
                properties: Vec::new(),
                qualified_properties: Vec::new(),
                constraints: Vec::new(),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let pair_id = ElementId::InstanceId {
            chunk_id,
            local_idx: pair_idx,
        };

        // Function `f(p: Pair[1]): Pair[1]` — both param and return
        // reference Pair *without* the required <U, V>.
        let pair_te_no_args = TypeExpr::Named {
            element: pair_id,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
        };
        let fn_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("f_Pair_1__Pair_1_"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg,
            },
            ModelElement::Function(Function {
                function_name: SmolStr::new("f"),
                is_native: false,
                parameters: std::sync::Arc::from(vec![Parameter {
                    name: SmolStr::new("p"),
                    type_expr: pair_te_no_args.clone(),
                    multiplicity: Multiplicity::PureOne,
                    source_info: si.clone(),
                }]),
                return_type: pair_te_no_args,
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let fn_id = ElementId::InstanceId {
            chunk_id,
            local_idx: fn_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, pair_id);
        model.register_element(pkg, fn_id);

        let errors = validate(&model);

        // Both the parameter type AND the return type reference
        // `Pair` without args — expect at least one error mentioning
        // Pair / type arguments.
        let arg_err = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("pair") && (m.contains("type argument") || m.contains("type-argument"))
        });
        assert!(
            arg_err.is_some(),
            "expected a missing-type-argument error mentioning Pair, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}
