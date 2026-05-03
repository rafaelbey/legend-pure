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
    // Stage 2 upgrades the round-trip contract from "every input
    // token appears in the composed output" to "the composed output
    // re-parses to the same `OpExpr` AST". Stronger guarantee — locks
    // the structural sub-grammar in addition to token preservation.
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

    // Surface-level: the structurally-aware composer glues `{target}`
    // back into a single token cluster (Stage 1's generic spacing
    // policy emitted `{ target }` — Stage 2 fixes this).
    for tok in ["t.prodId", "{target}.id", "and", "t.name", "{target}.label"] {
        assert!(
            composed.contains(tok),
            "expected composed output to contain token-cluster '{tok}'\ncomposed:\n{composed}"
        );
    }

    // Re-parse the composed output and confirm the join body parses
    // to the same OpExpr shape (modulo source-info spans, which the
    // parser re-derives from the new positions).
    let file2 = parse("token_survival_2.pure", &composed);
    let dbs2 = collect_databases(&file2);
    let dbs1_again = collect_databases(&file);
    let DatabaseElement::Join(j1) = &dbs1_again[0].elements[1] else {
        panic!("expected Join at index 1");
    };
    let DatabaseElement::Join(j2) = &dbs2[0].elements[1] else {
        panic!("expected Join at index 1 after round-trip");
    };
    assert!(
        op_expr_structurally_eq(&j1.body, &j2.body),
        "join body OpExpr must survive round-trip\noriginal: {:?}\nround-tripped: {:?}",
        j1.body,
        j2.body
    );
}

// ---------------------------------------------------------------------------
// Stage-2 op-grammar round-trip fixtures.
//
// Each test composes the source, re-parses, and asserts the AST
// structure (stripped of source spans) matches — the Stage-2 form of
// `feedback_token_survival_round_trip`, lifted from token survival
// to AST survival now that we have a structured op_operation tree.
// ---------------------------------------------------------------------------

#[test]
fn round_trip_op_simple_equality() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Join j (t.id = {target}.id)
        )
    "});
}

#[test]
fn round_trip_op_and_or_with_grouping() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, status VARCHAR(2), qty INT)
          Filter f ((t.status = 'A' or t.status = 'B') and t.qty > 0)
        )
    "});
}

#[test]
fn round_trip_op_is_null_and_not_null() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, region VARCHAR(2))
          Filter active (t.region is not null)
          Filter inactive (t.region is null)
        )
    "});
}

#[test]
fn round_trip_op_function_call_arg_list() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, status VARCHAR(2))
          Filter f (concat(t.status, 'X') = 'AX')
        )
    "});
}

#[test]
fn round_trip_op_all_comparison_operators() {
    // One Filter per operator to lock surface-level spelling. Single
    // round-trip exercise covers all 7 comparison forms.
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (x INT PRIMARY KEY)
          Filter eq (t.x = 0)
          Filter gt (t.x > 0)
          Filter lt (t.x < 0)
          Filter gte (t.x >= 0)
          Filter lte (t.x <= 0)
          Filter neq (t.x != 0)
          Filter neq2 (t.x <> 0)
        )
    "});
}

#[test]
fn round_trip_op_string_literal_and_negative_integer() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, status VARCHAR(2), offset INT)
          Filter active (t.status = 'A')
          Filter offset (t.offset = -1)
        )
    "});
}

// ---------------------------------------------------------------------------
// Stage-3 milestoning round-trip fixtures.
// ---------------------------------------------------------------------------

#[test]
fn round_trip_business_milestoning() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
            id INT PRIMARY KEY, fromZ DATE, thruZ DATE
          )
        )
    "});
}

#[test]
fn round_trip_processing_milestoning() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning ( processing (PROCESSING_IN=in_z, PROCESSING_OUT=out_z) )
            id INT PRIMARY KEY, in_z DATE, out_z DATE
          )
        )
    "});
}

#[test]
fn round_trip_bi_temporal_with_inclusive_flags() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning (
              processing (PROCESSING_IN=in_z, PROCESSING_OUT=out_z, OUT_IS_INCLUSIVE=true),
              business (BUS_FROM=from_z, BUS_THRU=thru_z, INFINITY_DATE=%2999-12-31)
            )
            id INT PRIMARY KEY, in_z DATE, out_z DATE, from_z DATE, thru_z DATE
          )
        )
    "});
}

#[test]
fn round_trip_snapshot_milestoning() {
    assert_round_trip(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table snap (
            milestoning ( business (BUS_SNAPSHOT_DATE=snap_date) )
            id INT PRIMARY KEY, snap_date DATE
          )
        )
    "});
}

/// Structural equality on `OpExpr` ignoring `source_info`. Used by
/// the Stage-2 round-trip tests where the composed output's source
/// positions necessarily differ from the original.
fn op_expr_structurally_eq(
    a: &legend_pure_dsl_relational::ast::OpExpr,
    b: &legend_pure_dsl_relational::ast::OpExpr,
) -> bool {
    use legend_pure_dsl_relational::ast::{OpColumn, OpExpr, OpLiteral};
    match (a, b) {
        (
            OpExpr::Bool {
                op: oa,
                lhs: la,
                rhs: ra,
                ..
            },
            OpExpr::Bool {
                op: ob,
                lhs: lb,
                rhs: rb,
                ..
            },
        ) => oa == ob && op_expr_structurally_eq(la, lb) && op_expr_structurally_eq(ra, rb),
        (
            OpExpr::Compare {
                op: oa,
                lhs: la,
                rhs: ra,
                ..
            },
            OpExpr::Compare {
                op: ob,
                lhs: lb,
                rhs: rb,
                ..
            },
        ) => oa == ob && op_expr_structurally_eq(la, lb) && op_expr_structurally_eq(ra, rb),
        (
            OpExpr::IsNull {
                expr: ea,
                negated: na,
                ..
            },
            OpExpr::IsNull {
                expr: eb,
                negated: nb,
                ..
            },
        ) => na == nb && op_expr_structurally_eq(ea, eb),
        (OpExpr::Group { inner: ia, .. }, OpExpr::Group { inner: ib, .. }) => {
            op_expr_structurally_eq(ia, ib)
        }
        (
            OpExpr::Function {
                name: na,
                args: aa,
                db: dba,
                ..
            },
            OpExpr::Function {
                name: nb,
                args: ab,
                db: dbb,
                ..
            },
        ) => {
            na.value == nb.value
                && aa.len() == ab.len()
                && aa
                    .iter()
                    .zip(ab)
                    .all(|(x, y)| op_expr_structurally_eq(x, y))
                && dba.as_ref().map(|p| p.name.clone()) == dbb.as_ref().map(|p| p.name.clone())
        }
        (OpExpr::Column(ca), OpExpr::Column(cb)) => match (ca, cb) {
            (
                OpColumn::Target {
                    column: ca,
                    primary_key: pa,
                    ..
                },
                OpColumn::Target {
                    column: cb,
                    primary_key: pb,
                    ..
                },
            ) => ca.value == cb.value && pa == pb,
            (
                OpColumn::Aliased {
                    alias: aa,
                    scope: sa,
                    primary_key: pa,
                    db: dba,
                    ..
                },
                OpColumn::Aliased {
                    alias: ab,
                    scope: sb,
                    primary_key: pb,
                    db: dbb,
                    ..
                },
            ) => {
                aa.value == ab.value
                    && pa == pb
                    && sa.len() == sb.len()
                    && sa.iter().zip(sb).all(|(x, y)| x.value == y.value)
                    && dba.as_ref().map(|p| p.name.clone()) == dbb.as_ref().map(|p| p.name.clone())
            }
            _ => false,
        },
        (OpExpr::Literal(la), OpExpr::Literal(lb)) => match (la, lb) {
            (OpLiteral::String { value: a, .. }, OpLiteral::String { value: b, .. }) => a == b,
            (OpLiteral::Integer { value: a, .. }, OpLiteral::Integer { value: b, .. }) => a == b,
            (OpLiteral::Float { value: a, .. }, OpLiteral::Float { value: b, .. }) => {
                (a - b).abs() < f64::EPSILON
            }
            _ => false,
        },
        _ => false,
    }
}
