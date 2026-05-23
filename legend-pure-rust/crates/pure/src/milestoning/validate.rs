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

/// **B-4.1 / B-4.3** — `%latest` usage validator. Walks every expression
/// body in the model after [`crate::milestoning::propagation::propagate_dates`]
/// has finished and emits diagnostics for two conditions:
///
/// - **B-4.1** — `%latest` appears outside a milestoning context. The
///   sentinel is only valid as an argument to `getAll(Class, …)`, to
///   a generated milestoning qualified-property call (`$x.address(%latest)`),
///   or to `getAllVersionsInRange` *as long as B-4.3 doesn't flag it
///   there too*. Anywhere else (e.g. `let d = %latest; …`) is a hard
///   error. Java parity:
///   `MilestoningFunctionExpressionValidator.validateLatestDateUsage`
///   (positive-context check).
///
/// - **B-4.3** — `%latest` supplied to `getAllVersionsInRange` is
///   forbidden because the range query needs concrete bounds.
pub fn validate_latest_usage(model: &crate::model::PureModel, errors: &mut Vec<CompilationError>) {
    use crate::model::Element;

    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            match element {
                Element::Function(func) => {
                    let owner_name = chunk.nodes.get(local_idx).name.clone();
                    for expr in func.body.iter() {
                        walk_for_latest_validation(expr, &owner_name, false, errors);
                    }
                }
                Element::Class(class) => {
                    for qp in &class.qualified_properties {
                        if qp.body.is_empty() {
                            continue;
                        }
                        let ctx_name = qp.name.clone();
                        for expr in qp.body.iter() {
                            walk_for_latest_validation(expr, &ctx_name, false, errors);
                        }
                    }
                    for con in &class.constraints {
                        let ctx_name = con
                            .name
                            .clone()
                            .unwrap_or_else(|| SmolStr::new_static("constraint"));
                        walk_for_latest_validation(&con.function, &ctx_name, false, errors);
                        if let Some(msg) = &con.message {
                            walk_for_latest_validation(msg, &ctx_name, false, errors);
                        }
                    }
                    for prop in &class.properties {
                        if let Some(dv) = &prop.default_value {
                            walk_for_latest_validation(dv, &prop.name, false, errors);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Walk an expression looking for `%latest` literals and emitting a
/// diagnostic for each that lands in a non-milestoning position.
///
/// `in_milestoning_arg_slot` is true when we're inside an argument to a
/// recognised milestoning host — `getAll` / `getAllVersions` /
/// generated milestoning QP — where `%latest` is permitted.
fn walk_for_latest_validation(
    expr: &crate::types::ValueSpec,
    context: &SmolStr,
    in_milestoning_arg_slot: bool,
    errors: &mut Vec<CompilationError>,
) {
    use crate::types::{DateValue, ExprKind};
    match expr.kind.as_ref() {
        ExprKind::DateLiteral(DateValue::Latest) => {
            if !in_milestoning_arg_slot {
                errors.push(CompilationError {
                    message: format!(
                        "`%latest` may only appear as an argument to a milestoning \
                         function (`getAll`, `getAllVersionsInRange`) or a generated \
                         milestoning qualified property; found in '{context}'."
                    ),
                    source_info: expr.source_info.clone(),
                    kind: CompilationErrorKind::MilestoningLatestOutsideMilestoningContext {
                        context: context.clone(),
                    },
                });
            }
        }
        ExprKind::FunctionCall(data) => {
            let is_getall = matches!(data.function_name.as_str(), "getAll" | "getAllVersions");
            let is_range = data.function_name.as_str() == "getAllVersionsInRange";
            for (i, arg) in data.arguments.iter().enumerate() {
                if is_range && i > 0 {
                    // B-4.3 — `%latest` is forbidden in either bound.
                    if matches!(arg.kind.as_ref(), ExprKind::DateLiteral(DateValue::Latest)) {
                        let position = u8::try_from(i).unwrap_or(0);
                        errors.push(CompilationError {
                            message: format!(
                                "`%latest` is not allowed as the {} bound of \
                                 `getAllVersionsInRange`; supply a concrete date.",
                                if i == 1 { "start" } else { "end" }
                            ),
                            source_info: arg.source_info.clone(),
                            kind: CompilationErrorKind::MilestoningLatestNotAllowedInRange {
                                arg_position: position,
                            },
                        });
                    }
                    walk_for_latest_validation(arg, context, false, errors);
                } else if is_getall && i > 0 {
                    walk_for_latest_validation(arg, context, true, errors);
                } else {
                    walk_for_latest_validation(arg, context, false, errors);
                }
            }
        }
        ExprKind::QualifiedPropertyCall(data) => {
            // Generated milestoning QPs accept `%latest` in their date
            // slots (slots 1..). Static detection of "is this QP a
            // milestoning one?" would need a cross-class lookup; the
            // looser rule "every QP non-receiver arg is a milestoning
            // slot" is precise enough — non-milestoning QPs don't
            // typically take `%latest` anyway, and a misuse there
            // surfaces as a Pure-level type error.
            for (i, arg) in data.arguments.iter().enumerate() {
                let milestoning_slot = i > 0;
                walk_for_latest_validation(arg, context, milestoning_slot, errors);
            }
        }
        ExprKind::PropertyCall(data) => {
            for arg in &data.arguments {
                walk_for_latest_validation(arg, context, false, errors);
            }
        }
        ExprKind::Lambda { body, .. } => {
            for e in body {
                walk_for_latest_validation(e, context, in_milestoning_arg_slot, errors);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                walk_for_latest_validation(e, context, false, errors);
            }
        }
        _ => {}
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
