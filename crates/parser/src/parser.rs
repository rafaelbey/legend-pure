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

//! Recursive descent parser for the Pure grammar.
//!
//! This parser is strictly responsible for **syntax analysis** — converting a
//! token stream into an AST. It does not perform semantic validation (e.g., type
//! checking, name resolution, or structural constraints on graph fetch trees).
//! See `docs/SEMANTIC_VALIDATIONS.md` for deferred validations.

use legend_pure_parser_ast::annotation::{
    PackageableElementPtr, Parameter, SpannedString, StereotypePtr, TagPtr, TaggedValue,
};
use legend_pure_parser_ast::element::{
    AggregationKind, AssociationDef, ClassDef, Constraint, Element, EnumDef, EnumValue,
    FunctionDef, FunctionTest, FunctionTestAssertion, FunctionTestData, FunctionTestDataValue,
    MeasureDef, NativeFunctionDef, ProfileDef, Property, QualifiedProperty, UnitDef,
};
use legend_pure_parser_ast::expression::{
    ArithmeticExpr, ArithmeticOp, ArrowFunction, BooleanLiteral, CollectionExpr, ComparisonExpr,
    ComparisonOp, DateTimeLiteral, DecimalLiteral, Expression, FloatLiteral, FunctionApplication,
    IntegerLiteral, KeyValuePair, Lambda, LetExpr, Literal, LogicalExpr, LogicalOp, MemberAccess,
    NewInstanceExpr, NotExpr, PackageableElementRef, QualifiedMemberAccess, SimpleMemberAccess,
    StrictDateLiteral, StrictTimeLiteral, StringLiteral, TypeReferenceExpr, UnaryMinusExpr,
    Variable,
};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_ast::section::{ImportStatement, Section, SourceFile};
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_ast::type_ref::{
    Multiplicity, Package, RELATION_TYPE_SENTINEL, RelationColumn, RelationType, TypeReference,
    TypeSpec, TypeVariableValue, UnitReference,
};
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

use crate::cursor::Cursor;
use crate::error::ParseError;
use crate::island::IslandParser;

type R<T> = Result<T, ParseError>;

/// Main parser struct wrapping a token cursor and island grammar plugins.
pub(crate) struct Parser {
    cursor: Cursor,
    island_parsers: Vec<Box<dyn IslandParser>>,
}

impl Parser {
    /// Create a parser with the default set of island grammar plugins.
    pub fn new(cursor: Cursor) -> Self {
        Self {
            cursor,
            island_parsers: crate::island::default_island_parsers(),
        }
    }

    /// Create a parser with a custom set of island grammar plugins.
    pub fn with_island_parsers(cursor: Cursor, island_parsers: Vec<Box<dyn IslandParser>>) -> Self {
        Self {
            cursor,
            island_parsers,
        }
    }

    // ── Top-level ───────────────────────────────────────────────────────

    pub fn parse_source_file(&mut self) -> R<SourceFile> {
        let start = self.cursor.current_source_info();
        let mut sections = Vec::new();

        while !self.cursor.check(TokenKind::Eof) {
            sections.push(self.parse_section()?);
        }

        if sections.is_empty() {
            sections.push(Section {
                kind: SmolStr::new("Pure"),
                imports: vec![],
                elements: vec![],
                source_info: start.clone(),
            });
        }

        Ok(SourceFile {
            sections,
            source_info: start,
        })
    }

    fn parse_section(&mut self) -> R<Section> {
        let start = self.cursor.current_source_info();
        let kind = if self.cursor.check(TokenKind::SectionHeader) {
            let tok = self.cursor.advance().clone();
            SmolStr::new(tok.text.trim_start_matches('#'))
        } else {
            SmolStr::new("Pure")
        };

        let mut imports = Vec::new();
        while self.cursor.check(TokenKind::Import) {
            imports.push(self.parse_import()?);
        }

        let mut elements = Vec::new();
        while !self.cursor.check(TokenKind::SectionHeader) && !self.cursor.check(TokenKind::Eof) {
            elements.push(self.parse_element()?);
        }

        Ok(Section {
            kind,
            imports,
            elements,
            source_info: start,
        })
    }

    fn parse_import(&mut self) -> R<ImportStatement> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Import)?;
        let path = self.parse_package_path()?;
        // Consume ::* if present
        if self.cursor.eat(TokenKind::PathSep) {
            self.cursor.expect(TokenKind::Star)?;
        }
        self.cursor.expect(TokenKind::Semicolon)?;
        Ok(ImportStatement {
            path,
            source_info: start,
        })
    }

    fn parse_element(&mut self) -> R<Element> {
        match self.cursor.peek_kind() {
            TokenKind::Profile => self.parse_profile(),
            TokenKind::Enum => self.parse_enum(),
            TokenKind::Class => self.parse_class(),
            TokenKind::Association => self.parse_association(),
            TokenKind::Measure => self.parse_measure(),
            TokenKind::Function => self.parse_function(),
            TokenKind::Native => self.parse_native_function(),
            _ => Err(ParseError::unexpected(
                format!("Unexpected token {}", self.cursor.peek().text),
                self.cursor.current_source_info(),
            )),
        }
    }

    // ── Package path: my::pkg::Name ─────────────────────────────────────

    fn parse_package_path(&mut self) -> R<Package> {
        let (name, si) = self.cursor.expect_identifier_or_keyword()?;
        let mut pkg = Package::root(name, si);
        while self.cursor.check(TokenKind::PathSep) && !is_wildcard_ahead(&self.cursor) {
            self.cursor.advance();
            let (seg, si) = self.cursor.expect_identifier_or_keyword()?;
            pkg = pkg.child(seg, si);
        }
        Ok(pkg)
    }

    /// Parse a qualified name, returning (package, name).
    fn parse_qualified_name(&mut self) -> R<(Option<Package>, SmolStr, SourceInfo)> {
        let (first, first_si) = self.cursor.expect_identifier_or_keyword()?;
        if !self.cursor.check(TokenKind::PathSep) {
            return Ok((None, first, first_si));
        }
        let mut pkg = Package::root(first, first_si.clone());
        while self.cursor.eat(TokenKind::PathSep) {
            let (seg, si) = self.cursor.expect_identifier_or_keyword()?;
            if self.cursor.check(TokenKind::PathSep) {
                pkg = pkg.child(seg, si);
            } else {
                return Ok((Some(pkg), seg, first_si));
            }
        }
        // Last segment in package is actually the name
        let name = SmolStr::new(pkg.name());
        Ok((pkg.parent().cloned(), name, first_si))
    }

    // ── Annotations ─────────────────────────────────────────────────────

    fn parse_stereotypes(&mut self) -> R<Vec<StereotypePtr>> {
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

    fn parse_stereotype_ptr(&mut self) -> R<StereotypePtr> {
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

    fn parse_tagged_values(&mut self) -> R<Vec<TaggedValue>> {
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

    fn is_tagged_value_start(&self) -> bool {
        // Pattern: { <ident> . <ident> = <string> }
        if self.cursor.peek_kind() != TokenKind::LBrace {
            return false;
        }
        let k1 = self.cursor.peek_kind_at(1);
        let k2 = self.cursor.peek_kind_at(2);
        (k1 == TokenKind::Identifier || k1 == TokenKind::StringLiteral)
            && (k2 == TokenKind::Dot || k2 == TokenKind::PathSep)
    }

    fn parse_tagged_value(&mut self) -> R<TaggedValue> {
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

    // ── Profile ─────────────────────────────────────────────────────────

    fn parse_profile(&mut self) -> R<Element> {
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

    // ── Enum ────────────────────────────────────────────────────────────

    fn parse_enum(&mut self) -> R<Element> {
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

    // ── Class ───────────────────────────────────────────────────────────

    fn parse_class(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Class)?;
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (package, name, _) = self.parse_qualified_name()?;

        let type_parameters = if self.cursor.eat(TokenKind::Less) {
            let mut params = Vec::new();
            loop {
                let (p, _) = self.cursor.expect_identifier()?;
                params.push(p);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.cursor.expect(TokenKind::Greater)?;
            params
        } else {
            vec![]
        };

        let super_types = if self.cursor.eat(TokenKind::Extends) {
            let mut supers = Vec::new();
            loop {
                supers.push(self.parse_type_reference()?);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            supers
        } else {
            vec![]
        };

        let constraints = if self.cursor.check(TokenKind::LBracket) {
            self.parse_constraints()?
        } else {
            vec![]
        };

        self.cursor.expect(TokenKind::LBrace)?;
        let (properties, qualified_properties) = self.parse_class_body()?;
        self.cursor.expect(TokenKind::RBrace)?;

        Ok(Element::Class(ClassDef {
            package,
            name,
            type_parameters,
            super_types,
            properties,
            qualified_properties,
            constraints,
            stereotypes,
            tagged_values,
            source_info: start,
        }))
    }

    fn parse_constraints(&mut self) -> R<Vec<Constraint>> {
        self.cursor.expect(TokenKind::LBracket)?;
        let mut result = Vec::new();
        loop {
            result.push(self.parse_constraint()?);
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::RBracket)?;
        Ok(result)
    }

    fn parse_constraint(&mut self) -> R<Constraint> {
        let start = self.cursor.current_source_info();
        let mut name = None;
        let mut function_definition;
        let mut enforcement_level = None;
        let mut external_id = None;
        let mut message = None;

        if self.is_named_constraint() {
            let (n, _) = self.cursor.expect_identifier()?;
            if self.cursor.check(TokenKind::LParen)
                && self.cursor.peek_kind_at(1) == TokenKind::Tilde
            {
                // Extended constraint: name ( ~function: ... ~enforcementLevel: ... )
                self.cursor.advance(); // (
                function_definition = Expression::Literal(Literal::Boolean(BooleanLiteral {
                    value: true,
                    source_info: start.clone(),
                }));
                while self.cursor.check(TokenKind::Tilde) {
                    self.cursor.advance(); // ~
                    let (key, _) = self.cursor.expect_identifier_or_keyword()?;
                    self.cursor.expect(TokenKind::Colon)?;
                    match key.as_str() {
                        "function" => function_definition = self.parse_expression()?,
                        "enforcementLevel" => {
                            let (level, _) = self.cursor.expect_identifier_or_keyword()?;
                            enforcement_level = Some(level);
                        }
                        "externalId" => {
                            let tok = self.cursor.expect(TokenKind::StringLiteral)?;
                            external_id = Some(unquote_string(&tok.text));
                        }
                        "message" => {
                            if self.cursor.check(TokenKind::StringLiteral) {
                                let tok = self.cursor.advance().clone();
                                message =
                                    Some(Expression::Literal(Literal::String(StringLiteral {
                                        value: unquote_string(&tok.text),
                                        source_info: tok.source_info.clone(),
                                    })));
                            } else {
                                message = Some(self.parse_expression()?);
                            }
                        }
                        other => {
                            return Err(ParseError::unexpected(
                                format!("Unknown constraint field '~{other}'"),
                                self.cursor.current_source_info(),
                            ));
                        }
                    }
                }
                self.cursor.expect(TokenKind::RParen)?;
                name = Some(n);
            } else {
                self.cursor.expect(TokenKind::Colon)?;
                function_definition = self.parse_expression()?;
                name = Some(n);
            }
        } else {
            function_definition = self.parse_expression()?;
        }
        Ok(Constraint {
            name,
            function_definition,
            enforcement_level,
            external_id,
            message,
            source_info: start,
        })
    }

    fn is_named_constraint(&self) -> bool {
        let k0 = self.cursor.peek_kind();
        let k1 = self.cursor.peek_kind_at(1);
        (k0 == TokenKind::Identifier || k0 == TokenKind::StringLiteral)
            && (k1 == TokenKind::Colon
                || (k1 == TokenKind::LParen && self.cursor.peek_kind_at(2) == TokenKind::Tilde))
    }

    fn parse_class_body(&mut self) -> R<(Vec<Property>, Vec<QualifiedProperty>)> {
        let mut props = Vec::new();
        let mut qprops = Vec::new();
        while !self.cursor.check(TokenKind::RBrace) {
            let stereos = self.parse_stereotypes()?;
            let tvs = self.parse_tagged_values()?;
            let aggregation = self.parse_aggregation()?;
            let (prop_name, prop_si) = self.cursor.expect_identifier_or_keyword()?;

            if self.cursor.check(TokenKind::LParen) {
                // Qualified property: name(params) { body }: RetType[mult];
                qprops.push(self.parse_qualified_property(prop_name, prop_si, stereos, tvs)?);
            } else {
                // Regular property: name: Type[mult];
                self.cursor.expect(TokenKind::Colon)?;
                let type_ref = self.parse_type_spec()?;
                self.cursor.expect(TokenKind::LBracket)?;
                let multiplicity = self.parse_multiplicity()?;
                self.cursor.expect(TokenKind::RBracket)?;
                let default_value = if self.cursor.eat(TokenKind::Equals) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                self.cursor.expect(TokenKind::Semicolon)?;
                props.push(Property {
                    name: prop_name,
                    type_ref,
                    multiplicity,
                    aggregation,
                    default_value,
                    stereotypes: stereos,
                    tagged_values: tvs,
                    source_info: prop_si,
                });
            }
        }
        Ok((props, qprops))
    }

    fn parse_aggregation(&mut self) -> R<Option<AggregationKind>> {
        if !self.cursor.check(TokenKind::LParen) {
            return Ok(None);
        }
        // Lookahead: (shared), (composite), (none) — vs regular parens
        match self.cursor.peek_kind_at(1) {
            TokenKind::Shared => {
                self.cursor.advance();
                self.cursor.advance();
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Some(AggregationKind::Shared))
            }
            TokenKind::Composite => {
                self.cursor.advance();
                self.cursor.advance();
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Some(AggregationKind::Composite))
            }
            TokenKind::None => {
                self.cursor.advance();
                self.cursor.advance();
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Some(AggregationKind::None))
            }
            TokenKind::Identifier if self.cursor.peek_kind_at(2) == TokenKind::RParen => {
                let si = self.cursor.current_source_info();
                self.cursor.advance(); // (
                let tok = self.cursor.advance().clone(); // bad keyword
                self.cursor.advance(); // )
                Err(ParseError::unexpected(
                    format!(
                        "Invalid aggregation kind '{}'. Expected 'shared', 'composite', or 'none'",
                        tok.text
                    ),
                    si,
                ))
            }
            _ => Ok(None),
        }
    }

    fn parse_qualified_property(
        &mut self,
        name: SmolStr,
        si: SourceInfo,
        stereotypes: Vec<StereotypePtr>,
        tagged_values: Vec<TaggedValue>,
    ) -> R<QualifiedProperty> {
        self.cursor.expect(TokenKind::LParen)?;
        let mut parameters = Vec::new();
        while !self.cursor.check(TokenKind::RParen) {
            parameters.push(self.parse_parameter()?);
            self.cursor.eat(TokenKind::Comma);
        }
        self.cursor.expect(TokenKind::RParen)?;
        self.cursor.expect(TokenKind::LBrace)?;
        let body = self.parse_expression_list()?;
        self.cursor.expect(TokenKind::RBrace)?;
        self.cursor.expect(TokenKind::Colon)?;
        let return_type = self.parse_type_spec()?;
        self.cursor.expect(TokenKind::LBracket)?;
        let return_multiplicity = self.parse_multiplicity()?;
        self.cursor.expect(TokenKind::RBracket)?;
        self.cursor.expect(TokenKind::Semicolon)?;
        Ok(QualifiedProperty {
            name,
            parameters,
            return_type,
            return_multiplicity,
            body,
            stereotypes,
            tagged_values,
            source_info: si,
        })
    }

    // ── Association ─────────────────────────────────────────────────────

    fn parse_association(&mut self) -> R<Element> {
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

    // ── Measure ─────────────────────────────────────────────────────────

    fn parse_measure(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Measure)?;
        let (package, name, _) = self.parse_qualified_name()?;
        self.cursor.expect(TokenKind::LBrace)?;
        let mut canonical_unit = None;
        let mut non_canonical_units = Vec::new();
        while !self.cursor.check(TokenKind::RBrace) {
            let is_canonical = self.cursor.eat(TokenKind::Star);
            let unit = self.parse_unit_def()?;
            if is_canonical {
                canonical_unit = Some(unit);
            } else {
                non_canonical_units.push(unit);
            }
        }
        self.cursor.expect(TokenKind::RBrace)?;
        Ok(Element::Measure(MeasureDef {
            package,
            name,
            canonical_unit,
            non_canonical_units,
            source_info: start,
        }))
    }

    fn parse_unit_def(&mut self) -> R<UnitDef> {
        let (name, si) = self.cursor.expect_identifier_or_keyword()?;
        if self.cursor.eat(TokenKind::Colon) {
            let (param, _) = self.cursor.expect_identifier()?;
            self.cursor.expect(TokenKind::Arrow)?;
            let body = self.parse_expression()?;
            self.cursor.expect(TokenKind::Semicolon)?;
            Ok(UnitDef {
                name,
                conversion_param: Some(param),
                conversion_body: Some(body),
                source_info: si,
            })
        } else {
            self.cursor.expect(TokenKind::Semicolon)?;
            Ok(UnitDef {
                name,
                conversion_param: None,
                conversion_body: None,
                source_info: si,
            })
        }
    }

    // ── Function ────────────────────────────────────────────────────────

    fn parse_function(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Function)?;
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (package, name, _) = self.parse_qualified_name()?;
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
        self.cursor.expect(TokenKind::RBrace)?;
        // Parse optional function test block: { testName | func(args) => expected; }
        let tests = if self.cursor.check(TokenKind::LBrace) {
            self.parse_function_tests()?
        } else {
            vec![]
        };
        Ok(Element::Function(FunctionDef {
            package,
            name,
            parameters,
            return_type,
            return_multiplicity,
            body,
            stereotypes,
            tagged_values,
            tests,
            source_info: start,
        }))
    }

    // ── Native Function ─────────────────────────────────────────────────

    /// Parse `native function pkg::name(params): ReturnType[mult];`
    fn parse_native_function(&mut self) -> R<Element> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::Native)?;
        self.cursor.expect(TokenKind::Function)?;
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (package, name, _) = self.parse_qualified_name()?;
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
        self.cursor.expect(TokenKind::Semicolon)?;
        Ok(Element::NativeFunction(NativeFunctionDef {
            package,
            name,
            parameters,
            return_type,
            return_multiplicity,
            stereotypes,
            tagged_values,
            source_info: start,
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
    fn parse_function_tests(&mut self) -> R<Vec<FunctionTest>> {
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
    fn is_test_data_binding(&self) -> bool {
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
    fn is_named_suite(&self) -> bool {
        self.cursor.peek_kind() == TokenKind::Identifier
            && self.cursor.peek_kind_at(1) == TokenKind::LParen
    }

    /// Parse a named test suite: `SuiteName ( data* assertion+ )`
    fn parse_named_suite(&mut self) -> R<FunctionTest> {
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
    fn parse_function_test_data(&mut self) -> R<FunctionTestData> {
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
    fn parse_function_test_assertion(&mut self) -> R<FunctionTestAssertion> {
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

    fn parse_parameter(&mut self) -> R<Parameter> {
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
                    type_variable_values: vec![],
                    source_info: col.source_info,
                })
                .collect();
            TypeReference {
                package: None,
                name: SmolStr::new(RELATION_TYPE_SENTINEL),
                type_arguments: col_args,
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

    // ── Type references ─────────────────────────────────────────────────

    fn parse_type_reference(&mut self) -> R<TypeReference> {
        let start = self.cursor.current_source_info();
        let path = self.parse_package_path()?;
        let (pkg, name) = split_package_name(&path);
        self.finish_type_reference(start, pkg, name)
    }

    /// Completes type reference parsing after the package path has been consumed.
    ///
    /// This allows callers (like `parse_type_spec`) to parse the path first,
    /// inspect ahead for `<(` column-spec syntax, and then delegate here for
    /// the standard `<TypeArgs>(TypeVarValues)` continuation.
    fn finish_type_reference(
        &mut self,
        start: SourceInfo,
        pkg: Option<Package>,
        name: SmolStr,
    ) -> R<TypeReference> {
        let type_arguments = if self.cursor.eat(TokenKind::Less) {
            let mut args = Vec::new();
            if self.cursor.check(TokenKind::LParen) {
                // Column specification: <(a:Integer, b:String)>
                // Encode columns as type_arguments for backward compatibility.
                // In parse_type_spec context, these are promoted to TypeSpec::Relation.
                let cols = self.parse_relation_columns()?;
                for col in cols {
                    args.push(TypeReference {
                        package: None,
                        name: col.name,
                        type_arguments: vec![col.type_ref],
                        type_variable_values: vec![],
                        source_info: col.source_info,
                    });
                }
            } else {
                loop {
                    args.push(self.parse_type_reference()?);
                    if !self.cursor.eat(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.cursor.expect(TokenKind::Greater)?;
            args
        } else {
            vec![]
        };
        let type_variable_values = if self.cursor.eat(TokenKind::LParen) {
            let mut vals = Vec::new();
            loop {
                vals.push(self.parse_type_variable_value()?);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.cursor.expect(TokenKind::RParen)?;
            vals
        } else {
            vec![]
        };
        Ok(TypeReference {
            package: pkg,
            name,
            type_arguments,
            type_variable_values,
            source_info: start,
        })
    }

    /// Parses a type specification that can be a type, unit reference, or relation type.
    ///
    /// Used in positions where the Pure grammar accepts:
    /// - Regular types: `String`, `Map<K, V>`
    /// - Unit references: `NewMeasure~UnitOne`
    /// - Relation types: `(a:Integer, b:String)` or `Relation<(a:Integer, b:String)>`
    fn parse_type_spec(&mut self) -> R<TypeSpec> {
        // Bare relation type: (col:Type, ...)
        if self.cursor.check(TokenKind::LParen) {
            return self.parse_relation_type();
        }

        let start = self.cursor.current_source_info();

        // Parse the qualified name first (e.g. `Relation`, `meta::pure::Relation`).
        let path = self.parse_package_path()?;

        // Check for `<(` — this is column-spec syntax (e.g. `Relation<(a:Integer)>`).
        // We detect it structurally by token sequence rather than name-matching,
        // so user types named "Relation" with regular type args aren't affected.
        if self.cursor.check(TokenKind::Less) && self.cursor.peek_kind_at(1) == TokenKind::LParen {
            self.cursor.expect(TokenKind::Less)?;
            let columns = self.parse_relation_columns()?;
            self.cursor.expect(TokenKind::Greater)?;
            return Ok(TypeSpec::Relation(RelationType {
                columns,
                source_info: start,
            }));
        }

        // Standard type reference: parse remaining <TypeArgs>(TypeVarValues)
        let (pkg, name) = split_package_name(&path);
        let type_ref = self.finish_type_reference(start, pkg, name)?;

        if self.cursor.eat(TokenKind::Tilde) {
            let si = self.cursor.current_source_info();
            let (unit_name, _) = self.cursor.expect_identifier_or_keyword()?;
            Ok(TypeSpec::Unit(UnitReference {
                measure: type_ref,
                unit: unit_name,
                source_info: si,
            }))
        } else {
            Ok(TypeSpec::Type(type_ref))
        }
    }

    /// Parse a bare relation type: `(col:Type[mult]?, ...)`
    fn parse_relation_type(&mut self) -> R<TypeSpec> {
        let si = self.cursor.current_source_info();
        let columns = self.parse_relation_columns()?;
        Ok(TypeSpec::Relation(RelationType {
            columns,
            source_info: si,
        }))
    }

    /// Parse relation columns: `(name:Type[mult]?, ...)`
    ///
    /// Shared by both bare relation types and `Relation<(...)>` syntax.
    fn parse_relation_columns(&mut self) -> R<Vec<RelationColumn>> {
        self.cursor.expect(TokenKind::LParen)?;
        let mut cols = Vec::new();
        loop {
            let col_si = self.cursor.current_source_info();
            let (col_name, _) = self.cursor.expect_identifier_or_keyword()?;
            self.cursor.expect(TokenKind::Colon)?;
            let col_type = self.parse_type_reference()?;
            let multiplicity = if self.cursor.check(TokenKind::LBracket) {
                self.cursor.expect(TokenKind::LBracket)?;
                let mult = self.parse_multiplicity()?;
                self.cursor.expect(TokenKind::RBracket)?;
                Some(mult)
            } else {
                None
            };
            cols.push(RelationColumn {
                name: col_name,
                type_ref: col_type,
                multiplicity,
                source_info: col_si,
            });
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::RParen)?;
        Ok(cols)
    }

    fn parse_type_variable_value(&mut self) -> R<TypeVariableValue> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            TokenKind::IntegerLiteral => {
                let tok = self.cursor.advance().clone();
                let val: i64 = tok.text.parse().unwrap_or(0);
                Ok(TypeVariableValue::Integer(val, si))
            }
            TokenKind::StringLiteral => {
                let tok = self.cursor.advance().clone();
                Ok(TypeVariableValue::String(unquote_string(&tok.text), si))
            }
            _ => {
                let (id, _) = self.cursor.expect_identifier()?;
                Ok(TypeVariableValue::String(id.to_string(), si))
            }
        }
    }

    fn parse_multiplicity(&mut self) -> R<Multiplicity> {
        match self.cursor.peek_kind() {
            TokenKind::Star => {
                self.cursor.advance();
                Ok(Multiplicity::zero_or_many())
            }
            TokenKind::IntegerLiteral => {
                let lo_tok = self.cursor.advance().clone();
                let lo: u32 = lo_tok.text.parse().unwrap_or(0);
                if self.cursor.eat(TokenKind::Dot) {
                    self.cursor.expect(TokenKind::Dot)?;
                    if self.cursor.check(TokenKind::Star) {
                        self.cursor.advance();
                        Ok(Multiplicity::range(lo, None))
                    } else {
                        let hi_tok = self.cursor.expect(TokenKind::IntegerLiteral)?;
                        let hi: u32 = hi_tok.text.parse().unwrap_or(lo);
                        Ok(Multiplicity::range(lo, Some(hi)))
                    }
                } else {
                    Ok(Multiplicity::range(lo, Some(lo)))
                }
            }
            _ => Err(ParseError::expected(
                "multiplicity",
                self.cursor.peek_kind(),
                self.cursor.current_source_info(),
            )),
        }
    }

    // ── Expressions (precedence-climbing recursive descent) ─────────────

    fn parse_expression_list(&mut self) -> R<Vec<Expression>> {
        let mut exprs = Vec::new();
        while !self.cursor.check(TokenKind::RBrace) && !self.cursor.check(TokenKind::Eof) {
            exprs.push(self.parse_expression()?);
            self.cursor.eat(TokenKind::Semicolon);
        }
        Ok(exprs)
    }

    /// Parse an expression (entry point — handles precedence via recursive descent).
    pub fn parse_expression(&mut self) -> R<Expression> {
        self.parse_or_expression()
    }

    fn parse_or_expression(&mut self) -> R<Expression> {
        let mut left = self.parse_and_expression()?;
        while self.cursor.check(TokenKind::PipePipe) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_and_expression()?;
            left = Expression::Logical(LogicalExpr {
                left: Box::new(left),
                op: LogicalOp::Or,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    fn parse_and_expression(&mut self) -> R<Expression> {
        let mut left = self.parse_comparison()?;
        while self.cursor.check(TokenKind::AmpAmp) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_comparison()?;
            left = Expression::Logical(LogicalExpr {
                left: Box::new(left),
                op: LogicalOp::And,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> R<Expression> {
        let mut left = self.parse_additive()?;
        if let Some(op) = self.comparison_op() {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_additive()?;
            left = Expression::Comparison(ComparisonExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    fn comparison_op(&self) -> Option<ComparisonOp> {
        match self.cursor.peek_kind() {
            TokenKind::EqualEqual => Some(ComparisonOp::Equal),
            TokenKind::BangEqual => Some(ComparisonOp::NotEqual),
            TokenKind::Less => Some(ComparisonOp::LessThan),
            TokenKind::LessEqual => Some(ComparisonOp::LessThanOrEqual),
            TokenKind::Greater => Some(ComparisonOp::GreaterThan),
            TokenKind::GreaterEqual => Some(ComparisonOp::GreaterThanOrEqual),
            _ => None,
        }
    }

    fn parse_additive(&mut self) -> R<Expression> {
        let mut left = self.parse_multiplicative()?;
        while matches!(self.cursor.peek_kind(), TokenKind::Plus | TokenKind::Minus) {
            let si = self.cursor.current_source_info();
            let op = if self.cursor.peek_kind() == TokenKind::Plus {
                ArithmeticOp::Plus
            } else {
                ArithmeticOp::Minus
            };
            self.cursor.advance();
            let right = self.parse_multiplicative()?;
            left = Expression::Arithmetic(ArithmeticExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> R<Expression> {
        let mut left = self.parse_unary()?;
        while matches!(self.cursor.peek_kind(), TokenKind::Star | TokenKind::Slash) {
            let si = self.cursor.current_source_info();
            let op = if self.cursor.peek_kind() == TokenKind::Star {
                ArithmeticOp::Times
            } else {
                ArithmeticOp::Divide
            };
            self.cursor.advance();
            let right = self.parse_unary()?;
            left = Expression::Arithmetic(ArithmeticExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> R<Expression> {
        if self.cursor.check(TokenKind::Bang) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let expr = self.parse_unary()?;
            return Ok(Expression::Not(NotExpr {
                operand: Box::new(expr),
                source_info: si,
            }));
        }
        if self.cursor.check(TokenKind::Minus) && !self.cursor.check(TokenKind::Arrow) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let expr = self.parse_unary()?;
            return Ok(Expression::UnaryMinus(UnaryMinusExpr {
                operand: Box::new(expr),
                source_info: si,
            }));
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> R<Expression> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.cursor.check(TokenKind::Arrow) {
                let si = self.cursor.current_source_info();
                self.cursor.advance();
                let (func_name, func_si) = self.cursor.expect_identifier_or_keyword()?;
                // Build fully qualified path for the function
                let mut path = Package::root(func_name, func_si);
                while self.cursor.eat(TokenKind::PathSep) {
                    let (seg, seg_si) = self.cursor.expect_identifier_or_keyword()?;
                    path = path.child(seg, seg_si);
                }
                let (pkg, name) = split_package_name(&path);
                let func = PackageableElementPtr {
                    package: pkg,
                    name,
                    source_info: si.clone(),
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut args = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    args.push(self.parse_expression()?);
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RParen)?;
                expr = Expression::ArrowFunction(ArrowFunction {
                    target: Box::new(expr),
                    function: func,
                    arguments: args,
                    source_info: si,
                });
            } else if self.cursor.check(TokenKind::Dot) {
                let si = self.cursor.current_source_info();
                self.cursor.advance();
                let (member, _) = self.cursor.expect_identifier_or_keyword()?;
                if self.cursor.check(TokenKind::LParen) {
                    // Qualified member access: expr.member(args)
                    self.cursor.advance();
                    let mut args = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        args.push(self.parse_expression()?);
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    expr =
                        Expression::MemberAccess(MemberAccess::Qualified(QualifiedMemberAccess {
                            target: Box::new(expr),
                            member,
                            arguments: args,
                            source_info: si,
                        }));
                } else {
                    expr = Expression::MemberAccess(MemberAccess::Simple(SimpleMemberAccess {
                        target: Box::new(expr),
                        member,
                        source_info: si,
                    }));
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    #[allow(clippy::too_many_lines)]
    fn parse_primary(&mut self) -> R<Expression> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            // Literals
            TokenKind::IntegerLiteral => {
                let tok = self.cursor.advance().clone();
                let value: i64 = tok.text.parse().unwrap_or(0);
                Ok(Expression::Literal(Literal::Integer(IntegerLiteral {
                    value,
                    source_info: si,
                })))
            }
            TokenKind::FloatLiteral => {
                let tok = self.cursor.advance().clone();
                let value: f64 = tok.text.parse().unwrap_or(0.0);
                Ok(Expression::Literal(Literal::Float(FloatLiteral {
                    value,
                    source_info: si,
                })))
            }
            TokenKind::DecimalLiteral => {
                let tok = self.cursor.advance().clone();
                let text = tok.text.trim_end_matches(['D', 'd']);
                Ok(Expression::Literal(Literal::Decimal(DecimalLiteral {
                    value: text.to_string(),
                    source_info: si,
                })))
            }
            TokenKind::StringLiteral => {
                let tok = self.cursor.advance().clone();
                Ok(Expression::Literal(Literal::String(StringLiteral {
                    value: unquote_string(&tok.text),
                    source_info: si,
                })))
            }
            TokenKind::True => {
                self.cursor.advance();
                Ok(Expression::Literal(Literal::Boolean(BooleanLiteral {
                    value: true,
                    source_info: si,
                })))
            }
            TokenKind::False => {
                self.cursor.advance();
                Ok(Expression::Literal(Literal::Boolean(BooleanLiteral {
                    value: false,
                    source_info: si,
                })))
            }
            TokenKind::DateLiteral => {
                let tok = self.cursor.advance().clone();
                let raw = tok.text.trim_start_matches('%');
                if raw.contains('T') {
                    Ok(Expression::Literal(Literal::DateTime(DateTimeLiteral {
                        value: raw.to_string(),
                        source_info: si,
                    })))
                } else if raw.starts_with(|c: char| c.is_ascii_digit()) && raw.contains('-') {
                    Ok(Expression::Literal(Literal::StrictDate(
                        StrictDateLiteral {
                            value: raw.to_string(),
                            source_info: si,
                        },
                    )))
                } else {
                    Ok(Expression::Literal(Literal::StrictTime(
                        StrictTimeLiteral {
                            value: raw.to_string(),
                            source_info: si,
                        },
                    )))
                }
            }
            // Variable: $name
            TokenKind::Dollar => {
                self.cursor.advance();
                let (name, _) = self.cursor.expect_identifier_or_keyword()?;
                Ok(Expression::Variable(Variable {
                    name,
                    source_info: si,
                }))
            }
            // let binding
            TokenKind::Let => {
                self.cursor.advance();
                let (name, _) = self.cursor.expect_identifier_or_keyword()?;
                self.cursor.expect(TokenKind::Equals)?;
                let value = self.parse_expression()?;
                Ok(Expression::Let(LetExpr {
                    name,
                    value: Box::new(value),
                    source_info: si,
                }))
            }
            // No-param bare lambda: | body
            TokenKind::Pipe => {
                self.cursor.advance();
                let body = self.parse_expression()?;
                Ok(Expression::Lambda(Lambda {
                    parameters: vec![],
                    body: vec![body],
                    source_info: si,
                }))
            }
            // New instance: ^Type(props)
            TokenKind::Caret => {
                self.cursor.advance();
                let path = self.parse_package_path()?;
                let (pkg, name) = split_package_name(&path);
                let class_ref = PackageableElementPtr {
                    package: pkg,
                    name,
                    source_info: si.clone(),
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut assignments = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    let kv_si = self.cursor.current_source_info();
                    let (prop, _) = self.cursor.expect_identifier()?;
                    self.cursor.expect(TokenKind::Equals)?;
                    let val = self.parse_expression()?;
                    assignments.push(KeyValuePair {
                        key: prop,
                        value: val,
                        source_info: kv_si,
                    });
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Expression::NewInstance(NewInstanceExpr {
                    class: class_ref,
                    assignments,
                    source_info: si,
                }))
            }
            // Cast: @Type
            TokenKind::At => {
                self.cursor.advance();
                let type_ref = self.parse_type_reference()?;
                Ok(Expression::TypeReferenceExpr(TypeReferenceExpr {
                    type_ref,
                    source_info: si,
                }))
            }
            // Collection: [expr, ...]
            TokenKind::LBracket => {
                self.cursor.advance();
                let mut elements = Vec::new();
                while !self.cursor.check(TokenKind::RBracket) {
                    elements.push(self.parse_expression()?);
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RBracket)?;
                Ok(Expression::Collection(CollectionExpr {
                    elements,
                    multiplicity: None,
                    source_info: si,
                }))
            }
            // Parenthesized expression: (expr)
            TokenKind::LParen => {
                self.cursor.advance();
                let expr = self.parse_expression()?;
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Expression::Group(Box::new(expr)))
            }
            // Lambda or block: {x | body}, {| body}, {x: String[1], y | body}, or {expr; expr}
            TokenKind::LBrace => {
                self.cursor.advance();
                if self.is_lambda_start() {
                    let mut params = Vec::new();
                    // Only parse params if the next token isn't pipe ({|body})
                    if !self.cursor.check(TokenKind::Pipe) {
                        loop {
                            params.push(self.parse_lambda_param()?);
                            if !self.cursor.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.cursor.expect(TokenKind::Pipe)?;
                    let body = self.parse_expression_list()?;
                    self.cursor.expect(TokenKind::RBrace)?;
                    Ok(Expression::Lambda(Lambda {
                        parameters: params,
                        body,
                        source_info: si,
                    }))
                } else {
                    let exprs = self.parse_expression_list()?;
                    self.cursor.expect(TokenKind::RBrace)?;
                    if exprs.len() == 1 {
                        if let Some(single) = exprs.into_iter().next() {
                            Ok(single)
                        } else {
                            unreachable!("len() == 1 guarantees at least one element");
                        }
                    } else {
                        Ok(Expression::Collection(CollectionExpr {
                            elements: exprs,
                            multiplicity: None,
                            source_info: si,
                        }))
                    }
                }
            }
            // Bare lambda: x|body or x: Type[1]|body
            // Must be checked before identifier since both start with an identifier.
            TokenKind::Identifier if self.is_bare_lambda() => self.parse_bare_lambda(),
            // Identifier or element keyword used as a name.
            // Element keywords (Class, Enum, etc.) are valid identifiers in
            // expression position — e.g., `Class('arg')` or `my::Enum::VAL`.
            TokenKind::Identifier
            | TokenKind::Class
            | TokenKind::Enum
            | TokenKind::Profile
            | TokenKind::Function => {
                let path = self.parse_package_path()?;
                if self.cursor.check(TokenKind::LParen) {
                    // Function call: name(args)
                    self.cursor.advance();
                    let mut args = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        args.push(self.parse_expression()?);
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    let (pkg, name) = split_package_name(&path);
                    let func = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: si.clone(),
                    };
                    Ok(Expression::FunctionApplication(FunctionApplication {
                        function: func,
                        arguments: args,
                        source_info: si,
                    }))
                } else {
                    // Bare element reference (no parens): String, my::Enum, MyClass
                    let (pkg, name) = split_package_name(&path);
                    let element = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: si.clone(),
                    };
                    Ok(Expression::PackageableElementRef(PackageableElementRef {
                        element,
                        source_info: si,
                    }))
                }
            }
            // Island grammar: #{ content }# or #tag{ content }#
            TokenKind::HashLBrace => {
                self.cursor.advance();
                // Currently only the default tag "" is supported.
                // When tagged islands (#>{}#, #sql{}#) are added, the tag
                // will be extracted from the token stream here.
                let tag = "";

                // Temporarily take the island parsers to avoid borrow conflict
                // (we need &self.island_parsers for lookup and &mut self for
                // ParserContext simultaneously).
                let parsers = std::mem::take(&mut self.island_parsers);
                let result = (|| {
                    let island_parser =
                        parsers.iter().find(|p| p.tag() == tag).ok_or_else(|| {
                            ParseError::expected(
                                &format!("island grammar for tag '{tag}'"),
                                self.cursor.peek_kind(),
                                si.clone(),
                            )
                        })?;
                    let mut ctx = ParserContext { parser: self };
                    island_parser.parse(&mut ctx)
                })();
                self.island_parsers = parsers;

                let content = result?;
                self.cursor.expect(TokenKind::RBraceHash)?;
                Ok(Expression::Island(IslandExpression {
                    content,
                    source_info: si,
                }))
            }
            _ => Err(ParseError::expected(
                "expression",
                self.cursor.peek_kind(),
                si,
            )),
        }
    }

    /// Detects whether the current token starts a lambda inside braces.
    ///
    /// Matches patterns after `{` has been consumed:
    /// - `{x |`            — untyped single param
    /// - `{x, y |`         — untyped multi param
    /// - `{x: Type[1] |`   — typed param
    /// - `{|`              — no-param braced lambda
    fn is_lambda_start(&self) -> bool {
        // {| body}
        if self.cursor.check(TokenKind::Pipe) {
            return true;
        }
        // {ident ... |}
        matches!(
            self.cursor.peek_kind(),
            TokenKind::Identifier | TokenKind::StringLiteral
        ) && matches!(
            self.cursor.peek_kind_at(1),
            TokenKind::Pipe | TokenKind::Comma | TokenKind::Colon
        )
    }

    /// Detects bare lambda in expression position (outside braces).
    ///
    /// Only single-param bare lambdas are valid (matching Java grammar):
    /// - `x|body`              — untyped
    /// - `x: Type[mult]|body`  — typed
    fn is_bare_lambda(&self) -> bool {
        // ident followed by pipe: x|body
        if matches!(self.cursor.peek_kind_at(1), TokenKind::Pipe) {
            return true;
        }
        // ident followed by colon: x: Type[mult]|body
        // We need to scan past the type + multiplicity to check for pipe
        if matches!(self.cursor.peek_kind_at(1), TokenKind::Colon) {
            return self.scan_past_type_for_pipe(2);
        }
        false
    }

    /// Scans forward from `offset` past a type reference and multiplicity
    /// bracket to check if the next token is `|` (pipe).
    ///
    /// Used to detect typed bare lambdas: `x: path::Type<Args>[mult]|body`.
    fn scan_past_type_for_pipe(&self, start_offset: usize) -> bool {
        let mut offset = start_offset;
        // Skip the type path: identifier (:: identifier)*
        if !matches!(
            self.cursor.peek_kind_at(offset),
            TokenKind::Identifier | TokenKind::StringLiteral
        ) {
            return false;
        }
        offset += 1;
        // Skip :: segments
        while matches!(self.cursor.peek_kind_at(offset), TokenKind::PathSep) {
            offset += 1; // ::
            offset += 1; // ident
        }
        // Skip <TypeArgs>
        if matches!(self.cursor.peek_kind_at(offset), TokenKind::Less) {
            let mut depth = 1;
            offset += 1;
            while depth > 0 {
                match self.cursor.peek_kind_at(offset) {
                    TokenKind::Less => depth += 1,
                    TokenKind::Greater => depth -= 1,
                    TokenKind::Eof => return false,
                    _ => {}
                }
                offset += 1;
            }
        }
        // Expect [multiplicity]
        if !matches!(self.cursor.peek_kind_at(offset), TokenKind::LBracket) {
            return false;
        }
        offset += 1;
        // Skip multiplicity content until ]
        while !matches!(
            self.cursor.peek_kind_at(offset),
            TokenKind::RBracket | TokenKind::Eof
        ) {
            offset += 1;
        }
        if matches!(self.cursor.peek_kind_at(offset), TokenKind::RBracket) {
            offset += 1;
        }
        // Check for pipe
        matches!(self.cursor.peek_kind_at(offset), TokenKind::Pipe)
    }

    /// Parses a bare lambda: `x|body` or `x: Type[mult]|body`.
    ///
    /// Only single-param bare lambdas are valid per the Java grammar.
    fn parse_bare_lambda(&mut self) -> R<Expression> {
        let si = self.cursor.current_source_info();
        let param = self.parse_lambda_param()?;
        self.cursor.expect(TokenKind::Pipe)?;
        let body = self.parse_expression()?;
        Ok(Expression::Lambda(Lambda {
            parameters: vec![param],
            body: vec![body],
            source_info: si,
        }))
    }

    /// Parses a lambda parameter: `name` or `name: Type[multiplicity]`.
    fn parse_lambda_param(&mut self) -> R<Parameter> {
        let si = self.cursor.current_source_info();
        let (name, _) = self.cursor.expect_identifier()?;
        if self.cursor.eat(TokenKind::Colon) {
            let type_ref = self.parse_type_reference()?;
            self.cursor.expect(TokenKind::LBracket)?;
            let multiplicity = self.parse_multiplicity()?;
            self.cursor.expect(TokenKind::RBracket)?;
            Ok(Parameter {
                name,
                type_ref: Some(type_ref),
                multiplicity: Some(multiplicity),
                source_info: si,
            })
        } else {
            Ok(Parameter {
                name,
                type_ref: None,
                multiplicity: None,
                source_info: si,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// ParserContext — shared interface for island grammar plugins
// ---------------------------------------------------------------------------

/// Shared parser context passed to island grammar plugins.
///
/// Provides access to the token [`Cursor`] (via `.cursor`) and the host
/// parser's expression/path parsing capabilities.
///
/// # Example
///
/// ```rust,ignore
/// fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<...> {
///     let si = ctx.cursor().current_source_info();
///     ctx.cursor().expect(TokenKind::LBrace)?;
///     let expr = ctx.parse_expression()?;
///     // ...
/// }
/// ```
pub struct ParserContext<'a> {
    /// The underlying parser — provides cursor access and expression parsing.
    pub(crate) parser: &'a mut Parser,
}

impl ParserContext<'_> {
    /// Access the token cursor directly.
    pub(crate) fn cursor(&mut self) -> &mut Cursor {
        &mut self.parser.cursor
    }

    /// Parse a full expression using the host parser's expression grammar.
    ///
    /// This allows island grammars to embed standard Pure expressions
    /// (e.g., qualified property parameters, type references).
    pub fn parse_expression(&mut self) -> R<Expression> {
        self.parser.parse_expression()
    }

    /// Parse a package path: `my::pkg::Name`.
    pub fn parse_package_path(&mut self) -> R<Package> {
        self.parser.parse_package_path()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

pub(crate) fn split_package_name(pkg: &Package) -> (Option<Package>, SmolStr) {
    let name = SmolStr::new(pkg.name());
    (pkg.parent().cloned(), name)
}

pub(crate) fn unquote_string(s: &str) -> String {
    let inner = &s[1..s.len() - 1]; // strip surrounding quotes
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            #[allow(clippy::match_same_arms)] // Semantically distinct escapes
            match chars.next() {
                Some('\'') => result.push('\''),
                Some('\\') => result.push('\\'),
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some(other) => {
                    // Unrecognized escape — preserve verbatim
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'), // trailing backslash
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn is_wildcard_ahead(cursor: &Cursor) -> bool {
    cursor.peek_kind_at(1) == TokenKind::Star
}
