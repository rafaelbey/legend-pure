use super::Parser;
use super::R;
use crate::error::ParseError;
use legend_pure_parser_ast::annotation::SpannedString;
use legend_pure_parser_ast::element::{Element, ProfileDef};
use legend_pure_parser_lexer::TokenKind;

impl Parser {
    // ── Profile ─────────────────────────────────────────────────────────

    pub(crate) fn parse_profile(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Profile)?;
        let (package, name, _) = self.parse_qualified_name()?;
        self.cursor.expect(TokenKind::LBrace)?;

        let mut stereotypes = Vec::new();
        let mut tags = Vec::new();

        while !self.cursor.check(TokenKind::RBrace) {
            if self.cursor.check(TokenKind::Stereotypes) {
                self.cursor.advance();
                self.cursor.expect(TokenKind::Colon)?;
                self.cursor.expect(TokenKind::LBracket)?;
                loop {
                    let (val, si) = self.cursor.expect_identifier_or_keyword()?;
                    stereotypes.push(SpannedString {
                        value: val,
                        source_info: si,
                    });
                    if !self.cursor.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.cursor.expect(TokenKind::RBracket)?;
                self.cursor.expect(TokenKind::Semicolon)?;
            } else if self.cursor.check(TokenKind::Tags) {
                self.cursor.advance();
                self.cursor.expect(TokenKind::Colon)?;
                self.cursor.expect(TokenKind::LBracket)?;
                loop {
                    let (val, si) = self.cursor.expect_identifier_or_keyword()?;
                    tags.push(SpannedString {
                        value: val,
                        source_info: si,
                    });
                    if !self.cursor.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.cursor.expect(TokenKind::RBracket)?;
                self.cursor.expect(TokenKind::Semicolon)?;
            } else {
                return Err(ParseError::expected(
                    "'stereotypes' or 'tags'",
                    self.cursor.peek_kind(),
                    self.cursor.current_source_info(),
                ));
            }
        }
        self.cursor.expect(TokenKind::RBrace)?;
        Ok(Element::Profile(ProfileDef {
            package,
            name,
            stereotypes,
            tags,
            source_info: start,
        }))
    }
}
