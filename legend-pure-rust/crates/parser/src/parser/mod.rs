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

use legend_pure_parser_ast::annotation::{Parameter, SpannedString, StereotypePtr, TaggedValue};
use legend_pure_parser_ast::element::{Constraint, Element};
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::section::{ImportStatement, Section, SourceFile};
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_ast::type_ref::{Multiplicity, Package, TypeReference};
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

mod annotation;
mod association;
mod class;
mod enum_def;
mod expression;
mod function;
pub mod helpers;
mod measure;
mod primitive;
mod profile;
mod type_ref;

pub(crate) use helpers::{is_wildcard_ahead, split_package_name, unquote_string};

use crate::cursor::Cursor;
use crate::error::ParseError;
use crate::island::IslandParser;

type R<T> = Result<T, ParseError>;

/// The common prefix of every packageable element definition:
///
/// ```text
/// KEYWORD (<<stereotypes>> | {tagged_values})* pkg::Name
/// ```
///
/// Stereotypes and tagged values may appear in any order and repeat.
pub(crate) struct ElementHeader {
    /// Stereotypes applied to the element.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values applied to the element.
    pub tagged_values: Vec<TaggedValue>,
    /// The package (if qualified), e.g. `meta::pure::profiles` → `Some(...)`.
    pub package: Option<Package>,
    /// The element name with its exact token source location.
    ///
    /// Using [`SpannedString`] rather than separate `name: SmolStr` +
    /// `name_source_info: SourceInfo` fields ensures the position can never
    /// be computed and then silently dropped — both travel together.
    pub name: SpannedString,
}

/// Main parser struct wrapping a token cursor and grammar plug-ins.
pub(crate) struct Parser {
    cursor: Cursor,
    island_parsers: Vec<Box<dyn IslandParser>>,
    section_parsers: Vec<Box<dyn crate::SectionParser>>,
}

impl Parser {
    /// Create a parser with the default set of island grammar plug-ins
    /// and no section parsers (only `Pure` sections recognised).
    pub fn new(cursor: Cursor) -> Self {
        Self {
            cursor,
            island_parsers: crate::island::default_island_parsers(),
            section_parsers: Vec::new(),
        }
    }

    /// Create a parser with a custom set of island grammar plug-ins.
    pub fn with_island_parsers(cursor: Cursor, island_parsers: Vec<Box<dyn IslandParser>>) -> Self {
        Self {
            cursor,
            island_parsers,
            section_parsers: Vec::new(),
        }
    }

    /// Create a parser with both island and section grammar plug-ins.
    pub fn with_plugins(
        cursor: Cursor,
        island_parsers: Vec<Box<dyn IslandParser>>,
        section_parsers: Vec<Box<dyn crate::SectionParser>>,
    ) -> Self {
        Self {
            cursor,
            island_parsers,
            section_parsers,
        }
    }

    // ── Top-level ───────────────────────────────────────────────────────

    /// Parse the entire source file with element-level error recovery.
    ///
    /// Returns `Ok(SourceFile)` if all elements parse successfully, or
    /// `Err(PartialSourceFile)` with the valid elements and collected
    /// errors if any elements fail.
    pub fn parse_source_file(&mut self) -> Result<SourceFile, crate::PartialSourceFile> {
        let start = self.cursor.current_source_info();
        let mut sections = Vec::new();
        let mut errors = Vec::new();

        while !self.cursor.check(TokenKind::Eof) {
            sections.push(self.parse_section(&mut errors));
        }

        if sections.is_empty() {
            sections.push(Section {
                kind: SmolStr::new("Pure"),
                imports: vec![],
                elements: vec![],
                source_info: start.clone(),
            });
        }

        let source_file = SourceFile {
            sections,
            source_info: start,
        };

        if errors.is_empty() {
            Ok(source_file)
        } else {
            Err(crate::PartialSourceFile {
                source_file,
                errors,
            })
        }
    }

    /// Parse a section with element-level error recovery.
    ///
    /// When an element fails to parse, the error is collected and the cursor
    /// skips to the next element boundary (a top-level keyword like `function`,
    /// `class`, `native`, etc.).
    ///
    /// If a registered [`SectionParser`](crate::SectionParser) matches the
    /// section header (e.g. `###Diagram`), the body is delegated to that
    /// plug-in and the resulting `DSLElement`s are wrapped as
    /// `Element::DSLElement` entries. The default `Pure` section continues
    /// to use the M3 element grammar.
    fn parse_section(&mut self, errors: &mut Vec<ParseError>) -> Section {
        let start = self.cursor.current_source_info();
        let kind = if self.cursor.check(TokenKind::SectionHeader) {
            let tok = self.cursor.advance().clone();
            SmolStr::new(tok.text.trim_start_matches('#'))
        } else {
            SmolStr::new("Pure")
        };

        // Imports still fail-fast — a malformed import is unusual and
        // likely indicates a fundamentally broken file structure.
        let mut imports = Vec::new();
        while self.cursor.check(TokenKind::Import) {
            match self.parse_import() {
                Ok(imp) => imports.push(imp),
                Err(e) => {
                    errors.push(e);
                    self.skip_to_next_element();
                }
            }
        }

        // Section-level plug-in dispatch: if a `SectionParser` is
        // registered for this section's kind, hand the cursor over.
        // The plug-in consumes tokens up to the next section header
        // / EOF and returns its own DSL elements.
        if let Some(idx) = self
            .section_parsers
            .iter()
            .position(|p| p.kind() == kind.as_str())
        {
            // Take the parser out of the Vec to avoid an aliasing
            // borrow on `self`. The plug-in only needs the cursor.
            let plug_in = self.section_parsers.swap_remove(idx);
            let dsl_elements = plug_in.parse_body(&mut self.cursor, errors);
            // Restore the parser; order doesn't matter since lookup
            // is by kind, not index.
            self.section_parsers.push(plug_in);
            let elements = dsl_elements
                .into_iter()
                .map(legend_pure_parser_ast::element::Element::DSLElement)
                .collect();
            return Section {
                kind,
                imports,
                elements,
                source_info: start,
            };
        }

        let mut elements = Vec::new();
        while !self.cursor.check(TokenKind::SectionHeader) && !self.cursor.check(TokenKind::Eof) {
            match self.parse_element() {
                Ok(elem) => elements.push(elem),
                Err(e) => {
                    errors.push(e);
                    self.skip_to_next_element();
                }
            }
        }

        Section {
            kind,
            imports,
            elements,
            source_info: start,
        }
    }

    /// Skip tokens until the cursor reaches a token that starts a new
    /// top-level element or the end of the section/file.
    ///
    /// Element boundary tokens: `function`, `native`, `class`, `enum`,
    /// `profile`, `association`, `measure`, section headers, and EOF.
    fn skip_to_next_element(&mut self) {
        loop {
            match self.cursor.peek_kind() {
                // These tokens can start a new top-level element
                TokenKind::Function
                | TokenKind::Native
                | TokenKind::Class
                | TokenKind::Enum
                | TokenKind::Profile
                | TokenKind::Association
                | TokenKind::Measure
                | TokenKind::SectionHeader
                | TokenKind::Eof => break,
                _ => {
                    self.cursor.advance();
                }
            }
        }
    }

    pub(crate) fn parse_import(&mut self) -> R<ImportStatement> {
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

    pub(crate) fn parse_element(&mut self) -> R<Element> {
        match self.cursor.peek_kind() {
            TokenKind::Profile => self.parse_profile(),
            TokenKind::Enum => self.parse_enum(),
            TokenKind::Class => self.parse_class(),
            TokenKind::Association => self.parse_association(),
            TokenKind::Measure => self.parse_measure(),
            TokenKind::Primitive => self.parse_primitive_def(),
            TokenKind::Function => self.parse_function(),
            TokenKind::Native => self.parse_native_function(),
            _ => Err(ParseError::unexpected(
                format!("Unexpected token {}", self.cursor.peek().text),
                self.cursor.current_source_info(),
            )),
        }
    }

    // ── Package path: my::pkg::Name ─────────────────────────────────────

    pub(crate) fn parse_package_path(&mut self) -> R<Package> {
        // Handle leading :: for root-qualified paths (e.g., ::meta::pure)
        // or standalone :: as a reference to the root package itself
        if self.cursor.check(TokenKind::PathSep) {
            self.cursor.advance();
            // If no identifier follows (e.g., `elementToPath(::)`), return root package
            if !self.cursor.peek_kind().is_identifier_like() {
                return Ok(Package::root(
                    SmolStr::new_static(""),
                    self.cursor.current_source_info(),
                ));
            }
        }
        let (name, si) = self.cursor.expect_identifier_or_keyword()?;
        let mut pkg = Package::root(name, si);
        while self.cursor.check(TokenKind::PathSep) && !is_wildcard_ahead(&self.cursor) {
            self.cursor.advance();
            let (seg, si) = self.cursor.expect_identifier_or_keyword()?;
            pkg = pkg.child(seg, si);
        }
        Ok(pkg)
    }

    /// Parse a qualified name, returning `(package, name, name_source_info)`.
    ///
    /// `name_source_info` points at the **last** segment — the simple name
    /// itself, not the full FQN span. Callers that need the full span can
    /// reconstruct it from the package's first-segment span; the name span
    /// is for `SourceInformation.line`/`column` parity with Java Pure.
    pub(crate) fn parse_qualified_name(&mut self) -> R<(Option<Package>, SmolStr, SourceInfo)> {
        let (first, first_si) = self.cursor.expect_identifier_or_keyword()?;
        if !self.cursor.check(TokenKind::PathSep) {
            return Ok((None, first, first_si));
        }
        let mut pkg = Package::root(first, first_si);
        while self.cursor.eat(TokenKind::PathSep) {
            let (seg, si) = self.cursor.expect_identifier_or_keyword()?;
            if self.cursor.check(TokenKind::PathSep) {
                pkg = pkg.child(seg, si);
            } else {
                return Ok((Some(pkg), seg, si));
            }
        }
        // Last segment in package is actually the name — shouldn't happen
        // since the inner return covers the has-trailing-segment case, but
        // preserve graceful handling for malformed input.
        let name = SmolStr::new(pkg.name());
        let si = legend_pure_parser_ast::source_info::Spanned::source_info(&pkg).clone();
        Ok((pkg.parent().cloned(), name, si))
    }

    /// Parses the common element header: `(<<stereotypes>> | {tagged_values})* pkg::Name`.
    ///
    /// This is the shared prefix for all packageable element definitions.
    /// Stereotypes and tagged values may appear in any order and can repeat.
    /// The element keyword must have already been consumed before calling this.
    pub(crate) fn parse_element_header(&mut self) -> R<ElementHeader> {
        let mut stereotypes = Vec::new();
        let mut tagged_values = Vec::new();

        // Parse annotations in any order: <<stereos>>, {tags}, <<stereos>>, ...
        loop {
            if self.cursor.check(TokenKind::LessLess) {
                stereotypes.extend(self.parse_stereotypes()?);
            } else if self.is_tagged_value_start() {
                tagged_values.extend(self.parse_tagged_values()?);
            } else {
                break;
            }
        }

        let (package, name_str, name_si) = self.parse_qualified_name()?;
        Ok(ElementHeader {
            stereotypes,
            tagged_values,
            package,
            name: SpannedString {
                value: name_str,
                source_info: name_si,
            },
        })
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
    ///
    /// Public so external island-parser plug-ins (`dsl-graph`,
    /// `dsl-store`, `dsl-tds`, …) can consume tokens directly when
    /// their grammar isn't expressible via the high-level helpers
    /// (`parse_expression`, `parse_package_path`, etc.).
    pub fn cursor(&mut self) -> &mut Cursor {
        &mut self.parser.cursor
    }

    /// Parse a full expression using the host parser's expression grammar.
    pub fn parse_expression(&mut self) -> R<Expression> {
        self.parser.parse_expression()
    }

    /// Parse a package path: `my::pkg::Name`.
    pub fn parse_package_path(&mut self) -> R<Package> {
        self.parser.parse_package_path()
    }

    /// Parse a qualified name, returning (package, name, `source_info`).
    pub fn parse_qualified_name(&mut self) -> R<(Option<Package>, SmolStr, SourceInfo)> {
        self.parser.parse_qualified_name()
    }

    /// Parse a type reference: `my::Class[1]`.
    pub fn parse_type_reference(&mut self) -> R<TypeReference> {
        self.parser.parse_type_reference()
    }

    /// Parse a multiplicity: `[1]`, `[1..*]`.
    pub fn parse_multiplicity(&mut self) -> R<Multiplicity> {
        self.parser.parse_multiplicity()
    }

    /// Parse stereotypes: `<<profile.stereotype>>`.
    pub fn parse_stereotypes(&mut self) -> R<Vec<StereotypePtr>> {
        self.parser.parse_stereotypes()
    }

    /// Parse tagged values: `{profile.tag = 'value'}`.
    pub fn parse_tagged_values(&mut self) -> R<Vec<TaggedValue>> {
        self.parser.parse_tagged_values()
    }

    /// Parse constraints: `[name: expr]`.
    pub fn parse_constraints(&mut self) -> R<Vec<Constraint>> {
        self.parser.parse_constraints()
    }

    /// Parse a parameter: `name: Type[1]`.
    pub fn parse_parameter(&mut self) -> R<Parameter> {
        self.parser.parse_parameter()
    }
}
