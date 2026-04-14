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

use legend_pure_parser_ast::element::{Element, PrimitiveDef};
use legend_pure_parser_lexer::TokenKind;

use super::{Parser, R};

impl Parser {
    /// Parses: `Primitive [(<<stereos>> | {tags})*] fqn::Name[(params)] extends BaseType [\n constraints\n]`
    pub(crate) fn parse_primitive_def(&mut self) -> R<Element> {
        let si = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Primitive)?;
        let header = self.parse_element_header()?;

        // Skip optional type variable parameters: (x:Integer[1])
        if self.cursor.eat(TokenKind::LParen) {
            self.cursor
                .skip_balanced(TokenKind::LParen, TokenKind::RParen);
        }

        self.cursor.expect(TokenKind::Extends)?;
        let super_type = self.parse_type_reference()?;

        // Skip optional constraint block: [ $this < $x ]
        if self.cursor.eat(TokenKind::LBracket) {
            self.cursor
                .skip_balanced(TokenKind::LBracket, TokenKind::RBracket);
        }

        Ok(Element::Primitive(PrimitiveDef {
            package: header.package,
            name: header.name,
            super_type,
            source_info: si,
        }))
    }
}
