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

#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Runtime-side hydration for the Mapping DSL.
//!
//! At evaluator setup, each `Element::DSLInstance(dsl_name="Mapping",
//! classifier_fqn="meta::pure::mapping::Mapping", data=…)` becomes a
//! bare metamodel heap row with only its classifier set. Pure-side
//! navigation like `Mapping.all()->first().classMappings` then
//! returns an empty collection.
//!
//! [`MappingDSLPopulator`] closes that gap: it decodes the
//! [`legend_pure_dsl_mapping::compiler::MappingSnapshot`] payload,
//! allocates one heap row per class-mapping with the appropriate
//! M3 classifier (`PureInstanceSetImplementation`,
//! `OperationSetImplementation`, `EnumerationMapping`, etc.), seeds
//! `id` / `class` / `root` / `name`, and writes the resulting
//! collection back onto the Mapping handle's `classMappings`
//! property.
//!
//! Pilot scope (commit 1): the populator only fills slots that the
//! snapshot already carries — `classMappings`, `includes`. Per-body
//! deep slots (`mainTableAlias`, `propertyMappings`, `filter`,
//! `srcClass`, …) land in follow-up commits once
//! `ClassMappingSnapshot` grows the corresponding fields.
//!
//! Wired via [`legend_pure_runtime::dsl::DSLPopulator`] and consumed
//! by callers through
//! [`legend_pure_runtime::eval::Evaluator::new_default_with_dsl_populators`].

use legend_pure_dsl_mapping::compiler::{ClassMappingSnapshot, MAPPING_DSL_NAME, MappingSnapshot};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::dsl::{DSLPopulationCtx, DSLPopulator};
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// FQN-prefixed M3 classifier returned for each `body_kind` tag.
///
/// Mirrors the Java M3 hierarchy in `meta::pure::mapping::*` plus the
/// store-DSL extension (`meta::relational::mappings::*`). Body kinds
/// not in this table — e.g. a future foreign DSL we haven't taught
/// the table about — fall back to the abstract `SetImplementation`.
#[must_use]
pub fn classifier_for_body_kind(body_kind: &str) -> &'static str {
    match body_kind {
        "Pure" => "meta::pure::mapping::PureInstanceSetImplementation",
        "Operation" => "meta::pure::mapping::OperationSetImplementation",
        "Enumeration" => "meta::pure::mapping::EnumerationMapping",
        "AggregationAware" => {
            "meta::pure::mapping::aggregationAware::AggregationAwareSetImplementation"
        }
        // XStore is exposed as an association implementation in Java's M3.
        "XStore" => "meta::pure::mapping::xStore::XStoreAssociationImplementation",
        // Relational class-mapping bodies project to Java's
        // RootRelationalInstanceSetImplementation. The body kind tag
        // is whatever the foreign-DSL extension returns from `kind()`.
        "Relational" => "meta::relational::mappings::RootRelationalInstanceSetImplementation",
        _ => "meta::pure::mapping::SetImplementation",
    }
}

/// DSL populator for `###Mapping` graph elements.
///
/// Zero-sized: every method dispatches off the snapshot payload, so
/// there is no per-evaluator state.
pub struct MappingDSLPopulator;

impl DSLPopulator for MappingDSLPopulator {
    fn dsl_name(&self) -> &'static str {
        MAPPING_DSL_NAME
    }

    fn populate(&self, mut ctx: DSLPopulationCtx<'_>) {
        let Ok(snapshot) = MappingSnapshot::decode(ctx.instance_data) else {
            return; // corrupt payload — stay evaluable
        };

        // ---- children: one heap row per class-mapping ----
        let mut class_mapping_values: Vec<Value> =
            Vec::with_capacity(snapshot.class_mappings.len());
        for cm in &snapshot.class_mappings {
            let cm_handle = allocate_class_mapping_row(&mut ctx, cm);
            class_mapping_values.push(Value::Object(cm_handle));
        }
        let _ = ctx
            .heap
            .mutate_set(&ctx.instance_handle, "classMappings", &class_mapping_values);

        // ---- includes: resolve each FQN to a Mapping element ref ----
        // Java keeps `includes` as `MappingInclude[*]` with includer
        // metadata; the pilot scope returns each *included Mapping*
        // as a `Value::Element` reference so downstream nav like
        // `m.includes->first().name` works on the included Mapping
        // directly. The `MappingInclude` wrapper grows in a follow-up
        // commit once the snapshot carries store-substitution data.
        let include_values: Vec<Value> = snapshot
            .includes
            .iter()
            .filter_map(|fqn| resolve_fqn(ctx.model, fqn).map(Value::Element))
            .collect();
        if !include_values.is_empty() {
            let _ = ctx
                .heap
                .mutate_set(&ctx.instance_handle, "includes", &include_values);
        }

        // ---- Mapping's own simple slots ----
        // name = simple name (last segment of FQN).
        if let Some(simple_name) = snapshot.fqn.as_str().rsplit("::").next() {
            let _ = ctx.heap.mutate_set(
                &ctx.instance_handle,
                "name",
                &[Value::String(SmolStr::new(simple_name))],
            );
        }
    }
}

fn allocate_class_mapping_row(
    ctx: &mut DSLPopulationCtx<'_>,
    cm: &ClassMappingSnapshot,
) -> legend_pure_runtime::heap::ObjectHandle {
    let classifier = classifier_for_body_kind(cm.body_kind.as_str());
    let handle = ctx.heap.alloc_dynamic(classifier);

    // id: explicit `[id]` first, fall back to class FQN (matches
    // `visible_class_mapping_ids` in dsl-mapping/compiler.rs).
    let id_value = cm.id.clone().unwrap_or_else(|| cm.class_fqn.clone());
    let _ = ctx
        .heap
        .mutate_set(&handle, "id", &[Value::String(id_value)]);

    // class: Value::Element when the class resolves.
    if let Some(class_id) = resolve_fqn(ctx.model, &cm.class_fqn) {
        let _ = ctx
            .heap
            .mutate_set(&handle, "class", &[Value::Element(class_id)]);
    }

    // root flag (the `*` prefix on `*Mapping pkg::Class : Pure {...}`).
    let _ = ctx
        .heap
        .mutate_set(&handle, "root", &[Value::Boolean(cm.is_root)]);

    // Optional mapping-instance display name following parserName.
    if let Some(name) = &cm.mapping_name {
        let _ = ctx
            .heap
            .mutate_set(&handle, "name", &[Value::String(name.clone())]);
    }

    handle
}

fn resolve_fqn(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(SmolStr::is_empty) {
        return None;
    }
    model.resolve_by_path(&segments)
}
