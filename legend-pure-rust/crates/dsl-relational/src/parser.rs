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
    ColumnDef, DatabaseDef, DatabaseElement, DatabaseInclude, Filter, Join, MultiGrainFilter,
    SECTION_KIND, Schema, Table, TokenSlice, View,
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

    // Stage-3-deferred guard: Java grammar allows
    // `Table x ( milestoning ( … ) col1 INT, col2 VARCHAR(200) )`.
    // Stage 1 doesn't parse milestoning specs; reject early with a
    // pointed message rather than failing further down.
    if is_keyword(ctx.cursor(), "milestoning") {
        let bad = ctx.cursor().peek().source_info.clone();
        return Err(err(
            "Table milestoning specs are not supported in Stage 1 (deferred to Stage 3 — \
             see ~/.claude/plans/lets-plan-for-implementing-lucky-scott.md)"
                .into(),
            bad,
        ));
    }

    let columns = parse_columns(ctx)?;
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(Table {
        name,
        columns,
        source_info: merge_si(&kw.source_info, &close.source_info),
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
    let op_body = capture_paren_body(ctx)?;
    let end_si = op_body.source_info.clone();
    Ok(Join {
        name,
        op_body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

fn parse_filter(ctx: &mut ParserContext<'_>) -> Result<Filter, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "Filter"
    let name = parse_relational_identifier(ctx)?;
    let op_body = capture_paren_body(ctx)?;
    let end_si = op_body.source_info.clone();
    Ok(Filter {
        name,
        op_body,
        source_info: merge_si(&kw.source_info, &end_si),
    })
}

fn parse_multi_grain_filter(ctx: &mut ParserContext<'_>) -> Result<MultiGrainFilter, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "MultiGrainFilter"
    let name = parse_relational_identifier(ctx)?;
    let op_body = capture_paren_body(ctx)?;
    let end_si = op_body.source_info.clone();
    Ok(MultiGrainFilter {
        name,
        op_body,
        source_info: merge_si(&kw.source_info, &end_si),
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
