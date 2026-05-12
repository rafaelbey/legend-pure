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

//! Runtime-side hydration for the Relational DSL.
//!
//! At evaluator setup each `Element::DSLInstance(dsl_name="RelationalDatabase",
//! classifier_fqn="meta::relational::metamodel::Database", data=…)` becomes
//! a bare metamodel heap row with only its classifier set. Pure-side
//! navigation like `Database.all()->first().schemas->first().tables` then
//! returns an empty collection.
//!
//! [`RelationalDatabaseDSLPopulator`] closes that gap: it decodes the
//! [`legend_pure_dsl_relational::compiler::DatabaseSnapshot`] payload,
//! allocates `Schema`, `Table`, `View`, `Join`, `Filter` heap rows with
//! the correct M3 classifiers + `name` slot, then writes
//! `schemas` / `joins` / `filters` / `multiGrainFilters` onto the
//! Database handle.
//!
//! Pilot scope (commit 2 of T1.3): the populator only fills slots the
//! snapshot already carries — names of schemas/tables/views/joins/filters/
//! MGFs. **Column-level details and op-body trees are not in the
//! snapshot today** and land in a follow-up commit when the snapshot
//! grows them. `mainTableAlias` wiring from
//! `RootRelationalInstanceSetImplementation` (T1.3 commit 3) reads
//! the table heap rows produced here.
//!
//! Wired via [`legend_pure_runtime::dsl::DSLPopulator`] and consumed by
//! callers through
//! [`legend_pure_runtime::eval::Evaluator::new_default_with_dsl_populators`].

use legend_pure_dsl_relational::compiler::{
    DATABASE_DSL_NAME, DatabaseSnapshot, RELATIONAL_CLASS_MAPPING_DSL_NAME,
    RelationalClassMappingSnapshot, SchemaSnapshot,
};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::dsl::{DSLPopulationCtx, DSLPopulator};
use legend_pure_runtime::heap::ObjectHandle;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// M3 classifier for `meta::relational::metamodel::TableAlias`
/// rows produced by [`RelationalClassMappingDSLPopulator`].
pub const TABLE_ALIAS_CLASSIFIER: &str = "meta::relational::metamodel::TableAlias";

/// M3 classifier FQN for the parent Mapping (used to locate the
/// parent heap row by element-id chain).
pub const MAPPING_CLASSIFIER: &str = "meta::pure::mapping::Mapping";

/// M3 classifier FQNs used by relational metamodel.
pub const DATABASE_CLASSIFIER: &str = "meta::relational::metamodel::Database";
/// Schema heap-row classifier.
pub const SCHEMA_CLASSIFIER: &str = "meta::relational::metamodel::Schema";
/// Table heap-row classifier.
pub const TABLE_CLASSIFIER: &str = "meta::relational::metamodel::relation::Table";
/// View heap-row classifier.
pub const VIEW_CLASSIFIER: &str = "meta::relational::metamodel::relation::View";
/// Join heap-row classifier.
pub const JOIN_CLASSIFIER: &str = "meta::relational::metamodel::join::Join";
/// Filter heap-row classifier.
pub const FILTER_CLASSIFIER: &str = "meta::relational::metamodel::Filter";
/// MultiGrainFilter heap-row classifier (extends Filter).
pub const MULTI_GRAIN_FILTER_CLASSIFIER: &str = "meta::relational::metamodel::MultiGrainFilter";

/// Synthetic default-schema name used when a Database declares
/// top-level tables/views (i.e. outside a `Schema` block). Mirrors
/// Java's `meta::relational::metamodel::Database`-level default of
/// `"default"`.
pub const DEFAULT_SCHEMA_NAME: &str = "default";

/// DSL populator for `###Relational` `Database` graph elements.
///
/// Zero-sized — all dispatch flows through the snapshot payload.
pub struct RelationalDatabaseDSLPopulator;

impl DSLPopulator for RelationalDatabaseDSLPopulator {
    fn dsl_name(&self) -> &'static str {
        DATABASE_DSL_NAME
    }

    fn populate(&self, mut ctx: DSLPopulationCtx<'_>) {
        let snapshot = match DatabaseSnapshot::decode(ctx.instance_data) {
            Ok(s) => s,
            Err(_) => return, // corrupt payload — stay evaluable
        };

        // -- name (simple-name segment of the Database FQN) --
        if let Some(simple_name) = snapshot.fqn.as_str().rsplit("::").next() {
            let _ = ctx.heap.mutate_set(
                &ctx.instance_handle,
                "name",
                &[Value::String(SmolStr::new(simple_name))],
            );
        }

        // -- includes: each included Database resolves to a
        //    `Value::Element` ref. Wrapped `MappingInclude`-style
        //    objects are a follow-up; for the pilot the element ref
        //    is enough to navigate `db.includes->first().name`.
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

        // -- schemas: explicit Schema blocks PLUS a synthetic
        //    "default" Schema gathering top-level tables/views when
        //    any exist. Java exposes every NamedRelation via some
        //    Schema parent, so the synthetic node keeps navigation
        //    uniform.
        let mut schema_values: Vec<Value> = Vec::with_capacity(snapshot.schemas.len() + 1);
        if !snapshot.default_tables.is_empty() || !snapshot.default_views.is_empty() {
            let default_schema = SchemaSnapshot {
                name: SmolStr::new(DEFAULT_SCHEMA_NAME),
                tables: snapshot.default_tables.clone(),
                views: snapshot.default_views.clone(),
            };
            schema_values.push(Value::Object(allocate_schema_row(
                &mut ctx,
                &default_schema,
            )));
        }
        for s in &snapshot.schemas {
            schema_values.push(Value::Object(allocate_schema_row(&mut ctx, s)));
        }
        if !schema_values.is_empty() {
            let _ = ctx
                .heap
                .mutate_set(&ctx.instance_handle, "schemas", &schema_values);
        }

        // -- joins / filters / multi-grain filters (name-only rows). --
        // Allocate rows up front so the mutate_set call against
        // ctx.instance_handle doesn't compete with ctx.heap's mutable
        // borrow during allocate_named_rows.
        let joins = allocate_named_rows(&mut ctx, JOIN_CLASSIFIER, &snapshot.joins);
        let filters = allocate_named_rows(&mut ctx, FILTER_CLASSIFIER, &snapshot.filters);
        let mgfs = allocate_named_rows(
            &mut ctx,
            MULTI_GRAIN_FILTER_CLASSIFIER,
            &snapshot.multi_grain_filters,
        );
        let _ = ctx.heap.mutate_set(&ctx.instance_handle, "joins", &joins);
        let _ = ctx
            .heap
            .mutate_set(&ctx.instance_handle, "filters", &filters);
        let _ = ctx
            .heap
            .mutate_set(&ctx.instance_handle, "multiGrainFilters", &mgfs);
    }
}

fn allocate_schema_row(ctx: &mut DSLPopulationCtx<'_>, snap: &SchemaSnapshot) -> ObjectHandle {
    let handle = ctx.heap.alloc_dynamic(SCHEMA_CLASSIFIER);
    let _ = ctx
        .heap
        .mutate_set(&handle, "name", &[Value::String(snap.name.clone())]);
    let tables = allocate_named_rows(ctx, TABLE_CLASSIFIER, &snap.tables);
    if !tables.is_empty() {
        let _ = ctx.heap.mutate_set(&handle, "tables", &tables);
    }
    let views = allocate_named_rows(ctx, VIEW_CLASSIFIER, &snap.views);
    if !views.is_empty() {
        let _ = ctx.heap.mutate_set(&handle, "views", &views);
    }
    handle
}

fn allocate_named_rows(
    ctx: &mut DSLPopulationCtx<'_>,
    classifier: &str,
    names: &[SmolStr],
) -> Vec<Value> {
    names
        .iter()
        .map(|name| {
            let h = ctx.heap.alloc_dynamic(classifier);
            let _ = ctx
                .heap
                .mutate_set(&h, "name", &[Value::String(name.clone())]);
            Value::Object(h)
        })
        .collect()
}

fn resolve_fqn(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(SmolStr::is_empty) {
        return None;
    }
    model.resolve_by_path(&segments)
}

// ---------------------------------------------------------------------------
// RelationalClassMappingDSLPopulator
// ---------------------------------------------------------------------------

/// Populator for the sidecar `RelationalClassMapping` DSLInstance
/// rows emitted by `RelationalExtension::declare`.
///
/// Patches an already-allocated `RootRelationalInstanceSetImplementation`
/// heap row (created by `MappingDSLPopulator`) with a `mainTableAlias`
/// `TableAlias` heap row whose `relationalElement` slot points at the
/// Table heap row allocated by [`RelationalDatabaseDSLPopulator`].
///
/// Depends on the runtime walking populators in chunk-element order,
/// which means:
///
/// 1. `MappingDSLPopulator` runs first (Mapping rows declared before
///    Database / RelationalClassMapping in the dsl-relational source
///    walk),
/// 2. `RelationalDatabaseDSLPopulator` runs next (Database rows
///    declared before sidecar rows in the same source walk),
/// 3. `RelationalClassMappingDSLPopulator` runs last (sidecar rows
///    declared after both Mapping and Database rows are populated).
///
/// Pilot scope (T1.3 commit 3): only the `mainTableAlias` slot.
/// Property mappings (`propertyMappings` collection), filters, and
/// the join sequence are deferred to follow-up commits when the
/// snapshot grows the corresponding fields.
pub struct RelationalClassMappingDSLPopulator;

impl DSLPopulator for RelationalClassMappingDSLPopulator {
    fn dsl_name(&self) -> &'static str {
        RELATIONAL_CLASS_MAPPING_DSL_NAME
    }

    fn populate(&self, ctx: DSLPopulationCtx<'_>) {
        let snapshot = match RelationalClassMappingSnapshot::decode(ctx.instance_data) {
            Ok(s) => s,
            Err(_) => return,
        };

        let Some(main_table) = snapshot.main_table.as_ref() else {
            // Class-mapping has no `~mainTable`; nothing for this
            // populator to do (Java treats this case as
            // inherited-via-extends and other validators handle it).
            return;
        };

        // -- find the class-mapping heap row to patch --
        let Some(cm_handle) = find_class_mapping_row(
            ctx.model,
            ctx.heap,
            &snapshot.mapping_fqn,
            &snapshot.class_mapping_id,
        ) else {
            return;
        };

        // -- find the Table heap row inside the resolved Database --
        let Some(table_handle) = find_table_row(
            ctx.model,
            ctx.heap,
            &main_table.database_fqn,
            &main_table.schema_name,
            &main_table.table_name,
        ) else {
            return;
        };

        // -- allocate the TableAlias and wire it up --
        let alias = ctx.heap.alloc_dynamic(TABLE_ALIAS_CLASSIFIER);
        let _ = ctx.heap.mutate_set(
            &alias,
            "name",
            &[Value::String(main_table.table_name.clone())],
        );
        let _ = ctx
            .heap
            .mutate_set(&alias, "relationalElement", &[Value::Object(table_handle)]);

        let _ = ctx.heap.mutate_set(
            &ctx.instance_handle,
            "mainTableAlias",
            &[Value::Object(alias.clone())],
        );
        // The class-mapping row created by `MappingDSLPopulator`
        // gets the same TableAlias on its mainTableAlias slot so
        // Pure-side navigation `m.classMappings->first().mainTableAlias`
        // works whether the user starts from the parent Mapping or
        // (in a future revision) directly from the sidecar.
        let _ = ctx
            .heap
            .mutate_set(&cm_handle, "mainTableAlias", &[Value::Object(alias)]);
    }
}

fn find_class_mapping_row(
    model: &PureModel,
    heap: &legend_pure_runtime::heap::RuntimeHeap,
    mapping_fqn: &str,
    class_mapping_id: &str,
) -> Option<ObjectHandle> {
    let mapping_id = resolve_fqn(model, mapping_fqn)?;
    let mapping_handle = heap.object_for_element(mapping_id)?;
    let cms = mapping_handle.borrow().get_property_values("classMappings");
    for v in cms.iter() {
        let Value::Object(h) = v else {
            continue;
        };
        let id_values = h.borrow().get_property_values("id");
        let Some(Value::String(s)) = id_values.iter().next().cloned() else {
            continue;
        };
        if s.as_str() == class_mapping_id {
            return Some(h.clone());
        }
    }
    None
}

fn find_table_row(
    model: &PureModel,
    heap: &legend_pure_runtime::heap::RuntimeHeap,
    database_fqn: &str,
    schema_name: &str,
    table_name: &str,
) -> Option<ObjectHandle> {
    let db_id = resolve_fqn(model, database_fqn)?;
    let db_handle = heap.object_for_element(db_id)?;
    let schemas = db_handle.borrow().get_property_values("schemas");
    for s_val in schemas.iter() {
        let Value::Object(s_handle) = s_val else {
            continue;
        };
        let s_name = s_handle.borrow().get_property_values("name");
        let Some(Value::String(s_name_str)) = s_name.iter().next().cloned() else {
            continue;
        };
        if s_name_str.as_str() != schema_name {
            continue;
        }
        let tables = s_handle.borrow().get_property_values("tables");
        for t_val in tables.iter() {
            let Value::Object(t_handle) = t_val else {
                continue;
            };
            let t_name = t_handle.borrow().get_property_values("name");
            let Some(Value::String(t_name_str)) = t_name.iter().next().cloned() else {
                continue;
            };
            if t_name_str.as_str() == table_name {
                return Some(t_handle.clone());
            }
        }
    }
    None
}
