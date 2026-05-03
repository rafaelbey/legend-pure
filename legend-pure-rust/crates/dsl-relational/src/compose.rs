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

//! Composer: emit `###Relational` source from [`DatabaseDef`] AST.
//!
//! Round-trip contract: `parse(compose(db)) == db` modulo
//! `source_info` fields and modulo per-token whitespace inside
//! captured op-bodies / view-bodies (Stage 1 stores those as
//! [`TokenSlice`](crate::ast::TokenSlice)s and re-emits with
//! single-space separation — the *tokens* round-trip but the
//! original whitespace doesn't). Verified by `tests/compose_smoke.rs`.

use crate::ast::{
    ColumnDef, DatabaseDef, DatabaseElement, DatabaseInclude, Filter, Join, MultiGrainFilter,
    OpColumn, OpExpr, OpLiteral, Schema, Table, TokenSlice, View,
};
use legend_pure_parser_ast::annotation::PackageableElementPtr;

/// Compose one `Database` declaration as a string. The output does
/// not include the `###Relational` section header; use
/// [`compose_relational_section`] to wrap one or more databases in a
/// section.
#[must_use]
pub fn compose_database(db: &DatabaseDef) -> String {
    let mut out = String::new();
    write_database(&mut out, db);
    out
}

/// Compose a `###Relational` section containing the given databases,
/// in source order.
#[must_use]
pub fn compose_relational_section(dbs: &[&DatabaseDef]) -> String {
    let mut out = String::from("###Relational\n");
    for db in dbs {
        write_database(&mut out, db);
        out.push('\n');
    }
    out
}

fn write_database(out: &mut String, db: &DatabaseDef) {
    out.push_str("Database ");
    if let Some(pkg) = &db.package {
        for seg in pkg.segments() {
            out.push_str(seg.as_str());
            out.push_str("::");
        }
    }
    out.push_str(db.name.value.as_str());
    out.push_str("\n(\n");
    for inc in &db.includes {
        write_include(out, inc);
    }
    if !db.includes.is_empty() && !db.elements.is_empty() {
        out.push('\n');
    }
    for elem in &db.elements {
        write_element(out, elem);
    }
    out.push_str(")\n");
}

fn write_include(out: &mut String, inc: &DatabaseInclude) {
    out.push_str("  include ");
    if let Some(pkg) = &inc.included.package {
        for seg in pkg.segments() {
            out.push_str(seg.as_str());
            out.push_str("::");
        }
    }
    out.push_str(inc.included.name.as_str());
    out.push('\n');
}

fn write_element(out: &mut String, elem: &DatabaseElement) {
    match elem {
        DatabaseElement::Schema(s) => write_schema(out, s),
        DatabaseElement::Table(t) => write_table(out, t, "  "),
        DatabaseElement::View(v) => write_view(out, v, "  "),
        DatabaseElement::Join(j) => write_join(out, j),
        DatabaseElement::Filter(f) => write_filter(out, f),
        DatabaseElement::MultiGrainFilter(m) => write_multi_grain_filter(out, m),
    }
    out.push('\n');
}

fn write_schema(out: &mut String, s: &Schema) {
    out.push_str("  Schema ");
    out.push_str(s.name.value.as_str());
    out.push_str("\n  (\n");
    for t in &s.tables {
        write_table(out, t, "    ");
        out.push('\n');
    }
    for v in &s.views {
        write_view(out, v, "    ");
        out.push('\n');
    }
    out.push_str("  )");
}

fn write_table(out: &mut String, t: &Table, indent: &str) {
    out.push_str(indent);
    out.push_str("Table ");
    out.push_str(t.name.value.as_str());
    out.push_str(" (");
    for (i, c) in t.columns.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_column(out, c);
    }
    out.push(')');
}

fn write_column(out: &mut String, c: &ColumnDef) {
    out.push_str(c.name.value.as_str());
    out.push(' ');
    out.push_str(c.type_name.value.as_str());
    if let Some(size) = c.size {
        out.push('(');
        let _ = std::fmt::Write::write_fmt(out, format_args!("{size}"));
        if let Some(scale) = c.scale {
            out.push_str(", ");
            let _ = std::fmt::Write::write_fmt(out, format_args!("{scale}"));
        }
        out.push(')');
    }
    if c.primary_key {
        out.push_str(" PRIMARY KEY");
    } else if c.not_null {
        out.push_str(" NOT NULL");
    }
}

fn write_view(out: &mut String, v: &View, indent: &str) {
    out.push_str(indent);
    out.push_str("View ");
    out.push_str(v.name.value.as_str());
    out.push_str(" (");
    write_token_slice(out, &v.body);
    out.push(')');
}

fn write_join(out: &mut String, j: &Join) {
    out.push_str("  Join ");
    out.push_str(j.name.value.as_str());
    out.push_str(" (");
    write_op_expr(out, &j.body);
    out.push(')');
}

fn write_filter(out: &mut String, f: &Filter) {
    out.push_str("  Filter ");
    out.push_str(f.name.value.as_str());
    out.push_str(" (");
    write_op_expr(out, &f.body);
    out.push(')');
}

fn write_multi_grain_filter(out: &mut String, m: &MultiGrainFilter) {
    out.push_str("  MultiGrainFilter ");
    out.push_str(m.name.value.as_str());
    out.push_str(" (");
    write_op_expr(out, &m.body);
    out.push(')');
}

fn write_token_slice(out: &mut String, slice: &TokenSlice) {
    out.push_str(&slice.render_with_spaces());
}

// ---------------------------------------------------------------------------
// op_operation composition (Stage 2)
// ---------------------------------------------------------------------------

/// Round-trip composer for [`OpExpr`]. Output re-parses to the same
/// AST shape under [`crate::parser::parse_op_operation`] (verified by
/// the compose-smoke tests). Whitespace is normalised to single
/// spaces between tokens; explicit `Group` nodes preserve user-written
/// parentheses; implicit precedence-driven parens are not added (the
/// grammar's right-associative `Bool` and single-shot `Compare`
/// chains parse unambiguously without them).
fn write_op_expr(out: &mut String, expr: &OpExpr) {
    match expr {
        OpExpr::Bool { op, lhs, rhs, .. } => {
            write_op_expr(out, lhs);
            out.push(' ');
            out.push_str(op.as_str());
            out.push(' ');
            write_op_expr(out, rhs);
        }
        OpExpr::Compare { op, lhs, rhs, .. } => {
            write_op_expr(out, lhs);
            out.push(' ');
            out.push_str(op.as_str());
            out.push(' ');
            write_op_expr(out, rhs);
        }
        OpExpr::IsNull { expr, negated, .. } => {
            write_op_expr(out, expr);
            out.push_str(if *negated { " is not null" } else { " is null" });
        }
        OpExpr::Group { inner, .. } => {
            out.push('(');
            write_op_expr(out, inner);
            out.push(')');
        }
        OpExpr::Function { db, name, args, .. } => {
            write_optional_db(out, db.as_ref());
            out.push_str(name.value.as_str());
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_op_expr(out, a);
            }
            out.push(')');
        }
        OpExpr::Column(c) => write_op_column(out, c),
        OpExpr::Literal(l) => write_op_literal(out, l),
    }
}

fn write_op_column(out: &mut String, c: &OpColumn) {
    match c {
        OpColumn::Target {
            column,
            primary_key,
            ..
        } => {
            out.push_str("{target}.");
            out.push_str(column.value.as_str());
            if *primary_key {
                out.push_str(" PRIMARY KEY");
            }
        }
        OpColumn::Aliased {
            db,
            alias,
            scope,
            primary_key,
            ..
        } => {
            write_optional_db(out, db.as_ref());
            out.push_str(alias.value.as_str());
            for s in scope {
                out.push('.');
                out.push_str(s.value.as_str());
            }
            if *primary_key {
                out.push_str(" PRIMARY KEY");
            }
        }
    }
}

fn write_op_literal(out: &mut String, l: &OpLiteral) {
    match l {
        OpLiteral::String { value, .. } => {
            out.push('\'');
            out.push_str(value.as_str());
            out.push('\'');
        }
        OpLiteral::Integer { value, .. } => {
            let _ = std::fmt::Write::write_fmt(out, format_args!("{value}"));
        }
        OpLiteral::Float { value, .. } => {
            // Pure's float literal grammar accepts plain decimal
            // notation; format with at least one fractional digit so
            // the round-trip parser still recognises it as a float.
            let formatted = format!("{value}");
            if formatted.contains('.') || formatted.contains('e') {
                out.push_str(&formatted);
            } else {
                out.push_str(&formatted);
                out.push_str(".0");
            }
        }
    }
}

fn write_optional_db(out: &mut String, db: Option<&PackageableElementPtr>) {
    let Some(db) = db else { return };
    out.push('[');
    if let Some(pkg) = &db.package {
        for seg in pkg.segments() {
            out.push_str(seg.as_str());
            out.push_str("::");
        }
    }
    out.push_str(db.name.as_str());
    out.push(']');
}
