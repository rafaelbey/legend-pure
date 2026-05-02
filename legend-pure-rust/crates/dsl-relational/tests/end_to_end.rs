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

//! End-to-end tests for the Relational DSL.
//!
//! 1. **Stage-0a sanity** (`relational_metamodel_classes_resolve_in_loaded_platform`):
//!    embedded `platform_store_relational` metamodel classes resolve
//!    as Class elements through `load_platform()`. Mirrors the
//!    crate-internal smoke test in `legend-pure-runtime/tests/eval_tests.rs`
//!    but locks the metamodel from a perspective inside dsl-relational
//!    so a downstream consumer can exercise the embedding path
//!    without depending on runtime test code.
//!
//! 2. **Stage-1 user-source parsing** (`user_relational_block_parses_against_loaded_platform`):
//!    a relational source taken from one of the upstream Java
//!    fixtures parses cleanly with `RelationalSectionParser` and
//!    round-trips through the composer. The platform model is
//!    available alongside, so a future Stage-4 validator can
//!    consult it without rewiring the test.

use indoc::indoc;
use legend_pure_dsl_relational::ast::{DatabaseDef, DatabaseElement};
use legend_pure_dsl_relational::compose::compose_relational_section;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_pure::model::Element as ModelElement;
use smol_str::SmolStr;

#[test]
fn relational_metamodel_classes_resolve_in_loaded_platform() {
    let model = match legend_pure_core_platform::platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    for fqn in [
        ["meta", "relational", "metamodel", "Database"].as_slice(),
        ["meta", "relational", "metamodel", "Schema"].as_slice(),
        ["meta", "relational", "metamodel", "relation", "Table"].as_slice(),
        ["meta", "relational", "metamodel", "relation", "View"].as_slice(),
        ["meta", "relational", "metamodel", "join", "Join"].as_slice(),
        ["meta", "relational", "metamodel", "Filter"].as_slice(),
        ["meta", "relational", "metamodel", "Column"].as_slice(),
        ["meta", "relational", "mapping", "RelationalPropertyMapping"].as_slice(),
    ] {
        let segments: Vec<SmolStr> = fqn.iter().map(|s| SmolStr::new(*s)).collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("relational metamodel class missing: {fqn:?}"));
        assert!(
            matches!(model.get_element(id), ModelElement::Class(_)),
            "expected {fqn:?} to be a Class element"
        );
    }
}

#[test]
fn user_relational_block_parses_against_loaded_platform() {
    // Source taken near-verbatim from the upstream Java fixture
    // `legend-pure-store-relational/.../TestCyclicMappingIncludeInMappingHierarchy.java`'s
    // `STORE_SOURCE_CODE`, with a few extra body elements added so
    // we exercise Schema, Join, and Filter on the round-trip.
    let user_source = indoc! {r"
        ###Relational
        Database test::ADatabase
        (
          Schema sales
          (
            Table tradeTable (id INT PRIMARY KEY, prodId INT, qty FLOAT(10, 2))
            Table productTable (id INT PRIMARY KEY, name VARCHAR(200))
          )

          Join tradeProduct (tradeTable.prodId = productTable.id)
          Filter activeOnly (tradeTable.qty > 0)
        )
    "};

    // Loading the platform makes the metamodel reachable. We don't
    // run any compile-time validators against the user source today
    // (Stage 4 ports the validators); that load is here so the test
    // shape is the same in Stages 4+ — the only diff later will be
    // wiring the future RelationalExtension into compile_with_extensions.
    let _platform = match legend_pure_core_platform::platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    // Parse the user source via parse_with_sections + the relational plug-in.
    let result = legend_pure_parser_parser::parse_with_sections(
        user_source,
        "user_relational.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    let file = result.expect("user relational source must parse cleanly");

    // Confirm the structural shape.
    let dbs = collect_databases(&file);
    assert_eq!(dbs.len(), 1, "expected one Database");
    let db = dbs[0];
    assert_eq!(db.name.value.as_str(), "ADatabase");
    let pkg_segments: Vec<&str> = db
        .package
        .as_ref()
        .map(|p| p.segments().iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(pkg_segments, vec!["test"], "package qualifier mismatch");
    assert_eq!(db.elements.len(), 3, "Schema + Join + Filter");
    let DatabaseElement::Schema(s) = &db.elements[0] else {
        panic!("expected Schema at index 0");
    };
    assert_eq!(s.tables.len(), 2);
    assert!(matches!(db.elements[1], DatabaseElement::Join(_)));
    assert!(matches!(db.elements[2], DatabaseElement::Filter(_)));

    // Round-trip: composing then re-parsing yields the same AST
    // shape (modulo source spans / token whitespace).
    let composed = compose_relational_section(&dbs);
    let result2 = legend_pure_parser_parser::parse_with_sections(
        &composed,
        "user_relational_round_trip.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    let file2 = result2.expect("composed relational source must round-trip");
    let dbs2 = collect_databases(&file2);
    assert_eq!(dbs2.len(), dbs.len(), "round-trip database count");
    assert_eq!(
        dbs2[0].elements.len(),
        db.elements.len(),
        "round-trip element count"
    );
}

fn collect_databases(file: &SourceFile) -> Vec<&DatabaseDef> {
    let mut out = Vec::new();
    for section in &file.sections {
        if section.kind.as_str() != "Relational" {
            continue;
        }
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem {
                if let Some(db) = boxed.as_any().downcast_ref::<DatabaseDef>() {
                    out.push(db);
                }
            }
        }
    }
    out
}
