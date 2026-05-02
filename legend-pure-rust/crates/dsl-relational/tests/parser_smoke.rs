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

//! Stage-1 parser smoke: minimal `###Relational` blocks parse into
//! the expected AST shape via `parse_with_sections` +
//! `RelationalSectionParser`.

use indoc::indoc;
use legend_pure_dsl_relational::ast::{ColumnDef, DatabaseDef, DatabaseElement, Table, View};
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

fn parse(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "parser_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    match result {
        Ok(file) => file,
        Err(p) => panic!(
            "Expected parse to succeed, but got error(s): {:?}",
            p.errors
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        ),
    }
}

fn first_database(file: &SourceFile) -> &DatabaseDef {
    let section = file
        .sections
        .iter()
        .find(|s| s.kind.as_str() == "Relational")
        .expect("expected one ###Relational section");
    let AstElement::DSLElement(boxed) = section
        .elements
        .first()
        .expect("expected one database element")
    else {
        panic!("expected DSLElement");
    };
    boxed
        .as_any()
        .downcast_ref::<DatabaseDef>()
        .expect("expected DatabaseDef")
}

#[test]
fn parses_minimal_database_with_one_table() {
    let source = indoc! {r"
        ###Relational
        Database pkg::myDb
        (
          Table tradeTable (id INT PRIMARY KEY, name VARCHAR(200))
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    assert_eq!(db.name.value.as_str(), "myDb");
    assert!(db.includes.is_empty());
    assert_eq!(db.elements.len(), 1);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        panic!("expected Table at index 0, got {:?}", db.elements[0]);
    };
    assert_table(
        t,
        "tradeTable",
        &[
            ("id", "INT", None, None, true, false),
            ("name", "VARCHAR", Some(200), None, false, false),
        ],
    );
}

#[test]
fn parses_decimal_with_precision_scale() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table prices (price DECIMAL(10, 2) NOT NULL)
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        unreachable!();
    };
    assert_table(
        t,
        "prices",
        &[("price", "DECIMAL", Some(10), Some(2), false, true)],
    );
}

#[test]
fn parses_schema_block_with_tables_and_views() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema sales
          (
            Table tradeTable (id INT PRIMARY KEY)
            View activeTrades (~distinct quantity : tradeTable.qty)
          )
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    assert_eq!(db.elements.len(), 1);
    let DatabaseElement::Schema(s) = &db.elements[0] else {
        panic!("expected Schema");
    };
    assert_eq!(s.name.value.as_str(), "sales");
    assert_eq!(s.tables.len(), 1);
    assert_eq!(s.tables[0].name.value.as_str(), "tradeTable");
    assert_eq!(s.views.len(), 1);
    assert_view_token_count_at_least(&s.views[0], "activeTrades", 5);
}

#[test]
fn parses_join_filter_multigrain_at_top_level() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeTable (id INT PRIMARY KEY, prodId INT, status VARCHAR(1))
          Join tradeProduct (tradeTable.prodId = {target}.id)
          Filter active (tradeTable.status = 'A')
          MultiGrainFilter byStatus (tradeTable.status = 'A')
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    assert_eq!(db.elements.len(), 4);

    let DatabaseElement::Join(j) = &db.elements[1] else {
        panic!("expected Join at index 1");
    };
    assert_eq!(j.name.value.as_str(), "tradeProduct");
    assert!(!j.op_body.tokens.is_empty());

    let DatabaseElement::Filter(f) = &db.elements[2] else {
        panic!("expected Filter at index 2");
    };
    assert_eq!(f.name.value.as_str(), "active");

    let DatabaseElement::MultiGrainFilter(m) = &db.elements[3] else {
        panic!("expected MultiGrainFilter at index 3");
    };
    assert_eq!(m.name.value.as_str(), "byStatus");
}

#[test]
fn parses_includes_before_elements() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          include other::baseDb
          include third::moreDb

          Table localTable (id INT PRIMARY KEY)
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    assert_eq!(db.includes.len(), 2);
    assert_eq!(db.includes[0].included.name.as_str(), "baseDb");
    assert_eq!(db.includes[1].included.name.as_str(), "moreDb");
    assert_eq!(db.elements.len(), 1);
}

#[test]
fn rejects_include_after_first_element() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          include other::lateDb
        )
    "};
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "bad_include.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    let partial = result.expect_err("expected error: include after table");
    let msgs: Vec<String> = partial.errors.iter().map(ToString::to_string).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("`include` directives must precede")),
        "expected ordering diagnostic, got {msgs:?}"
    );
}

#[test]
fn rejects_milestoning_specs_with_stage3_pointer() {
    // Stage 3 ships milestoning; for now reject early with a pointed
    // message rather than mis-parsing.
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeTable (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
            id INT PRIMARY KEY
          )
        )
    "};
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "milestoned.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    let partial = result.expect_err("milestoning unsupported in Stage 1");
    let msgs: Vec<String> = partial.errors.iter().map(ToString::to_string).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("milestoning") && m.contains("Stage 3")),
        "expected milestoning-deferred diagnostic, got {msgs:?}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn assert_table(
    t: &Table,
    expected_name: &str,
    expected_cols: &[(&str, &str, Option<i64>, Option<i64>, bool, bool)],
) {
    assert_eq!(t.name.value.as_str(), expected_name, "table name");
    assert_eq!(t.columns.len(), expected_cols.len(), "column count");
    for (i, (col, exp)) in t.columns.iter().zip(expected_cols.iter()).enumerate() {
        assert_column(col, *exp, i);
    }
}

fn assert_column(
    c: &ColumnDef,
    exp: (&str, &str, Option<i64>, Option<i64>, bool, bool),
    idx: usize,
) {
    let (name, ty, size, scale, pk, nn) = exp;
    assert_eq!(c.name.value.as_str(), name, "col[{idx}] name");
    assert_eq!(c.type_name.value.as_str(), ty, "col[{idx}] type");
    assert_eq!(c.size, size, "col[{idx}] size");
    assert_eq!(c.scale, scale, "col[{idx}] scale");
    assert_eq!(c.primary_key, pk, "col[{idx}] PK");
    assert_eq!(c.not_null, nn, "col[{idx}] NOT NULL");
}

fn assert_view_token_count_at_least(v: &View, expected_name: &str, min_tokens: usize) {
    assert_eq!(v.name.value.as_str(), expected_name);
    assert!(
        v.body.tokens.len() >= min_tokens,
        "view body should capture at least {min_tokens} tokens; got {}",
        v.body.tokens.len()
    );
}
