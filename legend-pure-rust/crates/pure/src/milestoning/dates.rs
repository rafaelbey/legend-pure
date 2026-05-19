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

//! `MilestoningDates` — compile-time carrier of the in-scope dates that
//! propagate through milestoned property access.
//!
//! Mirrors Java's `MilestoningDates` (`legend-pure-core/.../milestoning/MilestoningDates.java`).
//! Carries up to two dates plus the stereotype each one applies to:
//!
//! - `business_date`: the businessDate for a businesstemporal source, or the
//!   businessDate half of a bitemporal source.
//! - `processing_date`: the processingDate for a processingtemporal source, or
//!   the processingDate half of a bitemporal source.
//!
//! Construction is via stereotype-indexed unpacking of a positional date
//! argument list: a `businesstemporal` source's args slot `[0]` is the
//! businessDate; a `processingtemporal` source's args slot `[0]` is the
//! processingDate; a `bitemporal` source's args are `[processingDate,
//! businessDate]`.
//!
//! Consumption: [`Self::for_target_stereotype`] produces the *injection*
//! arg list to splice into a milestoned-property call, ordered for the
//! target class's stereotype. A bitemporal source narrowing to a
//! businesstemporal target drops the processingDate; a bitemporal source
//! widening to a bitemporal target keeps both. A businesstemporal source
//! targeting a processingtemporal class is a semantic mismatch — we
//! return `None` so the propagation pass can leave the call alone and
//! the missing-date validator fires.

use crate::milestoning::MilestoningStereotype;
use crate::types::ValueSpec;

/// Compile-time milestoning context carried by an in-scope variable or
/// produced by a milestoning-aware expression (`getAll(Class, $d)`,
/// `$x.qp($d)`, …).
#[derive(Debug, Clone)]
pub struct MilestoningDates {
    /// Business date expression — populated for businesstemporal and
    /// bitemporal sources. `None` for processingtemporal sources.
    pub business_date: Option<ValueSpec>,
    /// Processing date expression — populated for processingtemporal and
    /// bitemporal sources. `None` for businesstemporal sources.
    pub processing_date: Option<ValueSpec>,
}

impl MilestoningDates {
    /// Build a carrier from the *source* class's stereotype and a positional
    /// list of date argument expressions, mirroring Java's
    /// `new MilestoningDates(stereotype, args)` constructor.
    ///
    /// Returns `None` when the arg list arity doesn't match the stereotype's
    /// expected arity (1 for single-temporal, 2 for bitemporal) — the caller
    /// should then treat the expression as a non-milestoning source.
    #[must_use]
    pub fn from_source(stereotype: MilestoningStereotype, args: &[ValueSpec]) -> Option<Self> {
        match (stereotype, args.len()) {
            (MilestoningStereotype::BusinessTemporal, 1) => Some(Self {
                business_date: Some(args[0].clone()),
                processing_date: None,
            }),
            (MilestoningStereotype::ProcessingTemporal, 1) => Some(Self {
                business_date: None,
                processing_date: Some(args[0].clone()),
            }),
            (MilestoningStereotype::Bitemporal, 2) => Some(Self {
                // Java order: processingDate first, then businessDate.
                processing_date: Some(args[0].clone()),
                business_date: Some(args[1].clone()),
            }),
            _ => None,
        }
    }

    /// Produce the date arguments to inject into a milestoned-property
    /// call whose **target class** carries `target`.
    ///
    /// Returns `None` when this carrier doesn't have the date(s) the target
    /// needs (e.g. a businesstemporal source can't supply a processingDate
    /// to a processingtemporal target). The caller should then leave the
    /// call alone so B-4's missing-date validator fires.
    ///
    /// Ordering matches Java's `applyPropertyFunctionExpressionMilestonedDates`:
    /// businesstemporal targets receive `[businessDate]`,
    /// processingtemporal targets receive `[processingDate]`, bitemporal
    /// targets receive `[processingDate, businessDate]`.
    #[must_use]
    pub fn for_target_stereotype(&self, target: MilestoningStereotype) -> Option<Vec<ValueSpec>> {
        match target {
            MilestoningStereotype::BusinessTemporal => {
                self.business_date.as_ref().map(|d| vec![d.clone()])
            }
            MilestoningStereotype::ProcessingTemporal => {
                self.processing_date.as_ref().map(|d| vec![d.clone()])
            }
            MilestoningStereotype::Bitemporal => match (&self.processing_date, &self.business_date)
            {
                (Some(pd), Some(bd)) => Some(vec![pd.clone(), bd.clone()]),
                _ => None,
            },
        }
    }
}
