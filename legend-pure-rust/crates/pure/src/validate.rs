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
//!   [`crate::pipeline::finalize_model`]): repo-boundary visibility,
//!   the `<<access.private/protected>>` use-site walker, repo
//!   `pattern` membership, property default-value compat, and
//!   constructor key-binding compat. Validators whose inputs span
//!   multiple chunks OR need inferred `type_info` populated by
//!   Pass 2.5.
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
//! | Property default value type+mult compatible with declared shape | cross-chunk | `PropertyDefaultValueIncompatible` |
//! | Subclass property is a legal override of the inherited one (LSP) | cross-chunk | `PropertyConflict` |
//! | `^Class(prop=val)` value type+mult compatible with property | cross-chunk | `ConstructorPropertyTypeMismatch` |
//! | `^Class(unknownKey=...)` resolves to a real property | cross-chunk | `UnknownProperty` |
//! | `^Class()` supplies every required-no-default property | cross-chunk | `ConstructorMissingRequiredProperty` |

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
///
/// `chunk_set` bounds the cross-chunk walkers to a subset of chunks
/// (LSP-side incremental recompile, T-20260513-01). `None` =
/// whole-model, preserves the pre-incremental behaviour.
#[tracing::instrument(level = "info", name = "validate", skip_all)]
pub(crate) fn validate(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    // Repo-boundary visibility — cross-repo refs are inherently a
    // multi-chunk concern; only resolvable on the merged model.
    errors.extend(validate_repo_visibility(model, chunk_set));

    // Repo `pattern` membership — has to be cross-chunk because DSL
    // extensions allocate `Element::DSLInstance` rows directly through
    // their `define()` paths, bypassing
    // `pipeline::hydrate_element_signature`. A per-element call there
    // would silently miss every Mapping / Database / Diagram declaration.
    errors.extend(validate_repo_pattern_membership(model, chunk_set));

    // Property default-value type+multiplicity compat — cross-chunk
    // because the default-value expression's inferred `type_info` is
    // populated by Pass 2.5 inference, not Pass 2b lowering.
    errors.extend(validate_property_default_values(model, chunk_set));

    // Cross-hierarchy property override compat — cross-chunk because a
    // subclass and its superclass routinely live in different chunks
    // (user code extending a `.purem`-loaded platform class). Java
    // parity: `ClassValidator.validatePropertyOverrides`.
    errors.extend(validate_property_overrides(model, chunk_set));

    // `^Class(prop = val)` value compat — cross-chunk for the same
    // reason. T-04 (unknown key) and T-02 (missing required) fire
    // eagerly at `lower::new_instance::lower_new_instance`; only the
    // value-vs-property compat lives here.
    errors.extend(validate_constructor_bindings(model, chunk_set));

    // Access-level (`<<access.private/protected>>`) use-site walk:
    // walks every `ElementId` reference inside each non-bootstrap
    // element and checks the *target's* access level. Targets
    // routinely live in earlier chunks, so this stays cross-chunk.
    errors.extend(validate_access_levels(model, chunk_set));

    errors
}

/// Returns true when the chunk should be skipped under `chunk_set`
/// (used by the cross-chunk walkers below). A `None` chunk_set means
/// "iterate every chunk" — preserves whole-model behaviour.
#[inline]
fn skip_chunk(chunk_set: Option<&HashSet<u16>>, chunk_id: u16) -> bool {
    match chunk_set {
        None => false,
        Some(set) => !set.contains(&chunk_id),
    }
}

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

/// Walks every class and checks that every property (simple +
/// qualified, plus association-injected) in its inheritance closure
/// is a *legal override* of any like-named property declared higher
/// in that closure.
///
/// Java parity: [`ClassValidator.validatePropertyOverrides`] —
/// the full nested-MRO algorithm, not the simpler own-vs-ancestors
/// shortcut. The outer loop walks every class C in the model; for each
/// C we materialise the MRO closure (C + every strict ancestor in BFS
/// order) and compare *every* pair `(MRO[i], MRO[j])` with `i < j`.
/// This catches three distinct cases under one loop:
///
/// * **Direct override** — `i = 0`, spec = C itself, genl = some
///   ancestor. C's own property must be a legal override of the
///   inherited one.
/// * **Transitive override** — `i > 0`, spec and genl both ancestors of
///   C. An ancestor's redeclaration that conflicts with a further-up
///   ancestor surfaces here (when validating C, or earlier when
///   validating the immediate descendant — dedup picks whichever fires
///   first).
/// * **Diamond** — `i > 0`, spec and genl on parallel branches of C's
///   MRO. Neither is an ancestor of the other, so neither catches the
///   conflict on its own; only at the join class do both become
///   visible. C is the most-derived such join point.
///
/// **Simple properties** are invariant — a redeclaration must carry the
/// **same** type and multiplicity as the inherited declaration. (This
/// is what the M3 platform exercises when `Type.name` shadows
/// `ModelElement.name`: both are `String[1]`, so the override is legal.)
///
/// **Qualified properties** follow Liskov-substitution rules:
///
/// | Position | Direction | Rule |
/// |---|---|---|
/// | return type | covariant | spec return must be the same or a subtype of genl return |
/// | return mult | covariant | spec return multiplicity must be subsumed by genl return mult |
/// | param count | invariant | identical |
/// | param type (per slot) | contravariant | spec param must be the same or a supertype of genl param |
/// | param mult (per slot) | contravariant | spec param multiplicity must subsume genl param mult |
///
/// **Cross-chunk placement.** A subclass and its superclass routinely
/// live in different chunks — user code extending a platform class
/// loaded from `.purem` is the canonical example. Per-element-eager
/// would silently miss those, so this walker runs in the merged-model
/// `validate()` entry point alongside `validate_property_default_values`.
///
/// **Dedup.** Without intervention, a conflict between `A.prop` and
/// `X.prop` (with `A extends X`) reports once when validating A *and*
/// again when validating any descendant of A. The shared
/// `(spec_owner, genl_owner, name)` key collapses those duplicates so
/// each unique conflict is reported once. Iteration order is
/// chunk-then-local-idx, which in practice matches declaration order
/// (parent before child), so attribution lands on the closest class
/// that exposes the conflict — A for direct conflicts, the join class
/// for diamonds.
fn validate_property_overrides(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    let mut errors = Vec::new();
    // (spec_owner_id, genl_owner_id, property_name, kind) — collapses
    // the same conflict reported once per descendant.
    let mut seen: HashSet<ConflictKey> = HashSet::new();

    for chunk in &model.chunks {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            if !matches!(element, Element::Class(_)) {
                continue;
            }
            let class_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let class_name = model.element_name(class_id).clone();

            let mro = collect_mro(class_id, model);
            if mro.len() < 2 {
                // No strict ancestors → nothing to check.
                continue;
            }

            // Materialise visible properties once per MRO entry.
            let simple_by_idx: Vec<Vec<VisibleProperty<'_>>> = mro
                .iter()
                .map(|id| collect_simple_properties(*id, model))
                .collect();
            let qualified_by_idx: Vec<Vec<VisibleQualifiedProperty<'_>>> = mro
                .iter()
                .map(|id| collect_qualified_properties(*id, model))
                .collect();

            for (i, spec_simple) in simple_by_idx.iter().enumerate() {
                for spec in spec_simple {
                    for genl_simple in &simple_by_idx[(i + 1)..] {
                        for genl in genl_simple.iter().filter(|p| p.prop.name == spec.prop.name) {
                            if !is_simple_property_override_valid(spec.prop, genl.prop, model) {
                                let key = ConflictKey::Simple {
                                    spec_owner: spec.owner_id,
                                    genl_owner: genl.owner_id,
                                    name: spec.prop.name.clone(),
                                };
                                if seen.insert(key) {
                                    emit_property_conflict(
                                        &class_name,
                                        &model.element_name(spec.owner_id).clone(),
                                        &model.element_name(genl.owner_id).clone(),
                                        &spec.prop.name,
                                        &spec.prop.source_info,
                                        &mut errors,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            for (i, spec_qual) in qualified_by_idx.iter().enumerate() {
                for spec in spec_qual {
                    let spec_arity = spec.qp.parameters.len();
                    for genl_qual in &qualified_by_idx[(i + 1)..] {
                        for genl in genl_qual.iter().filter(|q| {
                            q.qp.name == spec.qp.name && q.qp.parameters.len() == spec_arity
                        }) {
                            if !is_qualified_property_override_valid(spec.qp, genl.qp, model) {
                                let key = ConflictKey::Qualified {
                                    spec_owner: spec.owner_id,
                                    genl_owner: genl.owner_id,
                                    name: spec.qp.name.clone(),
                                    param_count: spec_arity,
                                };
                                if seen.insert(key) {
                                    emit_property_conflict(
                                        &class_name,
                                        &model.element_name(spec.owner_id).clone(),
                                        &model.element_name(genl.owner_id).clone(),
                                        &spec.qp.name,
                                        &spec.qp.source_info,
                                        &mut errors,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    errors
}

/// Dedup key for `validate_property_overrides`. Simple and qualified
/// conflicts live in disjoint name-spaces — a qualified `name(x)` on
/// A and a simple `name` on B should still both report if they
/// somehow co-occur, so the kind tag is part of the key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConflictKey {
    Simple {
        spec_owner: ElementId,
        genl_owner: ElementId,
        name: SmolStr,
    },
    Qualified {
        spec_owner: ElementId,
        genl_owner: ElementId,
        name: SmolStr,
        param_count: usize,
    },
}

/// A simple property visible at a class, with the element id of
/// whichever class or association actually declares it. Used both for
/// the comparison loop and for the dedup key — assoc-injected ends
/// dedupe under the Association's id, not the class they surface on.
struct VisibleProperty<'m> {
    prop: &'m Property,
    owner_id: ElementId,
}

struct VisibleQualifiedProperty<'m> {
    qp: &'m QualifiedProperty,
    owner_id: ElementId,
}

/// Walk `class_id`'s MRO BFS-style, returning `[class_id, ancestors…]`.
/// The class itself is included so the override pair loop can treat it
/// uniformly. Cycles are guarded by the `visited` set — defence in
/// depth; `validate_super_types` already rejects self-inheritance, but
/// a malformed model shouldn't loop here.
fn collect_mro(class_id: ElementId, model: &PureModel) -> Vec<ElementId> {
    use std::collections::VecDeque;
    let mut out: Vec<ElementId> = Vec::new();
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut queue: VecDeque<ElementId> = VecDeque::new();
    queue.push_back(class_id);
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        out.push(current);
        if let Element::Class(c) = model.get_element(current) {
            for st in &c.super_types {
                if let TypeExpr::Named { element, .. } = st {
                    queue.push_back(*element);
                }
            }
        }
    }
    out
}

/// Collect every simple property visible at `class_id`: declared +
/// association-injected. Each item carries the element id of its true
/// owner — the class for declared, the association for injected.
fn collect_simple_properties<'m>(
    class_id: ElementId,
    model: &'m PureModel,
) -> Vec<VisibleProperty<'m>> {
    let mut out: Vec<VisibleProperty<'m>> = Vec::new();
    if let Element::Class(c) = model.get_element(class_id) {
        for p in &c.properties {
            out.push(VisibleProperty {
                prop: p,
                owner_id: class_id,
            });
        }
    }
    for (assoc_id, self_idx) in model.association_properties(class_id) {
        if let Element::Association(assoc) = model.get_element(*assoc_id) {
            let other_idx = 1 - *self_idx;
            if let Some(p) = assoc.properties.get(other_idx) {
                out.push(VisibleProperty {
                    prop: p,
                    owner_id: *assoc_id,
                });
            }
        }
    }
    out
}

/// Collect every qualified property visible at `class_id`. Java
/// doesn't surface qualified properties through associations, so we
/// only walk the declared list.
fn collect_qualified_properties<'m>(
    class_id: ElementId,
    model: &'m PureModel,
) -> Vec<VisibleQualifiedProperty<'m>> {
    let mut out: Vec<VisibleQualifiedProperty<'m>> = Vec::new();
    if let Element::Class(c) = model.get_element(class_id) {
        for q in &c.qualified_properties {
            out.push(VisibleQualifiedProperty {
                qp: q,
                owner_id: class_id,
            });
        }
    }
    out
}

/// Java parity: `ClassValidator.isPropertyOverrideValid`. Simple
/// properties are invariant — the inherited declaration's type and
/// multiplicity must match exactly. Structural compatibility in *both*
/// directions (`A <: B && B <: A`) substitutes for raw `==` so
/// `TypeExpr` spans don't leak into equality.
fn is_simple_property_override_valid(spec: &Property, genl: &Property, model: &PureModel) -> bool {
    types_structurally_equal(&spec.type_expr, &genl.type_expr, model)
        && spec.multiplicity == genl.multiplicity
}

/// Java parity: `ClassValidator.isQualifiedPropertyOverrideValid`.
/// Covariant return (`spec <: genl`), contravariant params
/// (`genl <: spec`), invariant param count.
fn is_qualified_property_override_valid(
    spec: &QualifiedProperty,
    genl: &QualifiedProperty,
    model: &PureModel,
) -> bool {
    // Return type — covariant (spec must be same or more specific than genl).
    if !crate::resolve::is_type_compatible_structural(&spec.return_type, &genl.return_type, model) {
        return false;
    }
    // Return multiplicity — spec must be subsumed by genl.
    if !crate::resolve::is_multiplicity_compatible(
        Some(&spec.return_multiplicity),
        &genl.return_multiplicity,
    ) {
        return false;
    }
    // Param count is the join key — caller filtered on it, so this is a
    // defensive guard. Keeps `validate_qualified_property_overrides`
    // honest if someone reuses this helper later.
    if spec.parameters.len() != genl.parameters.len() {
        return false;
    }
    // Per-slot params: contravariant (genl <: spec on type, genl subsumed by
    // spec on multiplicity). The Rust port stores QP parameters WITHOUT
    // the implicit `this` slot (added explicitly at inference time in
    // `pipeline.rs`), so iterate `0..n` — *not* `1..n` like Java.
    for (spec_p, genl_p) in spec.parameters.iter().zip(genl.parameters.iter()) {
        if !crate::resolve::is_type_compatible_structural(
            &genl_p.type_expr,
            &spec_p.type_expr,
            model,
        ) {
            return false;
        }
        if !crate::resolve::is_multiplicity_compatible(
            Some(&genl_p.multiplicity),
            &spec_p.multiplicity,
        ) {
            return false;
        }
    }
    true
}

/// Structural identity on `TypeExpr` via two-direction
/// `is_type_compatible_structural`. `TypeExpr::Named` carries spans
/// that diverge for two declarations even when the types are
/// semantically identical, so bare `==` is unsafe.
fn types_structurally_equal(a: &TypeExpr, b: &TypeExpr, model: &PureModel) -> bool {
    crate::resolve::is_type_compatible_structural(a, b, model)
        && crate::resolve::is_type_compatible_structural(b, a, model)
}

/// Java parity: `ClassValidator.throwPropertyConflictException`. The
/// `class_name` is the class being validated (the one whose MRO
/// exposes both halves of the conflict). `spec_class_name` and
/// `genl_class_name` are the actual declaration sites — same names
/// when a class directly redeclares an inherited property, distinct
/// names for transitive or diamond conflicts.
fn emit_property_conflict(
    class_name: &SmolStr,
    spec_class_name: &SmolStr,
    genl_class_name: &SmolStr,
    property_name: &SmolStr,
    source_info: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    errors.push(CompilationError {
        message: format!(
            "Property conflict on class {class_name}: property '{property_name}' \
             defined on {spec_class_name} conflicts with property '{property_name}' \
             defined on {genl_class_name}"
        ),
        source_info: source_info.clone(),
        kind: CompilationErrorKind::PropertyConflict {
            class_name: class_name.clone(),
            super_class_name: genl_class_name.clone(),
            property_name: property_name.clone(),
        },
    });
}

/// Walks every cross-repo `ElementId` reference and emits `NotVisible`
/// errors for each target whose home repo is not in the use-site repo's
/// declared dependencies. No-op when `model.repo_visibility` is empty,
/// so existing tests that build a model without going through a real
/// loader stay green.
fn validate_repo_visibility(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    use crate::purem::walk::walk_element_ids;
    use crate::visibility::{check_element_visible, source_repo_name};

    let mut errors = Vec::new();
    if model.repo_visibility.is_empty() {
        return errors;
    }

    // Skip chunk 0 (bootstrap) — its source has no repo prefix.
    for chunk in model.chunks.iter().skip(1) {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
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
fn validate_repo_pattern_membership(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    use crate::visibility::source_repo_name;

    let mut errors = Vec::new();
    if model.repo_patterns.is_empty() {
        return errors;
    }

    // Skip chunk 0 (bootstrap) — its source has no repo prefix.
    for chunk in model.chunks.iter().skip(1) {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
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

/// Walks every class and association property whose `default_value`
/// is `Some(_)` and checks that the lowered expression's inferred
/// type+multiplicity is compatible with the property's declared
/// `T[m]`. Cross-chunk because the value's `type_info` is populated
/// by Pass 2.5 inference, not by Pass 2b lowering.
///
/// Java parity: `M3PropertyValidator` family. Reuses
/// `resolve::is_type_compatible_structural` +
/// `resolve::is_multiplicity_compatible` — the same helpers that gate
/// qualified-property argument binding.
fn validate_property_default_values(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    for chunk in model.chunks.iter().skip(1) {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            let owner_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            match element {
                Element::Class(c) => {
                    let owner_fqn = SmolStr::new(
                        crate::purem::fqn_path::element_fqn_path(model, owner_id).join("::"),
                    );
                    for prop in &c.properties {
                        check_property_default(&owner_fqn, prop, model, &mut errors);
                    }
                }
                Element::Association(a) => {
                    let owner_fqn = SmolStr::new(
                        crate::purem::fqn_path::element_fqn_path(model, owner_id).join("::"),
                    );
                    for prop in &a.properties {
                        check_property_default(&owner_fqn, prop, model, &mut errors);
                    }
                }
                _ => {}
            }
        }
    }
    errors
}

fn check_property_default(
    owner_fqn: &SmolStr,
    prop: &Property,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    let Some(dv) = &prop.default_value else {
        return;
    };
    // Pass 2.5 should have populated type_info on every reachable
    // ValueSpec. If it's missing here, lowering or inference failed for
    // this expression — a separate diagnostic will have been emitted.
    // Don't compound the error.
    let Some(rt) = dv.type_info.as_deref() else {
        return;
    };
    let type_ok =
        crate::resolve::is_type_compatible_structural(&rt.type_expr, &prop.type_expr, model);
    let mult_ok =
        crate::resolve::is_multiplicity_compatible(Some(&rt.multiplicity), &prop.multiplicity);
    if !type_ok || !mult_ok {
        let expected = crate::infer::render_type(model, &prop.type_expr, &prop.multiplicity);
        let actual = crate::infer::render_type(model, &rt.type_expr, &rt.multiplicity);
        errors.push(CompilationError {
            message: format!(
                "Default value of property '{owner}.{prop}' is '{actual}', incompatible with declared '{expected}'",
                owner = owner_fqn,
                prop = prop.name,
            ),
            source_info: prop.source_info.clone(),
            kind: CompilationErrorKind::PropertyDefaultValueIncompatible {
                class_name: owner_fqn.clone(),
                property_name: prop.name.clone(),
                expected,
                actual,
            },
        });
    }
}

/// Walks every `^Class(...)` constructor expression in the model and
/// fires the constructor-binding validators:
///
/// - T-20260511-04 (unknown key): each supplied key must resolve to a
///   property on the class (walking supertypes + association-injected).
/// - T-20260511-02 (missing required): each property with multiplicity
///   lower bound >= 1 and no declared default value must be supplied.
/// - T-20260511-03 (value compat): each supplied value's inferred
///   type+multiplicity must be compatible with the property's declared
///   shape.
///
/// All three live cross-chunk because they depend on the derived
/// `association_properties` index — `rebuild_derived_indexes()` only
/// runs at the *end* of `compile_repo_slice`, after every per-element
/// lowering has completed, so `lower_new_instance` can't see
/// association-injected ends yet. Plus T-03 needs inferred `type_info`
/// from Pass 2.5.
///
/// `+=` (augmented) bindings are handled with the same compat rule as
/// `=` in v1. Correct for `[*]` / `[1..*]` slots; conservatively
/// rejects `+=` to single-value slots (which is semantically invalid
/// anyway). The looser "element-type compat against the collection
/// slot's element type" rule is a v2 follow-up.
fn validate_constructor_bindings(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    for chunk in model.chunks.iter().skip(1) {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
        for (_local_idx, element) in chunk.elements.iter() {
            visit_value_specs_in_element(element, &mut |vs| {
                check_new_call(vs, model, &mut errors);
            });
        }
    }
    errors
}

fn check_new_call(vs: &ValueSpec, model: &PureModel, errors: &mut Vec<CompilationError>) {
    let ExprKind::FunctionCall(data) = vs.kind.as_ref() else {
        return;
    };
    if data.function_name.as_str() != "new" {
        return;
    }
    let args = &data.arguments;
    // Layout: args[0] class ref, args[1] simple name, args[2] type-args
    // collection, args[3] type-var-values collection, args[4..] = repeating
    // (key: String, val: ValueSpec, augmented: Bool) triples.
    if args.len() < 4 {
        return;
    }
    let ExprKind::PackageableElementRef { element: class_id } = args[0].kind.as_ref() else {
        return;
    };
    let class_id = *class_id;
    let class_fqn: SmolStr =
        SmolStr::new(crate::purem::fqn_path::element_fqn_path(model, class_id).join("::"));

    let mut supplied: std::collections::HashSet<SmolStr> = std::collections::HashSet::new();
    let mut i = 4;
    while i + 2 < args.len() {
        let key_arg = &args[i];
        let val_arg = &args[i + 1];
        // augmented bool at args[i + 2] — not used in v1 compat rule.
        i += 3;

        let ExprKind::StringLiteral(key) = key_arg.kind.as_ref() else {
            continue;
        };
        supplied.insert(key.clone());

        let Some(prop) = crate::resolve::find_property_full_with_inheritance(class_id, key, model)
        else {
            // T-04: unknown key.
            errors.push(CompilationError {
                message: format!("Class '{class_fqn}' has no property '{key}'"),
                source_info: key_arg.source_info.clone(),
                kind: CompilationErrorKind::UnknownProperty {
                    type_name: class_fqn.clone(),
                    property_name: key.clone(),
                },
            });
            continue;
        };
        // T-03: value type+multiplicity compat.
        let Some(rt) = val_arg.type_info.as_deref() else {
            continue;
        };
        let type_ok =
            crate::resolve::is_type_compatible_structural(&rt.type_expr, &prop.type_expr, model);
        let mult_ok =
            crate::resolve::is_multiplicity_compatible(Some(&rt.multiplicity), &prop.multiplicity);
        if !type_ok || !mult_ok {
            let expected = crate::infer::render_type(model, &prop.type_expr, &prop.multiplicity);
            let actual = crate::infer::render_type(model, &rt.type_expr, &rt.multiplicity);
            errors.push(CompilationError {
                message: format!(
                    "Property '{class_fqn}.{prop}' value '{actual}' is incompatible with declared '{expected}'",
                    prop = prop.name,
                ),
                source_info: val_arg.source_info.clone(),
                kind: CompilationErrorKind::ConstructorPropertyTypeMismatch {
                    class_name: class_fqn.clone(),
                    property_name: prop.name.clone(),
                    expected,
                    actual,
                },
            });
        }
    }
    // T-02: every required-no-default *declared* property must be
    // supplied. Association-injected ends are excluded — Java's
    // `NewInstance` validator treats those as bidirectional runtime
    // links populated from the other side, not constructor inputs.
    for prop in crate::resolve::all_declared_properties_with_inheritance(class_id, model) {
        let (lower, _upper) = crate::resolve::mult_bounds(&prop.multiplicity);
        let is_required = lower >= 1 && prop.default_value.is_none();
        if is_required && !supplied.contains(&prop.name) {
            errors.push(CompilationError {
                message: format!(
                    "Missing required property '{prop}' on class '{class_fqn}'",
                    prop = prop.name,
                ),
                source_info: vs.source_info.clone(),
                kind: CompilationErrorKind::ConstructorMissingRequiredProperty {
                    class_name: class_fqn.clone(),
                    property_name: prop.name.clone(),
                },
            });
        }
    }
}

/// Recursively visit every `ValueSpec` inside `element` — used by the
/// constructor-binding validator to find every `^Class(...)`
/// expression regardless of nesting depth.
fn visit_value_specs_in_element<F>(element: &Element, visit: &mut F)
where
    F: FnMut(&ValueSpec),
{
    match element {
        Element::Class(c) => {
            for prop in &c.properties {
                if let Some(dv) = &prop.default_value {
                    visit_value_spec_rec(dv, visit);
                }
            }
            for qp in &c.qualified_properties {
                for e in qp.body.iter() {
                    visit_value_spec_rec(e, visit);
                }
            }
            for con in &c.constraints {
                visit_value_spec_rec(&con.function, visit);
                if let Some(m) = &con.message {
                    visit_value_spec_rec(m, visit);
                }
            }
        }
        Element::Function(f) => {
            for e in f.body.iter() {
                visit_value_spec_rec(e, visit);
            }
        }
        Element::Association(a) => {
            for prop in &a.properties {
                if let Some(dv) = &prop.default_value {
                    visit_value_spec_rec(dv, visit);
                }
            }
            for qp in &a.qualified_properties {
                for e in qp.body.iter() {
                    visit_value_spec_rec(e, visit);
                }
            }
        }
        _ => {}
    }
}

fn visit_value_spec_rec<F>(vs: &ValueSpec, visit: &mut F)
where
    F: FnMut(&ValueSpec),
{
    visit(vs);
    match vs.kind.as_ref() {
        ExprKind::FunctionCall(d)
        | ExprKind::PropertyCall(d)
        | ExprKind::QualifiedPropertyCall(d) => {
            for a in &d.arguments {
                visit_value_spec_rec(a, visit);
            }
        }
        ExprKind::Lambda { body, .. } => {
            for e in body {
                visit_value_spec_rec(e, visit);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                visit_value_spec_rec(e, visit);
            }
        }
        ExprKind::PathLiteral { steps, .. } => {
            for step in steps {
                for p in &step.parameters {
                    visit_value_spec_rec(p, visit);
                }
            }
        }
        _ => {}
    }
}

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
fn validate_access_levels(
    model: &PureModel,
    chunk_set: Option<&HashSet<u16>>,
) -> Vec<CompilationError> {
    let mut errors = Vec::new();

    if access::access_profile_id(model).is_none() {
        return errors;
    }

    // Memoize per-element access level — callees are referenced many
    // times across a chunk; recomputing per ref is wasteful.
    let mut level_cache: HashMap<ElementId, AccessLevel> = HashMap::new();

    // Skip chunk 0 (M3 bootstrap is compiler-trusted).
    for chunk in model.chunks.iter().skip(1) {
        if skip_chunk(chunk_set, chunk.chunk_id) {
            continue;
        }
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
        TypeExpr::GenericTypeOperation {
            left: a, right: b, ..
        } => {
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
        | ExprKind::MultiplicityReference { .. } => {}
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
        // No element references inside a bare multiplicity literal —
        // shares the no-op shape with `Column`.
        ExprKind::PackageableElementRef { element } => {
            emit(*element, si);
        }
        ExprKind::RelationLiteral { columns } | ExprKind::ColSpecArrayLiteral { columns, .. } => {
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
                    element: ElementId::InstanceId {
                        chunk_id: 0,
                        local_idx: 0,
                    },
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
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
                    source_info: None,
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
