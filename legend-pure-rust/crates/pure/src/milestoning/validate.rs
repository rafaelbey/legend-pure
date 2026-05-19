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

//! Declare-side validators for milestoned classes (Phase A).
//!
//! Java parity: `MilestoningClassValidator`. The four validators fire
//! hydration-inline (callers in `pipeline.rs::hydrate_element_signature`)
//! against the already-resolved stereotype slice + property slice. Each
//! pushes onto the shared `Vec<CompilationError>` accumulator.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::annotations::StereotypeRef;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::milestoning::{
    MilestoningStereotype, direct_temporal_stereotypes, reserved_property_names_for,
};
use crate::nodes::class::Property;

/// **A4.1** — A class cannot carry more than one of
/// `{businesstemporal, processingtemporal, bitemporal}`. Java parity:
/// `MilestoningClassValidator.validateOnlyOneTemporalStereotype`.
pub fn validate_at_most_one_temporal_stereotype(
    stereotypes: &[StereotypeRef],
    temporal_profile: ElementId,
    class_name: &SmolStr,
    class_si: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    let mut seen: Vec<MilestoningStereotype> = Vec::new();
    let mut conflict_span: Option<SourceInfo> = None;
    for s in stereotypes {
        if s.profile != temporal_profile {
            continue;
        }
        if let Some(kind) = MilestoningStereotype::from_value_name(&s.value) {
            if seen.contains(&kind) {
                continue;
            }
            if !seen.is_empty() && conflict_span.is_none() {
                conflict_span = s.source_info.clone().or_else(|| Some(class_si.clone()));
            }
            seen.push(kind);
        }
    }
    if seen.len() >= 2 {
        let span = conflict_span.unwrap_or_else(|| class_si.clone());
        let names: Vec<&'static str> = seen.iter().map(|k| k.value_name()).collect();
        errors.push(CompilationError {
            message: format!(
                "Class '{class_name}' carries more than one temporal stereotype: {}. \
                 A class may only carry one of <<temporal.businesstemporal>>, \
                 <<temporal.processingtemporal>>, <<temporal.bitemporal>>.",
                names.join(", ")
            ),
            source_info: span,
            kind: CompilationErrorKind::MilestoningStereotypeConflict {
                class_name: class_name.clone(),
                stereotypes: names.iter().map(|n| SmolStr::new(*n)).collect(),
            },
        });
    }
}

/// **A4.2** — Reserved property names. On a milestoned class, user code
/// cannot declare a property whose name collides with any of the names
/// milestoning will synthesize (`businessDate`, `processingDate`,
/// `milestoning`, `from`, `thru`, `in`, `out` — exact set depends on the
/// stereotype). Java parity:
/// `MilestoningClassValidator.validatePropertyNamesAgainstMilestoning`.
pub fn validate_reserved_property_names(
    properties: &[Property],
    stereotypes: &[StereotypeRef],
    temporal_profile: ElementId,
    class_name: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let temporal = direct_temporal_stereotypes(stereotypes, temporal_profile);
    let Some(&kind) = temporal.first() else {
        return;
    };
    let reserved = reserved_property_names_for(kind);
    for prop in properties {
        if reserved.iter().any(|r| *r == prop.name.as_str()) {
            errors.push(CompilationError {
                message: format!(
                    "Property '{prop}' on class '{class_name}' collides with a name reserved by \
                     milestoning. Stereotype <<temporal.{stereo}>> reserves: {names}.",
                    prop = prop.name,
                    class_name = class_name,
                    stereo = kind.value_name(),
                    names = reserved.join(", "),
                ),
                source_info: prop.source_info.clone(),
                kind: CompilationErrorKind::MilestoningReservedPropertyName {
                    class_name: class_name.clone(),
                    property_name: prop.name.clone(),
                    stereotype: SmolStr::new(kind.value_name()),
                },
            });
        }
    }
}

/// **A4.3** — Hierarchy consistency. If a class is milestoned, every
/// ancestor that's also milestoned must carry the same stereotype kind. A
/// `businesstemporal` class extending a `processingtemporal` parent is
/// rejected. Java parity:
/// `MilestoningClassValidator.validateAllSubtypesInATemporalHierarchyHaveTheSameTemporalStereotype`.
///
/// Note: the validator only checks **immediate** supertypes; the cross-chunk
/// validator pass already walks the full hierarchy for visibility, but
/// hydration-inline this check fires per-element with the per-class
/// supertype slice available. A grandparent inconsistency surfaces via the
/// intermediate's own check.
pub fn validate_temporal_hierarchy_consistency(
    own_stereotypes: &[StereotypeRef],
    super_temporal_stereotypes: &[(SmolStr, Option<MilestoningStereotype>)],
    temporal_profile: ElementId,
    class_name: &SmolStr,
    class_si: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    let own = direct_temporal_stereotypes(own_stereotypes, temporal_profile);
    let Some(&own_kind) = own.first() else {
        return;
    };
    for (super_name, super_kind_opt) in super_temporal_stereotypes {
        if let Some(super_kind) = super_kind_opt
            && *super_kind != own_kind
        {
            errors.push(CompilationError {
                message: format!(
                    "Class '{class_name}' carries <<temporal.{own_st}>> but extends '{super_name}' \
                     which carries <<temporal.{super_st}>>. Temporal stereotypes must match \
                     across an inheritance chain.",
                    own_st = own_kind.value_name(),
                    super_st = super_kind.value_name(),
                ),
                source_info: class_si.clone(),
                kind: CompilationErrorKind::MilestoningHierarchyMismatch {
                    class_name: class_name.clone(),
                    own_stereotype: SmolStr::new(own_kind.value_name()),
                    super_class_name: super_name.clone(),
                    super_stereotype: SmolStr::new(super_kind.value_name()),
                },
            });
        }
    }
}

/// **A4.4** — Edge-point name collision. Fires from inside the synthesis
/// pass when a candidate `pAllVersions` / `pAllVersionsInRange` name
/// matches a user-declared property already on the class. Java doesn't
/// guard this explicitly because Java's setProperty replaces, which would
/// silently shadow; we surface it as a hard error so the user knows their
/// declared property is masked.
pub fn report_edge_point_collision(
    class_name: &SmolStr,
    user_property: &Property,
    synthesized_name: &str,
    errors: &mut Vec<CompilationError>,
) {
    errors.push(CompilationError {
        message: format!(
            "Property '{user}' on class '{class_name}' collides with the synthesized milestoning \
             edge-point name '{synth}'. Rename the user property or remove the milestoned \
             reference that triggered synthesis.",
            user = user_property.name,
            class_name = class_name,
            synth = synthesized_name,
        ),
        source_info: user_property.source_info.clone(),
        kind: CompilationErrorKind::MilestoningEdgePointCollision {
            class_name: class_name.clone(),
            user_property_name: user_property.name.clone(),
            synthesized_name: SmolStr::new(synthesized_name),
        },
    });
}
