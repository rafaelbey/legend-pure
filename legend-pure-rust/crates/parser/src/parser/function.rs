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
use super::unquote_string;
use crate::error::ParseError;
use legend_pure_parser_ast::annotation::Parameter;
use legend_pure_parser_ast::element::{
    Element, FunctionDef, FunctionTest, FunctionTestAssertion, FunctionTestData,
    FunctionTestDataValue, NativeFunctionDef,
};
use legend_pure_parser_ast::type_ref::{RELATION_TYPE_SENTINEL, TypeReference};
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

impl Parser {
    // ── Function ────────────────────────────────────────────────────────

    pub(crate) fn parse_function(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Function)?;
        let header = self.parse_element_header()?;
        let (type_parameters, multiplicity_parameters) =
            self.parse_type_and_multiplicity_parameters()?;
        self.cursor.expect(TokenKind::LParen)?;
        let mut parameters = Vec::new();
        while !self.cursor.check(TokenKind::RParen) {
            parameters.push(self.parse_parameter()?);
            self.cursor.eat(TokenKind::Comma);
        }
        self.cursor.expect(TokenKind::RParen)?;
        self.cursor.expect(TokenKind::Colon)?;
        let return_type = self.parse_type_spec()?;
        self.cursor.expect(TokenKind::LBracket)?;
        let return_multiplicity = self.parse_multiplicity()?;
        self.cursor.expect(TokenKind::RBracket)?;
        self.cursor.expect(TokenKind::LBrace)?;
        let body = self.parse_expression_list()?;
        let body_close = self.cursor.expect(TokenKind::RBrace)?;
        let mut span = start.merge(&body_close.source_info);
        // Parse optional function test block: { testName | func(args) => expected; }
        let tests = if self.cursor.check(TokenKind::LBrace) {
            let parsed = self.parse_function_tests()?;
            // Function tests live inside the function's declaration; extend
            // the span to cover them too (matches Java Pure endLine).
            if let Some(last) = parsed
                .last()
                .map(legend_pure_parser_ast::source_info::Spanned::source_info)
            {
                span = span.merge(last);
            }
            parsed
        } else {
            vec![]
        };
        Ok(Element::Function(FunctionDef {
            package: header.package,
            name: header.name,
            type_parameters,
            multiplicity_parameters,
            parameters,
            return_type,
            return_multiplicity,
            body,
            stereotypes: header.stereotypes,
            tagged_values: header.tagged_values,
            tests,
            source_info: span,
        }))
    }

    // ── Native Function ─────────────────────────────────────────────────

    /// Parse `native function pkg::name(params): ReturnType[mult];`
    pub(crate) fn parse_native_function(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Native)?;
        self.cursor.expect(TokenKind::Function)?;
        let header = self.parse_element_header()?;
        let (type_parameters, multiplicity_parameters) =
            self.parse_type_and_multiplicity_parameters()?;
        self.cursor.expect(TokenKind::LParen)?;
        let mut parameters = Vec::new();
        while !self.cursor.check(TokenKind::RParen) {
            parameters.push(self.parse_parameter()?);
            self.cursor.eat(TokenKind::Comma);
        }
        self.cursor.expect(TokenKind::RParen)?;
        self.cursor.expect(TokenKind::Colon)?;
        let return_type = self.parse_type_spec()?;
        self.cursor.expect(TokenKind::LBracket)?;
        let return_multiplicity = self.parse_multiplicity()?;
        self.cursor.expect(TokenKind::RBracket)?;
        let semi = self.cursor.expect(TokenKind::Semicolon)?;
        Ok(Element::NativeFunction(NativeFunctionDef {
            package: header.package,
            name: header.name,
            type_parameters,
            multiplicity_parameters,
            parameters,
            return_type,
            return_multiplicity,
            stereotypes: header.stereotypes,
            tagged_values: header.tagged_values,
            source_info: start.merge(&semi.source_info),
        }))
    }

    /// Parse function test block: `{ (data | assertion | suite)* }`
    ///
    /// Grammar:
    /// ```text
    /// functionTestSuiteDef: '{' (simpleFunctionTest | simpleFunctionSuite | functionData)* '}'
    /// simpleFunctionSuite:  identifier '(' (functionData)* simpleFunctionTest+ ')'
    /// functionData:         qualifiedName ':' functionDataValue ';'
    /// simpleFunctionTest:   identifier (STRING)? '|' identifier '(' params ')' '=>' (externalFormatValue | primitiveValue) ';'
    /// ```
    pub(crate) fn parse_function_tests(&mut self) -> R<Vec<FunctionTest>> {
        let si = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::LBrace)?;

        let mut data = Vec::new();
        let mut assertions = Vec::new();
        let mut named_suites = Vec::new();

        while !self.cursor.check(TokenKind::RBrace) {
            // Disambiguate: peek at what follows the identifier(s).
            //
            // identifier '|'        → assertion (no doc)
            // identifier STRING '|' → assertion (with doc)
            // identifier '('        → named suite
            // identifier '::' ...   → could be data binding (path) OR start of assertion
            // identifier ':'        → data binding (single-segment store name)
            //
            // The key insight: data bindings always have a ':' after the full
            // qualified name, while assertions always have '|' (possibly after
            // a doc string).
            if self.is_test_data_binding() {
                data.push(self.parse_function_test_data()?);
            } else if self.is_named_suite() {
                named_suites.push(self.parse_named_suite()?);
            } else {
                assertions.push(self.parse_function_test_assertion()?);
            }
        }
        self.cursor.expect(TokenKind::RBrace)?;

        // If there are only unnamed data + assertions, wrap them in a single
        // unnamed suite (matching Engine behavior).
        if !data.is_empty() || !assertions.is_empty() {
            named_suites.insert(
                0,
                FunctionTest {
                    name: None,
                    data,
                    assertions,
                    source_info: si.clone(),
                },
            );
        }

        // If nothing was parsed, produce an empty unnamed suite.
        if named_suites.is_empty() {
            named_suites.push(FunctionTest {
                name: None,
                data: vec![],
                assertions: vec![],
                source_info: si,
            });
        }

        Ok(named_suites)
    }

    /// Lookahead: is this a test data binding? (`qualifiedName ':' ...`)
    ///
    /// Data bindings look like: `store::MyStore: (JSON) '{}';`
    /// Assertions look like: `testName | func() => result;`
    ///
    /// We scan forward from the current token, skipping `identifier ::` pairs,
    /// until we see `:` (data) or `|`/STRING (assertion).
    pub(crate) fn is_test_data_binding(&self) -> bool {
        let mut offset = 0;
        // Skip the first identifier
        if self.cursor.peek_kind_at(offset) != TokenKind::Identifier {
            return false;
        }
        offset += 1;

        // Skip `:: identifier` path segments
        while self.cursor.peek_kind_at(offset) == TokenKind::PathSep {
            offset += 1; // skip ::
            offset += 1; // skip identifier
        }

        // If next is `:`, it's a data binding
        self.cursor.peek_kind_at(offset) == TokenKind::Colon
    }

    /// Lookahead: is this a named test suite? (`identifier '(' ...`)
    pub(crate) fn is_named_suite(&self) -> bool {
        self.cursor.peek_kind() == TokenKind::Identifier
            && self.cursor.peek_kind_at(1) == TokenKind::LParen
    }

    /// Parse a named test suite: `SuiteName ( data* assertion+ )`
    pub(crate) fn parse_named_suite(&mut self) -> R<FunctionTest> {
        let si = self.cursor.current_source_info();
        let (name, _) = self.cursor.expect_identifier_or_keyword()?;
        self.cursor.expect(TokenKind::LParen)?;

        let mut data = Vec::new();
        let mut assertions = Vec::new();

        while !self.cursor.check(TokenKind::RParen) {
            if self.is_test_data_binding() {
                data.push(self.parse_function_test_data()?);
            } else {
                assertions.push(self.parse_function_test_assertion()?);
            }
        }
        self.cursor.expect(TokenKind::RParen)?;

        Ok(FunctionTest {
            name: Some(name),
            data,
            assertions,
            source_info: si,
        })
    }

    /// Parse a test data binding: `qualifiedName: dataValue;`
    ///
    /// Data values:
    /// - Inline: `(JSON) '{...}'`
    /// - Reference: `testing::MyReference`
    /// - Embedded: `Relation #{ content }#`
    pub(crate) fn parse_function_test_data(&mut self) -> R<FunctionTestData> {
        let si = self.cursor.current_source_info();
        let store = self.parse_package_path()?;
        self.cursor.expect(TokenKind::Colon)?;

        // Determine data value type
        let (format, data) = if self.cursor.check(TokenKind::LParen) {
            // External format: (JSON) 'content'
            self.cursor.advance();
            let (fmt, _) = self.cursor.expect_identifier_or_keyword()?;
            self.cursor.expect(TokenKind::RParen)?;
            let content = self.cursor.expect(TokenKind::StringLiteral)?;
            let raw = unquote_string(&content.text);
            (Some(fmt), FunctionTestDataValue::Inline(raw))
        } else if self.cursor.peek_kind() == TokenKind::Identifier
            && self.cursor.peek_kind_at(1) == TokenKind::HashLBrace
        {
            // Embedded data: Relation #{ content }#
            let (type_name, _) = self.cursor.expect_identifier_or_keyword()?;
            self.cursor.expect(TokenKind::HashLBrace)?;
            // Collect raw token text until }#
            let mut parts = Vec::new();
            let mut depth = 1u32;
            while depth > 0 {
                match self.cursor.peek_kind() {
                    TokenKind::HashLBrace => {
                        depth += 1;
                        parts.push(self.cursor.advance().text.clone());
                    }
                    TokenKind::RBraceHash => {
                        depth -= 1;
                        if depth > 0 {
                            parts.push(self.cursor.advance().text.clone());
                        }
                    }
                    TokenKind::Eof => {
                        return Err(ParseError::unexpected(
                            "unexpected end of file in embedded data".to_string(),
                            self.cursor.current_source_info(),
                        ));
                    }
                    _ => {
                        parts.push(self.cursor.advance().text.clone());
                    }
                }
            }
            let content = SmolStr::new(parts.join(""));
            self.cursor.expect(TokenKind::RBraceHash)?;
            (
                None,
                FunctionTestDataValue::EmbeddedData { type_name, content },
            )
        } else {
            // Reference: testing::MyReference
            let ref_package = self.parse_package_path()?;
            (None, FunctionTestDataValue::Reference(ref_package))
        };

        self.cursor.expect(TokenKind::Semicolon)?;
        Ok(FunctionTestData {
            store,
            format,
            data,
            source_info: si,
        })
    }

    /// Parse: `testName ('doc')? | funcCall(args) => ((format) )? expected;`
    pub(crate) fn parse_function_test_assertion(&mut self) -> R<FunctionTestAssertion> {
        let si = self.cursor.current_source_info();
        let (test_name, _) = self.cursor.expect_identifier_or_keyword()?;

        // Optional doc string
        let doc = if self.cursor.check(TokenKind::StringLiteral) {
            let tok = self.cursor.advance().clone();
            Some(SmolStr::new(unquote_string(&tok.text)))
        } else {
            None
        };

        self.cursor.expect(TokenKind::Pipe)?;
        let invocation = self.parse_expression()?;
        self.cursor.expect(TokenKind::FatArrow)?;

        // Optional external format: (JSON) or (XML)
        let expected_format = if self.cursor.check(TokenKind::LParen) {
            // Lookahead: is this `(identifier)` followed by a value?
            // vs a regular expression `(expr)`
            if self.cursor.peek_kind_at(1) == TokenKind::Identifier
                && self.cursor.peek_kind_at(2) == TokenKind::RParen
            {
                self.cursor.advance(); // (
                let (fmt, _) = self.cursor.expect_identifier_or_keyword()?;
                self.cursor.expect(TokenKind::RParen)?;
                Some(fmt)
            } else {
                None
            }
        } else {
            None
        };

        let expected = self.parse_expression()?;
        self.cursor.expect(TokenKind::Semicolon)?;
        Ok(FunctionTestAssertion {
            name: test_name,
            doc,
            invocation,
            expected_format,
            expected,
            source_info: si,
        })
    }

    pub(crate) fn parse_parameter(&mut self) -> R<Parameter> {
        let start = self.cursor.current_source_info();
        let (name, _) = self.cursor.expect_identifier_or_keyword()?;
        self.cursor.expect(TokenKind::Colon)?;
        let type_ref = if self.cursor.check(TokenKind::LParen) {
            // Bare relation type in parameter position: (col:Type, ...)
            let cols = self.parse_relation_columns()?;
            // Encode as a synthetic TypeReference — the column info is
            // preserved in type_arguments for downstream consumers.
            let col_args: Vec<TypeReference> = cols
                .into_iter()
                .map(|col| TypeReference {
                    package: None,
                    name: col.name,
                    type_arguments: vec![col.type_ref],
                    multiplicity_arguments: vec![],
                    type_variable_values: vec![],
                    source_info: col.source_info,
                })
                .collect();
            TypeReference {
                package: None,
                name: SmolStr::new(RELATION_TYPE_SENTINEL),
                type_arguments: col_args,
                multiplicity_arguments: vec![],
                type_variable_values: vec![],
                source_info: start.clone(),
            }
        } else {
            self.parse_type_reference()?
        };
        self.cursor.expect(TokenKind::LBracket)?;
        let multiplicity = self.parse_multiplicity()?;
        self.cursor.expect(TokenKind::RBracket)?;
        Ok(Parameter {
            name,
            type_ref: Some(type_ref),
            multiplicity: Some(multiplicity),
            source_info: start,
        })
    }
}
