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

//! End-to-end test for tagged-island dispatch through `Hash` +
//! tag-token + plug-in.
//!
//! Two cases:
//! 1. **Curly-body** (`#>{ body }#`) — plug-in expects `LBrace`,
//!    consumes body, expects `RBraceHash`.
//! 2. **Raw-body** (`#TAG content #`) — plug-in scans tokens until
//!    a closing `Hash`.
//!
//! Locks the contract that the host parser correctly distinguishes
//! `HashLBrace` (empty tag, legacy graph-fetch path) from `Hash`
//! (tagged path), and that tagged plug-ins own their closing
//! delimiter.

use std::any::Any;

use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::IslandParser;
use legend_pure_parser_parser::error::ParseError;
use smol_str::SmolStr;

// ---------------------------------------------------------------------------
// Mock content + parser for `#>{ body }#` (curly-body, tag = ">").
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct CurlyEcho {
    text: SmolStr,
    source_info: SourceInfo,
}

impl IslandContent for CurlyEcho {
    fn tag(&self) -> &str {
        ">"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn IslandContent> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn IslandContent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

struct CurlyEchoParser;

impl IslandParser for CurlyEchoParser {
    fn tag(&self) -> &str {
        ">"
    }
    fn parse(
        &self,
        ctx: &mut legend_pure_parser_parser::ParserContext<'_>,
    ) -> Result<Box<dyn IslandContent>, ParseError> {
        let cursor = ctx.cursor();
        cursor.expect(TokenKind::LBrace)?;
        // Echo every identifier as concatenated text until RBraceHash.
        let mut buf = String::new();
        let start = cursor.current_source_info();
        while !cursor.check(TokenKind::RBraceHash) && !cursor.check(TokenKind::Eof) {
            let tok = cursor.advance().clone();
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(&tok.text);
        }
        let close = cursor.expect(TokenKind::RBraceHash)?;
        Ok(Box::new(CurlyEcho {
            text: SmolStr::new(buf),
            source_info: SourceInfo::new(
                start.source.clone(),
                start.start_line,
                start.start_column,
                close.source_info.end_line,
                close.source_info.end_column,
            ),
        }))
    }
}

// ---------------------------------------------------------------------------
// Mock content + parser for `#RAW content #` (raw-body, tag = "RAW").
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct RawEcho {
    text: SmolStr,
    source_info: SourceInfo,
}

impl IslandContent for RawEcho {
    fn tag(&self) -> &str {
        "RAW"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn IslandContent> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn IslandContent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

struct RawEchoParser;

impl IslandParser for RawEchoParser {
    fn tag(&self) -> &str {
        "RAW"
    }
    fn parse(
        &self,
        ctx: &mut legend_pure_parser_parser::ParserContext<'_>,
    ) -> Result<Box<dyn IslandContent>, ParseError> {
        let cursor = ctx.cursor();
        // Body extends until the closing single Hash.
        let mut buf = String::new();
        let start = cursor.current_source_info();
        while !cursor.check(TokenKind::Hash) && !cursor.check(TokenKind::Eof) {
            let tok = cursor.advance().clone();
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(&tok.text);
        }
        let close = cursor.expect(TokenKind::Hash)?;
        Ok(Box::new(RawEcho {
            text: SmolStr::new(buf),
            source_info: SourceInfo::new(
                start.source.clone(),
                start.start_line,
                start.start_column,
                close.source_info.end_line,
                close.source_info.end_column,
            ),
        }))
    }
}

// ---------------------------------------------------------------------------
// Helper: parse a single function body and return the lone expression.
// ---------------------------------------------------------------------------

fn parse_body_expression(src: &str, parsers: Vec<Box<dyn IslandParser>>) -> Expression {
    // Wrap the island in a function body so it parses as an expression.
    let wrapped = format!("function test::f(): String[1] {{ {src} }}");
    let file =
        legend_pure_parser_parser::parse_with_islands(&wrapped, "tag_dispatch.pure", parsers)
            .unwrap_or_else(|p| {
                panic!(
                    "parse failed: {:?}",
                    p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
                )
            });

    let elem = &file.sections[0].elements[0];
    let legend_pure_parser_ast::element::Element::Function(f) = elem else {
        panic!("expected Function, got {elem:?}");
    };
    f.body
        .first()
        .cloned()
        .expect("function body has at least one expression")
}

#[test]
fn tagged_curly_island_dispatches_via_hash_path() {
    let parsers: Vec<Box<dyn IslandParser>> = vec![Box::new(CurlyEchoParser)];
    let expr = parse_body_expression("#>{ alpha beta }#", parsers);
    let isl = match &expr {
        Expression::Island(i) => i,
        other => panic!("expected Island, got {other:?}"),
    };
    let echo = isl
        .content
        .as_any()
        .downcast_ref::<CurlyEcho>()
        .expect("CurlyEcho");
    assert_eq!(echo.text.as_str(), "alpha beta");
}

#[test]
fn tagged_raw_island_dispatches_and_consumes_own_closing_hash() {
    let parsers: Vec<Box<dyn IslandParser>> = vec![Box::new(RawEchoParser)];
    let expr = parse_body_expression("#RAW alpha beta #", parsers);
    let isl = match &expr {
        Expression::Island(i) => i,
        other => panic!("expected Island, got {other:?}"),
    };
    let echo = isl
        .content
        .as_any()
        .downcast_ref::<RawEcho>()
        .expect("RawEcho");
    assert_eq!(echo.text.as_str(), "alpha beta");
}

#[test]
fn unknown_tag_is_a_parse_error() {
    let parsers: Vec<Box<dyn IslandParser>> = vec![Box::new(CurlyEchoParser)];
    let wrapped = "function test::f(): String[1] { #UNKNOWN_TAG something # }";
    let result =
        legend_pure_parser_parser::parse_with_islands(wrapped, "tag_dispatch.pure", parsers);
    assert!(
        result.is_err(),
        "unregistered tag should fail; got {:?}",
        result.map(|f| f.element_count())
    );
}
