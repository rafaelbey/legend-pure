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

//! Milestoning — temporal-type post-processing (Phase A: declare-side).
//!
//! Pure's three temporal stereotypes — `<<temporal.businesstemporal>>`,
//! `<<temporal.processingtemporal>>`, `<<temporal.bitemporal>>` — drive a
//! post-processing rewrite that the Java engine performs in
//! `MilestoningClassProcessor` + `MilestoningPropertyProcessor`. This module is
//! the Rust port of that rewrite, scoped to **declare-side** behaviour:
//!
//! 1. **Class-level date synthesis.** Each milestoned class gets synthetic date
//!    properties (`businessDate`, `processingDate`, or both) plus a `milestoning`
//!    slot pointing at the corresponding `*Milestoning` carrier class.
//! 2. **Property-rewrite synthesis.** Each declared property whose target class
//!    is milestoned is moved to `original_milestoned_properties`, replaced by
//!    an edge-point property (`pAllVersions: T[*]`), and supplemented with
//!    qualified-property *signatures* (`p(td: Date[1])`, `pAllVersionsInRange`,
//!    optionally `p()`). Bodies are deferred to Phase B along with the runtime
//!    `getAll` / `getAllVersionsInRange` natives and the date-context
//!    propagation pass.
//! 3. **Validators.** Four declare-side checks: at-most-one temporal stereotype,
//!    reserved property names, hierarchy consistency, edge-point name
//!    collision.
//!
//! Phase A is the source-of-truth for the Class IR `original_milestoned_properties`
//! slot, but the surrounding runtime/dispatch story (date propagation, the three
//! `getAll(...)` natives, `%latest`-only-in-context validators, grammar
//! shortcuts) is Phase B/C work.

use smol_str::SmolStr;

use crate::annotations::StereotypeRef;
use crate::ids::ElementId;
use crate::model::{Element, PureModel};

pub mod dates;
pub mod propagation;
pub mod synthesis;
pub mod validate;

// ---------------------------------------------------------------------------
// Constants — Java parity
// ---------------------------------------------------------------------------

/// FQN of the `temporal` profile that declares the three temporal stereotypes.
///
/// The platform declaration lives in
/// `platform/pure/grammar/milestoning.pure`:
/// ```pure
/// Profile meta::pure::profiles::temporal
/// { stereotypes: [bitemporal, businesstemporal, processingtemporal]; }
/// ```
pub const TEMPORAL_PROFILE_FQN: &str = "meta::pure::profiles::temporal";

/// FQN of the `milestoning` profile carrying the
/// `generatedmilestoningproperty` / `generatedmilestoningdateproperty`
/// stereotypes used to mark synthesized properties.
pub const MILESTONING_PROFILE_FQN: &str = "meta::pure::profiles::milestoning";

/// Stereotype name applied to synthesized **edge-point** + **qualified**
/// properties so downstream consumers (validators, IDE views, codegen) can
/// distinguish them from user-declared properties.
pub const GENERATED_MILESTONING_STEREOTYPE: &str = "generatedmilestoningproperty";

/// Stereotype name applied to synthesized **date** properties
/// (`businessDate`, `processingDate`, the `milestoning` slot).
pub const GENERATED_MILESTONING_DATE_STEREOTYPE: &str = "generatedmilestoningdateproperty";

/// The synthesized slot pointing at the milestoning-carrier class
/// (`BusinessDateMilestoning`, `ProcessingDateMilestoning`, or
/// `BiTemporalMilestoning`). Multiplicity `[0..1]`.
pub const MILESTONING_SLOT_NAME: &str = "milestoning";

/// Synthesized business-date property name. Java parity:
/// `MilestoningStereotypeEnum.BUSINESS_DATE_PROPERTY_NAME`.
pub const BUSINESS_DATE_PROPERTY: &str = "businessDate";

/// Synthesized processing-date property name. Java parity:
/// `MilestoningStereotypeEnum.PROCESSING_DATE_PROPERTY_NAME`.
pub const PROCESSING_DATE_PROPERTY: &str = "processingDate";

/// `BusinessDateMilestoning` carrier property name (lower bound on
/// business-temporal ranges).
pub const FROM_PROPERTY: &str = "from";

/// `BusinessDateMilestoning` carrier property name (upper bound on
/// business-temporal ranges).
pub const THRU_PROPERTY: &str = "thru";

/// `ProcessingDateMilestoning` carrier property name (lower bound on
/// processing-temporal ranges).
pub const IN_PROPERTY: &str = "in";

/// `ProcessingDateMilestoning` carrier property name (upper bound on
/// processing-temporal ranges).
pub const OUT_PROPERTY: &str = "out";

/// FQN of the `BusinessDateMilestoning` carrier class declared in
/// `platform/pure/grammar/milestoning.pure`.
pub const BUSINESS_DATE_MILESTONING_FQN: &str = "meta::pure::milestoning::BusinessDateMilestoning";

/// FQN of the `ProcessingDateMilestoning` carrier class.
pub const PROCESSING_DATE_MILESTONING_FQN: &str =
    "meta::pure::milestoning::ProcessingDateMilestoning";

/// FQN of the `BiTemporalMilestoning` carrier class.
pub const BITEMPORAL_MILESTONING_FQN: &str = "meta::pure::milestoning::BiTemporalMilestoning";

// ---------------------------------------------------------------------------
// MilestoningStereotype — Java MilestoningStereotypeEnum parity
// ---------------------------------------------------------------------------

/// The three temporal stereotypes. Java parity:
/// `MilestoningStereotypeEnum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MilestoningStereotype {
    /// `<<temporal.businesstemporal>>` — adds `businessDate: Date[1]` and a
    /// `milestoning: BusinessDateMilestoning[0..1]` slot with `from`/`thru`.
    BusinessTemporal,
    /// `<<temporal.processingtemporal>>` — adds `processingDate: Date[1]` and
    /// a `milestoning: ProcessingDateMilestoning[0..1]` slot with `in`/`out`.
    ProcessingTemporal,
    /// `<<temporal.bitemporal>>` — combines both: `businessDate` +
    /// `processingDate` + a `milestoning: BiTemporalMilestoning[0..1]` slot
    /// with all four range properties.
    Bitemporal,
}

impl MilestoningStereotype {
    /// Parse a stereotype value name (`"businesstemporal"`,
    /// `"processingtemporal"`, `"bitemporal"`) into a [`MilestoningStereotype`].
    /// Returns `None` for any other value. Java parity: enum-`valueOf`.
    #[must_use]
    pub fn from_value_name(name: &str) -> Option<Self> {
        match name {
            "businesstemporal" => Some(Self::BusinessTemporal),
            "processingtemporal" => Some(Self::ProcessingTemporal),
            "bitemporal" => Some(Self::Bitemporal),
            _ => None,
        }
    }

    /// The temporal-date property names this stereotype synthesizes on the
    /// owning class. Java parity:
    /// `MilestoningStereotypeEnum.getTemporalDatePropertyNames`.
    #[must_use]
    pub fn temporal_date_property_names(self) -> &'static [&'static str] {
        match self {
            Self::BusinessTemporal => &[BUSINESS_DATE_PROPERTY],
            Self::ProcessingTemporal => &[PROCESSING_DATE_PROPERTY],
            // Java order: processingDate first, then businessDate
            // (`bitemporal.getTemporalDatePropertyNames`).
            Self::Bitemporal => &[PROCESSING_DATE_PROPERTY, BUSINESS_DATE_PROPERTY],
        }
    }

    /// The range-carrier property names this stereotype binds on the
    /// `*Milestoning` slot's target class. Provided for completeness — used
    /// by reserved-name validation (A4.2). Java parity:
    /// `MilestoningStereotypeEnum.getMilestoningPropertyNames`.
    #[must_use]
    pub fn milestoning_property_names(self) -> &'static [&'static str] {
        match self {
            Self::BusinessTemporal => &[FROM_PROPERTY, THRU_PROPERTY],
            Self::ProcessingTemporal => &[IN_PROPERTY, OUT_PROPERTY],
            // Java order: in/out first, then from/thru
            // (`bitemporal.getMilestoningPropertyNames`).
            Self::Bitemporal => &[IN_PROPERTY, OUT_PROPERTY, FROM_PROPERTY, THRU_PROPERTY],
        }
    }

    /// FQN of the milestoning-carrier class this stereotype binds to the
    /// synthesized `milestoning` slot. Java parity:
    /// `MilestoningStereotypeEnum.getMilestoningPropertyClassName`.
    #[must_use]
    pub fn milestoning_property_class_fqn(self) -> &'static str {
        match self {
            Self::BusinessTemporal => BUSINESS_DATE_MILESTONING_FQN,
            Self::ProcessingTemporal => PROCESSING_DATE_MILESTONING_FQN,
            Self::Bitemporal => BITEMPORAL_MILESTONING_FQN,
        }
    }

    /// The stereotype value name as it appears in `.pure` source after the
    /// profile reference (`<<temporal.businesstemporal>>` → `"businesstemporal"`).
    #[must_use]
    pub fn value_name(self) -> &'static str {
        match self {
            Self::BusinessTemporal => "businesstemporal",
            Self::ProcessingTemporal => "processingtemporal",
            Self::Bitemporal => "bitemporal",
        }
    }
}

// ---------------------------------------------------------------------------
// Name helpers
// ---------------------------------------------------------------------------

/// Edge-point property name for a milestoned-target property: `p` →
/// `pAllVersions`. Java parity:
/// `MilestoningFunctions.getEdgePointPropertyName`.
#[must_use]
pub fn edge_point_property_name(name: &str) -> SmolStr {
    SmolStr::from(format!("{name}AllVersions"))
}

/// Range-property name for a milestoned-target property: `p` →
/// `pAllVersionsInRange`. Java parity: implicit in the templates inside
/// `MilestoningStereotypeEnum.getSingleDateQualifiedRangePropertyCodeBlock`.
#[must_use]
pub fn range_property_name(name: &str) -> SmolStr {
    SmolStr::from(format!("{name}AllVersionsInRange"))
}

/// Names that are reserved on a milestoned class because synthesis will
/// claim them. Java parity: `MilestoningClassValidator.validatePropertyNames…`.
///
/// Validator A4.2 forbids user-declared properties from colliding with any
/// of these on a class that carries a temporal stereotype.
#[must_use]
pub fn reserved_property_names_for(s: MilestoningStereotype) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = s.temporal_date_property_names().to_vec();
    out.push(MILESTONING_SLOT_NAME);
    out.extend_from_slice(s.milestoning_property_names());
    out
}

// ---------------------------------------------------------------------------
// Stereotype-lookup helpers
// ---------------------------------------------------------------------------

/// Resolve the FQN of the `temporal` profile in this model. Returns `None`
/// when the platform isn't loaded yet (e.g. the bootstrap chunk on its own
/// — the temporal profile is declared in `milestoning.pure` which lives in
/// the platform repo).
///
/// Synthesis short-circuits when this returns `None`: without the profile we
/// can't recognise any class as milestoned, so there's nothing to do.
#[must_use]
pub fn resolve_temporal_profile(model: &PureModel) -> Option<ElementId> {
    model.resolve_fqn_str(TEMPORAL_PROFILE_FQN)
}

/// All direct temporal stereotypes carried by a `StereotypeRef` slice (i.e.
/// not walking supertypes). Java parity:
/// `MilestoningFunctions.getTemporalStereoTypesExcludingParents`.
///
/// Deduplicates while preserving insertion order so dependent code (A4.1
/// detects duplicates *before* this dedup is applied, so it inspects the raw
/// slice itself).
#[must_use]
pub fn direct_temporal_stereotypes(
    stereotypes: &[StereotypeRef],
    temporal_profile: ElementId,
) -> Vec<MilestoningStereotype> {
    let mut out: Vec<MilestoningStereotype> = Vec::new();
    for s in stereotypes {
        if s.profile == temporal_profile
            && let Some(kind) = MilestoningStereotype::from_value_name(&s.value)
            && !out.contains(&kind)
        {
            out.push(kind);
        }
    }
    out
}

/// Walk the class's transitive supertypes (Class only, ignoring `Any`) and
/// return the first temporal stereotype found. Mirrors Java's
/// `MilestoningFunctions.getTemporalStereoTypesFromTopMostNonTopTypeGeneralizations`.
///
/// Used by:
/// - **A3b** to detect a milestoned target class through ancestor chains
///   (`Customer.address: SubAddress` where `SubAddress` extends a temporal
///   `Address` should still trigger property rewriting).
/// - **A4.3** to enforce hierarchy consistency.
#[must_use]
pub fn inherited_temporal_stereotype(
    model: &PureModel,
    class_id: ElementId,
    temporal_profile: ElementId,
) -> Option<MilestoningStereotype> {
    let Element::Class(class) = model.try_get_element(class_id)? else {
        return None;
    };
    let direct = direct_temporal_stereotypes(&class.stereotypes, temporal_profile);
    if let Some(first) = direct.first() {
        return Some(*first);
    }
    for st in &class.super_types {
        if let crate::types::TypeExpr::Named { element, .. } = st
            && *element != crate::bootstrap::ANY_ID
            && let Some(s) = inherited_temporal_stereotype(model, *element, temporal_profile)
        {
            return Some(s);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stereotype_value_name() {
        assert_eq!(
            MilestoningStereotype::from_value_name("businesstemporal"),
            Some(MilestoningStereotype::BusinessTemporal)
        );
        assert_eq!(
            MilestoningStereotype::from_value_name("processingtemporal"),
            Some(MilestoningStereotype::ProcessingTemporal)
        );
        assert_eq!(
            MilestoningStereotype::from_value_name("bitemporal"),
            Some(MilestoningStereotype::Bitemporal)
        );
        assert_eq!(MilestoningStereotype::from_value_name("nope"), None);
    }

    #[test]
    fn date_property_names_match_java() {
        assert_eq!(
            MilestoningStereotype::BusinessTemporal.temporal_date_property_names(),
            &["businessDate"]
        );
        assert_eq!(
            MilestoningStereotype::ProcessingTemporal.temporal_date_property_names(),
            &["processingDate"]
        );
        // Java bitemporal order: processingDate first
        assert_eq!(
            MilestoningStereotype::Bitemporal.temporal_date_property_names(),
            &["processingDate", "businessDate"]
        );
    }

    #[test]
    fn carrier_class_fqns_match_platform() {
        assert_eq!(
            MilestoningStereotype::BusinessTemporal.milestoning_property_class_fqn(),
            "meta::pure::milestoning::BusinessDateMilestoning"
        );
        assert_eq!(
            MilestoningStereotype::ProcessingTemporal.milestoning_property_class_fqn(),
            "meta::pure::milestoning::ProcessingDateMilestoning"
        );
        assert_eq!(
            MilestoningStereotype::Bitemporal.milestoning_property_class_fqn(),
            "meta::pure::milestoning::BiTemporalMilestoning"
        );
    }

    #[test]
    fn edge_point_and_range_names() {
        assert_eq!(edge_point_property_name("address"), "addressAllVersions");
        assert_eq!(range_property_name("address"), "addressAllVersionsInRange");
    }

    #[test]
    fn reserved_names_cover_synthesized_surface() {
        let bt = reserved_property_names_for(MilestoningStereotype::BusinessTemporal);
        assert!(bt.contains(&"businessDate"));
        assert!(bt.contains(&"milestoning"));
        assert!(bt.contains(&"from"));
        assert!(bt.contains(&"thru"));

        let bi = reserved_property_names_for(MilestoningStereotype::Bitemporal);
        assert!(bi.contains(&"businessDate"));
        assert!(bi.contains(&"processingDate"));
        assert!(bi.contains(&"from"));
        assert!(bi.contains(&"thru"));
        assert!(bi.contains(&"in"));
        assert!(bi.contains(&"out"));
    }
}
