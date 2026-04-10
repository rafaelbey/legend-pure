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

//! Island grammar parser plugin infrastructure.
//!
//! Each island grammar (graph fetch, path, SQL, etc.) provides an
//! [`IslandParser`] implementation that can parse the content between
//! `#tag{` and `}#` tokens. The main parser dispatches to the
//! appropriate plugin based on the tag.
//!
//! # Adding a New Island Grammar
//!
//! 1. Define your AST types implementing [`IslandContent`]
//! 2. Implement [`IslandParser`] for your grammar
//! 3. Register it via [`default_island_parsers`] or pass to `parse_with_islands()`
//!
//! [`IslandContent`]: legend_pure_parser_ast::island::IslandContent

use legend_pure_parser_ast::island::IslandContent;

use crate::error::ParseError;
use crate::parser::ParserContext;

/// Trait for island grammar parsers.
///
/// Each island grammar provides a parser that can consume tokens between
/// `#tag{` and `}#` and produce a boxed [`IslandContent`].
///
/// The parser is called after the opening `#{` (or `#tag{`) has been consumed.
/// It must NOT consume the closing `}#` — the main parser handles that.
pub trait IslandParser {
    /// The tag this parser handles.
    ///
    /// - `""` for `#{…}#` (graph fetch)
    /// - `">"` for `#>{…}#` (path, future)
    /// - `"sql"` for `#sql{…}#` (embedded SQL, future)
    fn tag(&self) -> &str;

    /// Parse island content from the parser context.
    ///
    /// Called after `#{` (or `#tag{`) has been consumed. The
    /// `ParserContext` provides access to the token cursor and
    /// expression parsing utilities.
    ///
    /// # Errors
    ///
    /// Returns `ParseError` if the island content is malformed.
    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError>;
}

/// Returns the default set of built-in island parsers.
///
/// Currently includes only the graph fetch parser (`#{}#`).
/// External crates can extend this by providing additional parsers
/// to `parse_with_islands()`.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn IslandParser>> {
    vec![Box::new(graph_fetch::GraphFetchIslandParser)]
}

// ---------------------------------------------------------------------------
// Graph Fetch Island Parser
// ---------------------------------------------------------------------------

/// Graph fetch island parser implementation.
///
/// This module is public so the types are accessible for testing and
/// direct use, but the recommended entry point is via `default_island_parsers()`.
pub mod graph_fetch {
    use legend_pure_parser_ast::annotation::PackageableElementPtr;
    use legend_pure_parser_ast::expression::Expression;
    use legend_pure_parser_ast::island::{
        IslandContent, PropertyGraphFetchTree, RootGraphFetchTree, SubTypeGraphFetchTree,
    };
    use legend_pure_parser_lexer::TokenKind;
    use smol_str::SmolStr;

    use crate::error::ParseError;
    use crate::parser::{ParserContext, split_package_name, unquote_string};

    use super::IslandParser;

    /// Parser for graph fetch tree syntax: `#{Type{field1, field2{sub}}}#`.
    pub struct GraphFetchIslandParser;

    impl IslandParser for GraphFetchIslandParser {
        #[allow(clippy::unnecessary_literal_bound)]
        fn tag(&self) -> &str {
            ""
        }

        fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
            let tree = parse_graph_fetch_tree(ctx)?;
            Ok(Box::new(tree))
        }
    }

    /// Parse a graph fetch tree starting from the root class name.
    fn parse_graph_fetch_tree(
        ctx: &mut ParserContext<'_>,
    ) -> Result<RootGraphFetchTree, ParseError> {
        let si = ctx.cursor().current_source_info();
        let path = ctx.parse_package_path()?;
        let (pkg, name) = split_package_name(&path);
        let class_ref = PackageableElementPtr {
            package: pkg,
            name,
            source_info: si.clone(),
        };
        ctx.cursor().expect(TokenKind::LBrace)?;
        let (sub_trees, sub_type_trees) = parse_graph_fetch_fields(ctx)?;
        ctx.cursor().expect(TokenKind::RBrace)?;

        Ok(RootGraphFetchTree {
            class: class_ref,
            sub_trees,
            sub_type_trees,
            source_info: si,
        })
    }

    /// Parse comma-separated graph fetch fields inside `{ ... }`.
    fn parse_graph_fetch_fields(
        ctx: &mut ParserContext<'_>,
    ) -> Result<(Vec<PropertyGraphFetchTree>, Vec<SubTypeGraphFetchTree>), ParseError> {
        let mut sub_trees = Vec::new();
        let mut sub_type_trees = Vec::new();
        while !ctx.cursor().check(TokenKind::RBrace)
            && !ctx.cursor().check(TokenKind::RBraceHash)
            && !ctx.cursor().check(TokenKind::Eof)
        {
            // `subType(@Type){fields}` — polymorphic narrowing on the current class
            if ctx.cursor().peek().text == "subType"
                && ctx.cursor().peek_kind_at(1) == TokenKind::LParen
            {
                sub_type_trees.push(parse_graph_fetch_subtype(ctx)?);
            } else {
                sub_trees.push(parse_graph_fetch_field(ctx)?);
            }
            ctx.cursor().eat(TokenKind::Comma);
        }
        Ok((sub_trees, sub_type_trees))
    }

    /// Parse a single property field, optionally with alias, parameters, and sub-tree.
    fn parse_graph_fetch_field(
        ctx: &mut ParserContext<'_>,
    ) -> Result<PropertyGraphFetchTree, ParseError> {
        let si = ctx.cursor().current_source_info();

        // Check for alias: 'alias' : fieldName
        let alias = if ctx.cursor().check(TokenKind::StringLiteral)
            && ctx.cursor().peek_kind_at(1) == TokenKind::Colon
        {
            let tok = ctx.cursor().advance().clone();
            ctx.cursor().advance(); // :
            Some(SmolStr::from(unquote_string(&tok.text)))
        } else {
            None
        };

        let (field_name, _) = ctx.cursor().expect_identifier_or_keyword()?;

        // Qualified property parameters: field(args)
        let parameters = if ctx.cursor().check(TokenKind::LParen) {
            ctx.cursor().advance();
            let mut args = Vec::new();
            while !ctx.cursor().check(TokenKind::RParen) {
                args.push(ctx.parse_expression()?);
                ctx.cursor().eat(TokenKind::Comma);
            }
            ctx.cursor().expect(TokenKind::RParen)?;
            args
        } else {
            vec![]
        };

        // Optional subtype cast on property: property->subType(@Type)
        let sub_type = if ctx.cursor().check(TokenKind::Arrow) {
            ctx.cursor().advance(); // ->
            let (_func_name, _) = ctx.cursor().expect_identifier_or_keyword()?; // subType
            ctx.cursor().expect(TokenKind::LParen)?;
            let type_expr = ctx.parse_expression()?;
            ctx.cursor().expect(TokenKind::RParen)?;
            match type_expr {
                Expression::TypeReferenceExpr(t) => Some(PackageableElementPtr {
                    package: t.type_ref.package.clone(),
                    name: t.type_ref.name.clone(),
                    source_info: t.source_info,
                }),
                _ => {
                    return Err(ParseError::expected("@Type", ctx.cursor().peek_kind(), si));
                }
            }
        } else {
            None
        };

        // Sub-tree: { subfields }
        let (sub_trees, sub_type_trees) = if ctx.cursor().check(TokenKind::LBrace) {
            ctx.cursor().advance();
            let result = parse_graph_fetch_fields(ctx)?;
            ctx.cursor().expect(TokenKind::RBrace)?;
            result
        } else {
            (vec![], vec![])
        };

        Ok(PropertyGraphFetchTree {
            property: field_name,
            parameters,
            alias,
            sub_type,
            sub_trees,
            sub_type_trees,
            source_info: si,
        })
    }

    /// Parse `subType(@Type) { fields }` inside a graph fetch tree.
    ///
    /// This is the class-level polymorphic narrowing form — no arrow prefix.
    /// The arrow form `property->subType(@Type)` is handled by the property parser.
    fn parse_graph_fetch_subtype(
        ctx: &mut ParserContext<'_>,
    ) -> Result<SubTypeGraphFetchTree, ParseError> {
        let si = ctx.cursor().current_source_info();
        let (_func_name, _) = ctx.cursor().expect_identifier_or_keyword()?; // subType
        ctx.cursor().expect(TokenKind::LParen)?;

        // Parse @Type
        let type_expr = ctx.parse_expression()?;
        ctx.cursor().expect(TokenKind::RParen)?;

        let sub_type_class = match type_expr {
            Expression::TypeReferenceExpr(t) => PackageableElementPtr {
                package: t.type_ref.package.clone(),
                name: t.type_ref.name.clone(),
                source_info: t.source_info,
            },
            _ => {
                return Err(ParseError::expected("@Type", ctx.cursor().peek_kind(), si));
            }
        };

        let (sub_trees, sub_type_trees) = if ctx.cursor().check(TokenKind::LBrace) {
            ctx.cursor().advance();
            let result = parse_graph_fetch_fields(ctx)?;
            ctx.cursor().expect(TokenKind::RBrace)?;
            result
        } else {
            (vec![], vec![])
        };

        Ok(SubTypeGraphFetchTree {
            sub_type_class,
            sub_trees,
            sub_type_trees,
            source_info: si,
        })
    }
}
