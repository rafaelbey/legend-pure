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
    // Body is now a structured `OpExpr` tree (Stage 2). Smoke-test it
    // by asserting the top-level shape matches the source: a single
    // comparison whose right-hand side is a `{target}.id` column.
    let legend_pure_dsl_relational::ast::OpExpr::Compare {
        op, lhs: _, rhs, ..
    } = &j.body
    else {
        panic!("expected Join body to be a Compare; got {:?}", j.body);
    };
    assert_eq!(*op, legend_pure_dsl_relational::ast::BinOp::Eq);
    let legend_pure_dsl_relational::ast::OpExpr::Column(
        legend_pure_dsl_relational::ast::OpColumn::Target { column, .. },
    ) = rhs.as_ref()
    else {
        panic!("expected RHS to be a {{target}}.id column");
    };
    assert_eq!(column.value.as_str(), "id");

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
fn parses_business_milestoning_spec() {
    use legend_pure_dsl_relational::ast::MilestoneValue;
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeTable (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
            id INT PRIMARY KEY, fromZ DATE, thruZ DATE
          )
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        panic!("expected Table");
    };
    let spec = t
        .milestoning
        .as_ref()
        .expect("expected milestoning spec on table");
    assert_eq!(spec.definitions.len(), 1, "one definition: business");
    let def = &spec.definitions[0];
    assert_eq!(def.kind.value.as_str(), "business");
    assert_eq!(def.fields.len(), 2);
    assert_eq!(def.fields[0].key.value.as_str(), "BUS_FROM");
    assert!(
        matches!(&def.fields[0].value, MilestoneValue::Identifier(s) if s.value.as_str() == "fromZ")
    );
    assert_eq!(def.fields[1].key.value.as_str(), "BUS_THRU");
    assert!(
        matches!(&def.fields[1].value, MilestoneValue::Identifier(s) if s.value.as_str() == "thruZ")
    );
}

#[test]
fn parses_processing_and_business_bi_temporal_spec() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table myTable (
            milestoning (
              processing (PROCESSING_IN=in_z, PROCESSING_OUT=out_z),
              business (BUS_FROM=from_z, BUS_THRU=thru_z)
            )
            aId INT, in_z DATE, out_z DATE, from_z DATE, thru_z DATE
          )
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        panic!("expected Table");
    };
    let spec = t.milestoning.as_ref().expect("milestoning spec");
    assert_eq!(spec.definitions.len(), 2, "bi-temporal definitions");
    assert_eq!(spec.definitions[0].kind.value.as_str(), "processing");
    assert_eq!(spec.definitions[1].kind.value.as_str(), "business");
    assert_eq!(t.columns.len(), 5, "five trailing columns");
}

#[test]
fn parses_business_snapshot_date_spec() {
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table snap (
            milestoning ( business (BUS_SNAPSHOT_DATE=snap_date) )
            id INT PRIMARY KEY, snap_date DATE
          )
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        panic!("expected Table");
    };
    let spec = t.milestoning.as_ref().expect("milestoning spec");
    assert_eq!(spec.definitions[0].fields.len(), 1);
    assert_eq!(
        spec.definitions[0].fields[0].key.value.as_str(),
        "BUS_SNAPSHOT_DATE"
    );
}

#[test]
fn parses_milestoning_with_thru_is_inclusive_and_infinity_date_flags() {
    use legend_pure_dsl_relational::ast::MilestoneValue;
    let source = indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ, THRU_IS_INCLUSIVE=true, INFINITY_DATE=%2999-12-31) )
            id INT PRIMARY KEY, fromZ DATE, thruZ DATE
          )
        )
    "};
    let file = parse(source);
    let db = first_database(&file);
    let DatabaseElement::Table(t) = &db.elements[0] else {
        panic!("expected Table");
    };
    let def = &t.milestoning.as_ref().unwrap().definitions[0];
    assert_eq!(def.fields.len(), 4);
    // Boolean and date values.
    assert!(matches!(
        &def.fields[2].value,
        MilestoneValue::Boolean { value: true, .. }
    ));
    assert!(
        matches!(&def.fields[3].value, MilestoneValue::Date { literal, .. } if literal.as_str() == "%2999-12-31")
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

// ---------------------------------------------------------------------------
// Stage-2 op_operation fixtures
// ---------------------------------------------------------------------------

mod op_operation {
    use super::{first_database, parse};
    use indoc::indoc;
    use legend_pure_dsl_relational::ast::{
        BinOp, BoolOp, DatabaseElement, OpColumn, OpExpr, OpLiteral,
    };

    /// Helper: extract the structured body of the *only* Join element
    /// in the database (panics otherwise).
    fn extract_join_body(source: &str) -> OpExpr {
        let file = parse(source);
        let db = first_database(&file);
        let DatabaseElement::Join(j) = db
            .elements
            .iter()
            .find(|e| matches!(e, DatabaseElement::Join(_)))
            .expect("expected a Join element")
        else {
            unreachable!()
        };
        j.body.clone()
    }

    fn extract_filter_body(source: &str) -> OpExpr {
        let file = parse(source);
        let db = first_database(&file);
        let DatabaseElement::Filter(f) = db
            .elements
            .iter()
            .find(|e| matches!(e, DatabaseElement::Filter(_)))
            .expect("expected a Filter element")
        else {
            unreachable!()
        };
        f.body.clone()
    }

    #[test]
    fn parses_simple_equality() {
        let body = extract_join_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
              Join j (t.id = {target}.id)
            )
        "});
        let OpExpr::Compare { op, lhs, rhs, .. } = body else {
            panic!("expected Compare");
        };
        assert_eq!(op, BinOp::Eq);
        // LHS: t.id (alias = t, scope = [id]).
        let OpExpr::Column(OpColumn::Aliased { alias, scope, .. }) = lhs.as_ref() else {
            panic!("expected aliased column on LHS");
        };
        assert_eq!(alias.value.as_str(), "t");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope[0].value.as_str(), "id");
        // RHS: {target}.id.
        let OpExpr::Column(OpColumn::Target { column, .. }) = rhs.as_ref() else {
            panic!("expected {{target}}.id on RHS");
        };
        assert_eq!(column.value.as_str(), "id");
    }

    #[test]
    fn parses_and_chain() {
        let body = extract_join_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, qty INT, region VARCHAR(2))
              Join j (t.id = {target}.id and t.qty > 0 and t.region = 'US')
            )
        "});
        // `a and b and c` is right-recursive → Bool(and, a, Bool(and, b, c)).
        let OpExpr::Bool {
            op: outer,
            lhs: a,
            rhs: bc,
            ..
        } = body
        else {
            panic!("expected Bool at root");
        };
        assert_eq!(outer, BoolOp::And);
        assert!(matches!(a.as_ref(), OpExpr::Compare { .. }));
        let OpExpr::Bool { op: inner, .. } = bc.as_ref() else {
            panic!("expected nested Bool on RHS");
        };
        assert_eq!(*inner, BoolOp::And);
    }

    #[test]
    fn parses_mixed_and_or_with_explicit_grouping() {
        let body = extract_filter_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, status VARCHAR(2), qty INT)
              Filter f ((t.status = 'A' or t.status = 'B') and t.qty > 0)
            )
        "});
        // Top-level: Bool(and, Group(Bool(or, …, …)), Compare(>, t.qty, 0))
        let OpExpr::Bool { op, lhs, rhs, .. } = body else {
            panic!("expected Bool at root; got {body:?}");
        };
        assert_eq!(op, BoolOp::And);
        assert!(matches!(lhs.as_ref(), OpExpr::Group { .. }));
        assert!(matches!(rhs.as_ref(), OpExpr::Compare { .. }));
    }

    #[test]
    fn parses_is_null_and_is_not_null() {
        let body = extract_filter_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, region VARCHAR(2))
              Filter f (t.region is not null)
            )
        "});
        let OpExpr::IsNull { expr, negated, .. } = body else {
            panic!("expected IsNull at root");
        };
        assert!(negated);
        assert!(matches!(
            expr.as_ref(),
            OpExpr::Column(OpColumn::Aliased { .. })
        ));

        // Negation flag false-path
        let body2 = extract_filter_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, region VARCHAR(2))
              Filter f (t.region is null)
            )
        "});
        let OpExpr::IsNull { negated, .. } = body2 else {
            panic!("expected IsNull at root");
        };
        assert!(!negated);
    }

    #[test]
    fn parses_function_call_with_multiple_args() {
        let body = extract_filter_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, status VARCHAR(2))
              Filter f (concat(t.status, 'X') = 'AX')
            )
        "});
        let OpExpr::Compare { lhs, .. } = body else {
            panic!("expected Compare with function-call LHS");
        };
        let OpExpr::Function { name, args, .. } = lhs.as_ref() else {
            panic!("expected Function on LHS");
        };
        assert_eq!(name.value.as_str(), "concat");
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn parses_string_integer_and_float_literals() {
        let body = extract_filter_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY, name VARCHAR(80), price DECIMAL(10, 2))
              Filter f (t.price > 0)
            )
        "});
        let OpExpr::Compare { rhs, .. } = body else {
            panic!("expected Compare");
        };
        assert!(matches!(
            rhs.as_ref(),
            OpExpr::Literal(OpLiteral::Integer { value: 0, .. })
        ));
    }

    #[test]
    fn parses_all_comparison_operators() {
        for (src, expected) in [
            ("t.x = 0", BinOp::Eq),
            ("t.x > 0", BinOp::Gt),
            ("t.x < 0", BinOp::Lt),
            ("t.x >= 0", BinOp::GtEq),
            ("t.x <= 0", BinOp::LtEq),
            ("t.x != 0", BinOp::NotEq),
            ("t.x <> 0", BinOp::NotEq2),
        ] {
            let source = format!(
                "###Relational\nDatabase pkg::db\n(\n  Table t (x INT PRIMARY KEY)\n  Filter f ({src})\n)\n"
            );
            let body = extract_filter_body(&source);
            let OpExpr::Compare { op, .. } = body else {
                panic!("expected Compare for `{src}`");
            };
            assert_eq!(op, expected, "operator for `{src}`");
        }
    }

    #[test]
    fn parses_primary_key_flag_on_column() {
        let body = extract_join_body(indoc! {r"
            ###Relational
            Database pkg::db
            (
              Table t (id INT PRIMARY KEY)
              Join j ({target}.id PRIMARY KEY = t.id)
            )
        "});
        let OpExpr::Compare { lhs, .. } = body else {
            panic!("expected Compare");
        };
        let OpExpr::Column(OpColumn::Target { primary_key, .. }) = lhs.as_ref() else {
            panic!("expected target column on LHS");
        };
        assert!(*primary_key);
    }
}
