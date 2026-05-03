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

//! `SectionParser` for `###Relational` section bodies.
//!
//! Recognises a stream of `Database pkg::db ( includes* (schema |
//! table | join | filter | multiGrainFilter | view)* )`
//! declarations. Stage 1 parses the structural envelope and column
//! definitions; `op_operation` body content (joins / filters /
//! multi-grain filters) and view body content are captured as
//! verbatim [`TokenSlice`](crate::ast::TokenSlice)s for round-trip,
//! pending the Stage-2 op-grammar.
//!
//! Mirrors `legend-pure-store-relational/.../RelationalParser.g4` for
//! the top-level rule shape; section-scoped keywords (`Database`,
//! `Schema`, `Table`, `View`, `Join`, `Filter`, `MultiGrainFilter`,
//! `include`) are recognised as `TokenKind::Identifier`s and matched
//! by text — they're identifier-like outside `###Relational` and
//! the existing lexer grammar doesn't carry relational-specific
//! token kinds.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_lexer::{Token, TokenKind};
use legend_pure_parser_parser::ParserContext;
use legend_pure_parser_parser::cursor::Cursor;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::section_parser::SectionParser;
use smol_str::SmolStr;

use crate::ast::{
    BinOp, BoolOp, ColumnDef, DatabaseDef, DatabaseElement, DatabaseInclude, EmbeddedMapping,
    EmbeddedMappingTrailer, Filter, FilterMappingBlock, FilterMappingJoinSequence, InlineRef, Join,
    JoinColWithDbOrConstant, JoinSequence, LocalMappingProperty, MainTableBlock, MappingElement,
    MilestoneDef, MilestoneField, MilestoneSpec, MilestoneValue, MultiGrainFilter,
    NonePlusMappingLine, NonePlusMappingValue, OneJoin, OneJoinRight, OpColumn, OpExpr, OpLiteral,
    OtherwiseJoin, OtherwisePropertyMapping, PlusMappingLine, RelationalClassMappingBody,
    RelationalMapping, SECTION_KIND, Schema, ScopedMapping, SimpleScopeInfo, SingleMappingLine,
    Table, TokenSlice, Transformer, View,
};

fn err(message: String, source_info: SourceInfo) -> ParseError {
    ParseError::Unexpected {
        message,
        source_info,
    }
}

fn err_unexpected(expected: &str, found: &str, source_info: SourceInfo) -> ParseError {
    err(format!("Expected {expected}, found '{found}'"), source_info)
}

/// Plug-in that owns the `###Relational` section grammar.
///
/// Register with the core parser via
/// [`legend_pure_parser_parser::parse_with_sections`].
///
/// Stage 1 ships only the section parser. The Mapping-island plug-in
/// (a `ClassMappingBodyParser` for `: Relational { … }` inside
/// `###Mapping`) lands in Stage 5.
pub struct RelationalSectionParser;

impl SectionParser for RelationalSectionParser {
    fn kind(&self) -> &str {
        SECTION_KIND
    }

    fn parse_body(
        &self,
        ctx: &mut ParserContext<'_>,
        errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>> {
        let mut out: Vec<Box<dyn DSLElement>> = Vec::new();
        while !ctx.cursor().check(TokenKind::SectionHeader) && !ctx.cursor().check(TokenKind::Eof) {
            // Each iteration must make progress on Ok or Err to avoid
            // an infinite loop on malformed input.
            let pos_before = ctx.cursor().peek().source_info.clone();
            match parse_database(ctx) {
                Ok(db) => out.push(Box::new(db)),
                Err(e) => {
                    errors.push(e);
                    skip_to_recovery_point(ctx.cursor());
                }
            }
            if ctx.cursor().peek().source_info == pos_before {
                ctx.cursor().advance();
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

fn parse_database(ctx: &mut ParserContext<'_>) -> Result<DatabaseDef, ParseError> {
    let start = ctx.cursor().current_source_info();
    let kw = ctx.cursor().expect(TokenKind::Identifier)?;
    if kw.text != "Database" {
        return Err(err_unexpected("Database", &kw.text, kw.source_info));
    }

    let (package, name, name_si) = ctx.parse_qualified_name()?;

    ctx.cursor().expect(TokenKind::LParen)?;

    let mut includes = Vec::new();
    let mut elements = Vec::new();

    // Includes must precede other body elements (Java grammar: `include*`
    // before `(schema | table | …)*`). Once a non-include element is
    // seen, further `include` directives are an error.
    let mut includes_closed = false;
    while !ctx.cursor().check(TokenKind::RParen) && !ctx.cursor().check(TokenKind::Eof) {
        if is_keyword(ctx.cursor(), "include") {
            if includes_closed {
                let bad = ctx.cursor().peek().source_info.clone();
                return Err(err(
                    "`include` directives must precede schemas / tables / joins / filters \
                     in the database body"
                        .into(),
                    bad,
                ));
            }
            includes.push(parse_include(ctx)?);
            continue;
        }
        includes_closed = true;
        elements.push(parse_database_element(ctx)?);
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;

    Ok(DatabaseDef {
        package,
        name: SpannedString {
            value: name,
            source_info: name_si,
        },
        includes,
        elements,
        source_info: merge_si(&start, &close.source_info),
    })
}

fn is_keyword(cursor: &Cursor, kw: &str) -> bool {
    cursor.check(TokenKind::Identifier) && cursor.peek().text == kw
}

// ---------------------------------------------------------------------------
// Include
// ---------------------------------------------------------------------------

fn parse_include(ctx: &mut ParserContext<'_>) -> Result<DatabaseInclude, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "include"
    let included = parse_packageable_ptr(ctx)?;
    let end_si = included.source_info.clone();
    Ok(DatabaseInclude {
        included,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

// ---------------------------------------------------------------------------
// Database body elements
// ---------------------------------------------------------------------------

fn parse_database_element(ctx: &mut ParserContext<'_>) -> Result<DatabaseElement, ParseError> {
    let peek = ctx.cursor().peek().clone();
    if peek.kind != TokenKind::Identifier {
        return Err(err_unexpected(
            "Schema / Table / View / Join / Filter / MultiGrainFilter",
            &peek.text,
            peek.source_info,
        ));
    }
    match peek.text.as_str() {
        "Schema" => Ok(DatabaseElement::Schema(parse_schema(ctx)?)),
        "Table" => Ok(DatabaseElement::Table(parse_table(ctx)?)),
        "View" => Ok(DatabaseElement::View(parse_view(ctx)?)),
        "Join" => Ok(DatabaseElement::Join(parse_join(ctx)?)),
        "Filter" => Ok(DatabaseElement::Filter(parse_filter(ctx)?)),
        "MultiGrainFilter" => Ok(DatabaseElement::MultiGrainFilter(parse_multi_grain_filter(
            ctx,
        )?)),
        other => Err(err_unexpected(
            "Schema / Table / View / Join / Filter / MultiGrainFilter",
            other,
            peek.source_info,
        )),
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

fn parse_schema(ctx: &mut ParserContext<'_>) -> Result<Schema, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "Schema"
    let name = parse_relational_identifier(ctx)?;
    ctx.cursor().expect(TokenKind::LParen)?;
    let mut tables = Vec::new();
    let mut views = Vec::new();
    while !ctx.cursor().check(TokenKind::RParen) && !ctx.cursor().check(TokenKind::Eof) {
        let peek = ctx.cursor().peek().clone();
        match (peek.kind, peek.text.as_str()) {
            (TokenKind::Identifier, "Table") => tables.push(parse_table(ctx)?),
            (TokenKind::Identifier, "View") => views.push(parse_view(ctx)?),
            _ => {
                return Err(err_unexpected(
                    "Table or View",
                    &peek.text,
                    peek.source_info,
                ));
            }
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(Schema {
        name,
        tables,
        views,
        source_info: merge_si(&kw.source_info, &close.source_info),
    })
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

fn parse_table(ctx: &mut ParserContext<'_>) -> Result<Table, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "Table"
    let name = parse_relational_identifier(ctx)?;
    ctx.cursor().expect(TokenKind::LParen)?;

    // Optional `milestoning ( … )` block (Stage 3) — must precede
    // the column list per Java's grammar `table` rule.
    let milestoning = if is_keyword(ctx.cursor(), "milestoning") {
        Some(parse_milestone_spec(ctx)?)
    } else {
        None
    };

    let columns = parse_columns(ctx)?;
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(Table {
        name,
        milestoning,
        columns,
        source_info: merge_si(&kw.source_info, &close.source_info),
    })
}

// ---------------------------------------------------------------------------
// Milestoning specs (Stage 3)
//
// Java grammar (RelationalParser.g4 lines 295-335):
//
//   milestoneSpec : MILESTONING '(' milestoningDefinitions? ')' ;
//   milestoningDefinitions : milestoningDefinition (',' milestoningDefinition)* ;
//   milestoningDefinition  : identifier '(' milestoningContent ')' ;
//   milestoningContent     : (… key = value …)*
//
// Java's `milestoningContent` is permissive (any token sequence). The
// validators in Stage 4 enforce per-kind shape (`business` requires
// `BUS_FROM` + `BUS_THRU` + optional flags; `processing` requires
// `PROCESSING_IN` + `PROCESSING_OUT` + optional flags; snapshot
// variants take a single `*_SNAPSHOT_DATE`). Our parser is similarly
// permissive — accepts any `KEY = value` pair, validates in Stage 4.
// ---------------------------------------------------------------------------

fn parse_milestone_spec(ctx: &mut ParserContext<'_>) -> Result<MilestoneSpec, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // 'milestoning'
    debug_assert_eq!(kw.text.as_str(), "milestoning");
    ctx.cursor().expect(TokenKind::LParen)?;

    let mut definitions = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        loop {
            definitions.push(parse_milestone_definition(ctx)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(MilestoneSpec {
        definitions,
        source_info: merge_si(&kw.source_info, &close.source_info),
    })
}

fn parse_milestone_definition(ctx: &mut ParserContext<'_>) -> Result<MilestoneDef, ParseError> {
    let kind_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let kind = SpannedString {
        value: SmolStr::new(kind_tok.text),
        source_info: kind_tok.source_info.clone(),
    };
    ctx.cursor().expect(TokenKind::LParen)?;

    let mut fields = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        loop {
            fields.push(parse_milestone_field(ctx)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(MilestoneDef {
        kind,
        fields,
        source_info: merge_si(&kind_tok.source_info, &close.source_info),
    })
}

fn parse_milestone_field(ctx: &mut ParserContext<'_>) -> Result<MilestoneField, ParseError> {
    let key_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let key = SpannedString {
        value: SmolStr::new(key_tok.text),
        source_info: key_tok.source_info.clone(),
    };
    ctx.cursor().expect(TokenKind::Equals)?;
    let value_peek = ctx.cursor().peek().clone();
    let (value, end_si) = match value_peek.kind {
        TokenKind::DateLiteral => {
            ctx.cursor().advance();
            (
                MilestoneValue::Date {
                    literal: SmolStr::new(value_peek.text),
                    source_info: value_peek.source_info.clone(),
                },
                value_peek.source_info,
            )
        }
        TokenKind::True => {
            ctx.cursor().advance();
            (
                MilestoneValue::Boolean {
                    value: true,
                    source_info: value_peek.source_info.clone(),
                },
                value_peek.source_info,
            )
        }
        TokenKind::False => {
            ctx.cursor().advance();
            (
                MilestoneValue::Boolean {
                    value: false,
                    source_info: value_peek.source_info.clone(),
                },
                value_peek.source_info,
            )
        }
        TokenKind::Identifier => {
            let id = parse_relational_identifier(ctx)?;
            let si = id.source_info.clone();
            (MilestoneValue::Identifier(id), si)
        }
        _ => {
            return Err(err_unexpected(
                "milestoning field value (identifier, date literal, or boolean)",
                &value_peek.text,
                value_peek.source_info,
            ));
        }
    };
    Ok(MilestoneField {
        key,
        value,
        source_info: merge_si(&key_tok.source_info, &end_si),
    })
}

fn parse_columns(ctx: &mut ParserContext<'_>) -> Result<Vec<ColumnDef>, ParseError> {
    let mut columns = Vec::new();
    if ctx.cursor().check(TokenKind::RParen) {
        return Ok(columns);
    }
    loop {
        columns.push(parse_column(ctx)?);
        if !ctx.cursor().eat(TokenKind::Comma) {
            break;
        }
    }
    Ok(columns)
}

fn parse_column(ctx: &mut ParserContext<'_>) -> Result<ColumnDef, ParseError> {
    let name = parse_relational_identifier(ctx)?;
    let start = name.source_info.clone();

    // Type name. Pure-grammar SQL types like `INT`, `VARCHAR`,
    // `TIMESTAMP` are plain identifiers; some Java fixtures use
    // identifiers that lex as keywords-from-other-grammars, but for
    // Stage 1 we only accept Identifier here. Extend in a later stage
    // if a fixture forces it.
    let type_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let type_name = SpannedString {
        value: SmolStr::new(type_tok.text.clone()),
        source_info: type_tok.source_info.clone(),
    };

    // Optional `(size[, scale])`.
    let mut size = None;
    let mut scale = None;
    if ctx.cursor().eat(TokenKind::LParen) {
        size = Some(parse_int(ctx)?);
        if ctx.cursor().eat(TokenKind::Comma) {
            scale = Some(parse_int(ctx)?);
        }
        ctx.cursor().expect(TokenKind::RParen)?;
    }

    // Optional `PRIMARY KEY` / `NOT NULL` trailing flag. Both are
    // two-identifier phrases, so we peek for the first half and
    // consume both halves on a hit.
    let mut primary_key = false;
    let mut not_null = false;
    let mut end_si = type_tok.source_info.clone();
    if is_keyword(ctx.cursor(), "PRIMARY") {
        let p = ctx.cursor().expect(TokenKind::Identifier)?;
        let k = ctx.cursor().expect(TokenKind::Identifier)?;
        if k.text != "KEY" {
            return Err(err_unexpected(
                "KEY (after PRIMARY)",
                &k.text,
                k.source_info,
            ));
        }
        primary_key = true;
        end_si = merge_si(&p.source_info, &k.source_info);
    } else if is_keyword(ctx.cursor(), "NOT") {
        let n = ctx.cursor().expect(TokenKind::Identifier)?;
        let nu = ctx.cursor().expect(TokenKind::Identifier)?;
        if nu.text != "NULL" {
            return Err(err_unexpected("NULL (after NOT)", &nu.text, nu.source_info));
        }
        not_null = true;
        end_si = merge_si(&n.source_info, &nu.source_info);
    }

    Ok(ColumnDef {
        name,
        type_name,
        size,
        scale,
        primary_key,
        not_null,
        source_info: merge_si(&start, &end_si),
    })
}

fn parse_int(ctx: &mut ParserContext<'_>) -> Result<i64, ParseError> {
    let tok = ctx.cursor().expect(TokenKind::IntegerLiteral)?;
    tok.text.parse::<i64>().map_err(|_| {
        err(
            format!("expected integer literal, got '{}'", tok.text),
            tok.source_info,
        )
    })
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn parse_view(ctx: &mut ParserContext<'_>) -> Result<View, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "View"
    let name = parse_relational_identifier(ctx)?;
    let body = capture_paren_body(ctx)?;
    let end_si = body.source_info.clone();
    Ok(View {
        name,
        body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

// ---------------------------------------------------------------------------
// Join / Filter / MultiGrainFilter
// ---------------------------------------------------------------------------

fn parse_join(ctx: &mut ParserContext<'_>) -> Result<Join, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "Join"
    let name = parse_relational_identifier(ctx)?;
    let (body, end_si) = parse_paren_op_body(ctx)?;
    Ok(Join {
        name,
        body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

fn parse_filter(ctx: &mut ParserContext<'_>) -> Result<Filter, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "Filter"
    let name = parse_relational_identifier(ctx)?;
    let (body, end_si) = parse_paren_op_body(ctx)?;
    Ok(Filter {
        name,
        body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

fn parse_multi_grain_filter(ctx: &mut ParserContext<'_>) -> Result<MultiGrainFilter, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "MultiGrainFilter"
    let name = parse_relational_identifier(ctx)?;
    let (body, end_si) = parse_paren_op_body(ctx)?;
    Ok(MultiGrainFilter {
        name,
        body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

/// Consume `( <op_operation> )`, returning the parsed body and the
/// span of the closing `)`. Shared by Join / Filter / MultiGrainFilter.
fn parse_paren_op_body(ctx: &mut ParserContext<'_>) -> Result<(OpExpr, SourceInfo), ParseError> {
    ctx.cursor().expect(TokenKind::LParen)?;
    let body = parse_op_operation(ctx)?;
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok((body, close.source_info))
}

// ---------------------------------------------------------------------------
// op_operation — Stage 2 structured op-grammar
//
// Java grammar (RelationalParser.g4 lines 43-79):
//
//   op_operation                : (op_groupOperation | op_atomicOperation)
//                                 op_boolean_operation_right? ;
//   op_boolean_operation_right  : op_boolean_operator op_operation ;
//   op_groupOperation           : '(' op_operation ')' ;
//   op_atomicOperation          : op_function
//                               | colWithDbOrConstant ( (op_operator
//                                   colWithDbOrConstant) | ISNULL | ISNOTNULL ) ;
//   op_function                 : functionName '(' (functionArgument
//                                   (',' functionArgument)*)? ')' ;
//   functionArgument            : colWithDbOrConstant | arrayOfFunctionArguments ;
//   op_boolean_operator         : 'and' | 'or' ;
//   op_operator                 : '=' | '>' | '<' | '>=' | '<=' | '!=' | '<>' ;
//   colWithDbOrConstant         : (database? op_column) | constant ;
//   op_column                   : tableAliasColumn | tableAliasColumnWithScopeInfo ;
//   tableAliasColumn            : '{target}' '.' relationalIdentifier
//                                 'PRIMARY KEY'? ;
//   tableAliasColumnWithScopeInfo
//                               : (relationalIdentifier|'and'|'or')
//                                 (scopeInfo 'PRIMARY KEY'? )? ;
//   scopeInfo                   : '.' relationalIdentifier ('.'
//                                   relationalIdentifier)? ;
//
// Booleans are right-recursive in Java (a-and-b-and-c parses as
// `a and (b and c)`); we mirror that. Comparisons take a single
// col-or-constant on each side per the grammar — no chaining.
// ---------------------------------------------------------------------------

/// Public entry point used by Filter / Join / MultiGrainFilter.
pub(crate) fn parse_op_operation(ctx: &mut ParserContext<'_>) -> Result<OpExpr, ParseError> {
    let lhs = parse_op_atomic_or_group(ctx)?;
    if let Some(op) = peek_bool_op(ctx) {
        let op_si = ctx.cursor().peek().source_info.clone();
        ctx.cursor().advance(); // 'and' / 'or'
        let rhs = parse_op_operation(ctx)?; // right-recursive
        let span = merge_si(lhs.source_info(), rhs.source_info());
        Ok(OpExpr::Bool {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            source_info: merge_si(&op_si, &span),
        })
    } else {
        Ok(lhs)
    }
}

/// Returns `Some(BoolOp)` when the cursor is on `and` / `or`.
fn peek_bool_op(ctx: &mut ParserContext<'_>) -> Option<BoolOp> {
    let tok = ctx.cursor().peek();
    if tok.kind != TokenKind::Identifier {
        return None;
    }
    match tok.text.as_str() {
        "and" => Some(BoolOp::And),
        "or" => Some(BoolOp::Or),
        _ => None,
    }
}

/// `op_groupOperation | op_atomicOperation`. Disambiguated by the
/// cursor: `(` opens a group; anything else falls into atomic.
fn parse_op_atomic_or_group(ctx: &mut ParserContext<'_>) -> Result<OpExpr, ParseError> {
    if ctx.cursor().check(TokenKind::LParen) {
        let open = ctx.cursor().expect(TokenKind::LParen)?;
        let inner = parse_op_operation(ctx)?;
        let close = ctx.cursor().expect(TokenKind::RParen)?;
        Ok(OpExpr::Group {
            inner: Box::new(inner),
            source_info: merge_si(&open.source_info, &close.source_info),
        })
    } else {
        parse_op_atomic(ctx)
    }
}

/// `op_function | colWithDbOrConstant (op_operator col | ISNULL | ISNOTNULL)`.
///
/// We disambiguate function vs column-with-args by peeking past the
/// optional `[db]` qualifier and the leading identifier:
/// - identifier followed by `(` → either op_function or
///   `tableAliasColumnWithScopeInfo`'s args branch. We treat both as
///   `OpExpr::Function` at the AST level (Java distinguishes only by
///   argument-shape grammar; the surface text is identical).
/// - otherwise → colWithDbOrConstant, then optionally the trailing
///   comparison / IS NULL / IS NOT NULL.
fn parse_op_atomic(ctx: &mut ParserContext<'_>) -> Result<OpExpr, ParseError> {
    let lhs = parse_col_with_db_or_constant(ctx)?;

    // Trailing `is null` / `is not null`?
    if is_keyword(ctx.cursor(), "is") {
        let is_tok = ctx.cursor().expect(TokenKind::Identifier)?; // 'is'
        let negated = is_keyword(ctx.cursor(), "not");
        if negated {
            ctx.cursor().advance(); // 'not'
        }
        let null_tok = ctx.cursor().peek().clone();
        if null_tok.kind != TokenKind::Identifier || null_tok.text != "null" {
            return Err(err_unexpected(
                "'null'",
                &null_tok.text,
                null_tok.source_info,
            ));
        }
        ctx.cursor().advance(); // 'null'
        let span = merge_si(lhs.source_info(), &null_tok.source_info);
        return Ok(OpExpr::IsNull {
            expr: Box::new(lhs),
            negated,
            source_info: merge_si(&is_tok.source_info, &span),
        });
    }

    // Trailing comparison operator?
    if let Some(op) = peek_compare_op(ctx) {
        let op_span = consume_compare_op(ctx)?;
        let rhs = parse_col_with_db_or_constant(ctx)?;
        let span = merge_si(lhs.source_info(), rhs.source_info());
        return Ok(OpExpr::Compare {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            source_info: merge_si(&op_span, &span),
        });
    }

    // Bare colWithDbOrConstant (no trailing op) — only valid at the
    // top of op_atomic when the LHS itself is an `op_function` call.
    // The Java grammar requires a trailing op for non-function atoms,
    // but in practice our consumer (Filter / Join body) always has
    // one — we return the bare expression and let the caller error
    // if it isn't a Function.
    Ok(lhs)
}

/// `colWithDbOrConstant: (database? op_column) | constant`.
///
/// `database` is `[ qualifiedName ]`. Constants are STRING / INTEGER
/// / FLOAT literals. `op_column` is identifier-led (alias) or
/// `{target}.col`.
fn parse_col_with_db_or_constant(ctx: &mut ParserContext<'_>) -> Result<OpExpr, ParseError> {
    // Constant?
    let peek = ctx.cursor().peek();
    match peek.kind {
        TokenKind::StringLiteral => {
            return Ok(OpExpr::Literal(parse_string_literal(ctx)?));
        }
        TokenKind::IntegerLiteral => {
            return Ok(OpExpr::Literal(parse_integer_literal(ctx)?));
        }
        TokenKind::FloatLiteral => {
            return Ok(OpExpr::Literal(parse_float_literal(ctx)?));
        }
        // Signed numeric constants: '+' / '-' followed by a numeric
        // literal. Java's `INTEGER`/`FLOAT` lex this as one token; we
        // assemble it here.
        TokenKind::Plus | TokenKind::Minus => {
            let sign = ctx.cursor().advance().clone();
            let lit_tok = ctx.cursor().peek().clone();
            return match lit_tok.kind {
                TokenKind::IntegerLiteral => {
                    ctx.cursor().advance();
                    let value: i64 =
                        format!("{}{}", sign.text, lit_tok.text)
                            .parse()
                            .map_err(|_| {
                                err(
                                    format!(
                                        "expected integer literal, got '{}{}'",
                                        sign.text, lit_tok.text
                                    ),
                                    lit_tok.source_info.clone(),
                                )
                            })?;
                    Ok(OpExpr::Literal(OpLiteral::Integer {
                        value,
                        source_info: merge_si(&sign.source_info, &lit_tok.source_info),
                    }))
                }
                TokenKind::FloatLiteral => {
                    ctx.cursor().advance();
                    let value: f64 =
                        format!("{}{}", sign.text, lit_tok.text)
                            .parse()
                            .map_err(|_| {
                                err(
                                    format!(
                                        "expected float literal, got '{}{}'",
                                        sign.text, lit_tok.text
                                    ),
                                    lit_tok.source_info.clone(),
                                )
                            })?;
                    Ok(OpExpr::Literal(OpLiteral::Float {
                        value,
                        source_info: merge_si(&sign.source_info, &lit_tok.source_info),
                    }))
                }
                _ => Err(err_unexpected(
                    "numeric literal after sign",
                    &lit_tok.text,
                    lit_tok.source_info,
                )),
            };
        }
        _ => {}
    }

    // Optional `[db]` qualifier.
    let db = parse_optional_db_qualifier(ctx)?;

    // op_column.
    parse_op_column(ctx, db)
}

/// `database: '[' qualifiedName ']'`. Only consumes when the next
/// token is `[`; otherwise returns `None`.
fn parse_optional_db_qualifier(
    ctx: &mut ParserContext<'_>,
) -> Result<Option<PackageableElementPtr>, ParseError> {
    if !ctx.cursor().check(TokenKind::LBracket) {
        return Ok(None);
    }
    ctx.cursor().expect(TokenKind::LBracket)?;
    let ptr = parse_packageable_ptr(ctx)?;
    ctx.cursor().expect(TokenKind::RBracket)?;
    Ok(Some(ptr))
}

/// `op_column: tableAliasColumn | tableAliasColumnWithScopeInfo`.
///
/// `db` is the already-consumed optional `[db]` prefix (only legal on
/// the `tableAliasColumnWithScopeInfo` branch — `{target}.col` doesn't
/// take a db prefix).
fn parse_op_column(
    ctx: &mut ParserContext<'_>,
    db: Option<PackageableElementPtr>,
) -> Result<OpExpr, ParseError> {
    // `{target}` — the Filter-context implicit subject. Pure
    // relational syntax wraps the literal token in braces; our lexer
    // splits this into LBrace + Identifier("target") + RBrace.
    if ctx.cursor().check(TokenKind::LBrace) {
        if db.is_some() {
            return Err(err(
                "{target} column form does not accept a [db] prefix".into(),
                ctx.cursor().peek().source_info.clone(),
            ));
        }
        return parse_target_column(ctx).map(OpExpr::Column);
    }

    // Otherwise: identifier-led `tableAliasColumnWithScopeInfo`,
    // possibly followed by `(args)` (function call form).
    let alias = parse_relational_identifier(ctx)?;

    // `name(args)` — function-call form.
    if ctx.cursor().check(TokenKind::LParen) {
        let _open = ctx.cursor().expect(TokenKind::LParen)?;
        let mut args = Vec::new();
        if !ctx.cursor().check(TokenKind::RParen) {
            loop {
                args.push(parse_op_operation(ctx)?);
                if !ctx.cursor().eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        let close = ctx.cursor().expect(TokenKind::RParen)?;
        let span = merge_si(&alias.source_info, &close.source_info);
        return Ok(OpExpr::Function {
            db,
            name: alias,
            args,
            source_info: span,
        });
    }

    // Plain alias / alias.scope / alias.scope.scope, with optional
    // PRIMARY KEY trailer.
    let mut scope = Vec::new();
    while ctx.cursor().eat(TokenKind::Dot) && scope.len() < 2 {
        scope.push(parse_relational_identifier(ctx)?);
    }
    let scope_end = scope
        .last()
        .map(|s| s.source_info.clone())
        .unwrap_or_else(|| alias.source_info.clone());
    let pk_end = consume_primary_key_flag(ctx)?;
    let primary_key = pk_end.is_some();
    let end_si = pk_end.unwrap_or(scope_end);
    let start_si = db
        .as_ref()
        .map(|p| p.source_info.clone())
        .unwrap_or_else(|| alias.source_info.clone());
    Ok(OpExpr::Column(OpColumn::Aliased {
        db,
        alias,
        scope,
        primary_key,
        source_info: merge_si(&start_si, &end_si),
    }))
}

/// `tableAliasColumn: '{target}' '.' relationalIdentifier 'PRIMARY KEY'?`.
fn parse_target_column(ctx: &mut ParserContext<'_>) -> Result<OpColumn, ParseError> {
    let open = ctx.cursor().expect(TokenKind::LBrace)?;
    let kw = ctx.cursor().peek().clone();
    if kw.kind != TokenKind::Identifier || kw.text != "target" {
        return Err(err_unexpected(
            "'target' (after '{')",
            &kw.text,
            kw.source_info,
        ));
    }
    ctx.cursor().advance(); // 'target'
    ctx.cursor().expect(TokenKind::RBrace)?;
    ctx.cursor().expect(TokenKind::Dot)?;
    let column = parse_relational_identifier(ctx)?;
    let pk_end = consume_primary_key_flag(ctx)?;
    let primary_key = pk_end.is_some();
    let end_si = pk_end.unwrap_or_else(|| column.source_info.clone());
    Ok(OpColumn::Target {
        column,
        primary_key,
        source_info: merge_si(&open.source_info, &end_si),
    })
}

/// `'PRIMARY KEY'?` — two-identifier phrase. On hit, consumes both
/// tokens and returns the span of the closing `KEY`. On miss, leaves
/// the cursor in place and returns `Ok(None)`.
fn consume_primary_key_flag(ctx: &mut ParserContext<'_>) -> Result<Option<SourceInfo>, ParseError> {
    if !is_keyword(ctx.cursor(), "PRIMARY") {
        return Ok(None);
    }
    ctx.cursor().advance(); // 'PRIMARY'
    let key = ctx.cursor().peek().clone();
    if key.kind != TokenKind::Identifier || key.text != "KEY" {
        return Err(err_unexpected(
            "'KEY' (after PRIMARY)",
            &key.text,
            key.source_info,
        ));
    }
    ctx.cursor().advance();
    Ok(Some(key.source_info))
}

/// Returns `Some(BinOp)` when the cursor is on a comparison operator.
/// Does not consume tokens.
fn peek_compare_op(ctx: &mut ParserContext<'_>) -> Option<BinOp> {
    let cur = ctx.cursor().peek();
    match cur.kind {
        TokenKind::Equals => Some(BinOp::Eq),
        TokenKind::Greater => {
            // Could be `>` or `>=` (the lexer may split `>=` into two
            // tokens depending on context).
            if ctx.cursor().peek_kind_at(1) == TokenKind::Equals {
                Some(BinOp::GtEq)
            } else {
                Some(BinOp::Gt)
            }
        }
        TokenKind::GreaterEqual => Some(BinOp::GtEq),
        TokenKind::Less => {
            // Could be `<`, `<=`, or `<>` (the lexer doesn't have a
            // single token for `<>`).
            let next = ctx.cursor().peek_kind_at(1);
            if next == TokenKind::Equals {
                Some(BinOp::LtEq)
            } else if next == TokenKind::Greater {
                Some(BinOp::NotEq2)
            } else {
                Some(BinOp::Lt)
            }
        }
        TokenKind::LessEqual => Some(BinOp::LtEq),
        TokenKind::BangEqual => Some(BinOp::NotEq),
        _ => None,
    }
}

/// Consume the comparison operator that `peek_compare_op` matched.
/// Some of the spellings (`>=`, `<=`, `<>`) span two lexer tokens
/// because Pure's lexer doesn't have first-class `<>` and the others
/// come through depending on lex options; consume both halves when
/// the second half is an `Equals` / `Greater` token immediately
/// following the first.
fn consume_compare_op(ctx: &mut ParserContext<'_>) -> Result<SourceInfo, ParseError> {
    let first = ctx.cursor().advance().clone();
    match first.kind {
        TokenKind::Greater | TokenKind::Less => {
            let next = ctx.cursor().peek().kind;
            if next == TokenKind::Equals
                || (first.kind == TokenKind::Less && next == TokenKind::Greater)
            {
                let second = ctx.cursor().advance().clone();
                Ok(merge_si(&first.source_info, &second.source_info))
            } else {
                Ok(first.source_info)
            }
        }
        _ => Ok(first.source_info),
    }
}

fn parse_string_literal(ctx: &mut ParserContext<'_>) -> Result<OpLiteral, ParseError> {
    let tok = ctx.cursor().expect(TokenKind::StringLiteral)?;
    let raw = tok.text.as_str();
    let bare = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(raw);
    Ok(OpLiteral::String {
        value: SmolStr::new(bare),
        source_info: tok.source_info,
    })
}

fn parse_integer_literal(ctx: &mut ParserContext<'_>) -> Result<OpLiteral, ParseError> {
    let tok = ctx.cursor().expect(TokenKind::IntegerLiteral)?;
    let value: i64 = tok.text.parse().map_err(|_| {
        err(
            format!("expected integer literal, got '{}'", tok.text),
            tok.source_info.clone(),
        )
    })?;
    Ok(OpLiteral::Integer {
        value,
        source_info: tok.source_info,
    })
}

fn parse_float_literal(ctx: &mut ParserContext<'_>) -> Result<OpLiteral, ParseError> {
    let tok = ctx.cursor().expect(TokenKind::FloatLiteral)?;
    let value: f64 = tok.text.parse().map_err(|_| {
        err(
            format!("expected float literal, got '{}'", tok.text),
            tok.source_info.clone(),
        )
    })?;
    Ok(OpLiteral::Float {
        value,
        source_info: tok.source_info,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Capture the tokens between matching outer `(` and `)`, preserving
/// their text + kind for round-trip composition. Tracks nested
/// parens so an inner `()` doesn't close the outer scope. Stage 2
/// replaces these slices with structurally parsed
/// [`Operation`](crate::ast)-level AST nodes.
fn capture_paren_body(ctx: &mut ParserContext<'_>) -> Result<TokenSlice, ParseError> {
    let open = ctx.cursor().expect(TokenKind::LParen)?;
    let mut tokens = Vec::new();
    let mut depth: usize = 1;
    let mut last_si = open.source_info.clone();
    while depth > 0 {
        if ctx.cursor().check(TokenKind::Eof) {
            return Err(err(
                "unexpected end of input inside (...) body".into(),
                last_si,
            ));
        }
        if ctx.cursor().check(TokenKind::SectionHeader) {
            return Err(err(
                "unexpected `###` section header inside (...) body".into(),
                ctx.cursor().peek().source_info.clone(),
            ));
        }
        let kind = ctx.cursor().peek().kind;
        if kind == TokenKind::LParen {
            depth += 1;
        } else if kind == TokenKind::RParen {
            depth -= 1;
            if depth == 0 {
                let close = ctx.cursor().advance().clone();
                let span = merge_si(&open.source_info, &close.source_info);
                return Ok(TokenSlice {
                    tokens,
                    source_info: span,
                });
            }
        }
        let tok: Token = ctx.cursor().advance().clone();
        last_si = tok.source_info.clone();
        tokens.push(tok);
    }
    // Unreachable: the `depth == 0` arm returns.
    Err(err(
        "unbalanced parentheses in body capture".into(),
        last_si,
    ))
}

/// Java grammar: `relationalIdentifier: identifier | QUOTED_STRING`.
/// Stage 1 accepts an Identifier or a StringLiteral (string literals
/// in Pure use single quotes; the Java QUOTED_STRING uses double
/// quotes — Pure's lexer emits StringLiteral for the Pure form, and
/// we accept that for now).
fn parse_relational_identifier(ctx: &mut ParserContext<'_>) -> Result<SpannedString, ParseError> {
    let tok = ctx.cursor().peek().clone();
    match tok.kind {
        TokenKind::Identifier => {
            ctx.cursor().advance();
            Ok(SpannedString {
                value: SmolStr::new(tok.text),
                source_info: tok.source_info,
            })
        }
        TokenKind::StringLiteral => {
            ctx.cursor().advance();
            // Strip the surrounding quotes from the literal text so
            // the AST holds the bare identifier.
            let raw = tok.text.as_str();
            let bare = raw
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(raw);
            Ok(SpannedString {
                value: SmolStr::new(bare),
                source_info: tok.source_info,
            })
        }
        _ => Err(err_unexpected(
            "relational identifier",
            &tok.text,
            tok.source_info,
        )),
    }
}

fn parse_packageable_ptr(ctx: &mut ParserContext<'_>) -> Result<PackageableElementPtr, ParseError> {
    let (package, name, si) = ctx.parse_qualified_name()?;
    Ok(PackageableElementPtr {
        package,
        name,
        source_info: si,
    })
}

fn merge_si(start: &SourceInfo, end: &SourceInfo) -> SourceInfo {
    SourceInfo::new(
        start.source.clone(),
        start.start_line,
        start.start_column,
        end.end_line,
        end.end_column,
    )
}

/// Skip tokens until the next `Database` keyword, section header, or
/// EOF — so a malformed declaration doesn't poison the rest of the
/// file.
fn skip_to_recovery_point(cursor: &mut Cursor) {
    while !cursor.check(TokenKind::Eof) && !cursor.check(TokenKind::SectionHeader) {
        if cursor.check(TokenKind::Identifier) && cursor.peek().text == "Database" {
            return;
        }
        cursor.advance();
    }
}

// ===========================================================================
// Stage 5: Relational class-mapping body parser
//
// Java grammar (RelationalParser.g4 lines 165-265):
//
//   classMapping  : mappingBlock (mappingElements)? EOF ;
//   mappingBlock  : filterMappingBlock? DISTINCTCMD?
//                   mappingBlockGroupBy? primaryKey? mainTableBlock? ;
//
// Real fixtures (TestMappingGrammar) sometimes declare the headers in
// a different order than the grammar (`~filter` before `~mainTable`,
// for example), so the parser is permissive about ordering — it sees
// `~`, advances, dispatches by the next identifier's text, and
// accumulates each header into the body. Duplicate headers raise a
// pointed error.
// ===========================================================================

/// Plug-in `ClassMappingBodyParser` for the Relational mapping-island
/// shape. Register with `MappingSectionParser::with_body_parsers`.
///
/// The body block looks like
///
/// ```text
/// Class : Relational {
///   ~mainTable [db]Table
///   ~filter [db]filterName
///   ~distinct
///   ~primaryKey([db]Table.col, …)
///   ~groupBy([db]Table.col, …)
///   (prop : col, prop2 : col2, …)
/// }
/// ```
pub struct RelationalClassMappingBodyParser;

impl legend_pure_dsl_mapping::parser::ClassMappingBodyParser for RelationalClassMappingBodyParser {
    fn kind(&self) -> &str {
        crate::ast::CLASS_MAPPING_BODY_KIND
    }

    fn parse(
        &self,
        ctx: &mut ParserContext<'_>,
    ) -> Result<Box<dyn legend_pure_dsl_mapping::ast::ForeignClassMappingBody>, ParseError> {
        let body = parse_relational_class_mapping_body(ctx)?;
        Ok(Box::new(body))
    }
}

fn parse_relational_class_mapping_body(
    ctx: &mut ParserContext<'_>,
) -> Result<RelationalClassMappingBody, ParseError> {
    let open = ctx.cursor().expect(TokenKind::LBrace)?;

    let mut filter: Option<FilterMappingBlock> = None;
    let mut distinct = false;
    let mut group_by: Option<Vec<JoinColWithDbOrConstant>> = None;
    let mut primary_key: Option<Vec<JoinColWithDbOrConstant>> = None;
    let mut main_table: Option<MainTableBlock> = None;

    // Mapping-block headers — `~<keyword> ...` clauses. Permissive
    // about ordering; duplicate headers error.
    while ctx.cursor().check(TokenKind::Tilde) {
        let tilde = ctx.cursor().expect(TokenKind::Tilde)?;
        let kw = ctx.cursor().expect(TokenKind::Identifier)?;
        match kw.text.as_str() {
            "filter" => {
                if filter.is_some() {
                    return Err(err(
                        "duplicate `~filter` header in Relational class-mapping body".into(),
                        kw.source_info,
                    ));
                }
                filter = Some(parse_filter_mapping_block_after_tilde(
                    ctx,
                    &tilde.source_info,
                    &kw.source_info,
                )?);
            }
            "distinct" => {
                if distinct {
                    return Err(err(
                        "duplicate `~distinct` header in Relational class-mapping body".into(),
                        kw.source_info,
                    ));
                }
                distinct = true;
            }
            "groupBy" => {
                if group_by.is_some() {
                    return Err(err(
                        "duplicate `~groupBy` header in Relational class-mapping body".into(),
                        kw.source_info,
                    ));
                }
                group_by = Some(parse_paren_join_col_list(ctx)?);
            }
            "primaryKey" => {
                if primary_key.is_some() {
                    return Err(err(
                        "duplicate `~primaryKey` header in Relational class-mapping body".into(),
                        kw.source_info,
                    ));
                }
                primary_key = Some(parse_paren_join_col_list(ctx)?);
            }
            "mainTable" => {
                if main_table.is_some() {
                    return Err(err(
                        "duplicate `~mainTable` header in Relational class-mapping body".into(),
                        kw.source_info,
                    ));
                }
                main_table = Some(parse_main_table_block_after_tilde(
                    ctx,
                    &tilde.source_info,
                    &kw.source_info,
                )?);
            }
            other => {
                return Err(err(
                    format!(
                        "unknown Relational class-mapping body header `~{other}` \
                         (expected one of: ~filter, ~distinct, ~groupBy, ~primaryKey, ~mainTable)"
                    ),
                    kw.source_info,
                ));
            }
        }
    }

    // mappingElements — wrapped in `(...)` per Java's grammar. The
    // wrapping parens are part of `classMapping`'s outer rule, but
    // the body is delivered to us between the class-mapping `{ … }`
    // braces, so the parens around mapping elements are required
    // when any are present. Empty body is legal (header-only).
    let mut mapping_elements: Vec<MappingElement> = Vec::new();
    if ctx.cursor().check(TokenKind::LParen) {
        ctx.cursor().expect(TokenKind::LParen)?;
        if !ctx.cursor().check(TokenKind::RParen) {
            loop {
                mapping_elements.push(parse_mapping_element(ctx)?);
                if !ctx.cursor().eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        ctx.cursor().expect(TokenKind::RParen)?;
    }

    let close = ctx.cursor().expect(TokenKind::RBrace)?;
    Ok(RelationalClassMappingBody {
        filter,
        distinct,
        group_by,
        primary_key,
        main_table,
        mapping_elements,
        source_info: merge_si(&open.source_info, &close.source_info),
    })
}

fn parse_filter_mapping_block_after_tilde(
    ctx: &mut ParserContext<'_>,
    tilde_si: &SourceInfo,
    _kw_si: &SourceInfo,
) -> Result<FilterMappingBlock, ParseError> {
    // `~filter [db] (joinSeq | [db2])? <id>`
    let db = parse_required_db_qualifier(ctx)?;

    // Lookahead: a join sequence starts with `@` or `(group)?@`.
    let join_sequence = if ctx.cursor().check(TokenKind::At)
        || (ctx.cursor().check(TokenKind::LParen)
            && ctx.cursor().peek_kind_at(1) == TokenKind::Identifier)
    {
        // Parse `(group)? oneJoin (oneJoinRight)* | [db2]`.
        let group_id = if ctx.cursor().check(TokenKind::LParen)
            && ctx.cursor().peek_kind_at(1) == TokenKind::Identifier
            && ctx.cursor().peek_kind_at(2) == TokenKind::RParen
        {
            ctx.cursor().expect(TokenKind::LParen)?;
            let id = parse_relational_identifier(ctx)?;
            ctx.cursor().expect(TokenKind::RParen)?;
            Some(id)
        } else {
            None
        };
        let head = parse_one_join(ctx)?;
        let mut right = Vec::new();
        while ctx.cursor().check(TokenKind::Greater) {
            right.push(parse_one_join_right(ctx)?);
        }
        ctx.cursor().expect(TokenKind::Pipe)?;
        let second_db = parse_required_db_qualifier(ctx)?;
        let span = merge_si(&head.source_info, &second_db.source_info);
        Some(FilterMappingJoinSequence {
            group_id,
            head,
            right,
            second_db,
            source_info: span,
        })
    } else {
        None
    };

    let filter_name = parse_relational_identifier(ctx)?;
    let span = merge_si(tilde_si, &filter_name.source_info);
    Ok(FilterMappingBlock {
        db,
        join_sequence,
        filter_name,
        source_info: span,
    })
}

fn parse_main_table_block_after_tilde(
    ctx: &mut ParserContext<'_>,
    tilde_si: &SourceInfo,
    _kw_si: &SourceInfo,
) -> Result<MainTableBlock, ParseError> {
    let db = parse_required_db_qualifier(ctx)?;
    let scope = parse_simple_scope_info(ctx)?;
    let span = merge_si(tilde_si, &scope.source_info);
    Ok(MainTableBlock {
        db,
        scope,
        source_info: span,
    })
}

fn parse_simple_scope_info(ctx: &mut ParserContext<'_>) -> Result<SimpleScopeInfo, ParseError> {
    let table = parse_relational_identifier(ctx)?;
    let mut scope = Vec::new();
    while ctx.cursor().eat(TokenKind::Dot) && scope.len() < 2 {
        scope.push(parse_relational_identifier(ctx)?);
    }
    let end = scope
        .last()
        .map(|s| s.source_info.clone())
        .unwrap_or_else(|| table.source_info.clone());
    Ok(SimpleScopeInfo {
        table: table.clone(),
        scope,
        source_info: merge_si(&table.source_info, &end),
    })
}

fn parse_paren_join_col_list(
    ctx: &mut ParserContext<'_>,
) -> Result<Vec<JoinColWithDbOrConstant>, ParseError> {
    ctx.cursor().expect(TokenKind::LParen)?;
    let mut out = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        loop {
            out.push(parse_join_col_with_db_or_constant(ctx)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    ctx.cursor().expect(TokenKind::RParen)?;
    Ok(out)
}

fn parse_required_db_qualifier(
    ctx: &mut ParserContext<'_>,
) -> Result<PackageableElementPtr, ParseError> {
    parse_optional_db_qualifier(ctx)?.ok_or_else(|| {
        err(
            "expected `[db]` qualifier".into(),
            ctx.cursor().peek().source_info.clone(),
        )
    })
}

fn parse_join_col_with_db_or_constant(
    ctx: &mut ParserContext<'_>,
) -> Result<JoinColWithDbOrConstant, ParseError> {
    let start_si = ctx.cursor().peek().source_info.clone();

    // Constant?
    let peek_kind = ctx.cursor().peek().kind;
    if matches!(
        peek_kind,
        TokenKind::StringLiteral
            | TokenKind::IntegerLiteral
            | TokenKind::FloatLiteral
            | TokenKind::Plus
            | TokenKind::Minus
    ) {
        let lit = parse_constant_literal(ctx)?;
        let end = lit.source_info().clone();
        return Ok(JoinColWithDbOrConstant {
            db: None,
            join: None,
            column: None,
            literal: Some(lit),
            source_info: merge_si(&start_si, &end),
        });
    }

    let db = parse_optional_db_qualifier(ctx)?;
    // Either: joinSequence (PIPE op_column)? | op_column
    if ctx.cursor().check(TokenKind::At) {
        // join sequence path
        let head = parse_one_join(ctx)?;
        let mut right = Vec::new();
        while ctx.cursor().check(TokenKind::Greater) {
            right.push(parse_one_join_right(ctx)?);
        }
        let seq_end = right
            .last()
            .map(|r| r.source_info.clone())
            .unwrap_or_else(|| head.source_info.clone());
        let join = JoinSequence {
            head,
            right,
            source_info: merge_si(&start_si, &seq_end),
        };
        // Optional `| op_column` continuation.
        let column = if ctx.cursor().eat(TokenKind::Pipe) {
            Some(parse_op_column_inner(ctx)?)
        } else {
            None
        };
        let end = column
            .as_ref()
            .map(|c| c.source_info().clone())
            .unwrap_or_else(|| join.source_info.clone());
        return Ok(JoinColWithDbOrConstant {
            db,
            join: Some(join),
            column,
            literal: None,
            source_info: merge_si(&start_si, &end),
        });
    }

    // op_column path.
    let column = parse_op_column_inner(ctx)?;
    let end = column.source_info().clone();
    Ok(JoinColWithDbOrConstant {
        db,
        join: None,
        column: Some(column),
        literal: None,
        source_info: merge_si(&start_si, &end),
    })
}

/// Parse just the `op_column` half of `colWithDbOrConstant` — i.e.,
/// an `OpColumn` value (Target or Aliased), without the surrounding
/// db-qualifier or join-sequence handling. Used by
/// `parse_join_col_with_db_or_constant`.
fn parse_op_column_inner(ctx: &mut ParserContext<'_>) -> Result<OpColumn, ParseError> {
    if ctx.cursor().check(TokenKind::LBrace) {
        return parse_target_column(ctx);
    }
    // Aliased form: name (.scope)? PRIMARY KEY?
    let alias = parse_relational_identifier(ctx)?;
    let mut scope = Vec::new();
    while ctx.cursor().eat(TokenKind::Dot) && scope.len() < 2 {
        scope.push(parse_relational_identifier(ctx)?);
    }
    let scope_end = scope
        .last()
        .map(|s| s.source_info.clone())
        .unwrap_or_else(|| alias.source_info.clone());
    let pk_end = consume_primary_key_flag(ctx)?;
    let primary_key = pk_end.is_some();
    let end_si = pk_end.unwrap_or(scope_end);
    Ok(OpColumn::Aliased {
        db: None,
        alias: alias.clone(),
        scope,
        primary_key,
        source_info: merge_si(&alias.source_info, &end_si),
    })
}

fn parse_constant_literal(ctx: &mut ParserContext<'_>) -> Result<OpLiteral, ParseError> {
    let peek_kind = ctx.cursor().peek().kind;
    match peek_kind {
        TokenKind::StringLiteral => parse_string_literal(ctx),
        TokenKind::IntegerLiteral => parse_integer_literal(ctx),
        TokenKind::FloatLiteral => parse_float_literal(ctx),
        TokenKind::Plus | TokenKind::Minus => {
            let sign = ctx.cursor().advance().clone();
            let lit_tok = ctx.cursor().peek().clone();
            match lit_tok.kind {
                TokenKind::IntegerLiteral => {
                    ctx.cursor().advance();
                    let value: i64 =
                        format!("{}{}", sign.text, lit_tok.text)
                            .parse()
                            .map_err(|_| {
                                err(
                                    format!(
                                        "expected integer literal, got '{}{}'",
                                        sign.text, lit_tok.text
                                    ),
                                    lit_tok.source_info.clone(),
                                )
                            })?;
                    Ok(OpLiteral::Integer {
                        value,
                        source_info: merge_si(&sign.source_info, &lit_tok.source_info),
                    })
                }
                TokenKind::FloatLiteral => {
                    ctx.cursor().advance();
                    let value: f64 =
                        format!("{}{}", sign.text, lit_tok.text)
                            .parse()
                            .map_err(|_| {
                                err(
                                    format!(
                                        "expected float literal, got '{}{}'",
                                        sign.text, lit_tok.text
                                    ),
                                    lit_tok.source_info.clone(),
                                )
                            })?;
                    Ok(OpLiteral::Float {
                        value,
                        source_info: merge_si(&sign.source_info, &lit_tok.source_info),
                    })
                }
                _ => Err(err_unexpected(
                    "numeric literal after sign",
                    &lit_tok.text,
                    lit_tok.source_info,
                )),
            }
        }
        _ => {
            let tok = ctx.cursor().peek().clone();
            Err(err_unexpected(
                "constant literal",
                &tok.text,
                tok.source_info,
            ))
        }
    }
}

fn parse_one_join(ctx: &mut ParserContext<'_>) -> Result<OneJoin, ParseError> {
    let at = ctx.cursor().expect(TokenKind::At)?;
    let name = parse_relational_identifier(ctx)?;
    let span = merge_si(&at.source_info, &name.source_info);
    Ok(OneJoin {
        name,
        source_info: span,
    })
}

fn parse_one_join_right(ctx: &mut ParserContext<'_>) -> Result<OneJoinRight, ParseError> {
    let gt = ctx.cursor().expect(TokenKind::Greater)?;
    // `(group)?`
    let group_id = if ctx.cursor().check(TokenKind::LParen)
        && ctx.cursor().peek_kind_at(1) == TokenKind::Identifier
        && ctx.cursor().peek_kind_at(2) == TokenKind::RParen
    {
        ctx.cursor().expect(TokenKind::LParen)?;
        let id = parse_relational_identifier(ctx)?;
        ctx.cursor().expect(TokenKind::RParen)?;
        Some(id)
    } else {
        None
    };
    let db = parse_optional_db_qualifier(ctx)?;
    let join = parse_one_join(ctx)?;
    let span = merge_si(&gt.source_info, &join.source_info);
    Ok(OneJoinRight {
        group_id,
        db,
        join,
        source_info: span,
    })
}

// ---------------------------------------------------------------------------
// Mapping elements: bare line | scope-wrapped lines
// ---------------------------------------------------------------------------

fn parse_mapping_element(ctx: &mut ParserContext<'_>) -> Result<MappingElement, ParseError> {
    if is_keyword(ctx.cursor(), "scope") {
        Ok(MappingElement::Scope(parse_scope(ctx)?))
    } else {
        Ok(MappingElement::Single(parse_single_mapping_line(ctx)?))
    }
}

fn parse_scope(ctx: &mut ParserContext<'_>) -> Result<ScopedMapping, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // 'scope'
    debug_assert_eq!(kw.text.as_str(), "scope");
    ctx.cursor().expect(TokenKind::LParen)?;
    let db = parse_required_db_qualifier(ctx)?;
    let scope = if !ctx.cursor().check(TokenKind::RParen) {
        Some(parse_simple_scope_info(ctx)?)
    } else {
        None
    };
    ctx.cursor().expect(TokenKind::RParen)?;
    ctx.cursor().expect(TokenKind::LParen)?;
    let mut mapping_lines = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        loop {
            mapping_lines.push(parse_single_mapping_line(ctx)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(ScopedMapping {
        db,
        scope,
        mapping_lines,
        source_info: merge_si(&kw.source_info, &close.source_info),
    })
}

fn parse_single_mapping_line(ctx: &mut ParserContext<'_>) -> Result<SingleMappingLine, ParseError> {
    if ctx.cursor().check(TokenKind::Plus) {
        Ok(SingleMappingLine::Plus(parse_plus_mapping_line(ctx)?))
    } else {
        Ok(SingleMappingLine::NonePlus(parse_none_plus_mapping_line(
            ctx,
        )?))
    }
}

fn parse_none_plus_mapping_line(
    ctx: &mut ParserContext<'_>,
) -> Result<NonePlusMappingLine, ParseError> {
    let property = parse_relational_identifier(ctx)?;
    let mut source_id: Option<SpannedString> = None;
    let mut target_id: Option<SpannedString> = None;
    if ctx.cursor().eat(TokenKind::LBracket) {
        source_id = Some(parse_relational_identifier(ctx)?);
        if ctx.cursor().eat(TokenKind::Comma) {
            target_id = Some(parse_relational_identifier(ctx)?);
        }
        ctx.cursor().expect(TokenKind::RBracket)?;
    }
    // Stage-6 dispatch: `(` opens an embedded body; `:` continues
    // with the Stage-5 relational mapping form.
    let value = if ctx.cursor().check(TokenKind::LParen) {
        NonePlusMappingValue::Embedded(parse_embedded_mapping(ctx)?)
    } else {
        NonePlusMappingValue::Relational(parse_relational_mapping(ctx)?)
    };
    let value_si = match &value {
        NonePlusMappingValue::Relational(m) => m.source_info.clone(),
        NonePlusMappingValue::Embedded(e) => e.source_info.clone(),
    };
    let span = merge_si(&property.source_info, &value_si);
    Ok(NonePlusMappingLine {
        property,
        source_id,
        target_id,
        value,
        source_info: span,
    })
}

// ---------------------------------------------------------------------------
// Stage 6: embedded / inline / otherwise property mappings
//
// Java grammar (RelationalParser.g4 lines 240-285):
//
//   embeddedMapping :
//       '(' (primaryKey? singleMappingLines)? ')'
//       (otherwiseEmbeddedMapping | inline)? ;
//   inline           : INLINE '[' identifier ']' ;
//   otherwiseEmbeddedMapping
//                    : OTHERWISE '(' otherwisePropertyMappings ')' ;
//   otherwisePropertyMappings
//                    : otherwisePropertyMapping (',' otherwisePropertyMapping)* ;
//   otherwisePropertyMapping
//                    : '[' identifier ']' ':' otherwiseJoin ;
//   otherwiseJoin    : database? joinSequence ;
// ---------------------------------------------------------------------------

fn parse_embedded_mapping(ctx: &mut ParserContext<'_>) -> Result<EmbeddedMapping, ParseError> {
    let open = ctx.cursor().expect(TokenKind::LParen)?;

    // Optional `~primaryKey(...)` then mapping lines; `()` is empty.
    let mut primary_key: Option<Vec<JoinColWithDbOrConstant>> = None;
    if ctx.cursor().check(TokenKind::Tilde) && ctx.cursor().peek_kind_at(1) == TokenKind::Identifier
    {
        // Peek past `~`; if the next identifier is "primaryKey", commit.
        // We can't read the text without advancing, so save and check
        // by advancing then validating the text.
        let saved_kind = ctx.cursor().peek_kind();
        let _ = saved_kind;
        // Commit-on-Tilde: consume `~`, then `primaryKey`, else error.
        ctx.cursor().expect(TokenKind::Tilde)?;
        let kw = ctx.cursor().expect(TokenKind::Identifier)?;
        if kw.text.as_str() != "primaryKey" {
            return Err(err(
                format!(
                    "embedded mapping body only supports `~primaryKey(...)`; \
                     got `~{}`",
                    kw.text
                ),
                kw.source_info,
            ));
        }
        primary_key = Some(parse_paren_join_col_list(ctx)?);
    }

    let mut mapping_lines: Vec<SingleMappingLine> = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        loop {
            mapping_lines.push(parse_single_mapping_line(ctx)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;

    // Optional trailing `Inline [id]` or `Otherwise (…)`.
    let trailer = parse_optional_embedded_mapping_trailer(ctx)?;
    let end_si = trailer
        .as_ref()
        .map(|t| match t {
            EmbeddedMappingTrailer::Inline(r) => r.source_info.clone(),
            EmbeddedMappingTrailer::Otherwise(maps) => maps
                .last()
                .map(|m| m.source_info.clone())
                .unwrap_or_else(|| close.source_info.clone()),
        })
        .unwrap_or_else(|| close.source_info.clone());
    Ok(EmbeddedMapping {
        primary_key,
        mapping_lines,
        trailer,
        source_info: merge_si(&open.source_info, &end_si),
    })
}

fn parse_optional_embedded_mapping_trailer(
    ctx: &mut ParserContext<'_>,
) -> Result<Option<EmbeddedMappingTrailer>, ParseError> {
    if !ctx.cursor().check(TokenKind::Identifier) {
        return Ok(None);
    }
    let next_text = ctx.cursor().peek().text.clone();
    match next_text.as_str() {
        "Inline" => {
            let kw = ctx.cursor().expect(TokenKind::Identifier)?;
            ctx.cursor().expect(TokenKind::LBracket)?;
            let id = parse_relational_identifier(ctx)?;
            let close = ctx.cursor().expect(TokenKind::RBracket)?;
            Ok(Some(EmbeddedMappingTrailer::Inline(InlineRef {
                id,
                source_info: merge_si(&kw.source_info, &close.source_info),
            })))
        }
        "Otherwise" => {
            ctx.cursor().expect(TokenKind::Identifier)?; // 'Otherwise'
            ctx.cursor().expect(TokenKind::LParen)?;
            let mut mappings: Vec<OtherwisePropertyMapping> = Vec::new();
            if !ctx.cursor().check(TokenKind::RParen) {
                loop {
                    mappings.push(parse_otherwise_property_mapping(ctx)?);
                    if !ctx.cursor().eat(TokenKind::Comma) {
                        break;
                    }
                }
            }
            ctx.cursor().expect(TokenKind::RParen)?;
            Ok(Some(EmbeddedMappingTrailer::Otherwise(mappings)))
        }
        _ => Ok(None),
    }
}

fn parse_otherwise_property_mapping(
    ctx: &mut ParserContext<'_>,
) -> Result<OtherwisePropertyMapping, ParseError> {
    let open = ctx.cursor().expect(TokenKind::LBracket)?;
    let property = parse_relational_identifier(ctx)?;
    ctx.cursor().expect(TokenKind::RBracket)?;
    ctx.cursor().expect(TokenKind::Colon)?;
    let otherwise_join = parse_otherwise_join(ctx)?;
    let span = merge_si(&open.source_info, &otherwise_join.source_info);
    Ok(OtherwisePropertyMapping {
        property,
        otherwise_join,
        source_info: span,
    })
}

fn parse_otherwise_join(ctx: &mut ParserContext<'_>) -> Result<OtherwiseJoin, ParseError> {
    let start_si = ctx.cursor().peek().source_info.clone();
    let db = parse_optional_db_qualifier(ctx)?;
    // joinSequence is required: must start with `@`.
    let head = parse_one_join(ctx)?;
    let mut right = Vec::new();
    while ctx.cursor().check(TokenKind::Greater) {
        right.push(parse_one_join_right(ctx)?);
    }
    let seq_end = right
        .last()
        .map(|r| r.source_info.clone())
        .unwrap_or_else(|| head.source_info.clone());
    let join_sequence = JoinSequence {
        head,
        right,
        source_info: merge_si(&start_si, &seq_end),
    };
    Ok(OtherwiseJoin {
        db,
        join_sequence,
        source_info: merge_si(&start_si, &seq_end),
    })
}

fn parse_plus_mapping_line(ctx: &mut ParserContext<'_>) -> Result<PlusMappingLine, ParseError> {
    let plus = ctx.cursor().expect(TokenKind::Plus)?;
    let property = parse_relational_identifier(ctx)?;
    let local = parse_local_mapping_property(ctx)?;
    let mapping = parse_relational_mapping(ctx)?;
    let span = merge_si(&plus.source_info, &mapping.source_info);
    Ok(PlusMappingLine {
        property,
        local,
        mapping,
        source_info: span,
    })
}

fn parse_local_mapping_property(
    ctx: &mut ParserContext<'_>,
) -> Result<LocalMappingProperty, ParseError> {
    let colon = ctx.cursor().expect(TokenKind::Colon)?;
    let type_path = parse_packageable_ptr(ctx)?;
    ctx.cursor().expect(TokenKind::LBracket)?;
    let lower = parse_local_mult_part(ctx)?;
    // Pure's lexer emits two consecutive `Dot` tokens for `..`.
    let upper =
        if ctx.cursor().check(TokenKind::Dot) && ctx.cursor().peek_kind_at(1) == TokenKind::Dot {
            ctx.cursor().expect(TokenKind::Dot)?;
            ctx.cursor().expect(TokenKind::Dot)?;
            Some(parse_local_mult_part(ctx)?)
        } else {
            None
        };
    let close = ctx.cursor().expect(TokenKind::RBracket)?;
    Ok(LocalMappingProperty {
        type_path,
        mult_lower: lower,
        mult_upper: upper,
        source_info: merge_si(&colon.source_info, &close.source_info),
    })
}

fn parse_local_mult_part(ctx: &mut ParserContext<'_>) -> Result<SpannedString, ParseError> {
    let tok = ctx.cursor().peek().clone();
    match tok.kind {
        TokenKind::IntegerLiteral => {
            ctx.cursor().advance();
            Ok(SpannedString {
                value: SmolStr::new(tok.text),
                source_info: tok.source_info,
            })
        }
        TokenKind::Star => {
            ctx.cursor().advance();
            Ok(SpannedString {
                value: SmolStr::new("*"),
                source_info: tok.source_info,
            })
        }
        _ => Err(err_unexpected(
            "INTEGER or '*' (multiplicity)",
            &tok.text,
            tok.source_info,
        )),
    }
}

fn parse_relational_mapping(ctx: &mut ParserContext<'_>) -> Result<RelationalMapping, ParseError> {
    let colon = ctx.cursor().expect(TokenKind::Colon)?;
    let transformer = if is_keyword(ctx.cursor(), "EnumerationMapping") {
        Some(parse_transformer(ctx)?)
    } else {
        None
    };
    let value = parse_join_col_with_db_or_constant(ctx)?;
    let span = merge_si(&colon.source_info, &value.source_info);
    Ok(RelationalMapping {
        transformer,
        value,
        source_info: span,
    })
}

fn parse_transformer(ctx: &mut ParserContext<'_>) -> Result<Transformer, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?;
    debug_assert_eq!(kw.text.as_str(), "EnumerationMapping");
    let id = parse_relational_identifier(ctx)?;
    let close = ctx.cursor().expect(TokenKind::Colon)?;
    Ok(Transformer {
        enumeration_mapping: id,
        source_info: merge_si(&kw.source_info, &close.source_info),
    })
}
