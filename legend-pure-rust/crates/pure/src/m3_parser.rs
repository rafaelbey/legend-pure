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

//! Parser for the M3 metamodel serialization format (`m3.pure`).
//!
//! The `m3.pure` file uses a specialised instance-graph serialization
//! (not normal Pure syntax). Each top-level declaration is:
//!
//! ```text
//! ^ClassifierPath Name @ParentPath { property: value, ... }
//! ```
//!
//! This module parses that format and **directly allocates** [`Element`]
//! instances into the bootstrap [`Arena`] — no intermediate AST.
//!
//! # Extracted data
//!
//! | Classifier tail | → Element variant | Fields populated |
//! |---|---|---|
//! | `Class` | [`Element::Class`] | properties (name, type, multiplicity), generalizations, type parameters |
//! | `Enumeration` | [`Element::Enumeration`] | enum values |
//! | `Profile` | [`Element::Profile`] | stereotypes, tags |
//! | `PackageableMultiplicity` | [`Element::PackageableMultiplicity`] | lower/upper bounds |
//!
//! Everything else (classifierGenericType, multiplicityArguments, etc.)
//! is skipped — the parser just balances braces and brackets.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::arena::Arena;
use crate::bootstrap::BOOTSTRAP_CHUNK_ID;
use crate::ids::ElementId;
use crate::model::{Element, ElementNode};
use crate::nodes::class::{Class, Property};
use crate::nodes::enumeration::{EnumValue, Enumeration};
use crate::nodes::profile::Profile;
use crate::types::Multiplicity;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Registration info for one M3 element that was parsed and allocated.
///
/// The caller uses this to wire the element into the package tree
/// via `model.register_element(package_id, element_id)`.
#[derive(Debug, Clone)]
pub struct M3Registration {
    /// The `ElementId` of the allocated element in the bootstrap chunk.
    pub element_id: ElementId,
    /// Package path segments (e.g., `["meta", "pure", "metamodel", "type"]`).
    pub package_segments: Vec<SmolStr>,
}

/// Parses the m3.pure serialization and directly allocates elements
/// into the given arenas.
///
/// Returns a list of [`M3Registration`]s for the caller to wire into
/// the package tree.
///
/// # Panics
///
/// Panics on malformed input — the m3.pure file is a known-good input
/// from the Java codebase, so we treat parse failures as bugs.
pub fn parse_m3_into_chunk(
    source: &str,
    nodes: &mut Arena<ElementNode>,
    elements: &mut Arena<Element>,
) -> Vec<M3Registration> {
    let mut parser = M3Parser::new(source, nodes, elements);
    parser.parse_file();
    parser.registrations
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Caret,  // ^
    Dot,    // .
    LBrack, // [
    RBrack, // ]
    LBrace, // {
    RBrace, // }
    Colon,  // :
    Comma,  // ,
    At,     // @
    Ident(SmolStr),
    StringLit(SmolStr),
    IntLit(i64),
    Eof,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance_char(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_whitespace() {
                    self.advance_char();
                } else {
                    break;
                }
            }
            // Skip `// ...` line comments
            if self.pos + 1 < self.chars.len()
                && self.chars[self.pos] == '/'
                && self.chars[self.pos + 1] == '/'
            {
                while let Some(ch) = self.advance_char() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();

        let ch = match self.advance_char() {
            Some(c) => c,
            None => return Token::Eof,
        };

        match ch {
            '^' => Token::Caret,
            '.' => Token::Dot,
            '[' => Token::LBrack,
            ']' => Token::RBrack,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            ':' => Token::Colon,
            ',' => Token::Comma,
            '@' => Token::At,
            '\'' => {
                // String literal: 'content'
                let mut s = String::new();
                loop {
                    match self.advance_char() {
                        Some('\'') => break,
                        Some(c) => s.push(c),
                        None => break,
                    }
                }
                Token::StringLit(SmolStr::new(&s))
            }
            c if c.is_ascii_digit() || c == '-' => {
                // Integer literal
                let mut s = String::new();
                s.push(c);
                while let Some(ch) = self.peek_char() {
                    if ch.is_ascii_digit() {
                        s.push(ch);
                        self.advance_char();
                    } else {
                        break;
                    }
                }
                Token::IntLit(s.parse().unwrap_or(0))
            }
            c if c.is_alphanumeric() || c == '_' => {
                // Identifier
                let mut s = String::new();
                s.push(c);
                while let Some(ch) = self.peek_char() {
                    if ch.is_alphanumeric() || ch == '_' {
                        s.push(ch);
                        self.advance_char();
                    } else {
                        break;
                    }
                }
                Token::Ident(SmolStr::new(&s))
            }
            _ => {
                // Skip unexpected chars and try next
                self.next_token()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Synthetic source info for bootstrap elements (no real file position).
fn synthetic_source() -> SourceInfo {
    SourceInfo {
        source: SmolStr::new("<m3.pure>"),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
    }
}

struct M3Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    nodes: &'a mut Arena<ElementNode>,
    elements: &'a mut Arena<Element>,
    registrations: Vec<M3Registration>,
}

impl<'a> M3Parser<'a> {
    fn new(
        source: &str,
        nodes: &'a mut Arena<ElementNode>,
        elements: &'a mut Arena<Element>,
    ) -> Self {
        // Pre-tokenize the entire file
        let mut tokenizer = Tokenizer::new(source);
        let mut tokens = Vec::new();
        loop {
            let tok = tokenizer.next_token();
            if tok == Token::Eof {
                tokens.push(Token::Eof);
                break;
            }
            tokens.push(tok);
        }

        Self {
            tokens,
            pos: 0,
            nodes,
            elements,
            registrations: Vec::new(),
        }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if tok != Token::Eof {
            self.pos += 1;
        }
        tok
    }

    fn expect_ident(&mut self) -> SmolStr {
        match self.advance() {
            Token::Ident(s) => s,
            other => panic!(
                "m3_parser: expected Ident, got {other:?} at pos {}",
                self.pos
            ),
        }
    }

    fn at(&self, tok: &Token) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(tok)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.at(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token) {
        if !self.eat(expected) {
            panic!(
                "m3_parser: expected {expected:?}, got {:?} at pos {}",
                self.peek(),
                self.pos
            );
        }
    }

    // -----------------------------------------------------------------------
    // Top-level file parsing
    // -----------------------------------------------------------------------

    fn parse_file(&mut self) {
        loop {
            match self.peek() {
                Token::Eof => break,
                Token::Caret => self.parse_top_level_instance(),
                _ => {
                    // Skip unexpected tokens
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Instance declaration: ^ classifier_path name (@ parent_path)? body?
    // -----------------------------------------------------------------------

    fn parse_top_level_instance(&mut self) {
        self.expect(&Token::Caret);

        // Parse classifier path — the tail identifier tells us the type
        let classifier_tail = self.parse_classifier_path();

        // Parse instance name
        let name = self.expect_ident();

        // Parse optional @parent_path
        let package_segments = if self.eat(&Token::At) {
            self.parse_package_path()
        } else {
            vec![]
        };

        // Parse body (if present)
        match classifier_tail.as_str() {
            "Class" => self.parse_class_body(&name, &package_segments),
            "Enumeration" => self.parse_enumeration_body(&name, &package_segments),
            "Profile" => self.parse_profile_body(&name, &package_segments),
            "PackageableMultiplicity" => {
                self.parse_multiplicity_body(&name, &package_segments);
            }
            "Package" => {
                // Skip package bodies — we handle packages via the @path
                if self.at(&Token::LBrace) {
                    self.skip_balanced_braces();
                }
            }
            "PrimitiveType" => {
                // The 11 standard primitives are already in bootstrap slots 0-12.
                // Any additional PrimitiveTypes (e.g., LatestDate) must be allocated.
                const BOOTSTRAP_PRIMS: &[&str] = &[
                    "String",
                    "Boolean",
                    "Byte",
                    "StrictTime",
                    "Number",
                    "Integer",
                    "Float",
                    "Decimal",
                    "Date",
                    "StrictDate",
                    "DateTime",
                ];
                if BOOTSTRAP_PRIMS.contains(&name.as_str()) {
                    if self.at(&Token::LBrace) {
                        self.skip_balanced_braces();
                    }
                } else {
                    // Non-bootstrap primitive — parse body to extract supertype
                    self.parse_primitive_type_body(&name, &package_segments);
                }
            }
            _ => {
                // Unknown classifier — skip body if present
                if self.at(&Token::LBrace) {
                    self.skip_balanced_braces();
                }
            }
        }
    }

    /// Parses a classifier path like `Root.children[...].children[Class]`
    /// or just `Package`. Returns the tail identifier (e.g., "Class", "Package").
    fn parse_classifier_path(&mut self) -> SmolStr {
        let mut tail = self.expect_ident();

        // Follow `.children[X]` or `.properties[X]` chains
        while self.at(&Token::Dot) {
            self.advance(); // .
            let segment = self.expect_ident();
            if self.at(&Token::LBrack) {
                self.advance(); // [
                tail = self.expect_ident();
                self.expect(&Token::RBrack);
            } else {
                tail = segment;
            }
        }

        tail
    }

    /// Parses a path like `Root.children[meta].children[pure].children[metamodel].children`
    /// and extracts the package segments: `["meta", "pure", "metamodel"]`.
    fn parse_package_path(&mut self) -> Vec<SmolStr> {
        let mut segments = Vec::new();

        // Start: expect "Root" or first identifier
        let _root = self.expect_ident();

        // Parse .children[X] chains
        while self.at(&Token::Dot) {
            self.advance(); // .
            let _accessor = self.expect_ident(); // "children" or "properties" etc.
            if self.at(&Token::LBrack) {
                self.advance(); // [
                let seg = self.expect_ident();
                segments.push(seg);
                self.expect(&Token::RBrack);
            }
        }

        segments
    }

    // -----------------------------------------------------------------------
    // Allocator helpers
    // -----------------------------------------------------------------------

    fn alloc_element(
        &mut self,
        name: &SmolStr,
        package_segments: &[SmolStr],
        element: Element,
    ) -> ElementId {
        let idx = self.nodes.alloc(ElementNode {
            name: name.clone(),
            source_info: synthetic_source(),
            name_source_info: synthetic_source(),
            parent_package: crate::ids::PackageId(0), // placeholder — wired later
        });
        let elem_idx = self.elements.alloc(element);
        debug_assert_eq!(idx, elem_idx, "node and element arenas out of sync");

        let element_id = ElementId::InstanceId {
            chunk_id: BOOTSTRAP_CHUNK_ID,
            local_idx: idx,
        };

        self.registrations.push(M3Registration {
            element_id,
            package_segments: package_segments.to_vec(),
        });

        element_id
    }

    // -----------------------------------------------------------------------
    // Class parsing
    // -----------------------------------------------------------------------

    fn parse_class_body(&mut self, name: &SmolStr, package_segments: &[SmolStr]) {
        let mut properties = Vec::new();
        let mut super_types = Vec::new();
        let mut type_parameters: Vec<SmolStr> = Vec::new();

        if !self.at(&Token::LBrace) {
            // No body — allocate empty class
            self.alloc_element(
                name,
                package_segments,
                Element::Class(Class {
                    type_parameters: vec![],
                    super_types: vec![],
                    properties: vec![],
                    qualified_properties: vec![],
                    constraints: vec![],
                    stereotypes: vec![],
                    tagged_values: vec![],
                }),
            );
            return;
        }

        self.advance(); // {

        // Parse body assignments
        loop {
            match self.peek() {
                Token::RBrace | Token::Eof => break,
                Token::Caret => {
                    // Inline instance — could be inside properties list,
                    // but at top-level of the body it's unexpected.
                    // Skip it.
                    self.skip_inline_instance();
                }
                _ => {
                    // Parse: property_path ':' value
                    let prop_path = self.parse_property_path_tail();

                    if !self.eat(&Token::Colon) {
                        // Malformed — skip to comma or closing brace
                        self.skip_to_comma_or_brace();
                        continue;
                    }

                    match prop_path.as_str() {
                        "properties" => {
                            // Class.properties[properties] : [...]
                            properties = self.parse_property_list();
                        }
                        "generalizations" => {
                            // Type.properties[generalizations] : [...]
                            // In m3.pure, generalizations stored on a type include entries
                            // for ALL subclasses, not just this class itself. We filter by
                            // `specific` to only keep entries for the current class.
                            super_types = self.parse_generalizations(&name);
                        }
                        "typeParameters" => {
                            // Class.properties[typeParameters] : [...]
                            type_parameters = self.parse_type_parameters();
                        }
                        _ => {
                            // Skip the value
                            self.skip_value();
                        }
                    }
                }
            }

            // Expect comma between assignments
            self.eat(&Token::Comma);
        }

        self.eat(&Token::RBrace);

        self.alloc_element(
            name,
            package_segments,
            Element::Class(Class {
                type_parameters,
                super_types,
                properties,
                qualified_properties: vec![],
                constraints: vec![],
                stereotypes: vec![],
                tagged_values: vec![],
            }),
        );
    }

    /// Parses a non-bootstrap `PrimitiveType` body from m3.pure.
    ///
    /// Extracts the supertype from its generalizations and allocates it as a
    /// `PrimitiveType` element. The supertype is resolved later during the
    /// compilation pipeline when package registration wires up the references.
    fn parse_primitive_type_body(&mut self, name: &SmolStr, package_segments: &[SmolStr]) {
        let mut super_type_name: Option<SmolStr> = None;

        if self.eat(&Token::LBrace) {
            loop {
                match self.peek() {
                    Token::RBrace | Token::Eof => break,
                    Token::Caret => {
                        self.skip_inline_instance();
                    }
                    _ => {
                        let prop_path = self.parse_property_path_tail();
                        if !self.eat(&Token::Colon) {
                            self.skip_to_comma_or_brace();
                            continue;
                        }

                        if prop_path.as_str() == "generalizations" {
                            let supers = self.parse_generalizations(name);
                            if let Some(crate::types::TypeExpr::Generic(s)) = supers.first() {
                                super_type_name = Some(s.clone());
                            }
                        } else {
                            self.skip_value();
                        }
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrace);
        }

        // Look up the supertype ElementId from the bootstrap primitives
        let super_type = super_type_name.and_then(|sn| {
            crate::bootstrap::BOOTSTRAP_PRIMITIVES
                .iter()
                .find(|(n, _, _)| *n == sn.as_str())
                .map(|(_, id, _)| *id)
        });

        self.alloc_element(
            name,
            package_segments,
            Element::PrimitiveType(crate::types::PrimitiveType {
                super_type,
                super_type_value_arguments: Vec::new(),
                type_variable_parameters: Vec::new(),
                constraints: Vec::new(),
            }),
        );
    }

    /// Parses a list of `^Property` instances inside `[...]`.
    fn parse_property_list(&mut self) -> Vec<Property> {
        let mut props = Vec::new();

        if self.eat(&Token::LBrack) {
            // Array of properties
            loop {
                match self.peek() {
                    Token::RBrack | Token::Eof => break,
                    Token::Caret => {
                        if let Some(p) = self.parse_property_instance() {
                            props.push(p);
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrack);
        } else if self.at(&Token::Caret) {
            // Single property (not in array)
            if let Some(p) = self.parse_property_instance() {
                props.push(p);
            }
        } else {
            self.skip_value();
        }

        props
    }

    /// Parses a single `^...Property name { ... }` instance.
    fn parse_property_instance(&mut self) -> Option<Property> {
        self.expect(&Token::Caret);
        let _classifier = self.parse_classifier_path();
        let prop_name = self.expect_ident();

        let mut multiplicity = Multiplicity::PureOne;
        let mut type_name = SmolStr::new("Any");

        if self.eat(&Token::LBrace) {
            loop {
                match self.peek() {
                    Token::RBrace | Token::Eof => break,
                    Token::Caret => {
                        self.skip_inline_instance();
                    }
                    _ => {
                        let path_tail = self.parse_property_path_tail();
                        if !self.eat(&Token::Colon) {
                            self.skip_to_comma_or_brace();
                            continue;
                        }

                        match path_tail.as_str() {
                            "name" => {
                                // ModelElement.properties[name] : 'xxx'
                                // We already have the name from the instance header
                                self.skip_value();
                            }
                            "multiplicity" => {
                                // Extract multiplicity from a reference like:
                                // Root...multiplicity.children[PureOne]
                                multiplicity = self.parse_multiplicity_ref();
                            }
                            "genericType" => {
                                // Extract the raw type name from the GenericType
                                type_name = self.parse_generic_type_raw_type();
                            }
                            _ => {
                                self.skip_value();
                            }
                        }
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrace);
        }

        Some(Property {
            name: SmolStr::new(&prop_name),
            source_info: synthetic_source(),
            type_expr: crate::types::TypeExpr::Generic(type_name),
            multiplicity,
            aggregation: None,
            default_value: None,
            stereotypes: vec![],
            tagged_values: vec![],
        })
    }

    /// Parses the generalizations list and extracts super-type names.
    ///
    /// In m3.pure, `Type.properties[generalizations]` stores Generalization
    /// instances for ALL subclasses (e.g., Class, PrimitiveType), not just
    /// the current class. Each Generalization has:
    /// - `general.rawType` — the supertype
    /// - `specific` — the actual subtype this applies to
    ///
    /// We filter by `specific` to only keep entries where the subtype matches
    /// the class we are currently parsing (`class_name`).
    fn parse_generalizations(&mut self, class_name: &str) -> Vec<crate::types::TypeExpr> {
        let mut supers = Vec::new();

        if self.eat(&Token::LBrack) {
            loop {
                match self.peek() {
                    Token::RBrack | Token::Eof => break,
                    Token::Caret => {
                        // ^Generalization { general: ^GenericType { rawType: SuperClass }, specific: SubClass }
                        if let Some((general, specific)) = self.parse_generalization_instance() {
                            // Only keep this generalization if `specific` matches
                            // the class we are currently parsing.
                            if specific.as_deref() == Some(class_name) || specific.is_none() {
                                supers.push(crate::types::TypeExpr::Generic(general));
                            }
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrack);
        } else if self.at(&Token::Caret) {
            // Single generalization (not in array)
            if let Some((general, specific)) = self.parse_generalization_instance() {
                if specific.as_deref() == Some(class_name) || specific.is_none() {
                    supers.push(crate::types::TypeExpr::Generic(general));
                }
            }
        } else {
            self.skip_value();
        }

        supers
    }

    /// Parses a `^Generalization { general: ^GenericType{rawType: X}, specific: Y }`
    /// and returns `(general_raw_type, specific_name)`.
    ///
    /// The `specific` is the simple name of the subclass extracted from the
    /// path reference (e.g., `Root.children[...].children[Class]` → `"Class"`).
    fn parse_generalization_instance(&mut self) -> Option<(SmolStr, Option<SmolStr>)> {
        self.expect(&Token::Caret);
        let _classifier = self.parse_classifier_path();

        // May or may not have a name
        if self.at(&Token::Ident(SmolStr::default())) && !self.at(&Token::LBrace) {
            // Has a name — skip it
            if let Token::Ident(_) = self.peek() {
                if !self.at(&Token::LBrace) {
                    // Check if next token after ident is { — if so, it's a body
                    // Otherwise it might be something else
                }
            }
        }

        let mut general_type = None;
        let mut specific_type = None;

        if self.eat(&Token::LBrace) {
            loop {
                match self.peek() {
                    Token::RBrace | Token::Eof => break,
                    Token::Caret => {
                        self.skip_inline_instance();
                    }
                    _ => {
                        let path_tail = self.parse_property_path_tail();
                        if !self.eat(&Token::Colon) {
                            self.skip_to_comma_or_brace();
                            continue;
                        }

                        match path_tail.as_str() {
                            "general" => {
                                // The value is a ^GenericType{rawType: XXXX}
                                // We need to extract XXXX
                                general_type = Some(self.parse_generic_type_raw_type());
                            }
                            "specific" => {
                                // The value is a path reference like:
                                // Root.children[meta]...children[Class]
                                // Extract the last segment as the specific class name.
                                specific_type = Some(self.parse_classifier_path());
                            }
                            _ => {
                                self.skip_value();
                            }
                        }
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrace);
        }

        general_type.map(|g| (g, specific_type))
    }

    /// Parses a type parameters list: `[^TypeParameter{name:'T'}, ...]`
    fn parse_type_parameters(&mut self) -> Vec<SmolStr> {
        let mut params = Vec::new();

        if self.eat(&Token::LBrack) {
            loop {
                match self.peek() {
                    Token::RBrack | Token::Eof => break,
                    Token::Caret => {
                        if let Some(name) = self.parse_type_parameter_instance() {
                            params.push(name);
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrack);
        } else if self.at(&Token::Caret) {
            if let Some(name) = self.parse_type_parameter_instance() {
                params.push(name);
            }
        } else {
            self.skip_value();
        }

        params
    }

    /// Parses `^TypeParameter{name:'T', ...}` and returns `"T"`.
    fn parse_type_parameter_instance(&mut self) -> Option<SmolStr> {
        self.expect(&Token::Caret);
        let _classifier = self.parse_classifier_path();

        let mut name = None;

        if self.eat(&Token::LBrace) {
            loop {
                match self.peek() {
                    Token::RBrace | Token::Eof => break,
                    _ => {
                        let path_tail = self.parse_property_path_tail();
                        if !self.eat(&Token::Colon) {
                            self.skip_to_comma_or_brace();
                            continue;
                        }

                        if path_tail.as_str() == "name" {
                            if let Token::StringLit(s) = self.advance() {
                                name = Some(s);
                            }
                        } else {
                            self.skip_value();
                        }
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrace);
        }

        name
    }

    /// Extracts the raw type name from a `^GenericType{rawType: X}` or
    /// a simple element reference `Root.children[...].children[X]`.
    fn parse_generic_type_raw_type(&mut self) -> SmolStr {
        if self.at(&Token::Caret) {
            // Inline ^GenericType { rawType: ... }
            self.advance(); // ^
            let _classifier = self.parse_classifier_path();

            let mut raw_type = SmolStr::new("Any");

            if self.eat(&Token::LBrace) {
                loop {
                    match self.peek() {
                        Token::RBrace | Token::Eof => break,
                        _ => {
                            let path_tail = self.parse_property_path_tail();
                            if !self.eat(&Token::Colon) {
                                self.skip_to_comma_or_brace();
                                continue;
                            }

                            if path_tail.as_str() == "rawType" {
                                raw_type = self.parse_element_ref_tail();
                            } else {
                                self.skip_value();
                            }
                        }
                    }
                    self.eat(&Token::Comma);
                }
                self.eat(&Token::RBrace);
            }

            raw_type
        } else {
            // Direct element reference: Root.children[...].children[Type]
            self.parse_element_ref_tail()
        }
    }

    /// Parses a multiplicity reference like `Root...multiplicity.children[PureOne]`
    /// and returns the corresponding `Multiplicity` variant.
    fn parse_multiplicity_ref(&mut self) -> Multiplicity {
        if self.at(&Token::Caret) {
            // Inline multiplicity instance — skip and default to PureOne
            self.skip_inline_instance();
            return Multiplicity::PureOne;
        }

        let name = self.parse_element_ref_tail();
        match name.as_str() {
            "PureOne" => Multiplicity::PureOne,
            "ZeroOne" => Multiplicity::ZeroOrOne,
            "ZeroMany" => Multiplicity::ZeroOrMany,
            "OneMany" => Multiplicity::OneOrMany,
            _ => Multiplicity::PureOne,
        }
    }

    // -----------------------------------------------------------------------
    // Enumeration parsing
    // -----------------------------------------------------------------------

    fn parse_enumeration_body(&mut self, name: &SmolStr, package_segments: &[SmolStr]) {
        let mut values = Vec::new();

        if !self.at(&Token::LBrace) {
            self.alloc_element(
                name,
                package_segments,
                Element::Enumeration(Enumeration {
                    values: vec![],
                    stereotypes: vec![],
                    tagged_values: vec![],
                }),
            );
            return;
        }

        self.advance(); // {

        loop {
            match self.peek() {
                Token::RBrace | Token::Eof => break,
                _ => {
                    let path_tail = self.parse_property_path_tail();
                    if !self.eat(&Token::Colon) {
                        self.skip_to_comma_or_brace();
                        continue;
                    }

                    if path_tail.as_str() == "values" {
                        values = self.parse_enum_values();
                    } else {
                        self.skip_value();
                    }
                }
            }
            self.eat(&Token::Comma);
        }

        self.eat(&Token::RBrace);

        self.alloc_element(
            name,
            package_segments,
            Element::Enumeration(Enumeration {
                values,
                stereotypes: vec![],
                tagged_values: vec![],
            }),
        );
    }

    /// Parses `[^EnumType Value1 {...}, ^EnumType Value2 {...}]`.
    fn parse_enum_values(&mut self) -> Vec<EnumValue> {
        let mut values = Vec::new();

        if self.eat(&Token::LBrack) {
            loop {
                match self.peek() {
                    Token::RBrack | Token::Eof => break,
                    Token::Caret => {
                        self.advance(); // ^
                        let _classifier = self.parse_classifier_path();
                        let value_name = self.expect_ident();

                        // Skip body
                        if self.at(&Token::LBrace) {
                            self.skip_balanced_braces();
                        }

                        values.push(EnumValue {
                            name: value_name,
                            source_info: synthetic_source(),
                            stereotypes: vec![],
                            tagged_values: vec![],
                        });
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrack);
        } else {
            self.skip_value();
        }

        values
    }

    // -----------------------------------------------------------------------
    // Profile parsing
    // -----------------------------------------------------------------------

    fn parse_profile_body(&mut self, name: &SmolStr, package_segments: &[SmolStr]) {
        let mut stereotypes = Vec::new();
        let mut tags = Vec::new();

        if !self.at(&Token::LBrace) {
            self.alloc_element(
                name,
                package_segments,
                Element::Profile(Profile {
                    stereotypes: vec![],
                    tags: vec![],
                }),
            );
            return;
        }

        self.advance(); // {

        loop {
            match self.peek() {
                Token::RBrace | Token::Eof => break,
                _ => {
                    let path_tail = self.parse_property_path_tail();
                    if !self.eat(&Token::Colon) {
                        self.skip_to_comma_or_brace();
                        continue;
                    }

                    match path_tail.as_str() {
                        "p_stereotypes" => {
                            stereotypes = self.parse_annotation_list();
                        }
                        "p_tags" => {
                            tags = self.parse_annotation_list();
                        }
                        _ => {
                            self.skip_value();
                        }
                    }
                }
            }
            self.eat(&Token::Comma);
        }

        self.eat(&Token::RBrace);

        self.alloc_element(
            name,
            package_segments,
            Element::Profile(Profile { stereotypes, tags }),
        );
    }

    /// Parses `[^Stereotype name {value: 'x'}, ...]` into a list of names.
    fn parse_annotation_list(&mut self) -> Vec<SmolStr> {
        let mut names = Vec::new();

        if self.eat(&Token::LBrack) {
            loop {
                match self.peek() {
                    Token::RBrack | Token::Eof => break,
                    Token::Caret => {
                        self.advance(); // ^
                        let _classifier = self.parse_classifier_path();
                        let anno_name = self.expect_ident();

                        // Skip body
                        if self.at(&Token::LBrace) {
                            self.skip_balanced_braces();
                        }

                        names.push(anno_name);
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrack);
        } else {
            self.skip_value();
        }

        names
    }

    // -----------------------------------------------------------------------
    // PackageableMultiplicity parsing
    // -----------------------------------------------------------------------

    fn parse_multiplicity_body(&mut self, name: &SmolStr, package_segments: &[SmolStr]) {
        // Map well-known multiplicity names to Multiplicity variants
        let mult = match name.as_str() {
            "PureOne" => Multiplicity::PureOne,
            "PureZero" => Multiplicity::Range {
                lower: 0,
                upper: Some(0),
            },
            "ZeroMany" => Multiplicity::ZeroOrMany,
            "ZeroOne" => Multiplicity::ZeroOrOne,
            "OneMany" => Multiplicity::OneOrMany,
            _ => Multiplicity::PureOne,
        };

        // Skip body
        if self.at(&Token::LBrace) {
            self.skip_balanced_braces();
        }

        self.alloc_element(
            name,
            package_segments,
            Element::PackageableMultiplicity(mult),
        );
    }

    // -----------------------------------------------------------------------
    // Utility: path & reference parsing
    // -----------------------------------------------------------------------

    /// Parses a property-assignment path like
    /// `Root.children[...].children[Class].properties[properties]`
    /// and returns the tail bracket content (e.g., `"properties"`).
    fn parse_property_path_tail(&mut self) -> SmolStr {
        let mut tail = self.expect_ident();

        while self.at(&Token::Dot) {
            self.advance(); // .
            let segment = self.expect_ident();
            if self.at(&Token::LBrack) {
                self.advance(); // [
                tail = self.expect_ident();
                self.expect(&Token::RBrack);
            } else {
                tail = segment;
            }
        }

        tail
    }

    /// Parses an element reference path and returns the last bracket content.
    /// e.g., `Root.children[meta].children[...].children[PureOne]` → `"PureOne"`.
    fn parse_element_ref_tail(&mut self) -> SmolStr {
        let mut tail = self.expect_ident();

        while self.at(&Token::Dot) {
            self.advance(); // .
            let segment = self.expect_ident();
            if self.at(&Token::LBrack) {
                self.advance(); // [
                tail = self.expect_ident();
                self.expect(&Token::RBrack);
            } else {
                tail = segment;
            }
        }

        tail
    }

    // -----------------------------------------------------------------------
    // Skip helpers
    // -----------------------------------------------------------------------

    /// Skips a balanced `{ ... }` block.
    fn skip_balanced_braces(&mut self) {
        self.expect(&Token::LBrace);
        let mut depth = 1u32;
        loop {
            match self.advance() {
                Token::LBrace => depth += 1,
                Token::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                Token::Eof => return,
                _ => {}
            }
        }
    }

    /// Skips a value in a property assignment.
    fn skip_value(&mut self) {
        match self.peek() {
            Token::LBrack => {
                // Array value — skip balanced brackets
                self.advance();
                let mut depth = 1u32;
                loop {
                    match self.advance() {
                        Token::LBrack => depth += 1,
                        Token::RBrack => {
                            depth -= 1;
                            if depth == 0 {
                                return;
                            }
                        }
                        Token::Eof => return,
                        _ => {}
                    }
                }
            }
            Token::Caret => {
                self.skip_inline_instance();
            }
            Token::StringLit(_) | Token::IntLit(_) => {
                self.advance();
            }
            Token::Ident(_) => {
                // Element reference — consume path
                let _ = self.parse_element_ref_tail();
            }
            _ => {
                self.advance();
            }
        }
    }

    /// Skips an inline `^ClassifierPath Name? { body }` instance.
    fn skip_inline_instance(&mut self) {
        self.advance(); // ^
        let _ = self.parse_classifier_path();

        // Optional name
        if let Token::Ident(_) = self.peek() {
            // Check if this is a name followed by { or if it's something else
            let saved_pos = self.pos;
            self.advance(); // consume potential name
            if self.at(&Token::LBrace) {
                self.skip_balanced_braces();
                return;
            }
            // Not a body — restore position
            self.pos = saved_pos;
        }

        if self.at(&Token::LBrace) {
            self.skip_balanced_braces();
        }
    }

    /// Advances until we find a comma or closing brace at the current depth.
    fn skip_to_comma_or_brace(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.peek() {
                Token::Eof => return,
                Token::Comma if depth == 0 => return,
                Token::RBrace if depth == 0 => return,
                Token::LBrace => {
                    depth += 1;
                    self.advance();
                }
                Token::RBrace => {
                    depth -= 1;
                    self.advance();
                }
                Token::LBrack => {
                    depth += 1;
                    self.advance();
                }
                Token::RBrack => {
                    depth = depth.saturating_sub(1);
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fragment(source: &str) -> (Arena<ElementNode>, Arena<Element>, Vec<M3Registration>) {
        let mut nodes = Arena::new();
        let mut elements = Arena::new();
        let regs = parse_m3_into_chunk(source, &mut nodes, &mut elements);
        (nodes, elements, regs)
    }

    #[test]
    fn parse_profile_with_stereotypes() {
        let src = r#"
^Root.children[meta].children[pure].children[metamodel].children[extension].children[Profile] ProtocolInfo @Root.children[meta].children[pure].children[metamodel].children
{
    Root.children[meta].children[pure].children[metamodel].children[PackageableElement].properties[package] : Root.children[meta].children[pure].children[metamodel],
    Root.children[meta].children[pure].children[metamodel].children[ModelElement].properties[name] : 'ProtocolInfo',
    Root.children[meta].children[pure].children[metamodel].children[extension].children[Profile].properties[p_stereotypes] : [
        ^Root.children[meta].children[pure].children[metamodel].children[extension].children[Stereotype] inferred
        {
            Root.children[meta].children[pure].children[metamodel].children[extension].children[Annotation].properties[profile] : Root.children[meta].children[pure].children[metamodel].children[ProtocolInfo],
            Root.children[meta].children[pure].children[metamodel].children[extension].children[Annotation].properties[value] : 'inferred'
        },
        ^Root.children[meta].children[pure].children[metamodel].children[extension].children[Stereotype] excluded
        {
            Root.children[meta].children[pure].children[metamodel].children[extension].children[Annotation].properties[profile] : Root.children[meta].children[pure].children[metamodel].children[ProtocolInfo],
            Root.children[meta].children[pure].children[metamodel].children[extension].children[Annotation].properties[value] : 'excluded'
        }
    ]
}
        "#;

        let (nodes, elements, regs) = parse_fragment(src);
        assert_eq!(regs.len(), 1);
        assert_eq!(nodes.get(0).name, "ProtocolInfo");

        if let Element::Profile(p) = elements.get(0) {
            assert_eq!(
                p.stereotypes,
                vec![SmolStr::new("inferred"), SmolStr::new("excluded")]
            );
            assert!(p.tags.is_empty());
        } else {
            panic!("Expected Profile, got {:?}", elements.get(0));
        }
    }

    #[test]
    fn parse_enumeration_with_values() {
        let src = r#"
^Root.children[meta].children[pure].children[metamodel].children[type].children[Enumeration] AggregationKind @Root.children[meta].children[pure].children[metamodel].children[function].children[property].children
{
    Root.children[meta].children[pure].children[metamodel].children[type].children[Enumeration].properties[values]:
             [
                 ^Root.children[meta].children[pure].children[metamodel].children[function].children[property].children[AggregationKind] None {Root.children[meta].children[pure].children[metamodel].children[type].children[Enum].properties[name] : 'None'},
                 ^Root.children[meta].children[pure].children[metamodel].children[function].children[property].children[AggregationKind] Shared {Root.children[meta].children[pure].children[metamodel].children[type].children[Enum].properties[name] : 'Shared'},
                 ^Root.children[meta].children[pure].children[metamodel].children[function].children[property].children[AggregationKind] Composite {Root.children[meta].children[pure].children[metamodel].children[type].children[Enum].properties[name] : 'Composite'}
             ],
    Root.children[meta].children[pure].children[metamodel].children[ModelElement].properties[name]:'AggregationKind'
}
        "#;

        let (_nodes, elements, regs) = parse_fragment(src);
        assert_eq!(regs.len(), 1);

        if let Element::Enumeration(e) = elements.get(0) {
            let names: Vec<&str> = e.values.iter().map(|v| v.name.as_str()).collect();
            assert_eq!(names, vec!["None", "Shared", "Composite"]);
        } else {
            panic!("Expected Enumeration");
        }
    }

    #[test]
    fn parse_package_skipped() {
        let src = r#"
^Package meta @Root.children
{
    Root.children[meta].children[pure].children[metamodel].children[ModelElement].properties[name] : 'meta',
    Package.properties[children] : [],
    Root.children[meta].children[pure].children[metamodel].children[PackageableElement].properties[package] : Root
}
        "#;

        let (_nodes, _elements, regs) = parse_fragment(src);
        // Packages don't produce elements
        assert_eq!(regs.len(), 0);
    }

    #[test]
    fn parse_real_m3_pure() {
        let source = include_str!(
            "../../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/grammar/m3.pure"
        );

        let (nodes, elements, regs) = parse_fragment(source);

        // Should have parsed a significant number of elements
        assert!(
            regs.len() > 80,
            "expected > 80 M3 elements from m3.pure, got {}",
            regs.len()
        );

        // Count registrations with and without package segments
        let with_pkg = regs
            .iter()
            .filter(|r| !r.package_segments.is_empty())
            .count();
        eprintln!(
            "{} of {} registrations have package segments",
            with_pkg,
            regs.len()
        );

        // Verify ProtocolInfo profile has stereotypes
        let protocol_idx = (0..nodes.len())
            .find(|&i| nodes.get(i).name == "ProtocolInfo")
            .expect("ProtocolInfo should exist");
        if let Element::Profile(p) = elements.get(protocol_idx as u32) {
            assert_eq!(
                p.stereotypes,
                vec![SmolStr::new("inferred"), SmolStr::new("excluded")],
                "ProtocolInfo should have stereotypes inferred and excluded"
            );
        } else {
            panic!("ProtocolInfo should be a Profile");
        }

        // Verify AggregationKind enumeration has values
        let agg_idx = (0..nodes.len())
            .find(|&i| nodes.get(i).name == "AggregationKind")
            .expect("AggregationKind should exist");
        if let Element::Enumeration(e) = elements.get(agg_idx as u32) {
            let names: Vec<&str> = e.values.iter().map(|v| v.name.as_str()).collect();
            assert_eq!(names, vec!["None", "Shared", "Composite"]);
        } else {
            panic!("AggregationKind should be an Enumeration");
        }

        // Verify PureOne multiplicity exists
        let pure_one_idx = (0..nodes.len())
            .find(|&i| nodes.get(i).name == "PureOne")
            .expect("PureOne should exist");
        assert!(
            matches!(
                elements.get(pure_one_idx as u32),
                Element::PackageableMultiplicity(Multiplicity::PureOne)
            ),
            "PureOne should be PackageableMultiplicity(PureOne)"
        );

        eprintln!("Parsed {} M3 elements from m3.pure", regs.len());
    }
}
