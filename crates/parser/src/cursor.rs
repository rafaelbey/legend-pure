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

//! Token cursor with peek/advance/expect operations.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_lexer::{Token, TokenKind};
use smol_str::SmolStr;

use crate::error::ParseError;

/// A cursor over a token stream, supporting peek, advance, and expect.
pub(crate) struct Cursor {
    tokens: Vec<Token>,
    pos: usize,
}

impl Cursor {
    /// Creates a new cursor over the given token stream.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// Returns the current token without advancing.
    pub fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    /// Returns the kind of the current token.
    pub fn peek_kind(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    /// Returns the kind of the token at `offset` positions ahead.
    pub fn peek_kind_at(&self, offset: usize) -> TokenKind {
        let idx = self.pos + offset;
        if idx < self.tokens.len() {
            self.tokens[idx].kind
        } else {
            TokenKind::Eof
        }
    }

    /// Advances the cursor and returns the consumed token.
    pub fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if tok.kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    /// Returns true if the current token is of the given kind.
    pub fn check(&self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    /// If the current token matches `kind`, advances and returns `true`.
    pub fn eat(&mut self, kind: TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Expects the current token to be `kind`, advances, and returns the token.
    /// Returns an error if the current token doesn't match.
    pub fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance().clone())
        } else {
            Err(ParseError::expected(
                kind.description(),
                self.peek_kind(),
                self.peek().source_info.clone(),
            ))
        }
    }

    /// Expects the current token to be an identifier, advances, and returns its text.
    pub fn expect_identifier(&mut self) -> Result<(SmolStr, SourceInfo), ParseError> {
        let tok = self.peek().clone();
        // Keywords can also be used as identifiers in certain positions
        if tok.kind == TokenKind::Identifier
            || tok.kind == TokenKind::StringLiteral
        {
            self.advance();
            Ok((tok.text.clone(), tok.source_info.clone()))
        } else {
            Err(ParseError::expected(
                "identifier",
                tok.kind,
                tok.source_info.clone(),
            ))
        }
    }

    /// Like `expect_identifier`, but also accepts keywords as identifiers.
    /// This is needed where Pure allows keywords as names (e.g., in qualified paths).
    pub fn expect_identifier_or_keyword(&mut self) -> Result<(SmolStr, SourceInfo), ParseError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Identifier
            | TokenKind::StringLiteral
            | TokenKind::Class
            | TokenKind::Enum
            | TokenKind::Profile
            | TokenKind::Association
            | TokenKind::Measure
            | TokenKind::Function
            | TokenKind::Import
            | TokenKind::Extends
            | TokenKind::Let
            | TokenKind::Native
            | TokenKind::Stereotypes
            | TokenKind::Tags
            | TokenKind::Shared
            | TokenKind::Composite
            | TokenKind::None
            | TokenKind::True
            | TokenKind::False => {
                self.advance();
                Ok((tok.text.clone(), tok.source_info.clone()))
            }
            _ => Err(ParseError::expected(
                "identifier",
                tok.kind,
                tok.source_info.clone(),
            )),
        }
    }

    /// Returns the source info of the current token.
    pub fn current_source_info(&self) -> SourceInfo {
        self.peek().source_info.clone()
    }
}
