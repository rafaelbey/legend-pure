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

//! `IslandParser` for `#TDS\n cols\n rows\n#`.
//!
//! Tag: `"TDS"`. Body shape is **raw-content** (no `{ … }`):
//!
//! ```text
//! #TDS                          ← `#` consumed by host;
//!   value, other:Integer, name  ← `Identifier("TDS")` consumed by host
//!   1, 3, A
//!   2, 4, B
//! #                              ← plug-in consumes this `Hash`
//! ```
//!
//! Lines are detected by comparing token start-lines (the lexer eats
//! newlines as whitespace). The first non-`Hash` line becomes the
//! column spec; subsequent lines become data rows.
//!
//! Column spec grammar (mirrors Java's `colSpec` rule used by
//! `TDSExtension.parse`):
//!
//! ```text
//! col       : name (':' type)? ('[' multiplicity ']')?
//! name      : Identifier | StringLiteral   (single-quoted name unquoted)
//! type      : qualifiedName                (`::`-joined; arity args TBD)
//! multiplicity : raw bracket contents kept verbatim for compiler resolution
//! ```

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::{IslandParser, ParserContext};
use smol_str::SmolStr;

use crate::ast::{TAG, TDSCell, TDSColumn, TDSColumnType, TDSExpr};

/// Parser for `#TDS\n cols\n rows\n#` islands.
pub struct TDSIslandParser;

impl IslandParser for TDSIslandParser {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
        let start_si = ctx.cursor().current_source_info();

        // Identify the header line by the start_line of the first
        // token we see after the opening `#TDS`. Anything sharing
        // that start_line is a header column spec.
        let header_line = ctx.cursor().peek().source_info.start_line;

        let mut columns: Vec<TDSColumn> = Vec::new();
        let mut rows: Vec<Vec<TDSCell>> = Vec::new();

        // -- Header --
        loop {
            if ctx.cursor().check(TokenKind::Hash) || ctx.cursor().check(TokenKind::Eof) {
                // Empty body — no header line at all.
                break;
            }
            // If the next token is on a later line, the header is done.
            if ctx.cursor().peek().source_info.start_line != header_line {
                break;
            }
            let col = parse_column_spec(ctx)?;
            columns.push(col);
            // Header columns separated by commas.
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }

        // -- Rows --
        while !ctx.cursor().check(TokenKind::Hash) && !ctx.cursor().check(TokenKind::Eof) {
            let row_line = ctx.cursor().peek().source_info.start_line;
            let mut row: Vec<TDSCell> = Vec::new();
            loop {
                if ctx.cursor().check(TokenKind::Hash) || ctx.cursor().check(TokenKind::Eof) {
                    break;
                }
                if ctx.cursor().peek().source_info.start_line != row_line {
                    break;
                }
                row.push(parse_cell(ctx)?);
                if !ctx.cursor().eat(TokenKind::Comma) {
                    break;
                }
            }
            if !row.is_empty() {
                rows.push(row);
            }
        }

        // Closing `#`.
        ctx.cursor().expect(TokenKind::Hash)?;

        Ok(Box::new(TDSExpr {
            columns,
            rows,
            source_info: start_si,
        }))
    }
}

/// Convenience helper — `vec![Box::new(TDSIslandParser)]` for callers
/// that want the TDS DSL plug-in registered.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn IslandParser>> {
    vec![Box::new(TDSIslandParser)]
}

// ---------------------------------------------------------------------------
// Column spec
// ---------------------------------------------------------------------------

fn parse_column_spec(ctx: &mut ParserContext<'_>) -> Result<TDSColumn, ParseError> {
    let si = ctx.cursor().current_source_info();
    let name = parse_column_name(ctx)?;

    let type_ref = if ctx.cursor().check(TokenKind::Colon) {
        ctx.cursor().advance();
        Some(parse_column_type(ctx)?)
    } else {
        None
    };

    Ok(TDSColumn {
        name,
        type_ref,
        source_info: si,
    })
}

fn parse_column_name(ctx: &mut ParserContext<'_>) -> Result<SmolStr, ParseError> {
    let tok = ctx.cursor().peek().clone();
    match tok.kind {
        TokenKind::StringLiteral => {
            ctx.cursor().advance();
            // Strip leading/trailing single quotes.
            let raw = tok.text.as_str();
            let stripped = raw
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(raw);
            Ok(SmolStr::new(stripped))
        }
        kind if kind.is_identifier_like() => {
            ctx.cursor().advance();
            Ok(tok.text)
        }
        other => Err(ParseError::Unexpected {
            message: format!(
                "Expected column name (identifier or quoted string), found {}",
                other.description()
            ),
            source_info: tok.source_info,
        }),
    }
}

fn parse_column_type(ctx: &mut ParserContext<'_>) -> Result<TDSColumnType, ParseError> {
    let si = ctx.cursor().current_source_info();

    // Qualified type name: ident (`::` ident)*. Stop at `[`, `,`, end-of-line, `Hash`.
    let (first, _) = ctx.cursor().expect_identifier_or_keyword()?;
    let mut name = first.to_string();
    while ctx.cursor().check(TokenKind::PathSep) {
        ctx.cursor().advance();
        let (seg, _) = ctx.cursor().expect_identifier_or_keyword()?;
        name.push_str("::");
        name.push_str(seg.as_str());
    }

    // Optional `(args)` for parametric types like `Numeric(10,4)`.
    // Capture as raw text for the compiler extension to interpret.
    if ctx.cursor().check(TokenKind::LParen) {
        let depth_start = name.len();
        name.push('(');
        ctx.cursor().advance();
        let mut depth = 1usize;
        while depth > 0 {
            if ctx.cursor().check(TokenKind::Eof) {
                return Err(ParseError::Unexpected {
                    message: format!(
                        "Expected ')' to close type-parameter list, found {}",
                        ctx.cursor().peek_kind().description()
                    ),
                    source_info: ctx.cursor().current_source_info(),
                });
            }
            let inner = ctx.cursor().advance().clone();
            match inner.kind {
                TokenKind::LParen => {
                    depth += 1;
                    name.push('(');
                }
                TokenKind::RParen => {
                    depth -= 1;
                    name.push(')');
                }
                _ => {
                    name.push_str(inner.text.as_str());
                }
            }
        }
        debug_assert!(name.len() > depth_start);
    }

    // Optional multiplicity bracket.
    let multiplicity = if ctx.cursor().check(TokenKind::LBracket) {
        ctx.cursor().advance();
        let mut buf = String::new();
        while !ctx.cursor().check(TokenKind::RBracket) {
            if ctx.cursor().check(TokenKind::Eof) {
                return Err(ParseError::Unexpected {
                    message: format!(
                        "Expected ']' to close multiplicity, found {}",
                        ctx.cursor().peek_kind().description()
                    ),
                    source_info: ctx.cursor().current_source_info(),
                });
            }
            let inner = ctx.cursor().advance().clone();
            buf.push_str(inner.text.as_str());
        }
        ctx.cursor().expect(TokenKind::RBracket)?;
        Some(SmolStr::new(buf))
    } else {
        None
    };

    Ok(TDSColumnType {
        name: SmolStr::new(name),
        multiplicity,
        source_info: si,
    })
}

// ---------------------------------------------------------------------------
// Cell
// ---------------------------------------------------------------------------

fn parse_cell(ctx: &mut ParserContext<'_>) -> Result<TDSCell, ParseError> {
    // CSV semantics: a TDS cell is the sequence of tokens on a single
    // row line between two commas (or between row-start/`#` and a
    // comma). Most cells collapse to one token — `1`, `aa`, `'foo'` —
    // but several real shapes span tokens:
    //
    //  - Negatives: `-3` is `[Minus, IntegerLiteral]`.
    //  - Decimals: `99.99D` is one `DecimalLiteral`, but `-99.99D` is
    //    `[Minus, DecimalLiteral]`.
    //  - Unquoted datetimes: `2024-01-29T00:32:34.000+0000` arrives as
    //    many tokens (identifier `T00`, `:`, integers, `+`, …).
    //
    // The Java side delegates to Deephaven CSV which is character-
    // delimited, not token-delimited; we mirror that by gathering every
    // token on the same source line until we hit `,`, `#`, EOF, or a
    // newline. Concatenating `text` reconstructs the source-form value
    // since the lexer eats whitespace and TDS cells don't have
    // meaningful internal whitespace outside of quoted strings (which
    // arrive atomically as `StringLiteral`).
    let first = ctx.cursor().peek().clone();
    if matches!(
        first.kind,
        TokenKind::Comma | TokenKind::Hash | TokenKind::Eof
    ) {
        return Err(ParseError::Unexpected {
            message: format!(
                "Expected TDS cell value, found {}",
                first.kind.description()
            ),
            source_info: first.source_info,
        });
    }
    let start_si = first.source_info.clone();
    let start_line = first.source_info.start_line;
    let mut raw = String::new();

    loop {
        let cur = ctx.cursor().peek();
        if matches!(
            cur.kind,
            TokenKind::Comma | TokenKind::Hash | TokenKind::Eof
        ) || cur.source_info.start_line != start_line
        {
            break;
        }
        let tok = ctx.cursor().advance().clone();
        raw.push_str(tok.text.as_str());
    }

    Ok(TDSCell {
        raw: SmolStr::new(raw),
        source_info: start_si,
    })
}
