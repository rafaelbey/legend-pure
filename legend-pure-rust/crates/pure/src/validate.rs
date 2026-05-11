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

//! Validation entry points.
//!
//! Java-parity placement: validators run **next to the data they
//! inspect**. Adding a new element kind means adding match arms in
//! `create_shell` (syntactic shell) and `hydrate_element_signature`
//! (where each validator is called inline) — no batch passes to
//! wire up.
//!
//! Three seams, each as eager as soundness allows:
//!
//! * **Resolver-eager** (`crate::resolve`):
//!   - Generic-class type-arg completeness fires inside
//!     `resolve_type_ref` the moment a `TypeExpr::Named` is built.
//!   - Stereotype/tag profile-kind + name-existence fires inside
//!     `resolve_stereotypes` / `resolve_tagged_values` the moment
//!     the profile is resolved. Sound because `create_shell` (Pass 1)
//!     populates `Profile.stereotypes` / `tags` from the AST, so the
//!     referenced profile's name list is visible regardless of
//!     topological hydration order.
//!
//! * **Per-element-eager** (called directly from
//!   [`crate::pipeline::hydrate_element_signature`]):
//!   `validate_super_types`, `validate_association`,
//!   `validate_duplicate_properties`,
//!   `validate_no_multiple_access_levels`, and
//!   `validate_no_access_on_properties`. Each fires the moment its
//!   inputs (the slice of super-types, properties, stereotypes, …)
//!   are built, before the parent `Element` value is constructed.
//!
//! * **Cross-chunk** ([`validate`], called from
//!   [`crate::pipeline::finalize_model`]): repo-boundary visibility
//!   and the `<<access.private/protected>>` use-site walker. The
//!   only validators whose inputs *genuinely* span multiple chunks.
//!
//! `.purem` chunks are trusted at merge time — the schema_hash in
//! the purem header (`crate::purem::header`) is the coarse
//! compatibility gate. Semantic drift inside an already-shipped
//! slice is the intentional trade-off (the "Java .class at runtime"
//! analogy).
//!
//! # Checks Performed
//!
//! | Check | Seam | Error Kind |
//! |-------|------|------------|
//! | Generic class ref supplies its required type arguments | resolver | `InvalidAnnotation` |
//! | Stereotype / tag target is a Profile element | resolver | `InvalidAnnotation` |
//! | Stereotype / tag name exists in the referenced Profile | resolver | `InvalidAnnotation` |
//! | Association must have exactly 2 properties | per-element | `InvalidAssociation` |
//! | Association properties must reference a Class | per-element | `InvalidAssociation` |
//! | Supertype must be a Class (not Enum, Function, etc.) | per-element | `InvalidSuperType` |
//! | No self-inheritance | per-element | `InvalidSuperType` |
//! | No duplicate property names within a class | per-element | `DuplicateProperty` |
//! | At most one `<<access.X>>` stereotype per element | per-element | `MultipleAccessLevels` |
//! | Access stereotypes only on classes/functions | per-element | `AccessLevelNotAllowed` |
//! | `<<access.private/protected>>` respected across packages | cross-chunk | `NotAccessible` |
//! | Cross-repo refs respect declared dependencies | cross-chunk | `NotVisible` |
//! | Element's package matches its repo's allowed pattern | cross-chunk | `PackageNotInRepoPattern` |

use std::collections::{HashMap, HashSet};

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::access::{self, AccessLevel, render_package_fqn, render_target_descriptor};
use crate::annotations::StereotypeRef;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::{ElementId, PackageId};
use crate::model::{Element, PureModel};
use crate::nodes::association::Association;
use crate::nodes::class::{Class, Constraint, Property, QualifiedProperty};
use crate::nodes::function::Function;
use crate::types::{ExprKind, FunctionCallData, TypeExpr, ValueSpec};

/// Cross-chunk validation: visibility + access-level use-site walk.
///
/// Called from [`crate::pipeline::finalize_model`] once on the merged
/// model. Per-element checks fire eagerly inside
/// [`crate::pipeline::hydrate_element_signature`] via the piecewise
/// helpers below — this entry point keeps only validators whose
/// inputs span multiple chunks.
#[tracing::instrument(level = "info", name = "validate", skip_all)]
pub(crate) fn validate(model: &PureModel) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    // Repo-boundary visibility — cross-repo refs are inherently a
    // multi-chunk concern; only resolvable on the merged model.
    errors.extend(validate_repo_visibility(model));

    // Repo `pattern` membership — has to be cross-chunk because DSL
    // extensions allocate `Element::DSLInstance` rows directly through
    // their `define()` paths, bypassing
    // `pipeline::hydrate_element_signature`. A per-element call there
    // would silently miss every Mapping / Database / Diagram declaration.
    errors.extend(validate_repo_pattern_membership(model));

    // Access-level (`<<access.private/protected>>`) use-site walk:
    // walks every `ElementId` reference inside each non-bootstrap
    // element and checks the *target's* access level. Targets
    // routinely live in earlier chunks, so this stays cross-chunk.
    errors.extend(validate_access_levels(model));

    errors
}

// ---------------------------------------------------------------------------
// Association Validation
// ---------------------------------------------------------------------------

/// Validates an association's hydrated property list. Inlined into
/// [`crate::pipeline::hydrate_element_signature`] right after the
/// association's `properties` are lowered.
///
/// - Must have exactly 2 properties
/// - Each property must reference a Class
#[allow(clippy::collapsible_if)]
pub(crate) fn validate_association(
    model: &PureModel,
    assoc_name: &SmolStr,
    properties: &[Property],
    source_info: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    let prop_count = properties.len();
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

    // Each property must reference a Class. Sound at hydration time:
    // the kind discriminant (`Element::Class(_)` vs others) is set
    // at `create_shell` (Pass 1), so this check works regardless of
    // topological hydration order.
    for prop in properties {
        if let TypeExpr::Named {
            element: target_id, ..
        } = &prop.type_expr
        {
            if let Some(target_element) = model.try_get_element(*target_id) {
                if !matches!(target_element, Element::Class(_)) {
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
// Super-type Validation
// ---------------------------------------------------------------------------

/// Validates class super-types. Inlined into
/// [`crate::pipeline::hydrate_element_signature`] right after the
/// class's `super_types` are resolved.
///
/// - Must reference a Class (not Enum, Function, etc.)
/// - Must not be self-referential
#[allow(clippy::collapsible_if)]
pub(crate) fn validate_super_types(
    model: &PureModel,
    class_id: ElementId,
    class_name: &SmolStr,
    super_types: &[TypeExpr],
    source_info: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    for super_type in super_types {
        if let TypeExpr::Named {
            element: super_id, ..
        } = super_type
        {
            // Self-inheritance check
            if *super_id == class_id {
                errors.push(CompilationError {
                    message: format!("Class '{class_name}' cannot extend itself"),
                    source_info: source_info.clone(),
                    kind: CompilationErrorKind::InvalidSuperType {
                        class_name: class_name.clone(),
                        super_name: class_name.clone(),
                    },
                });
                continue;
            }

            // Kind check: super must be a Class. Sound at hydration
            // time because the kind discriminant is set at
            // `create_shell` (Pass 1), so even a not-yet-fully-
            // hydrated supertype reports its variant correctly.
            if let Some(super_element) = model.try_get_element(*super_id) {
                if !matches!(super_element, Element::Class(_)) {
                    let super_name = model.element_name(*super_id).clone();
                    errors.push(CompilationError {
                        message: format!(
                            "Class '{class_name}' cannot extend '{super_name}': \
                             only Classes can be extended"
                        ),
                        source_info: source_info.clone(),
                        kind: CompilationErrorKind::InvalidSuperType {
                            class_name: class_name.clone(),
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
// Access-Level Declaration Shape (Step A) — piecewise
// ---------------------------------------------------------------------------

/// Step A — multiple `<<access.X>>` stereotypes on a single element.
/// Inlined into [`crate::pipeline::hydrate_element_signature`] right
/// after the element's `stereotypes` are resolved. No-op when the
/// access profile isn't registered (unit-test fixtures without
/// bootstrap).
pub(crate) fn validate_no_multiple_access_levels(
    model: &PureModel,
    id: ElementId,
    stereotypes: &[StereotypeRef],
    source_info: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    let Some(access_profile) = access::access_profile_id(model) else {
        return;
    };
    let count = stereotypes
        .iter()
        .filter(|s| s.profile == access_profile)
        .count();
    if count > 1 {
        let descriptor = render_target_descriptor(model, id);
        errors.push(CompilationError {
            message: format!("{descriptor} has multiple access level stereotypes"),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::MultipleAccessLevels {
                element_fqn: descriptor,
            },
        });
    }
}

/// Step A — `<<access.X>>` on a class/association property or
/// qualified property is rejected (only classes and functions may
/// carry access levels). Inlined into
/// [`crate::pipeline::hydrate_element_signature`] right after
/// `properties` + `qualified_properties` are lowered.
pub(crate) fn validate_no_access_on_properties(
    model: &PureModel,
    owner_id: ElementId,
    properties: &[Property],
    qualified_properties: &[QualifiedProperty],
    errors: &mut Vec<CompilationError>,
) {
    let Some(access_profile) = access::access_profile_id(model) else {
        return;
    };
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

// ---------------------------------------------------------------------------
// Duplicate Property Validation
// ---------------------------------------------------------------------------

/// Checks that no two properties in a class share the same name.
/// Inlined into [`crate::pipeline::hydrate_element_signature`] right
/// after `properties` are lowered.
pub(crate) fn validate_duplicate_properties(
    class_name: &SmolStr,
    properties: &[Property],
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
// Repo Pattern Membership Validation
// ---------------------------------------------------------------------------

/// Walks every chunk's top-level elements and emits
/// [`CompilationErrorKind::PackageNotInRepoPattern`] for any element
/// whose package path falls outside its repo's allowed-package regex.
/// No-op when `model.repo_patterns` is empty, so existing tests that
/// build a model without going through a real loader stay green.
///
/// Cross-chunk because DSL extensions allocate `Element::DSLInstance`
/// rows directly in their `define()` paths, *bypassing*
/// [`crate::pipeline::hydrate_element_signature`]. A per-element call
/// at the M3 hydration seam would silently miss every Mapping /
/// Database / Diagram declaration. Walking chunks here covers every
/// allocation path uniformly — same shape as
/// [`validate_repo_visibility`].
///
/// Java parity: `RepositoryPackageValidator` →
/// `CodeRepository.isPackageAllowed`. Mirror the same error string so
/// platform-test diffs against the Java implementation stay stable:
///
/// > `Package <pkg> is not allowed in <repo>; only packages matching <pattern> are allowed`
///
/// where `<pkg>` is the element's package path (`::`-joined; empty for
/// root), `<repo>` is the use-site repo from
/// [`crate::visibility::source_repo_name`], and `<pattern>` is the
/// descriptor's pattern verbatim (not the `^(?:…)$`-wrapped internal
/// form).
fn validate_repo_pattern_membership(model: &PureModel) -> Vec<CompilationError> {
    use crate::visibility::source_repo_name;

    let mut errors = Vec::new();
    if model.repo_patterns.is_empty() {
        return errors;
    }

    // Skip chunk 0 (bootstrap) — its source has no repo prefix.
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, _element) in chunk.elements.iter() {
            let node = chunk.nodes.get(local_idx);
            let use_site = &node.source_info.source;
            let Some(repo_name) = source_repo_name(use_site) else {
                continue;
            };
            let Some(pat) = model.repo_patterns.get(&repo_name) else {
                continue;
            };

            // The element's package = parent package's FQN, regardless
            // of where the element name resolves. Using
            // `node.parent_package` (set at allocation time) avoids
            // the whole-tree scan in `element_fqn_path`.
            let pkg_segments = crate::purem::fqn_path::package_path(model, node.parent_package);
            let pkg_path: SmolStr = SmolStr::new(
                pkg_segments
                    .iter()
                    .map(SmolStr::as_str)
                    .collect::<Vec<_>>()
                    .join("::"),
            );

            if !pat.compiled.is_match(&pkg_path) {
                errors.push(CompilationError {
                    message: format!(
                        "Package {pkg_path} is not allowed in {repo_name}; \
                         only packages matching {pattern} are allowed",
                        pattern = pat.source,
                    ),
                    source_info: node.source_info.clone(),
                    kind: CompilationErrorKind::PackageNotInRepoPattern {
                        package: pkg_path,
                        repo: repo_name,
                        pattern: pat.source.clone(),
                    },
                });
            }
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Access-Level Validation (private / protected)
// ---------------------------------------------------------------------------

/// Use-site walker (Step B) for `<<access.private/protected>>`.
/// Walks every `ElementId` reference inside each non-bootstrap
/// element and emits `NotAccessible` against the *target's* effective
/// access level. Targets routinely live in earlier chunks, so this
/// stays in the cross-chunk Pass-3 entry. The declaration-shape
/// checks (Step A) are inline in
/// [`crate::pipeline::hydrate_element_signature`] via
/// [`validate_no_multiple_access_levels`] and
/// [`validate_no_access_on_properties`].
///
/// No-op when `meta::pure::profiles::access` isn't registered (so
/// the existing tests that build a model from raw sources without
/// bootstrap stay green).
fn validate_access_levels(model: &PureModel) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    if access::access_profile_id(model).is_none() {
        return errors;
    };

    // Memoize per-element access level — callees are referenced many
    // times across a chunk; recomputing per ref is wasteful.
    let mut level_cache: HashMap<ElementId, AccessLevel> = HashMap::new();

    // Skip chunk 0 (M3 bootstrap is compiler-trusted).
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let node = chunk.nodes.get(local_idx);
            // Step B — usage checks. Use-site package = this
            // top-level element's parent package.
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

    // The "stereotype-profile-as-package falls back without panic"
    // and "missing type arguments on generic class" cases are now
    // covered by the resolver folds (they fire at resolve time, not
    // in `validate(model)`). End-to-end coverage lives in
    // `tests/stereotype_resolution_tests.rs`.
}
