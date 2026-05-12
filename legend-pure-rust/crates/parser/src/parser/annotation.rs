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
use legend_pure_parser_ast::SourceInfo;
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
        // Capture position of the dot (still on the cursor's current
        // token) so the profile pointer's span ends right before it.
        let dot_si = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Dot)?;
        let (value, value_si) = self.cursor.expect_identifier_or_keyword()?;
        let (pkg, profile_name) = split_package_name(&profile_path);
        // Profile pointer span: from start through to the dot
        // (exclusive). That's the clickable region for "jump to
        // profile" navigation.
        let profile_si = SourceInfo::new(
            start.source.clone(),
            start.start_line,
            start.start_column,
            dot_si.start_line,
            dot_si.start_column,
        );
        let profile = PackageableElementPtr {
            package: pkg,
            name: profile_name,
            source_info: profile_si,
        };
        // Combined ref span covers the whole `Profile.stereotype`
        // reference — used by the LSP locator so cursor clicks
        // anywhere inside the ref refine to the StereotypeRef
        // ValueSpec, not just on the profile name.
        let combined_si = SourceInfo::new(
            start.source.clone(),
            start.start_line,
            start.start_column,
            value_si.end_line,
            value_si.end_column,
        );
        Ok(StereotypePtr {
            profile,
            value,
            source_info: combined_si,
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
        // Position of the dot before consuming, so the profile pointer
        // span can end exactly there. Same shape as the stereotype
        // parser fix.
        let dot_si = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Dot)?;
        let (tag_name, tag_si) = self.cursor.expect_identifier_or_keyword()?;
        self.cursor.expect(TokenKind::Equals)?;
        // Tagged values support string concatenation: 'text' + 'more text'
        let value_tok = self.cursor.expect(TokenKind::StringLiteral)?;
        let mut value = unquote_string(&value_tok.text);
        while self.cursor.eat(TokenKind::Plus) {
            let next = self.cursor.expect(TokenKind::StringLiteral)?;
            value.push_str(&unquote_string(&next.text));
        }
        let (pkg, profile_name) = split_package_name(&profile_path);
        // Profile span: from start through to the dot (exclusive).
        // The clickable region for "jump to profile".
        let profile_si = SourceInfo::new(
            start.source.clone(),
            start.start_line,
            start.start_column,
            dot_si.start_line,
            dot_si.start_column,
        );
        let profile = PackageableElementPtr {
            package: pkg,
            name: profile_name,
            source_info: profile_si,
        };
        // The full `profile.tag` reference — the IDE's clickable
        // region. Excludes the `= 'value'` since the value isn't a
        // navigable target.
        let tag_combined_si = SourceInfo::new(
            start.source.clone(),
            start.start_line,
            start.start_column,
            tag_si.end_line,
            tag_si.end_column,
        );
        let tag = TagPtr {
            profile,
            value: tag_name,
            source_info: tag_combined_si.clone(),
        };
        Ok(TaggedValue {
            tag,
            value,
            source_info: tag_combined_si,
        })
    }
}
