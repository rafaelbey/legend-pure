use super::Parser;
use super::R;
use legend_pure_parser_ast::element::{Element, EnumDef, EnumValue};
use legend_pure_parser_lexer::TokenKind;

impl Parser {
    // ── Enum ────────────────────────────────────────────────────────────

    pub(crate) fn parse_enum(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Enum)?;
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (package, name, _) = self.parse_qualified_name()?;
        self.cursor.expect(TokenKind::LBrace)?;
        let mut values = Vec::new();
        while !self.cursor.check(TokenKind::RBrace) {
            let val_stereos = self.parse_stereotypes()?;
            let val_tvs = self.parse_tagged_values()?;
            let (val_name, val_si) = self.cursor.expect_identifier_or_keyword()?;
            values.push(EnumValue {
                name: val_name,
                stereotypes: val_stereos,
                tagged_values: val_tvs,
                source_info: val_si,
            });
            self.cursor.eat(TokenKind::Comma);
        }
        self.cursor.expect(TokenKind::RBrace)?;
        Ok(Element::Enumeration(EnumDef {
            package,
            name,
            values,
            stereotypes,
            tagged_values,
            source_info: start,
        }))
    }
}
