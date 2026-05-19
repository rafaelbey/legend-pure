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

//! Pass 2a' — Milestoning Synthesis.
//!
//! Runs between Pass 2a (signatures) and Pass 2b (function bodies) in
//! [`crate::pipeline`]. After Pass 2a every property's `type_expr` is
//! resolved to a class `ElementId`, so this phase can:
//!
//! 1. Detect classes carrying a temporal stereotype and synthesize their
//!    date properties + `milestoning` slot.
//! 2. For each property whose target class is milestoned, synthesize an
//!    edge-point property + qualified-property *signatures* and move the
//!    original into [`Class::original_milestoned_properties`].
//!
//! Synthesized property types and parameter types are constructed
//! pre-resolved (no Pass 2a roundtrip), and synthesized qualified-property
//! bodies are intentionally empty in Phase A — populating them requires
//! the runtime-side `getAll(...)` natives and the date-context propagation
//! pass, both deferred to Phase B.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;
use std::sync::Arc;

use crate::annotations::StereotypeRef;
use crate::bootstrap::DATE_ID;
use crate::error::CompilationError;
use crate::ids::ElementId;
use crate::milestoning::{
    GENERATED_MILESTONING_DATE_STEREOTYPE, GENERATED_MILESTONING_STEREOTYPE,
    MILESTONING_PROFILE_FQN, MILESTONING_SLOT_NAME, MilestoningStereotype,
    direct_temporal_stereotypes, edge_point_property_name, inherited_temporal_stereotype,
    range_property_name, resolve_temporal_profile,
};
use crate::model::{Element, PureModel};
use crate::nodes::association::Association;
use crate::nodes::class::{Class, Property, QualifiedProperty};
use crate::types::{Multiplicity, Parameter, TypeExpr};

/// Run the milestoning synthesis pass over every chunk in the model.
///
/// Idempotent: a class that already has its `milestoning` slot populated
/// (e.g. a re-run after [`PureModel::rebuild_derived_indexes`]) is skipped.
/// Short-circuits when the `temporal` profile isn't loaded (bootstrap-only
/// models without the platform).
pub fn synthesize(model: &mut PureModel, errors: &mut Vec<CompilationError>) {
    let Some(temporal_profile) = resolve_temporal_profile(model) else {
        return;
    };
    let milestoning_profile = model.resolve_fqn_str(MILESTONING_PROFILE_FQN);
    let carrier_ids = CarrierIds::resolve(model);

    // Phase 1: class-level date property synthesis.
    //
    // Iterate over (chunk_id, local_idx) snapshots so we can borrow
    // `model` mutably inside the loop body without holding an iterator
    // across the mutation. We need read access to `model` for FQN
    // lookups and write access to mutate `Element::Class.properties`.
    let class_targets: Vec<(u16, u32)> = collect_class_targets(model);
    for (chunk_id, local_idx) in class_targets {
        synthesize_class_dates(
            model,
            chunk_id,
            local_idx,
            temporal_profile,
            milestoning_profile,
            &carrier_ids,
            errors,
        );
    }

    // Phase 2: property-rewrite synthesis (Class).
    let class_targets: Vec<(u16, u32)> = collect_class_targets(model);
    for (chunk_id, local_idx) in class_targets {
        synthesize_class_property_rewrites(
            model,
            chunk_id,
            local_idx,
            temporal_profile,
            milestoning_profile,
            errors,
        );
    }

    // Phase 3: property-rewrite synthesis (Association).
    let assoc_targets: Vec<(u16, u32)> = collect_association_targets(model);
    for (chunk_id, local_idx) in assoc_targets {
        synthesize_association_property_rewrites(
            model,
            chunk_id,
            local_idx,
            temporal_profile,
            milestoning_profile,
            errors,
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers — target collection
// ---------------------------------------------------------------------------

fn collect_class_targets(model: &PureModel) -> Vec<(u16, u32)> {
    let mut out: Vec<(u16, u32)> = Vec::new();
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            if let Element::Class(_) = element {
                out.push((chunk.chunk_id, local_idx));
            }
        }
    }
    out
}

fn collect_association_targets(model: &PureModel) -> Vec<(u16, u32)> {
    let mut out: Vec<(u16, u32)> = Vec::new();
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            if let Element::Association(_) = element {
                out.push((chunk.chunk_id, local_idx));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// CarrierIds — resolved `*Milestoning` carrier classes
// ---------------------------------------------------------------------------

/// Resolved `ElementId`s for the three `*Milestoning` carrier classes
/// declared in `platform/pure/grammar/milestoning.pure`. Each is `Some`
/// when the platform is loaded.
#[derive(Clone, Copy)]
struct CarrierIds {
    business_date: Option<ElementId>,
    processing_date: Option<ElementId>,
    bitemporal: Option<ElementId>,
}

impl CarrierIds {
    fn resolve(model: &PureModel) -> Self {
        Self {
            business_date: model.resolve_fqn_str(super::BUSINESS_DATE_MILESTONING_FQN),
            processing_date: model.resolve_fqn_str(super::PROCESSING_DATE_MILESTONING_FQN),
            bitemporal: model.resolve_fqn_str(super::BITEMPORAL_MILESTONING_FQN),
        }
    }

    fn for_stereotype(self, s: MilestoningStereotype) -> Option<ElementId> {
        match s {
            MilestoningStereotype::BusinessTemporal => self.business_date,
            MilestoningStereotype::ProcessingTemporal => self.processing_date,
            MilestoningStereotype::Bitemporal => self.bitemporal,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 1 — class-level date property synthesis
// ---------------------------------------------------------------------------

fn synthesize_class_dates(
    model: &mut PureModel,
    chunk_id: u16,
    local_idx: u32,
    temporal_profile: ElementId,
    milestoning_profile: Option<ElementId>,
    carrier_ids: &CarrierIds,
    errors: &mut Vec<CompilationError>,
) {
    let class_si = model.chunks[chunk_id as usize]
        .nodes
        .get(local_idx)
        .source_info
        .clone();

    let element = model.chunks[chunk_id as usize].elements.get_mut(local_idx);
    let Element::Class(class) = element else {
        return;
    };

    let stereotypes = class.stereotypes.clone();
    let temporal = direct_temporal_stereotypes(&stereotypes, temporal_profile);
    if temporal.is_empty() {
        return;
    }
    // A4.1 — at-most-one temporal stereotype validator runs at hydration time
    // (see `crate::milestoning::validate::validate_at_most_one_temporal_stereotype`).
    // Here we tolerantly synthesize against the *first* stereotype to keep
    // the model self-consistent even when the validator already flagged it.
    let stereotype = temporal[0];

    // Skip if already synthesized (idempotent re-run).
    if class
        .properties
        .iter()
        .any(|p| p.name == MILESTONING_SLOT_NAME)
    {
        return;
    }

    let generated_date_stereotype =
        date_stereotype_ref(milestoning_profile, class_si.clone(), errors);

    for date_name in stereotype.temporal_date_property_names() {
        class.properties.push(synth_date_property(
            date_name,
            class_si.clone(),
            generated_date_stereotype.clone(),
        ));
    }

    if let Some(carrier) = carrier_ids.for_stereotype(stereotype) {
        class.properties.push(synth_milestoning_slot_property(
            carrier,
            class_si.clone(),
            generated_date_stereotype.clone(),
        ));
    }
    // If the carrier class isn't resolvable (platform isn't loaded), skip
    // the slot property. The two date properties still synthesize so
    // dispatch sees the right shape for date-property access.
}

fn synth_date_property(
    name: &str,
    source_info: SourceInfo,
    generated_date_stereotype: Option<StereotypeRef>,
) -> Property {
    Property {
        name: SmolStr::new(name),
        source_info: source_info.clone(),
        type_expr: TypeExpr::Named {
            element: DATE_ID,
            source_info: None,
            type_arguments: vec![],
            multiplicity_arguments: vec![],
            value_arguments: vec![],
        },
        multiplicity: Multiplicity::PureOne,
        aggregation: None,
        default_value: None,
        stereotypes: generated_date_stereotype.into_iter().collect(),
        tagged_values: vec![],
    }
}

fn synth_milestoning_slot_property(
    carrier: ElementId,
    source_info: SourceInfo,
    generated_date_stereotype: Option<StereotypeRef>,
) -> Property {
    Property {
        name: SmolStr::new(MILESTONING_SLOT_NAME),
        source_info,
        type_expr: TypeExpr::Named {
            element: carrier,
            source_info: None,
            type_arguments: vec![],
            multiplicity_arguments: vec![],
            value_arguments: vec![],
        },
        multiplicity: Multiplicity::ZeroOrOne,
        aggregation: None,
        default_value: None,
        stereotypes: generated_date_stereotype.into_iter().collect(),
        tagged_values: vec![],
    }
}

fn date_stereotype_ref(
    milestoning_profile: Option<ElementId>,
    source_info: SourceInfo,
    errors: &mut Vec<CompilationError>,
) -> Option<StereotypeRef> {
    let _ = (source_info, errors);
    milestoning_profile.map(|profile| StereotypeRef {
        profile,
        value: SmolStr::new(GENERATED_MILESTONING_DATE_STEREOTYPE),
        source_info: None,
    })
}

fn generated_stereotype_ref(milestoning_profile: Option<ElementId>) -> Option<StereotypeRef> {
    milestoning_profile.map(|profile| StereotypeRef {
        profile,
        value: SmolStr::new(GENERATED_MILESTONING_STEREOTYPE),
        source_info: None,
    })
}

// ---------------------------------------------------------------------------
// Phase 2 — property-rewrite synthesis (Class)
// ---------------------------------------------------------------------------

fn synthesize_class_property_rewrites(
    model: &mut PureModel,
    chunk_id: u16,
    local_idx: u32,
    temporal_profile: ElementId,
    milestoning_profile: Option<ElementId>,
    errors: &mut Vec<CompilationError>,
) {
    let owner_id = ElementId::InstanceId {
        chunk_id,
        local_idx,
    };
    // Snapshot what we need from `model` while holding only an immutable
    // borrow: the class's own temporal stereotype (drives whether the
    // no-arg `p()` variant gets generated) and the property slice (to
    // build the rewrite plans). We can't keep an `&mut Class` alive
    // across the per-target stereotype lookups because those re-read
    // other classes from `model`.
    let Some(Element::Class(class_ref)) = model.try_get_element(owner_id) else {
        return;
    };
    let owner_stereotypes = class_ref.stereotypes.clone();
    let properties_snapshot: Vec<Property> = class_ref.properties.clone();
    let owner_temporal = direct_temporal_stereotypes(&owner_stereotypes, temporal_profile);
    let owner_name = model.element_name(owner_id).clone();

    let synth_plans = build_property_rewrite_plans(
        &properties_snapshot,
        owner_temporal.first().copied(),
        |class_id| inherited_temporal_stereotype(model, class_id, temporal_profile),
    );

    if synth_plans.is_empty() {
        return;
    }

    // Now safe to grab the mutable borrow — the plans carry all info we
    // need and don't reference `model`.
    let Element::Class(class) = model.chunks[chunk_id as usize].elements.get_mut(local_idx) else {
        return;
    };
    apply_property_rewrites(class, &owner_name, synth_plans, milestoning_profile, errors);
}

// ---------------------------------------------------------------------------
// Phase 3 — property-rewrite synthesis (Association)
// ---------------------------------------------------------------------------

fn synthesize_association_property_rewrites(
    model: &mut PureModel,
    chunk_id: u16,
    local_idx: u32,
    temporal_profile: ElementId,
    milestoning_profile: Option<ElementId>,
    errors: &mut Vec<CompilationError>,
) {
    let assoc_id = ElementId::InstanceId {
        chunk_id,
        local_idx,
    };
    let Some(Element::Association(a_ref)) = model.try_get_element(assoc_id) else {
        return;
    };
    let properties_snapshot: Vec<Property> = a_ref.properties.clone();
    let owner_name = model.element_name(assoc_id).clone();

    let synth_plans = build_property_rewrite_plans(&properties_snapshot, None, |class_id| {
        inherited_temporal_stereotype(model, class_id, temporal_profile)
    });

    if synth_plans.is_empty() {
        return;
    }

    let Element::Association(assoc) = model.chunks[chunk_id as usize].elements.get_mut(local_idx)
    else {
        return;
    };
    apply_property_rewrites_assoc(assoc, &owner_name, synth_plans, milestoning_profile, errors);
}

// ---------------------------------------------------------------------------
// Property-rewrite plan construction (shared between Class + Association)
// ---------------------------------------------------------------------------

struct PropertyRewritePlan {
    /// Index into the owner's `properties` vec of the original property
    /// to move. `None` for the no-target / no-rewrite case (filtered out
    /// before `apply_*` runs).
    original_index: usize,
    /// The target class's temporal stereotype.
    target_stereotype: MilestoningStereotype,
    /// Whether the no-arg `p()` variant should be synthesized. Java
    /// parity (`MilestoningStereotypeEnum.getSingleDateQualifiedPropertyCodeBlocks`):
    /// fires when the owner is also milestoned with the same stereotype
    /// kind, OR when the owner is bitemporal.
    emit_no_arg_variant: bool,
}

fn build_property_rewrite_plans<F>(
    properties: &[Property],
    owner_stereotype: Option<MilestoningStereotype>,
    mut target_temporal_of: F,
) -> Vec<PropertyRewritePlan>
where
    F: FnMut(ElementId) -> Option<MilestoningStereotype>,
{
    let mut plans = Vec::new();
    for (i, prop) in properties.iter().enumerate() {
        if let TypeExpr::Named { element, .. } = &prop.type_expr
            && let Some(target_st) = target_temporal_of(*element)
        {
            let emit_no_arg_variant = match (owner_stereotype, target_st) {
                (Some(owner), target) if owner == target => true,
                (Some(MilestoningStereotype::Bitemporal), _) => true,
                _ => false,
            };
            plans.push(PropertyRewritePlan {
                original_index: i,
                target_stereotype: target_st,
                emit_no_arg_variant,
            });
        }
    }
    plans
}

fn apply_property_rewrites(
    class: &mut Class,
    owner_name: &SmolStr,
    plans: Vec<PropertyRewritePlan>,
    milestoning_profile: Option<ElementId>,
    errors: &mut Vec<CompilationError>,
) {
    if plans.is_empty() {
        return;
    }
    let mut indices_to_move: Vec<usize> = plans.iter().map(|p| p.original_index).collect();
    indices_to_move.sort_unstable_by(|a, b| b.cmp(a));

    let generated_stereotype = generated_stereotype_ref(milestoning_profile);

    // Build replacement properties + qualified properties in source order
    // (matching the plan order) so the synthesized surface is stable.
    let mut to_append_props: Vec<Property> = Vec::new();
    let mut to_append_qps: Vec<QualifiedProperty> = Vec::new();
    // Survivor names — user properties + QPs that will remain on the
    // owner after the originals being rewritten are moved aside. These
    // are what synthesized edge-point / range names must not collide with.
    let moved_indices: std::collections::HashSet<usize> = indices_to_move.iter().copied().collect();
    let survivor_property_names: Vec<SmolStr> = class
        .properties
        .iter()
        .enumerate()
        .filter(|(i, _)| !moved_indices.contains(i))
        .map(|(_, p)| p.name.clone())
        .collect();
    let survivor_qp_names: Vec<SmolStr> = class
        .qualified_properties
        .iter()
        .map(|q| q.name.clone())
        .collect();

    for plan in &plans {
        let original = class.properties[plan.original_index].clone();
        synthesize_for_property(
            &original,
            plan.target_stereotype,
            plan.emit_no_arg_variant,
            generated_stereotype.as_ref(),
            owner_name,
            &survivor_property_names,
            &survivor_qp_names,
            &mut to_append_props,
            &mut to_append_qps,
            errors,
        );
    }

    // Move originals to `original_milestoned_properties`.
    for idx in indices_to_move {
        let prop = class.properties.remove(idx);
        class.original_milestoned_properties.push(prop);
    }

    // Append synthesized.
    class.properties.extend(to_append_props);
    class.qualified_properties.extend(to_append_qps);
}

fn apply_property_rewrites_assoc(
    assoc: &mut Association,
    owner_name: &SmolStr,
    plans: Vec<PropertyRewritePlan>,
    milestoning_profile: Option<ElementId>,
    errors: &mut Vec<CompilationError>,
) {
    if plans.is_empty() {
        return;
    }
    let mut indices_to_move: Vec<usize> = plans.iter().map(|p| p.original_index).collect();
    indices_to_move.sort_unstable_by(|a, b| b.cmp(a));

    let generated_stereotype = generated_stereotype_ref(milestoning_profile);

    let mut to_append_props: Vec<Property> = Vec::new();
    let mut to_append_qps: Vec<QualifiedProperty> = Vec::new();
    let moved_indices: std::collections::HashSet<usize> = indices_to_move.iter().copied().collect();
    let survivor_property_names: Vec<SmolStr> = assoc
        .properties
        .iter()
        .enumerate()
        .filter(|(i, _)| !moved_indices.contains(i))
        .map(|(_, p)| p.name.clone())
        .collect();
    let survivor_qp_names: Vec<SmolStr> = assoc
        .qualified_properties
        .iter()
        .map(|q| q.name.clone())
        .collect();
    for plan in &plans {
        let original = assoc.properties[plan.original_index].clone();
        synthesize_for_property(
            &original,
            plan.target_stereotype,
            plan.emit_no_arg_variant,
            generated_stereotype.as_ref(),
            owner_name,
            &survivor_property_names,
            &survivor_qp_names,
            &mut to_append_props,
            &mut to_append_qps,
            errors,
        );
    }

    for idx in indices_to_move {
        let prop = assoc.properties.remove(idx);
        assoc.original_milestoned_properties.push(prop);
    }

    assoc.properties.extend(to_append_props);
    assoc.qualified_properties.extend(to_append_qps);
}

// ---------------------------------------------------------------------------
// Per-property synthesis — edge-point + qualified-property signatures
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn synthesize_for_property(
    original: &Property,
    target: MilestoningStereotype,
    emit_no_arg_variant: bool,
    generated_stereotype: Option<&StereotypeRef>,
    owner_name: &SmolStr,
    survivor_property_names: &[SmolStr],
    survivor_qp_names: &[SmolStr],
    to_append_props: &mut Vec<Property>,
    to_append_qps: &mut Vec<QualifiedProperty>,
    errors: &mut Vec<CompilationError>,
) {
    // Edge-point property: `pAllVersions: T[*]` with multiplicity
    // widened (lower bound preserved, upper bound dropped to `*`).
    // A4.4 — collision check: if a user-declared property already owns
    // the candidate edge-point name, skip synthesis and report.
    let edge_name = edge_point_property_name(&original.name);
    if let Some(collide) = survivor_property_names.iter().find(|n| *n == &edge_name) {
        // Reconstruct a Property handle just for the collision report —
        // the survivor's actual source_info would be nicer but we don't
        // carry it through. Use the original property's span as the
        // anchor; the message names both sides.
        let stand_in = Property {
            name: collide.clone(),
            source_info: original.source_info.clone(),
            type_expr: original.type_expr.clone(),
            multiplicity: original.multiplicity.clone(),
            aggregation: None,
            default_value: None,
            stereotypes: vec![],
            tagged_values: vec![],
        };
        crate::milestoning::validate::report_edge_point_collision(
            owner_name,
            &stand_in,
            edge_name.as_str(),
            errors,
        );
        return;
    }
    let edge_mult = widen_to_many(&original.multiplicity);
    let edge_prop = Property {
        name: edge_name,
        source_info: original.source_info.clone(),
        type_expr: original.type_expr.clone(),
        multiplicity: edge_mult,
        aggregation: original.aggregation,
        default_value: None,
        stereotypes: generated_stereotype.into_iter().cloned().collect(),
        tagged_values: vec![],
    };
    to_append_props.push(edge_prop);

    // Same check for the range QP name (`pAllVersionsInRange`). Collision
    // can occur with either a user QP of the same name or — less commonly —
    // a user property. Either side wins as a collision.
    let range_name = range_property_name(&original.name);
    let range_collides =
        survivor_qp_names.contains(&range_name) || survivor_property_names.contains(&range_name);
    let skip_range = if range_collides {
        let stand_in = Property {
            name: range_name.clone(),
            source_info: original.source_info.clone(),
            type_expr: original.type_expr.clone(),
            multiplicity: original.multiplicity.clone(),
            aggregation: None,
            default_value: None,
            stereotypes: vec![],
            tagged_values: vec![],
        };
        crate::milestoning::validate::report_edge_point_collision(
            owner_name,
            &stand_in,
            range_name.as_str(),
            errors,
        );
        true
    } else {
        false
    };

    // Zero-arg variant `p()` — only when owner and target share kind, or
    // owner is bitemporal. The Java template uses `$this.<dateProp>` to
    // bind the date implicitly; the Phase-A signature is enough.
    if emit_no_arg_variant {
        to_append_qps.push(synth_qualified_property_signature(
            original.name.clone(),
            vec![],
            original,
            generated_stereotype,
        ));
    }

    // For bitemporal targets, add the two-parameter `p(pd, bd)` variant
    // (matches Java's `getBiTemporalMilestoningPropertyCodeBlockWithParams`).
    if matches!(target, MilestoningStereotype::Bitemporal) {
        to_append_qps.push(synth_qualified_property_signature(
            original.name.clone(),
            vec![
                synth_date_param("pd", original.source_info.clone()),
                synth_date_param("bd", original.source_info.clone()),
            ],
            original,
            generated_stereotype,
        ));
    }

    // Single-date `p(td: Date[1])` variant — always present.
    to_append_qps.push(synth_qualified_property_signature(
        original.name.clone(),
        vec![synth_date_param("td", original.source_info.clone())],
        original,
        generated_stereotype,
    ));

    // Range variant `pAllVersionsInRange(start, end)` — skipped if A4.4
    // flagged a collision with a user-declared property/QP of the same name.
    if !skip_range {
        to_append_qps.push(synth_qualified_property_signature(
            range_name,
            vec![
                synth_date_param("start", original.source_info.clone()),
                synth_date_param("end", original.source_info.clone()),
            ],
            original,
            generated_stereotype,
        ));
    }
}

fn synth_date_param(name: &str, source_info: SourceInfo) -> Parameter {
    Parameter {
        name: SmolStr::new(name),
        type_expr: TypeExpr::Named {
            element: DATE_ID,
            source_info: None,
            type_arguments: vec![],
            multiplicity_arguments: vec![],
            value_arguments: vec![],
        },
        multiplicity: Multiplicity::PureOne,
        source_info,
    }
}

fn synth_qualified_property_signature(
    name: SmolStr,
    parameters: Vec<Parameter>,
    original: &Property,
    generated_stereotype: Option<&StereotypeRef>,
) -> QualifiedProperty {
    QualifiedProperty {
        name,
        source_info: original.source_info.clone(),
        parameters: Arc::from(parameters),
        return_type: original.type_expr.clone(),
        return_multiplicity: original.multiplicity.clone(),
        // Phase A: empty body. Phase B will populate this with the
        // `filter` lambda + date-comparison expression once the
        // `getAll(...)` natives and date propagation pass land.
        body: Arc::from(Vec::new()),
        stereotypes: generated_stereotype.into_iter().cloned().collect(),
        tagged_values: vec![],
    }
}

fn widen_to_many(mult: &Multiplicity) -> Multiplicity {
    // Matches Java `MilestoningPropertyProcessor.getEdgePointMultiplicityWithManyUpperBound`:
    // lower bound 0 ⇒ `*`, otherwise `lower..*` (rendered as OneOrMany when
    // `lower=1`).
    match mult {
        Multiplicity::PureOne | Multiplicity::OneOrMany => Multiplicity::OneOrMany,
        Multiplicity::ZeroOrOne | Multiplicity::ZeroOrMany | Multiplicity::Variable(_) => {
            Multiplicity::ZeroOrMany
        }
        Multiplicity::Range { lower, .. } if *lower == 0 => Multiplicity::ZeroOrMany,
        Multiplicity::Range { lower, .. } if *lower == 1 => Multiplicity::OneOrMany,
        Multiplicity::Range { lower, .. } => Multiplicity::Range {
            lower: *lower,
            upper: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widen_lower_bounds_preserved() {
        assert_eq!(
            widen_to_many(&Multiplicity::PureOne),
            Multiplicity::OneOrMany
        );
        assert_eq!(
            widen_to_many(&Multiplicity::ZeroOrOne),
            Multiplicity::ZeroOrMany
        );
        assert_eq!(
            widen_to_many(&Multiplicity::ZeroOrMany),
            Multiplicity::ZeroOrMany
        );
        assert_eq!(
            widen_to_many(&Multiplicity::OneOrMany),
            Multiplicity::OneOrMany
        );
        assert_eq!(
            widen_to_many(&Multiplicity::Range {
                lower: 2,
                upper: Some(5)
            }),
            Multiplicity::Range {
                lower: 2,
                upper: None
            }
        );
    }
}
