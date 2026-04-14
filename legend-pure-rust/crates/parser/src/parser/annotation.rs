// Copyright 2026 The Legend Authors
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

use super::Parser;
use super::R;
use super::{split_package_name, unquote_string};
use legend_pure_parser_ast::annotation::{
    PackageableElementPtr, StereotypePtr, TagPtr, TaggedValue,
};
use legend_pure_parser_lexer::TokenKind;

impl Parser {
    // ── Annotations ─────────────────────────────────────────────────────

    pub(crate) fn parse_stereotypes(&mut self) -> R<Vec<StereotypePtr>> {
        if !self.cursor.check(TokenKind::LessLess) {
            return Ok(vec![]);
        }
        self.cursor.advance(); // <<
        let mut result = Vec::new();
        loop {
            result.push(self.parse_stereotype_ptr()?);
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::GreaterGreater)?;
        Ok(result)
    }

    pub(crate) fn parse_stereotype_ptr(&mut self) -> R<StereotypePtr> {
        let start = self.cursor.current_source_info();
        let profile_path = self.parse_package_path()?;
        self.cursor.expect(TokenKind::Dot)?;
        let (value, _) = self.cursor.expect_identifier_or_keyword()?;
        let (pkg, profile_name) = split_package_name(&profile_path);
        let profile = PackageableElementPtr {
            package: pkg,
            name: profile_name,
            source_info: start.clone(),
        };
        Ok(StereotypePtr {
            profile,
            value,
            source_info: start,
        })
    }

    pub(crate) fn parse_tagged_values(&mut self) -> R<Vec<TaggedValue>> {
        if !self.cursor.check(TokenKind::LBrace) {
            return Ok(vec![]);
        }
        // Lookahead: is this {profile.tag = 'val'} or a body {prop: Type}?
        // Tagged values always start with identifier.identifier =
        if !self.is_tagged_value_start() {
            return Ok(vec![]);
        }
        self.cursor.advance(); // {
        let mut result = Vec::new();
        loop {
            result.push(self.parse_tagged_value()?);
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::RBrace)?;
        Ok(result)
    }

    pub(crate) fn is_tagged_value_start(&self) -> bool {
        // Pattern: { <ident> . <ident> = <string> }
        if self.cursor.peek_kind() != TokenKind::LBrace {
            return false;
        }
        let k1 = self.cursor.peek_kind_at(1);
        let k2 = self.cursor.peek_kind_at(2);
        (k1 == TokenKind::Identifier || k1 == TokenKind::StringLiteral)
            && (k2 == TokenKind::Dot || k2 == TokenKind::PathSep)
    }

    pub(crate) fn parse_tagged_value(&mut self) -> R<TaggedValue> {
        let start = self.cursor.current_source_info();
        let profile_path = self.parse_package_path()?;
        self.cursor.expect(TokenKind::Dot)?;
        let (tag_name, _) = self.cursor.expect_identifier_or_keyword()?;
        self.cursor.expect(TokenKind::Equals)?;
        let value_tok = self.cursor.expect(TokenKind::StringLiteral)?;
        let value = unquote_string(&value_tok.text);
        let (pkg, profile_name) = split_package_name(&profile_path);
        let profile = PackageableElementPtr {
            package: pkg,
            name: profile_name,
            source_info: start.clone(),
        };
        let tag = TagPtr {
            profile,
            value: tag_name,
            source_info: start.clone(),
        };
        Ok(TaggedValue {
            tag,
            value,
            source_info: start,
        })
    }
}
