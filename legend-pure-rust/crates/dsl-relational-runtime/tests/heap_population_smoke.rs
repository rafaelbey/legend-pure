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

//! Pilot pin for `RelationalDatabaseDSLPopulator` runtime hydration.
//!
//! Locks the architectural contract: after building an evaluator with
//! `RelationalDatabaseDSLPopulator`, each `###Relational Database`'s
//! heap row exposes `schemas` / `joins` / `filters` /
//! `multiGrainFilters` as populated collections of child rows with
//! the correct M3 classifier and `name` slot.

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_relational_runtime::{
    DATABASE_CLASSIFIER, DEFAULT_SCHEMA_NAME, FILTER_CLASSIFIER, JOIN_CLASSIFIER,
    MULTI_GRAIN_FILTER_CLASSIFIER, RelationalDatabaseDSLPopulator, SCHEMA_CLASSIFIER,
    TABLE_CLASSIFIER,
};
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::value::Value;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "heap_population_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

fn compile(file: SourceFile) -> legend_pure_parser_pure::model::PureModel {
    let extension = RelationalExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts)
        .unwrap_or_else(|p| p.model)
}

/// Find the heap handle for the (single, expected) Database
/// `Element::DSLInstance` in the model. The runtime classifies the
/// row by `metatype_of`, which falls back to `Any` when the M3
/// metamodel for `meta::relational::metamodel::Database` isn't
/// loaded into the test model — so we look up via the element's
/// `ElementId`, not by classifier match.
fn find_database_handle<'m, H: legend_pure_runtime::hooks::EvalHooks>(
    model: &legend_pure_parser_pure::model::PureModel,
    evaluator: &Evaluator<'m, H>,
) -> Option<legend_pure_runtime::heap::ObjectHandle> {
    use legend_pure_parser_pure::ids::ElementId;
    use legend_pure_parser_pure::model::Element;
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::DSLInstance(d) = element else {
                continue;
            };
            if d.dsl_name.as_str() != "RelationalDatabase" {
                continue;
            }
            let eid = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if let Some(h) = evaluator.heap().object_for_element(eid) {
                return Some(h);
            }
        }
    }
    None
}

fn classifier_of(value: &Value) -> Option<String> {
    let Value::Object(handle) = value else {
        return None;
    };
    Some(handle.borrow().classifier().to_string())
}

fn name_of(value: &Value) -> Option<String> {
    let Value::Object(handle) = value else {
        return None;
    };
    let entry = handle.borrow();
    let values = entry.get_property_values("name");
    let first = values.iter().next()?;
    if let Value::String(s) = first {
        Some(s.to_string())
    } else {
        None
    }
}

#[test]
fn database_with_schema_populates_tables_and_views() {
    let source = indoc! {r"
        ###Relational
        Database my::test::PaymentsDb
        (
          Schema main
          (
            Table customers ( id INTEGER PRIMARY KEY, name VARCHAR(100) )
            Table orders    ( id INTEGER PRIMARY KEY, customer_id INTEGER )
            View   active_orders ( id : orders.id )
          )
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = RelationalDatabaseDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);

    let db_handle = find_database_handle(&model, &evaluator).expect("Database heap row");

    let name = db_handle.borrow().get_property_values("name");
    let name_first = name.iter().next().cloned();
    assert!(
        matches!(name_first, Some(Value::String(ref s)) if s.as_str() == "PaymentsDb"),
        "Database name must be the simple-name segment; got {name_first:?}"
    );

    let schemas: Vec<Value> = db_handle
        .borrow()
        .get_property_values("schemas")
        .iter()
        .cloned()
        .collect();
    assert_eq!(
        schemas.len(),
        1,
        "expected exactly one schema; got {}",
        schemas.len()
    );
    assert_eq!(
        classifier_of(&schemas[0]).as_deref(),
        Some(SCHEMA_CLASSIFIER)
    );
    assert_eq!(name_of(&schemas[0]).as_deref(), Some("main"));

    // Drill into schema's tables.
    let Value::Object(schema_handle) = &schemas[0] else {
        panic!("schema entry is not an Object");
    };
    let tables: Vec<Value> = schema_handle
        .borrow()
        .get_property_values("tables")
        .iter()
        .cloned()
        .collect();
    assert_eq!(tables.len(), 2);
    let mut table_names: Vec<String> = tables.iter().filter_map(name_of).collect();
    table_names.sort();
    assert_eq!(
        table_names,
        vec!["customers".to_string(), "orders".to_string()]
    );
    for t in &tables {
        assert_eq!(classifier_of(t).as_deref(), Some(TABLE_CLASSIFIER));
    }

    let views: Vec<Value> = schema_handle
        .borrow()
        .get_property_values("views")
        .iter()
        .cloned()
        .collect();
    assert_eq!(views.len(), 1);
    assert_eq!(name_of(&views[0]).as_deref(), Some("active_orders"));
}

#[test]
fn database_with_default_schema_gathers_top_level_tables() {
    // Top-level tables (declared outside any `Schema` block) must be
    // exposed via a synthetic schema named `"default"` so navigation
    // stays uniform.
    let source = indoc! {r"
        ###Relational
        Database my::test::FlatDb
        (
          Table trades ( id INTEGER PRIMARY KEY )
          Table products ( id INTEGER PRIMARY KEY, name VARCHAR(50) )
          Join trade_product (trades.id = {target}.id)
          Filter active (trades.id > 0)
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = RelationalDatabaseDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);

    let db_handle = find_database_handle(&model, &evaluator).expect("Database heap row");

    let schemas: Vec<Value> = db_handle
        .borrow()
        .get_property_values("schemas")
        .iter()
        .cloned()
        .collect();
    assert_eq!(schemas.len(), 1, "exactly one (default) schema");
    assert_eq!(name_of(&schemas[0]).as_deref(), Some(DEFAULT_SCHEMA_NAME));

    let Value::Object(schema_handle) = &schemas[0] else {
        panic!("default schema entry is not an Object");
    };
    let tables: Vec<Value> = schema_handle
        .borrow()
        .get_property_values("tables")
        .iter()
        .cloned()
        .collect();
    let mut tnames: Vec<String> = tables.iter().filter_map(name_of).collect();
    tnames.sort();
    assert_eq!(tnames, vec!["products".to_string(), "trades".to_string()]);

    let joins: Vec<Value> = db_handle
        .borrow()
        .get_property_values("joins")
        .iter()
        .cloned()
        .collect();
    assert_eq!(joins.len(), 1);
    assert_eq!(classifier_of(&joins[0]).as_deref(), Some(JOIN_CLASSIFIER));
    assert_eq!(name_of(&joins[0]).as_deref(), Some("trade_product"));

    let filters: Vec<Value> = db_handle
        .borrow()
        .get_property_values("filters")
        .iter()
        .cloned()
        .collect();
    assert_eq!(filters.len(), 1);
    assert_eq!(
        classifier_of(&filters[0]).as_deref(),
        Some(FILTER_CLASSIFIER)
    );
    assert_eq!(name_of(&filters[0]).as_deref(), Some("active"));
}

#[test]
fn database_with_multi_grain_filter_populates_separate_collection() {
    let source = indoc! {r"
        ###Relational
        Database my::test::MgfDb
        (
          Table trades ( id INTEGER PRIMARY KEY, month INTEGER )
          MultiGrainFilter byMonth ( trades.month = {target}.month )
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = RelationalDatabaseDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);

    let db_handle = find_database_handle(&model, &evaluator).expect("Database heap row");

    let mgfs: Vec<Value> = db_handle
        .borrow()
        .get_property_values("multiGrainFilters")
        .iter()
        .cloned()
        .collect();
    assert_eq!(mgfs.len(), 1);
    assert_eq!(
        classifier_of(&mgfs[0]).as_deref(),
        Some(MULTI_GRAIN_FILTER_CLASSIFIER)
    );
    assert_eq!(name_of(&mgfs[0]).as_deref(), Some("byMonth"));

    // Plain `filters` collection should be empty.
    let plain_filters: Vec<Value> = db_handle
        .borrow()
        .get_property_values("filters")
        .iter()
        .cloned()
        .collect();
    assert!(
        plain_filters.is_empty(),
        "MultiGrainFilter must not also appear in `filters`; got {plain_filters:?}"
    );

    // Sanity: DATABASE_CLASSIFIER const is what tests outside this
    // crate could match against the Database row in a platform-loaded
    // setup. We don't assert it here (the test model has no
    // platform), but referencing the const keeps the public re-export
    // exercised so it can't silently disappear.
    let _ = DATABASE_CLASSIFIER;
}

#[test]
fn populator_no_op_when_no_database() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Standalone
        {
          field : String[1];
        }
    "};
    let file = parse(source);
    let model = compile(file);

    let populator = RelationalDatabaseDSLPopulator;
    let _evaluator = Evaluator::new_default_with_dsl_populators(&model, &[&populator]);
    // Reaching this line without panic = no-op contract holds.
}
