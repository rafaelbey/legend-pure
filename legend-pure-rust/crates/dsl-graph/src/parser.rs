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

//! `IslandParser` for graph-fetch (`#{ Type { fields } }#`).
//!
//! Tag: `""` (the empty-tag opener `#{ … }#` dispatches here).
//!
//! Mirrors Java's graph-fetch ANTLR rules: a root class reference
//! followed by a brace-delimited body of property selections,
//! optional `subType(@T) { … }` polymorphic narrowings, and per-
//! field aliases / parameters / sub-trees / subtype casts.
//!
//! Originally lived in
//! `legend-pure-parser-parser::island::graph_fetch`; moved here so
//! core carries no graph-fetch knowledge. Plugged into the host
//! parser via [`legend_pure_parser_parser::parse_with_islands`].

use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::{IslandParser, ParserContext, split_package_name, unquote_string};
use smol_str::SmolStr;

use crate::ast::{PropertyGraphFetchTree, RootGraphFetchTree, SubTypeGraphFetchTree, TAG};

/// Parser for graph-fetch tree syntax: `#{ Type { field1, field2 { sub } } }#`.
pub struct GraphFetchIslandParser;

impl IslandParser for GraphFetchIslandParser {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
        let tree = parse_graph_fetch_tree(ctx)?;
        Ok(Box::new(tree))
    }
}

/// Convenience helper — `vec![Box::new(GraphFetchIslandParser)]` for
/// callers that want the empty-tag default island registered.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn IslandParser>> {
    vec![Box::new(GraphFetchIslandParser)]
}

fn parse_graph_fetch_tree(ctx: &mut ParserContext<'_>) -> Result<RootGraphFetchTree, ParseError> {
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

fn parse_graph_fetch_fields(
    ctx: &mut ParserContext<'_>,
) -> Result<(Vec<PropertyGraphFetchTree>, Vec<SubTypeGraphFetchTree>), ParseError> {
    let mut sub_trees = Vec::new();
    let mut sub_type_trees = Vec::new();
    while !ctx.cursor().check(TokenKind::RBrace)
        && !ctx.cursor().check(TokenKind::RBraceHash)
        && !ctx.cursor().check(TokenKind::Eof)
    {
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

fn parse_graph_fetch_field(
    ctx: &mut ParserContext<'_>,
) -> Result<PropertyGraphFetchTree, ParseError> {
    let si = ctx.cursor().current_source_info();

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

    let sub_type = if ctx.cursor().check(TokenKind::Arrow) {
        ctx.cursor().advance();
        let (_func_name, _) = ctx.cursor().expect_identifier_or_keyword()?;
        ctx.cursor().expect(TokenKind::LParen)?;
        let type_expr = ctx.parse_expression()?;
        ctx.cursor().expect(TokenKind::RParen)?;
        match type_expr {
            Expression::TypeReferenceExpr(t) => Some(PackageableElementPtr {
                package: t.type_ref.type_ref().package.clone(),
                name: t.type_ref.type_ref().name.clone(),
                source_info: t.source_info,
            }),
            _ => {
                return Err(ParseError::Unexpected {
                    message: format!(
                        "Expected @Type, found {}",
                        ctx.cursor().peek_kind().description()
                    ),
                    source_info: si,
                });
            }
        }
    } else {
        None
    };

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

fn parse_graph_fetch_subtype(
    ctx: &mut ParserContext<'_>,
) -> Result<SubTypeGraphFetchTree, ParseError> {
    let si = ctx.cursor().current_source_info();
    let (_func_name, _) = ctx.cursor().expect_identifier_or_keyword()?;
    ctx.cursor().expect(TokenKind::LParen)?;

    let type_expr = ctx.parse_expression()?;
    ctx.cursor().expect(TokenKind::RParen)?;

    let sub_type_class = match type_expr {
        Expression::TypeReferenceExpr(t) => PackageableElementPtr {
            package: t.type_ref.type_ref().package.clone(),
            name: t.type_ref.type_ref().name.clone(),
            source_info: t.source_info,
        },
        _ => {
            return Err(ParseError::Unexpected {
                message: format!(
                    "Expected @Type, found {}",
                    ctx.cursor().peek_kind().description()
                ),
                source_info: si,
            });
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
