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

//! Stage-1 composer round-trip:
//! `parse(compose(db)) == db` modulo `source_info` fields.
//!
//! Verbatim op-bodies / view-bodies (Stage 1 captures these as
//! [`TokenSlice`](legend_pure_dsl_relational::ast::TokenSlice)s) are
//! re-emitted with single-space separation between tokens — the
//! *tokens* round-trip but original whitespace doesn't, matching
//! the `feedback_token_survival_round_trip` rule.

use indoc::indoc;
use legend_pure_dsl_relational::ast::{DatabaseDef, DatabaseElement};
use legend_pure_dsl_relational::compose::compose_relational_section;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    );
    match result {
        Ok(file) => file,
        Err(p) => panic!(
            "Expected parse to succeed for {name}, got error(s): {:?}",
            p.errors
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        ),
    }
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

fn strip_source_info(s: &str) -> String {
    // Match `source_info: SourceInfo { … }` blocks across line breaks
    // and replace them with a fixed marker, so two ASTs that differ
    // only in source spans compare equal.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, _)) = chars.peek().copied() {
        if s[i..].starts_with("source_info:") {
            // Skip the ident, locate the next `{`, then walk balanced braces.
            let after_ident = i + "source_info:".len();
            let rest = &s[after_ident..];
            let Some(brace) = rest.find('{') else {
                break;
            };
            out.push_str("source_info: <stripped>");
            let mut depth = 1usize;
            let mut idx = after_ident + brace + 1;
            while depth > 0 && idx < s.len() {
                let c = s.as_bytes()[idx] as char;
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                }
                idx += 1;
            }
            // Advance the iterator past idx.
            while let Some(&(j, _)) = chars.peek() {
                if j >= idx {
                    break;
                }
                chars.next();
            }
            continue;
        }
        let (_, c) = chars.next().expect("checked above");
        out.push(c);
    }
    out
}

fn assert_round_trip(source: &str) {
    let file1 = parse("round_trip_1.pure", source);
    let dbs1 = collect_databases(&file1);
    let composed = compose_relational_section(&dbs1);

    let file2 = parse("round_trip_2.pure", &composed);
    let dbs2 = collect_databases(&file2);

    assert_eq!(
        dbs1.len(),
        dbs2.len(),
        "database count mismatch after round-trip: composed=\n{composed}"
    );
    for (db1, db2) in dbs1.iter().zip(dbs2.iter()) {
        assert_eq!(
            strip_source_info(&format!("{db1:#?}")),
            strip_source_info(&format!("{db2:#?}")),
            "database structure diverged after round-trip\ncomposed:\n{composed}"
        );
    }
}

#[test]
fn round_trip_minimal_database() {
    let source = indoc! {r"
        ###Relational
        Database pkg::myDb
        (
          Table tradeTable (id INT PRIMARY KEY, name VARCHAR(200))
        )
    "};
    assert_round_trip(source);
}

#[test]
fn round_trip_decimal_columns() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table prices (price DECIMAL(10, 2) NOT NULL, qty FLOAT(8))
        )
    "};
    assert_round_trip(source);
}

#[test]
fn round_trip_schema_with_tables_and_view() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema sales
          (
            Table tradeTable (id INT PRIMARY KEY, qty FLOAT(10, 2))
            View activeTrades (~distinct quantity : tradeTable.qty)
          )
        )
    "};
    assert_round_trip(source);
}

#[test]
fn round_trip_join_filter_multigrain() {
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
    assert_round_trip(source);
}

#[test]
fn round_trip_includes_before_elements() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          include other::baseDb
          include third::moreDb

          Table localTable (id INT PRIMARY KEY)
        )
    "};
    assert_round_trip(source);
}

#[test]
fn round_trip_token_survival_in_op_body() {
    // Locks the token-survival contract for verbatim op-bodies:
    // every input token must appear in the composed output. Stage 1
    // only guarantees token survival, not whitespace fidelity.
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, prodId INT, name VARCHAR(80))
          Join j (t.prodId = {target}.id and t.name = {target}.label)
        )
    "};
    let file = parse("token_survival.pure", source);
    let dbs = collect_databases(&file);
    let composed = compose_relational_section(&dbs);
    // The composer glues `Ident . Ident` (no space between them) so
    // `t.prodId` clusters survive verbatim. The `{target}` form
    // tokenizes as three lexer tokens (`{`, `target`, `}`) — the
    // generic Stage-1 token-spacing policy doesn't know that
    // pattern is meant to glue, so it emits `{ target }`. Stage 2
    // treats `{target}` as a single op-grammar atom and will
    // restore the glued form.
    for tok in [
        "t.prodId",
        "{ target }.id",
        "and",
        "t.name",
        "{ target }.label",
    ] {
        assert!(
            composed.contains(tok),
            "expected composed output to contain token-cluster '{tok}'\ncomposed:\n{composed}"
        );
    }

    // Re-parse the composed output and confirm the token slice in
    // the join body has the same length as the original — this is
    // the core token-survival guarantee.
    let file2 = parse("token_survival_2.pure", &composed);
    let dbs2 = collect_databases(&file2);
    let dbs1_again = collect_databases(&file);
    let DatabaseElement::Join(j1) = &dbs1_again[0].elements[1] else {
        panic!("expected Join at index 1");
    };
    let DatabaseElement::Join(j2) = &dbs2[0].elements[1] else {
        panic!("expected Join at index 1 after round-trip");
    };
    assert_eq!(
        j1.op_body.tokens.len(),
        j2.op_body.tokens.len(),
        "join op-body token count must survive round-trip"
    );
    for (t1, t2) in j1.op_body.tokens.iter().zip(j2.op_body.tokens.iter()) {
        assert_eq!(t1.kind, t2.kind, "token kind divergence");
        assert_eq!(t1.text, t2.text, "token text divergence");
    }
}
