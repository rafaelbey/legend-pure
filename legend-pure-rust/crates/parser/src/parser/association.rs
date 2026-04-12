use legend_pure_parser_ast::element::*;
use legend_pure_parser_lexer::TokenKind;
use super::R;
use super::Parser;

impl Parser {
    // ── Association ─────────────────────────────────────────────────────

    pub(crate) fn parse_association(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Association)?;
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (package, name, _) = self.parse_qualified_name()?;
        self.cursor.expect(TokenKind::LBrace)?;
        let (properties, qualified_properties) = self.parse_class_body()?;
        self.cursor.expect(TokenKind::RBrace)?;
        Ok(Element::Association(AssociationDef {
            package,
            name,
            properties,
            qualified_properties,
            stereotypes,
            tagged_values,
            source_info: start,
        }))
    }

}
