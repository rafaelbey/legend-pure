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

//! End-to-end pin for the `mainTableAlias` wiring across the three
//! populator commits (T1.3 commits 1 + 2 + 3).
//!
//! Compiles a fixture with a `###Mapping` containing a `: Relational
//! { ~mainTable [db]Schema.Table … }` body and a `###Relational
//! Database` declaring that Schema + Table. Constructs an `Evaluator`
//! with all three populators wired in slice order; asserts that
//! walking the chain
//!
//!   `<Mapping>.classMappings -> first -> mainTableAlias.relationalElement.name`
//!
//! returns the source table name. This is the load-bearing
//! Java-parity contract for downstream relational plan generation /
//! SQL emission.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::{
    RelationalClassMappingBodyParser, RelationalSectionParser,
};
use legend_pure_dsl_relational_runtime::{
    RelationalClassMappingDSLPopulator, RelationalDatabaseDSLPopulator, TABLE_ALIAS_CLASSIFIER,
    TABLE_CLASSIFIER,
};
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::heap::ObjectHandle;
use legend_pure_runtime::value::Value;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "main_table_wiring_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![
            Box::new(MappingSectionParser::with_body_parsers(vec![Box::new(
                RelationalClassMappingBodyParser,
            )])),
            Box::new(RelationalSectionParser),
        ],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

fn compile(file: SourceFile) -> legend_pure_parser_pure::model::PureModel {
    let mapping = MappingExtension::new();
    let relational = RelationalExtension::new();
    let exts: [&dyn CompilerExtension; 2] = [&mapping, &relational];
    legend_pure_parser_pure::pipeline::compile_with_extensions(&[file], &[], &exts)
        .unwrap_or_else(|p| p.model)
}

fn find_mapping_handle<'m, H: legend_pure_runtime::hooks::EvalHooks>(
    model: &legend_pure_parser_pure::model::PureModel,
    evaluator: &Evaluator<'m, H>,
) -> Option<ObjectHandle> {
    use legend_pure_parser_pure::ids::ElementId;
    use legend_pure_parser_pure::model::Element;
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::DSLInstance(d) = element else {
                continue;
            };
            if d.dsl_name.as_str() != "Mapping" {
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

fn first_object(handle: &ObjectHandle, prop: &str) -> Option<ObjectHandle> {
    let values = handle.borrow().get_property_values(prop);
    let first = values.iter().next().cloned()?;
    if let Value::Object(h) = first {
        Some(h)
    } else {
        None
    }
}

fn name_of(handle: &ObjectHandle) -> Option<String> {
    let values = handle.borrow().get_property_values("name");
    let first = values.iter().next().cloned()?;
    if let Value::String(s) = first {
        Some(s.to_string())
    } else {
        None
    }
}

#[test]
fn main_table_alias_wires_into_table_row_under_schema() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          legalName : String[1];
        }

        ###Relational
        Database my::test::FirmDb
        (
          Schema main
          (
            Table firms ( id INTEGER PRIMARY KEY, legal_name VARCHAR(100) )
          )
        )

        ###Mapping
        Mapping my::test::FirmMapping
        (
          *my::test::Firm[firm_set] : Relational
          {
            ~mainTable [my::test::FirmDb]main.firms
            (legalName : [my::test::FirmDb]main.firms.legal_name)
          }
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let mapping_pop = MappingDSLPopulator;
    let db_pop = RelationalDatabaseDSLPopulator;
    let cm_pop = RelationalClassMappingDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(
        &model,
        &[&mapping_pop, &db_pop, &cm_pop],
    );

    let mapping_handle = find_mapping_handle(&model, &evaluator).expect("Mapping heap row");

    let cm = first_object(&mapping_handle, "classMappings")
        .expect("Mapping.classMappings[0] must exist");

    let alias = first_object(&cm, "mainTableAlias")
        .expect("RootRelationalInstanceSetImplementation.mainTableAlias must be populated");
    assert_eq!(
        alias.borrow().classifier().to_string(),
        TABLE_ALIAS_CLASSIFIER,
        "mainTableAlias must use the TableAlias classifier"
    );
    assert_eq!(
        name_of(&alias).as_deref(),
        Some("firms"),
        "TableAlias.name should mirror the source table name"
    );

    let table = first_object(&alias, "relationalElement")
        .expect("TableAlias.relationalElement must point at the Table heap row");
    assert_eq!(
        table.borrow().classifier().to_string(),
        TABLE_CLASSIFIER,
        "relationalElement must classify as Table"
    );
    assert_eq!(
        name_of(&table).as_deref(),
        Some("firms"),
        "relationalElement should resolve to the firms table"
    );
}

#[test]
fn main_table_alias_resolves_top_level_table_via_default_schema() {
    // `~mainTable [db]firms` (no schema qualifier) — schema defaults
    // to `"default"`. The Database populator gathered the top-level
    // table under the synthetic `"default"` schema; the class-mapping
    // populator must follow the same convention.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }

        ###Relational
        Database my::test::FirmDb
        (
          Table firms ( id INTEGER PRIMARY KEY, legal_name VARCHAR(100) )
        )

        ###Mapping
        Mapping my::test::FirmMapping
        (
          *my::test::Firm[firm_set] : Relational
          {
            ~mainTable [my::test::FirmDb]firms
            (legalName : [my::test::FirmDb]firms.legal_name)
          }
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let mapping_pop = MappingDSLPopulator;
    let db_pop = RelationalDatabaseDSLPopulator;
    let cm_pop = RelationalClassMappingDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(
        &model,
        &[&mapping_pop, &db_pop, &cm_pop],
    );

    let mapping_handle = find_mapping_handle(&model, &evaluator).expect("Mapping heap row");
    let cm = first_object(&mapping_handle, "classMappings").expect("class mapping");
    let alias = first_object(&cm, "mainTableAlias").expect("mainTableAlias populated");
    let table =
        first_object(&alias, "relationalElement").expect("relationalElement resolves to Table");
    assert_eq!(name_of(&table).as_deref(), Some("firms"));
}

#[test]
fn missing_database_leaves_main_table_alias_empty() {
    // Class-mapping references a database that doesn't exist in the
    // model. The populator must stay silent (no panic, no partial
    // wiring) so subsequent validators can surface the unresolved-db
    // diagnostic without runtime cascade.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { legalName : String[1]; }

        ###Mapping
        Mapping my::test::FirmMapping
        (
          *my::test::Firm[firm_set] : Relational
          {
            ~mainTable [my::missing::Db]firms
            (legalName : [my::missing::Db]firms.legal_name)
          }
        )
    "};
    let file = parse(source);
    let model = compile(file);

    let mapping_pop = MappingDSLPopulator;
    let db_pop = RelationalDatabaseDSLPopulator;
    let cm_pop = RelationalClassMappingDSLPopulator;
    let evaluator = Evaluator::new_default_with_dsl_populators(
        &model,
        &[&mapping_pop, &db_pop, &cm_pop],
    );

    let mapping_handle = find_mapping_handle(&model, &evaluator).expect("Mapping heap row");
    let cm = first_object(&mapping_handle, "classMappings").expect("class mapping");
    // mainTableAlias slot stays empty when the database doesn't resolve.
    assert!(
        cm.borrow().get_property_values("mainTableAlias").is_empty(),
        "mainTableAlias must stay empty when [db] doesn't resolve"
    );
}
