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

use super::{Parser, R, split_package_name};

impl Parser {
    /// Parses: `Primitive fqn::Name extends BaseType`
    pub(crate) fn parse_primitive_def(&mut self) -> R<Element> {
        let si = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Primitive)?;
        let path = self.parse_package_path()?;
        let (pkg, name) = split_package_name(&path);
        self.cursor.expect(TokenKind::Extends)?;
        let super_type = self.parse_type_reference()?;
        Ok(Element::Primitive(PrimitiveDef {
            package: pkg,
            name,
            super_type,
            source_info: si,
        }))
    }
}
