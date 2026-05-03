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
//! `source_info` fields. Op-bodies, view bodies, and class-mapping
//! bodies all replay through structured AST nodes (no token-slice
//! capture remains). Verified by `tests/compose_smoke.rs`.

use crate::ast::{
    ColumnDef, DatabaseDef, DatabaseElement, DatabaseInclude, Filter, FilterViewBlock,
    FilterViewDbChain, Join, MilestoneDef, MilestoneField, MilestoneSpec, MilestoneValue,
    MultiGrainFilter, OpColumn, OpExpr, OpLiteral, Schema, Table, View, ViewColumnMappingLine,
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
    if let Some(spec) = &t.milestoning {
        write_milestone_spec(out, spec);
        out.push(' ');
    }
    for (i, c) in t.columns.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_column(out, c);
    }
    out.push(')');
}

fn write_milestone_spec(out: &mut String, spec: &MilestoneSpec) {
    out.push_str("milestoning(");
    for (i, def) in spec.definitions.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_milestone_definition(out, def);
    }
    out.push(')');
}

fn write_milestone_definition(out: &mut String, def: &MilestoneDef) {
    out.push_str(def.kind.value.as_str());
    out.push('(');
    for (i, field) in def.fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_milestone_field(out, field);
    }
    out.push(')');
}

fn write_milestone_field(out: &mut String, field: &MilestoneField) {
    out.push_str(field.key.value.as_str());
    out.push('=');
    match &field.value {
        MilestoneValue::Identifier(s) => out.push_str(s.value.as_str()),
        MilestoneValue::Date { literal, .. } => out.push_str(literal.as_str()),
        MilestoneValue::Boolean { value, .. } => {
            out.push_str(if *value { "true" } else { "false" });
        }
    }
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
    let mut wrote_header = false;
    if let Some(filter) = &v.filter {
        write_filter_view_block(out, filter);
        wrote_header = true;
    }
    if let Some(group_by) = &v.group_by {
        if wrote_header {
            out.push(' ');
        }
        out.push_str("~groupBy(");
        for (i, jc) in group_by.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_join_col_with_db_or_constant(out, jc);
        }
        out.push(')');
        wrote_header = true;
    }
    if v.distinct {
        if wrote_header {
            out.push(' ');
        }
        out.push_str("~distinct");
        wrote_header = true;
    }
    if wrote_header && !v.columns.is_empty() {
        out.push(' ');
    }
    for (i, col) in v.columns.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_view_column_mapping_line(out, col);
    }
    out.push(')');
}

fn write_filter_view_block(out: &mut String, b: &FilterViewBlock) {
    out.push_str("~filter ");
    if let Some(chain) = &b.db_chain {
        write_filter_view_db_chain(out, chain);
        out.push(' ');
    }
    out.push_str(b.filter_name.value.as_str());
}

fn write_filter_view_db_chain(out: &mut String, c: &FilterViewDbChain) {
    write_db_brackets(out, &c.first_db);
    write_join_sequence(out, &c.join_sequence);
    out.push_str(" | ");
    write_db_brackets(out, &c.second_db);
}

fn write_view_column_mapping_line(out: &mut String, line: &ViewColumnMappingLine) {
    out.push_str(line.column_name.value.as_str());
    if let Some(target) = &line.target_set_id {
        out.push('[');
        out.push_str(target.value.as_str());
        out.push(']');
    }
    out.push_str(" : ");
    write_join_col_with_db_or_constant(out, &line.value);
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
        OpExpr::Array { elements, .. } => {
            out.push('[');
            for (i, e) in elements.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_op_expr(out, e);
            }
            out.push(']');
        }
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

// ===========================================================================
// Stage 5: class-mapping body composer
// ===========================================================================

use crate::ast::{
    EmbeddedMapping, EmbeddedMappingTrailer, FilterMappingBlock, FilterMappingJoinSequence,
    InlineRef, JoinColWithDbOrConstant, JoinSequence, LocalMappingProperty, MainTableBlock,
    MappingElement, NonePlusMappingLine, NonePlusMappingValue, OneJoin, OneJoinRight,
    OtherwiseJoin, OtherwisePropertyMapping, PlusMappingLine, RelationalClassMappingBody,
    RelationalMapping, ScopedMapping, SimpleScopeInfo, SingleMappingLine, Transformer,
};

/// Round-trip the body produced by
/// `RelationalClassMappingBodyParser::parse`. Output replays the
/// declared headers (`~filter` / `~distinct` / `~groupBy` /
/// `~primaryKey` / `~mainTable`) in canonical order, even when the
/// source declared them in a different order — Java's grammar
/// requires the canonical order, so this normalises lazy fixtures.
pub fn write_relational_class_mapping_body(out: &mut String, body: &RelationalClassMappingBody) {
    out.push('{');
    let mut wrote_header = false;
    if let Some(filter) = &body.filter {
        if wrote_header {
            out.push(' ');
        }
        write_filter_mapping_block(out, filter);
        wrote_header = true;
    }
    if body.distinct {
        if wrote_header {
            out.push(' ');
        }
        out.push_str("~distinct");
        wrote_header = true;
    }
    if let Some(group_by) = &body.group_by {
        if wrote_header {
            out.push(' ');
        }
        out.push_str("~groupBy(");
        for (i, jc) in group_by.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_join_col_with_db_or_constant(out, jc);
        }
        out.push(')');
        wrote_header = true;
    }
    if let Some(primary_key) = &body.primary_key {
        if wrote_header {
            out.push(' ');
        }
        out.push_str("~primaryKey(");
        for (i, jc) in primary_key.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_join_col_with_db_or_constant(out, jc);
        }
        out.push(')');
        wrote_header = true;
    }
    if let Some(main_table) = &body.main_table {
        if wrote_header {
            out.push(' ');
        }
        write_main_table_block(out, main_table);
        wrote_header = true;
    }
    if !body.mapping_elements.is_empty() {
        if wrote_header {
            out.push(' ');
        }
        out.push('(');
        for (i, e) in body.mapping_elements.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_mapping_element(out, e);
        }
        out.push(')');
    }
    if let Some(lines) = &body.association_mapping {
        // Stage 7: AssociationMapping body. Mutually exclusive with
        // the class-mapping headers + mapping_elements above; the
        // parser ensures only one path produces non-empty content.
        if wrote_header {
            out.push(' ');
        }
        out.push_str("AssociationMapping(");
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_single_mapping_line(out, line);
        }
        out.push(')');
    }
    out.push('}');
}

fn write_filter_mapping_block(out: &mut String, b: &FilterMappingBlock) {
    out.push_str("~filter ");
    write_db_brackets(out, &b.db);
    if let Some(seq) = &b.join_sequence {
        write_filter_mapping_join_sequence(out, seq);
    }
    out.push_str(b.filter_name.value.as_str());
}

fn write_filter_mapping_join_sequence(out: &mut String, seq: &FilterMappingJoinSequence) {
    if let Some(g) = &seq.group_id {
        out.push('(');
        out.push_str(g.value.as_str());
        out.push(')');
    }
    write_one_join(out, &seq.head);
    for r in &seq.right {
        write_one_join_right(out, r);
    }
    out.push_str(" | ");
    write_db_brackets(out, &seq.second_db);
}

fn write_main_table_block(out: &mut String, b: &MainTableBlock) {
    out.push_str("~mainTable ");
    write_db_brackets(out, &b.db);
    write_simple_scope_info(out, &b.scope);
}

fn write_simple_scope_info(out: &mut String, s: &SimpleScopeInfo) {
    out.push_str(s.table.value.as_str());
    for seg in &s.scope {
        out.push('.');
        out.push_str(seg.value.as_str());
    }
}

fn write_join_col_with_db_or_constant(out: &mut String, jc: &JoinColWithDbOrConstant) {
    if let Some(db) = &jc.db {
        write_db_brackets(out, db);
    }
    if let Some(seq) = &jc.join {
        write_join_sequence(out, seq);
        if let Some(col) = &jc.column {
            out.push_str(" | ");
            write_op_column_inline(out, col);
        }
    } else if let Some(col) = &jc.column {
        write_op_column_inline(out, col);
    } else if let Some(lit) = &jc.literal {
        write_op_literal_inline(out, lit);
    }
}

fn write_join_sequence(out: &mut String, seq: &JoinSequence) {
    write_one_join(out, &seq.head);
    for r in &seq.right {
        write_one_join_right(out, r);
    }
}

fn write_one_join(out: &mut String, j: &OneJoin) {
    out.push('@');
    out.push_str(j.name.value.as_str());
}

fn write_one_join_right(out: &mut String, r: &OneJoinRight) {
    out.push_str(" > ");
    if let Some(g) = &r.group_id {
        out.push('(');
        out.push_str(g.value.as_str());
        out.push_str(") ");
    }
    if let Some(db) = &r.db {
        write_db_brackets(out, db);
    }
    write_one_join(out, &r.join);
}

fn write_op_column_inline(out: &mut String, c: &crate::ast::OpColumn) {
    use crate::ast::OpColumn;
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
            if let Some(d) = db {
                write_db_brackets(out, d);
            }
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

fn write_op_literal_inline(out: &mut String, l: &crate::ast::OpLiteral) {
    use crate::ast::OpLiteral;
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
            let f = format!("{value}");
            if f.contains('.') || f.contains('e') {
                out.push_str(&f);
            } else {
                out.push_str(&f);
                out.push_str(".0");
            }
        }
    }
}

fn write_db_brackets(out: &mut String, db: &PackageableElementPtr) {
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

fn write_mapping_element(out: &mut String, e: &MappingElement) {
    match e {
        MappingElement::Single(line) => write_single_mapping_line(out, line),
        MappingElement::Scope(s) => write_scope(out, s),
    }
}

fn write_single_mapping_line(out: &mut String, line: &SingleMappingLine) {
    match line {
        SingleMappingLine::NonePlus(np) => write_none_plus_line(out, np),
        SingleMappingLine::Plus(p) => write_plus_line(out, p),
    }
}

fn write_none_plus_line(out: &mut String, line: &NonePlusMappingLine) {
    out.push_str(line.property.value.as_str());
    if let Some(src) = &line.source_id {
        out.push('[');
        out.push_str(src.value.as_str());
        if let Some(t) = &line.target_id {
            out.push_str(", ");
            out.push_str(t.value.as_str());
        }
        out.push(']');
    }
    match &line.value {
        NonePlusMappingValue::Relational(m) => write_relational_mapping(out, m),
        NonePlusMappingValue::Embedded(e) => {
            out.push(' ');
            write_embedded_mapping(out, e);
        }
    }
}

fn write_embedded_mapping(out: &mut String, e: &EmbeddedMapping) {
    out.push('(');
    if let Some(pk) = &e.primary_key {
        out.push_str("~primaryKey(");
        for (i, jc) in pk.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_join_col_with_db_or_constant(out, jc);
        }
        out.push(')');
        // primaryKey isn't a singleMappingLine in the Java grammar
        // (`'(' (primaryKey? singleMappingLines)? ')'`), so it's
        // separated from the lines by whitespace, not a comma.
        if !e.mapping_lines.is_empty() {
            out.push(' ');
        }
    }
    for (i, line) in e.mapping_lines.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_single_mapping_line(out, line);
    }
    out.push(')');
    if let Some(trailer) = &e.trailer {
        match trailer {
            EmbeddedMappingTrailer::Inline(r) => {
                out.push(' ');
                write_inline_ref(out, r);
            }
            EmbeddedMappingTrailer::Otherwise(maps) => {
                out.push_str(" Otherwise(");
                for (i, m) in maps.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_otherwise_property_mapping(out, m);
                }
                out.push(')');
            }
        }
    }
}

fn write_inline_ref(out: &mut String, r: &InlineRef) {
    out.push_str("Inline[");
    out.push_str(r.id.value.as_str());
    out.push(']');
}

fn write_otherwise_property_mapping(out: &mut String, m: &OtherwisePropertyMapping) {
    out.push('[');
    out.push_str(m.property.value.as_str());
    out.push_str("] : ");
    write_otherwise_join(out, &m.otherwise_join);
}

fn write_otherwise_join(out: &mut String, oj: &OtherwiseJoin) {
    if let Some(db) = &oj.db {
        write_db_brackets(out, db);
    }
    write_join_sequence(out, &oj.join_sequence);
}

fn write_plus_line(out: &mut String, line: &PlusMappingLine) {
    out.push_str("+ ");
    out.push_str(line.property.value.as_str());
    write_local_mapping_property(out, &line.local);
    write_relational_mapping(out, &line.mapping);
}

fn write_local_mapping_property(out: &mut String, lp: &LocalMappingProperty) {
    out.push_str(" : ");
    if let Some(pkg) = &lp.type_path.package {
        for seg in pkg.segments() {
            out.push_str(seg.as_str());
            out.push_str("::");
        }
    }
    out.push_str(lp.type_path.name.as_str());
    out.push('[');
    out.push_str(lp.mult_lower.value.as_str());
    if let Some(u) = &lp.mult_upper {
        out.push_str("..");
        out.push_str(u.value.as_str());
    }
    out.push(']');
}

fn write_relational_mapping(out: &mut String, m: &RelationalMapping) {
    out.push_str(" : ");
    if let Some(t) = &m.transformer {
        write_transformer(out, t);
    }
    write_join_col_with_db_or_constant(out, &m.value);
}

fn write_transformer(out: &mut String, t: &Transformer) {
    out.push_str("EnumerationMapping ");
    out.push_str(t.enumeration_mapping.value.as_str());
    out.push_str(" : ");
}

fn write_scope(out: &mut String, s: &ScopedMapping) {
    out.push_str("scope(");
    write_db_brackets(out, &s.db);
    if let Some(scope) = &s.scope {
        write_simple_scope_info(out, scope);
    }
    out.push_str(") (");
    for (i, line) in s.mapping_lines.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_single_mapping_line(out, line);
    }
    out.push(')');
}
