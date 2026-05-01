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

//! `IslandParser` for `#>{ qualified::Store.relation }#`.
//!
//! Tag: `">"` — dispatched by the host parser when it sees a `Hash`
//! token followed by a single `Greater` token.
//!
//! Parser shape (mirrors Java
//! `org.finos.legend.pure.m2.inlinedsl.store.RelationStoreAccessor::parse`):
//!
//! ```text
//! #>{ pkg::sub::Store.tbl[.col[.…]] }#
//!     │             │
//!     │             └── path[1..] — dot-separated identifiers
//!     └─── path[0]   — qualified store path (`::`-separated)
//! ```
//!
//! Body delimiters are curly: the parser consumes `LBrace`, the path,
//! then `RBraceHash` (the host parser does **not** strip the closing
//! delimiter for tagged islands — that's the plug-in's contract).

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::{IslandParser, ParserContext};
use smol_str::SmolStr;

use crate::ast::{RelationStoreAccessorRef, TAG};

/// Parser for `#>{ store::path.col }#` islands.
pub struct RelationStoreAccessorParser;

impl IslandParser for RelationStoreAccessorParser {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
        let si = ctx.cursor().current_source_info();
        ctx.cursor().expect(TokenKind::LBrace)?;

        // First segment: a qualified package path (`my::mainDb`). Flatten
        // it into a single `::`-joined string to match the Java parser's
        // `info.split("\\.")` shape — the leading dotted segment keeps
        // its `::` separators inline.
        let pkg = ctx.parse_package_path()?;
        let mut path: Vec<SmolStr> = vec![SmolStr::new(pkg.to_string())];

        // Trailing `.identifier` segments after the qualified store path.
        while ctx.cursor().check(TokenKind::Dot) {
            ctx.cursor().advance();
            let (seg, _) = ctx.cursor().expect_identifier_or_keyword()?;
            path.push(seg);
        }

        if path.len() < 2 {
            return Err(ParseError::Unexpected {
                message: "RelationStoreAccessor must be of the form `#>{a::Store.table}#`"
                    .to_string(),
                source_info: si,
            });
        }

        ctx.cursor().expect(TokenKind::RBraceHash)?;
        Ok(Box::new(RelationStoreAccessorRef {
            path,
            source_info: si,
        }))
    }
}

/// Convenience helper — `vec![Box::new(RelationStoreAccessorParser)]`
/// for callers that want the store-DSL plug-in registered.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn IslandParser>> {
    vec![Box::new(RelationStoreAccessorParser)]
}
