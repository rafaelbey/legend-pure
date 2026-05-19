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

//! Runtime dispatch for milestoning-synthesized qualified properties.
//!
//! Phase A's compile-time pass synthesizes the QP *signatures* on
//! milestoned classes (`address(td: Date[1])`, `addressAllVersionsInRange`,
//! and so on) and leaves their bodies empty. Rather than constructing
//! lowered `ValueSpec` filter-by-date trees at compile time — which would
//! require resolving `filter` / `eq` / `toOne` `ElementId`s during
//! synthesis and threading them through the lowered IR — we recognize
//! these QPs at dispatch time by their
//! `<<milestoning.generatedmilestoningproperty>>` stereotype and execute
//! the filter-and-narrow semantics in Rust.
//!
//! Two entry points:
//!
//! - [`try_eval_synthesized_qp`] — called from the evaluator's
//!   `eval_qualified_property` after the QP is located but before the
//!   body is evaluated. Returns `Some(result)` if the QP is a
//!   milestoning-generated one, `None` otherwise so the caller falls
//!   through to ordinary body evaluation.
//! - [`is_generated_milestoning_qp`] — pure predicate, factored out so
//!   the evaluator can branch on it cheaply.

use legend_pure_parser_pure::milestoning::{
    BUSINESS_DATE_PROPERTY, GENERATED_MILESTONING_STEREOTYPE, MILESTONING_PROFILE_FQN,
    MilestoningStereotype, PROCESSING_DATE_PROPERTY, inherited_temporal_stereotype,
    resolve_temporal_profile,
};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::nodes::class::QualifiedProperty;
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};
use smol_str::SmolStr;

use crate::error::{PureException, PureRuntimeError};
use crate::heap::ObjectHandle;
use crate::value::Value;

/// `true` if `qp` carries the
/// `<<meta::pure::profiles::milestoning::generatedmilestoningproperty>>`
/// stereotype. Used by the evaluator to short-circuit into
/// [`try_eval_synthesized_qp`] before running the (empty) body.
#[must_use]
pub fn is_generated_milestoning_qp(model: &PureModel, qp: &QualifiedProperty) -> bool {
    let Some(profile) = model.resolve_fqn_str(MILESTONING_PROFILE_FQN) else {
        return false;
    };
    qp.stereotypes
        .iter()
        .any(|s| s.profile == profile && s.value == GENERATED_MILESTONING_STEREOTYPE)
}

/// Evaluate a milestoning-synthesized qualified property at runtime.
///
/// Returns `Some(Ok(value))` on success, `Some(Err(_))` on dispatch
/// errors (e.g. missing edge-point property, type mismatches), or
/// `None` when the QP doesn't match any recognized milestoning shape —
/// the caller should then fall through to ordinary body evaluation.
///
/// The QP shapes recognized here mirror the compile-time synthesis in
/// `crates/pure/src/milestoning/synthesis.rs::synthesize_for_property`:
///
/// | QP name                            | Arity | Meaning                                                 |
/// |------------------------------------|-------|---------------------------------------------------------|
/// | `<orig>`                           | 0     | No-arg: use owner's own milestoning date as the filter  |
/// | `<orig>` (with td:Date arg)        | 1     | Filter target's businessDate/processingDate against td  |
/// | `<orig>` (with pd, bd args)        | 2     | Bitemporal: filter target by both dates                 |
/// | `<orig>AllVersionsInRange`         | 2     | Inclusive range query on target's date property         |
///
/// All shapes read `$this.<orig>AllVersions` (the edge-point property
/// synthesised alongside the QPs) and filter the resulting collection.
/// The result is then narrowed to the QP's declared multiplicity
/// (e.g. `T[1]` → `toOne`).
#[allow(clippy::result_large_err)]
pub fn try_eval_synthesized_qp(
    model: &PureModel,
    qp: &QualifiedProperty,
    this: &ObjectHandle,
    args: &[Value],
    heap: &crate::heap::RuntimeHeap,
) -> Option<Result<Value, PureException>> {
    if !is_generated_milestoning_qp(model, qp) {
        return None;
    }

    // The target class is the QP's return type. We need its temporal
    // stereotype to know which date property to compare against.
    let target_class_id = match &qp.return_type {
        TypeExpr::Named { element, .. } => *element,
        _ => {
            return Some(Err(milestoning_error(format!(
                "Synthesized milestoning QP '{}' has a non-Named return type",
                qp.name
            ))));
        }
    };
    let Some(temporal_profile) = resolve_temporal_profile(model) else {
        return Some(Err(milestoning_error(
            "milestoning runtime: temporal profile not loaded".into(),
        )));
    };
    let Some(target_stereo) =
        inherited_temporal_stereotype(model, target_class_id, temporal_profile)
    else {
        return Some(Err(milestoning_error(format!(
            "Synthesized milestoning QP '{}' returns non-milestoned class",
            qp.name
        ))));
    };

    // Decode the QP shape from its name + arity. Returns `None` for an
    // unknown shape so the evaluator falls through to ordinary body
    // evaluation (which today returns an empty collection for these
    // synthesised QPs, but stays correct if a future change populates
    // them).
    let shape = decode_shape(&qp.name, args.len(), target_stereo)?;

    // Read the edge-point collection.
    let edge_name = shape.edge_point_name();
    let edge_values = match heap.get_property_values(this, edge_name.as_str()) {
        Ok(values) => values,
        Err(e) => return Some(Err(PureException::from(e))),
    };

    let result = match shape {
        QpShape::SingleDate {
            date_property,
            date_idx,
            ..
        } => {
            let date = match args.get(date_idx) {
                Some(v @ Value::Date(_)) => v.clone(),
                _ => {
                    return Some(Err(milestoning_error(format!(
                        "Synthesized milestoning QP '{}' expected a Date[1] argument at \
                         position {}",
                        qp.name, date_idx
                    ))));
                }
            };
            filter_by_date_equals(&edge_values, date_property, &date)
        }
        QpShape::Bitemporal { .. } => {
            let pd = match args.first() {
                Some(v @ Value::Date(_)) => v.clone(),
                _ => return Some(Err(missing_date_arg(&qp.name, 0))),
            };
            let bd = match args.get(1) {
                Some(v @ Value::Date(_)) => v.clone(),
                _ => return Some(Err(missing_date_arg(&qp.name, 1))),
            };
            filter_by_both_dates(&edge_values, &pd, &bd)
        }
        QpShape::NoArgUsesOwnerDate { .. } => {
            // Use $this's own milestoning date(s) as the filter. For
            // same-kind owner/target, $this.<dateProp> is a Date[1]; we
            // read it and filter the edge-point collection against it.
            let owner_dates = match heap.get_property_values(this, BUSINESS_DATE_PROPERTY) {
                Ok(v) if !v.is_empty() => v,
                _ => match heap.get_property_values(this, PROCESSING_DATE_PROPERTY) {
                    Ok(v) if !v.is_empty() => v,
                    _ => {
                        return Some(Err(milestoning_error(format!(
                            "Synthesized milestoning QP '{}' (no-arg): owner has no \
                             milestoning date set",
                            qp.name
                        ))));
                    }
                },
            };
            let Some(date) = owner_dates.into_iter().next() else {
                return Some(Err(milestoning_error(format!(
                    "Synthesized milestoning QP '{}' (no-arg): owner date is empty",
                    qp.name
                ))));
            };
            match target_stereo {
                MilestoningStereotype::BusinessTemporal => {
                    filter_by_date_equals(&edge_values, BUSINESS_DATE_PROPERTY, &date)
                }
                MilestoningStereotype::ProcessingTemporal => {
                    filter_by_date_equals(&edge_values, PROCESSING_DATE_PROPERTY, &date)
                }
                MilestoningStereotype::Bitemporal => {
                    // Owner-date path on a bitemporal target falls back
                    // to single-date filter on businessDate. Java's
                    // template for the bitemporal no-arg variant uses
                    // both `$this.processingDate` and `$this.businessDate`
                    // — Phase B-2 keeps the simpler single-date semantics
                    // until the bitemporal no-arg case has a concrete
                    // test driving the dual-date behaviour.
                    filter_by_date_equals(&edge_values, BUSINESS_DATE_PROPERTY, &date)
                }
            }
        }
        QpShape::Range { date_property, .. } => {
            let start = match args.first() {
                Some(v @ Value::Date(_)) => v.clone(),
                _ => return Some(Err(missing_date_arg(&qp.name, 0))),
            };
            let end = match args.get(1) {
                Some(v @ Value::Date(_)) => v.clone(),
                _ => return Some(Err(missing_date_arg(&qp.name, 1))),
            };
            filter_by_date_range(&edge_values, date_property, &start, &end)
        }
    };

    // Narrow to declared multiplicity.
    Some(Ok(narrow_to_multiplicity(
        result,
        &qp.return_multiplicity,
        &qp.name,
    )))
}

// ---------------------------------------------------------------------------
// QP shape decoding
// ---------------------------------------------------------------------------

enum QpShape {
    /// `<orig>(td: Date[1])` — single-date variant; `date_idx = 0`. For
    /// businesstemporal targets `date_property == businessDate`, for
    /// processingtemporal `date_property == processingDate`. Bitemporal
    /// targets use this shape too on the businessDate (Java template
    /// `getSingleTemporalMilestoningPropertyCodeBlockWithParam` for
    /// non-bitemporal subcase; bitemporal's own dual-arg variant is
    /// `QpShape::Bitemporal`).
    SingleDate {
        date_property: &'static str,
        date_idx: usize,
        original_name: SmolStr,
    },
    /// `<orig>(pd: Date[1], bd: Date[1])` — bitemporal explicit-date
    /// variant.
    Bitemporal { original_name: SmolStr },
    /// `<orig>()` — no-arg variant; owner's milestoning date is used.
    /// The target's date property is resolved in the dispatch arm directly
    /// from the target stereotype, so no extra field is needed here.
    NoArgUsesOwnerDate { original_name: SmolStr },
    /// `<orig>AllVersionsInRange(start: Date[1], end: Date[1])`.
    Range {
        date_property: &'static str,
        original_name: SmolStr,
    },
}

impl QpShape {
    fn edge_point_name(&self) -> SmolStr {
        let original = match self {
            QpShape::SingleDate { original_name, .. }
            | QpShape::Bitemporal { original_name, .. }
            | QpShape::NoArgUsesOwnerDate { original_name, .. }
            | QpShape::Range { original_name, .. } => original_name,
        };
        legend_pure_parser_pure::milestoning::edge_point_property_name(original)
    }
}

fn decode_shape(qp_name: &str, arity: usize, target: MilestoningStereotype) -> Option<QpShape> {
    let target_date = match target {
        // BusinessTemporal and Bitemporal both filter range queries on
        // businessDate (matches `crates/runtime/src/native/lang.rs::GetAllVersionsInRange`
        // for the bitemporal case).
        MilestoningStereotype::BusinessTemporal | MilestoningStereotype::Bitemporal => {
            BUSINESS_DATE_PROPERTY
        }
        MilestoningStereotype::ProcessingTemporal => PROCESSING_DATE_PROPERTY,
    };

    // Range variant: name ends with "AllVersionsInRange".
    if let Some(orig) = qp_name.strip_suffix("AllVersionsInRange") {
        if arity != 2 {
            return None;
        }
        return Some(QpShape::Range {
            date_property: target_date,
            original_name: SmolStr::new(orig),
        });
    }

    // Otherwise the QP name IS the original property name.
    let original_name = SmolStr::new(qp_name);
    match arity {
        0 => {
            let _ = target_date; // owner-date path doesn't read target_date directly
            Some(QpShape::NoArgUsesOwnerDate { original_name })
        }
        1 => Some(QpShape::SingleDate {
            date_property: target_date,
            date_idx: 0,
            original_name,
        }),
        2 => Some(QpShape::Bitemporal { original_name }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Filtering helpers
// ---------------------------------------------------------------------------

fn filter_by_date_equals(
    edge_values: &im_rc::Vector<Value>,
    date_property: &str,
    date: &Value,
) -> Vec<Value> {
    edge_values
        .iter()
        .filter(|v| crate::native::lang::instance_property_matches_date(v, date_property, date))
        .cloned()
        .collect()
}

fn filter_by_both_dates(
    edge_values: &im_rc::Vector<Value>,
    processing_date: &Value,
    business_date: &Value,
) -> Vec<Value> {
    edge_values
        .iter()
        .filter(|v| {
            crate::native::lang::instance_property_matches_date(
                v,
                PROCESSING_DATE_PROPERTY,
                processing_date,
            ) && crate::native::lang::instance_property_matches_date(
                v,
                BUSINESS_DATE_PROPERTY,
                business_date,
            )
        })
        .cloned()
        .collect()
}

fn filter_by_date_range(
    edge_values: &im_rc::Vector<Value>,
    date_property: &str,
    start: &Value,
    end: &Value,
) -> Vec<Value> {
    edge_values
        .iter()
        .filter(|v| {
            crate::native::lang::instance_property_date_in_range(v, date_property, start, end)
        })
        .cloned()
        .collect()
}

fn narrow_to_multiplicity(items: Vec<Value>, mult: &Multiplicity, qp_name: &str) -> Value {
    let _ = qp_name;
    match mult {
        Multiplicity::PureOne | Multiplicity::ZeroOrOne if items.len() <= 1 => {
            match items.into_iter().next() {
                Some(v) => v,
                None => Value::from_vec(Vec::new()),
            }
        }
        Multiplicity::PureOne => {
            // Multiplicity violation: filter returned >1 items for a [1]
            // QP. Return as collection rather than panic; stricter error
            // surface can come once milestoning has dedicated diagnostics.
            Value::from_vec(items)
        }
        _ => Value::from_vec(items),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn milestoning_error(msg: String) -> PureException {
    PureException::from(PureRuntimeError::EvaluationError(msg))
}

fn missing_date_arg(qp_name: &str, idx: usize) -> PureException {
    milestoning_error(format!(
        "Synthesized milestoning QP '{qp_name}' expected Date[1] at position {idx}"
    ))
}
