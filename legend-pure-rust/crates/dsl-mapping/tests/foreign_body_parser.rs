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

//! Stage-0b smoke: `MappingSectionParser::with_body_parsers` accepts a
//! foreign `ClassMappingBodyParser` and routes its `parserName` to the
//! plug-in. Locks the architectural decoupling between dsl-mapping and
//! store DSLs (Relational, future BigQuery / Delta / etc.): a foreign
//! body parser may be registered without modifying dsl-mapping itself.

use std::any::Any;

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, ForeignClassMappingBody, MappingDef};
use legend_pure_dsl_mapping::parser::{ClassMappingBodyParser, MappingSectionParser};
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::ParserContext;
use legend_pure_parser_parser::error::ParseError;

/// Minimal stub body whose grammar is just `{ "<some text>" }` —
/// captures the literal string so the round-trip check has something
/// to assert.
#[derive(Debug, Clone, PartialEq)]
struct FooBody {
    payload: String,
}

impl ForeignClassMappingBody for FooBody {
    fn kind(&self) -> &str {
        "Foo"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn ForeignClassMappingBody> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn ForeignClassMappingBody) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
    fn compose(&self, out: &mut String) {
        out.push_str("  {\n    ");
        out.push('\'');
        out.push_str(&self.payload);
        out.push('\'');
        out.push_str("\n  }");
    }
}

/// Parser for `: Foo { "<some text>" }` bodies. Consumes through the
/// matching `}`.
struct FooBodyParser;

impl ClassMappingBodyParser for FooBodyParser {
    fn kind(&self) -> &str {
        "Foo"
    }

    fn parse(
        &self,
        ctx: &mut ParserContext<'_>,
    ) -> Result<Box<dyn ForeignClassMappingBody>, ParseError> {
        ctx.cursor().expect(TokenKind::LBrace)?;
        let str_tok = ctx.cursor().expect(TokenKind::StringLiteral)?;
        ctx.cursor().expect(TokenKind::RBrace)?;
        // Strip the surrounding single quotes from the lexer's raw text.
        let raw = str_tok.text;
        let payload = raw
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or(&raw)
            .to_string();
        Ok(Box::new(FooBody { payload }))
    }
}

fn parse_with_foo(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "foreign_body_parser.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::with_body_parsers(vec![
            Box::new(FooBodyParser),
        ]))],
    );
    match result {
        Ok(file) => file,
        Err(p) => panic!(
            "Expected parse to succeed, but got error(s): {:?}",
            p.errors
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        ),
    }
}

fn first_mapping(file: &SourceFile) -> &MappingDef {
    let section = file
        .sections
        .iter()
        .find(|s| s.kind.as_str() == "Mapping")
        .expect("expected one ###Mapping section");
    let AstElement::DSLElement(boxed) = section
        .elements
        .first()
        .expect("expected one mapping element")
    else {
        panic!("expected DSLElement");
    };
    boxed
        .as_any()
        .downcast_ref::<MappingDef>()
        .expect("expected MappingDef")
}

#[test]
fn foreign_body_parser_routes_by_kind() {
    // The `: Foo { ... }` body lands in ClassMappingBody::Foreign and
    // downcasts back to FooBody.
    let source = indoc! {r#"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Foo
          {
            'hello relational world'
          }
        )
    "#};
    let file = parse_with_foo(source);
    let m = first_mapping(&file);
    assert_eq!(m.class_mappings.len(), 1);
    let cm = &m.class_mappings[0];
    let ClassMappingBody::Foreign(body) = &cm.body else {
        panic!("expected Foreign body variant, got {:?}", cm.body);
    };
    assert_eq!(body.kind(), "Foo");
    let foo = body
        .as_any()
        .downcast_ref::<FooBody>()
        .expect("downcast to FooBody");
    assert_eq!(foo.payload, "hello relational world");
}

#[test]
fn foreign_body_parser_unknown_kind_falls_through_to_error() {
    // No FooBodyParser registered → `Foo` is rejected.
    let source = indoc! {r#"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Foo
          {
            'x'
          }
        )
    "#};
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "no_foo.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    let partial = result.expect_err("Foo without registration must error");
    let msgs: Vec<String> = partial.errors.iter().map(ToString::to_string).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("Foo") && m.contains("with_body_parsers")),
        "expected error mentioning Foo and the registration mechanism; got {msgs:?}"
    );
}

#[test]
fn foreign_body_alongside_built_in_pure() {
    // Mixed body kinds within a single ###Mapping: built-in Pure +
    // foreign Foo coexist.
    let source = indoc! {r#"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Foo
          {
            'foreign-only'
          }
          pkg::Person : Pure
          {
            ~src pkg::PersonSource
            firstName : $src.first
          }
        )
    "#};
    let file = parse_with_foo(source);
    let m = first_mapping(&file);
    assert_eq!(m.class_mappings.len(), 2);
    assert!(matches!(
        m.class_mappings[0].body,
        ClassMappingBody::Foreign(_)
    ));
    assert!(matches!(
        m.class_mappings[1].body,
        ClassMappingBody::Pure(_)
    ));
}
