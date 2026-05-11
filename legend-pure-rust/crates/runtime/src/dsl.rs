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

//! DSL-instance heap-row hydration SPI.
//!
//! Every `Element::DSLInstance` in the model becomes a bare metamodel
//! heap row at evaluator setup (classifier set via `metatype_of` —
//! the instance's `classifier_fqn`; no property values populated).
//! For Pure-side navigation like `Mapping.all()->first().classMappings`
//! to return populated children, a DSL-aware **populator** has to
//! project the snapshot payload (e.g. `MappingSnapshot`) into a heap
//! tree.
//!
//! This module defines:
//!
//! - [`DSLPopulator`] — the trait DSL crates implement to hydrate
//!   their own DSLInstance rows.
//! - [`DSLPopulationCtx`] — bundle of context the runtime hands to
//!   each populator (model + heap + pre-allocated handle for the
//!   instance + the raw payload bytes).
//! - [`run_populators`] — the runtime-side driver that walks every
//!   DSLInstance in the model and dispatches to the matching
//!   populator. Called by [`crate::eval::Evaluator`] constructors
//!   that opt into DSL hydration.
//!
//! DSL crates should ship a sibling `*-runtime` crate that depends
//! on both this crate and the corresponding DSL crate (which owns
//! the snapshot type). See `crates/dsl-mapping-runtime/` for the
//! reference implementation.

use std::collections::HashMap;

use legend_pure_parser_pure::model::{Element, PureModel};

use crate::heap::{ObjectHandle, RuntimeHeap};

/// Plugin contract for hydrating `Element::DSLInstance` heap rows
/// during evaluator setup.
///
/// One implementation per DSL (Mapping, Diagram, RelationalDatabase,
/// …). The runtime calls [`Self::populate`] exactly once per
/// DSLInstance whose `dsl_name` matches [`Self::dsl_name`].
pub trait DSLPopulator: 'static {
    /// `Element::DSLInstance.dsl_name` this populator handles.
    /// E.g. `"Mapping"`, `"Diagram"`, `"RelationalDatabase"`.
    /// Must be a stable string constant.
    fn dsl_name(&self) -> &'static str;

    /// Project the DSLInstance payload into property values on the
    /// pre-allocated instance handle (and any nested heap rows the
    /// populator chooses to allocate for property children).
    ///
    /// Errors during decode / lookup should be silent: the runtime
    /// stays evaluable even if a snapshot is malformed. A future
    /// revision may add a diagnostics channel.
    fn populate(&self, ctx: DSLPopulationCtx<'_>);
}

/// Per-instance population context handed to [`DSLPopulator::populate`].
pub struct DSLPopulationCtx<'a> {
    /// Compiled model; populators read it for FQN resolution and
    /// `Element::Class` / `Element::Function` lookups that drive
    /// `class` / `function` property values.
    pub model: &'a PureModel,
    /// Mutable heap. Populators allocate nested rows via
    /// [`RuntimeHeap::alloc_dynamic`] and set properties via
    /// [`RuntimeHeap::mutate_set`] / [`RuntimeHeap::mutate_add`].
    pub heap: &'a mut RuntimeHeap,
    /// Heap handle for **this** DSLInstance; pre-allocated by
    /// `bootstrap_metamodel` so populators don't have to wire it
    /// into the element-to-object map themselves.
    pub instance_handle: ObjectHandle,
    /// Postcard-encoded payload from `DSLInstance.data`. Each DSL
    /// crate decodes via its own `Snapshot::decode` helper.
    pub instance_data: &'a [u8],
    /// The instance's `classifier_fqn` — e.g.
    /// `"meta::pure::mapping::Mapping"`. Populators rarely need it
    /// (classifier is already set on the handle), but it's available
    /// for diagnostics.
    pub classifier_fqn: &'a str,
}

/// Walk every `Element::DSLInstance` in the model; for each one,
/// look up the matching populator by `dsl_name` and invoke it.
///
/// Called once during evaluator setup, after
/// [`RuntimeHeap::bootstrap_metamodel`] has allocated the bare
/// metamodel rows.
pub fn run_populators(model: &PureModel, heap: &mut RuntimeHeap, populators: &[&dyn DSLPopulator]) {
    if populators.is_empty() {
        return;
    }
    // Snapshot dsl_name → populator into a HashMap for O(1) lookup.
    let by_name: HashMap<&'static str, &dyn DSLPopulator> =
        populators.iter().map(|p| (p.dsl_name(), *p)).collect();

    // Collect the DSLInstance work-items up front so we can release
    // the immutable borrow on `model` before calling into the
    // populator (which takes `&mut heap` + `&model`).
    let mut work: Vec<(
        legend_pure_parser_pure::ids::ElementId,
        smol_str::SmolStr,
        smol_str::SmolStr,
        Vec<u8>,
    )> = Vec::new();
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::DSLInstance(d) = element else {
                continue;
            };
            if !by_name.contains_key(d.dsl_name.as_str()) {
                continue;
            }
            let eid = legend_pure_parser_pure::ids::ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            work.push((
                eid,
                d.dsl_name.clone(),
                d.classifier_fqn.clone(),
                d.data.clone(),
            ));
        }
    }

    for (eid, dsl_name, classifier_fqn, data) in work {
        let Some(handle) = heap.object_for_element(eid) else {
            // Defensive — bootstrap_metamodel must have run, but
            // skip silently if not.
            continue;
        };
        let Some(populator) = by_name.get(dsl_name.as_str()) else {
            continue;
        };
        let ctx = DSLPopulationCtx {
            model,
            heap,
            instance_handle: handle,
            instance_data: &data,
            classifier_fqn: classifier_fqn.as_str(),
        };
        populator.populate(ctx);
    }
}
