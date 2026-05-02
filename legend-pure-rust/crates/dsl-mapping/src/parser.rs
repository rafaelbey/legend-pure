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

//! `SectionParser` for `###Mapping` section bodies.
//!
//! Recognises a stream of `Mapping pkg::M ( includes* classMappings* )`
//! declarations whose class-mapping bodies are dispatched by
//! `parserName` token. Currently supports `Pure` (model-to-model) and
//! `EnumerationMapping` (Stage 4). Other parser names produce an
//! `UnsupportedSubParser` error pointing at the staged roadmap.
//!
//! Mirrors `legend-pure-dsl-mapping/.../MappingParser.g4` for the
//! top-level rule shape; the body grammar is hand-coded rather than
//! re-using the `mappingInstanceElement: CONTENT*` opaque-content
//! design from Java, because the Rust port already has the lexer and
//! `ParserContext::parse_expression` available.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::ParserContext;
use legend_pure_parser_parser::cursor::Cursor;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::section_parser::SectionParser;
use smol_str::SmolStr;

use crate::ast::{
    ClassMapping, ClassMappingBody, EnumSourceValue, EnumValueMapping, EnumerationClassMappingBody,
    MappingDef, MappingInclude, PureClassMappingBody, PurePropertyMapping, SECTION_KIND,
    StoreSubstitution,
};

fn err_unexpected(expected: &str, found: &str, source_info: SourceInfo) -> ParseError {
    ParseError::Unexpected {
        message: format!("Expected {expected}, found '{found}'"),
        source_info,
    }
}

/// Plug-in that owns the `###Mapping` section grammar.
///
/// Register with the core parser via
/// [`legend_pure_parser_parser::parse_with_sections`].
pub struct MappingSectionParser;

impl SectionParser for MappingSectionParser {
    fn kind(&self) -> &str {
        SECTION_KIND
    }

    fn parse_body(
        &self,
        ctx: &mut ParserContext<'_>,
        errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>> {
        let mut out: Vec<Box<dyn DSLElement>> = Vec::new();
        while !ctx.cursor().check(TokenKind::SectionHeader) && !ctx.cursor().check(TokenKind::Eof) {
            // Each iteration must make progress on either Ok or Err to
            // avoid an infinite loop on malformed input.
            let pos_before = ctx.cursor().peek().source_info.clone();
            match parse_mapping(ctx) {
                Ok(m) => out.push(Box::new(m)),
                Err(e) => {
                    errors.push(e);
                    skip_to_recovery_point(ctx.cursor());
                }
            }
            if ctx.cursor().peek().source_info == pos_before {
                ctx.cursor().advance();
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

fn parse_mapping(ctx: &mut ParserContext<'_>) -> Result<MappingDef, ParseError> {
    let start = ctx.cursor().current_source_info();
    let kw = ctx.cursor().expect(TokenKind::Identifier)?;
    if kw.text != "Mapping" {
        return Err(err_unexpected("Mapping", &kw.text, kw.source_info.clone()));
    }

    let (package, name, name_si) = ctx.parse_qualified_name()?;

    ctx.cursor().expect(TokenKind::LParen)?;

    let mut includes = Vec::new();
    let mut class_mappings = Vec::new();
    while !ctx.cursor().check(TokenKind::RParen) && !ctx.cursor().check(TokenKind::Eof) {
        if is_include_keyword(ctx.cursor()) {
            includes.push(parse_include(ctx)?);
        } else {
            class_mappings.push(parse_class_mapping(ctx)?);
        }
    }
    let close = ctx.cursor().expect(TokenKind::RParen)?;

    let source_info = merge_si(&start, &close.source_info);

    Ok(MappingDef {
        package,
        name: SpannedString {
            value: name,
            source_info: name_si,
        },
        includes,
        class_mappings,
        source_info,
    })
}

fn is_include_keyword(cursor: &Cursor) -> bool {
    cursor.check(TokenKind::Identifier) && cursor.peek().text == "include"
}

// ---------------------------------------------------------------------------
// MappingInclude
// ---------------------------------------------------------------------------

fn parse_include(ctx: &mut ParserContext<'_>) -> Result<MappingInclude, ParseError> {
    let kw = ctx.cursor().expect(TokenKind::Identifier)?; // "include"
    let start = kw.source_info.clone();
    let included = parse_packageable_ptr(ctx)?;

    let mut store_substitutions = Vec::new();
    let mut end_si = included.source_info.clone();

    if ctx.cursor().eat(TokenKind::LBracket) {
        if !ctx.cursor().check(TokenKind::RBracket) {
            loop {
                store_substitutions.push(parse_store_substitution(ctx)?);
                if !ctx.cursor().eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        let close = ctx.cursor().expect(TokenKind::RBracket)?;
        end_si = close.source_info;
    }

    Ok(MappingInclude {
        included,
        store_substitutions,
        source_info: merge_si(&start, &end_si),
    })
}

fn parse_store_substitution(ctx: &mut ParserContext<'_>) -> Result<StoreSubstitution, ParseError> {
    let source = parse_packageable_ptr(ctx)?;
    ctx.cursor().expect(TokenKind::Arrow)?;
    let target = parse_packageable_ptr(ctx)?;
    let si = merge_si(&source.source_info, &target.source_info);
    Ok(StoreSubstitution {
        source,
        target,
        source_info: si,
    })
}

// ---------------------------------------------------------------------------
// ClassMapping
// ---------------------------------------------------------------------------

fn parse_class_mapping(ctx: &mut ParserContext<'_>) -> Result<ClassMapping, ParseError> {
    let start = ctx.cursor().current_source_info();

    let is_root = ctx.cursor().eat(TokenKind::Star);
    let class = parse_packageable_ptr(ctx)?;

    let id = if ctx.cursor().eat(TokenKind::LBracket) {
        let id_tok = ctx.cursor().expect(TokenKind::Identifier)?;
        let id_text = SmolStr::new(id_tok.text.clone());
        ctx.cursor().expect(TokenKind::RBracket)?;
        Some(id_text)
    } else {
        None
    };

    let extends = if ctx.cursor().eat(TokenKind::Extends) {
        ctx.cursor().expect(TokenKind::LBracket)?;
        let super_tok = ctx.cursor().expect(TokenKind::Identifier)?;
        let super_text = SmolStr::new(super_tok.text.clone());
        ctx.cursor().expect(TokenKind::RBracket)?;
        Some(super_text)
    } else {
        None
    };

    ctx.cursor().expect(TokenKind::Colon)?;
    let parser_name_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let parser_name = parser_name_tok.text.clone();
    let parser_name_si = parser_name_tok.source_info.clone();

    // Optional mapping-instance display name appearing between the
    // parserName and the body's `{`.
    let mapping_name =
        if ctx.cursor().check(TokenKind::Identifier) && !ctx.cursor().check(TokenKind::LBrace) {
            let nm = ctx.cursor().expect(TokenKind::Identifier)?;
            Some(SmolStr::new(nm.text.clone()))
        } else {
            None
        };

    let body = match parser_name.as_str() {
        "Pure" => ClassMappingBody::Pure(Box::new(parse_pure_body(ctx)?)),
        "EnumerationMapping" => ClassMappingBody::Enumeration(parse_enumeration_body(ctx)?),
        other => {
            return Err(ParseError::Unexpected {
                message: format!(
                    "Mapping sub-parser '{other}' is not supported yet (Stages 2/4 ship 'Pure' \
                     and 'EnumerationMapping'; Operation/AggregationAware/XStore/Relation arrive \
                     in Stages 5–8 — see ~/.claude/plans/what-is-left-to-iterative-sunrise.md)"
                ),
                source_info: parser_name_si,
            });
        }
    };

    let end_si = ctx.cursor().current_source_info();

    Ok(ClassMapping {
        is_root,
        class,
        id,
        extends,
        mapping_name,
        body,
        source_info: merge_si(&start, &end_si),
    })
}

// ---------------------------------------------------------------------------
// PureClassMappingBody — the Stage-2 body grammar
// ---------------------------------------------------------------------------

fn parse_pure_body(ctx: &mut ParserContext<'_>) -> Result<PureClassMappingBody, ParseError> {
    ctx.cursor().expect(TokenKind::LBrace)?;

    let mut src_class: Option<PackageableElementPtr> = None;
    let mut filter: Option<_> = None;
    let mut property_mappings = Vec::new();

    while !ctx.cursor().check(TokenKind::RBrace) && !ctx.cursor().check(TokenKind::Eof) {
        if ctx.cursor().check(TokenKind::Tilde) {
            // `~src ...` or `~filter ...`
            ctx.cursor().advance();
            let kw = ctx.cursor().expect(TokenKind::Identifier)?;
            match kw.text.as_str() {
                "src" => {
                    if src_class.is_some() {
                        return Err(err_unexpected(
                            "single ~src clause",
                            "duplicate ~src",
                            kw.source_info,
                        ));
                    }
                    src_class = Some(parse_packageable_ptr(ctx)?);
                }
                "filter" => {
                    if filter.is_some() {
                        return Err(err_unexpected(
                            "single ~filter clause",
                            "duplicate ~filter",
                            kw.source_info,
                        ));
                    }
                    filter = Some(ctx.parse_expression()?);
                }
                other => {
                    return Err(err_unexpected(
                        "src or filter (after `~`)",
                        other,
                        kw.source_info,
                    ));
                }
            }
            // Optional comma between ~clauses and following entries.
            ctx.cursor().eat(TokenKind::Comma);
        } else {
            // `propertyName : transform`
            let prop_tok = ctx.cursor().expect(TokenKind::Identifier)?;
            let prop_si = prop_tok.source_info.clone();
            let prop_name = SmolStr::new(prop_tok.text.clone());
            ctx.cursor().expect(TokenKind::Colon)?;
            let transform = ctx.parse_expression()?;
            // Capture span up to the end of the transform — for the
            // entry-level source_info.
            let end_si = ctx.cursor().current_source_info();
            property_mappings.push(PurePropertyMapping {
                property_name: prop_name,
                transform,
                transformer: None,
                explode: false,
                source_info: merge_si(&prop_si, &end_si),
            });
            // Property entries are comma-separated; trailing comma OK.
            ctx.cursor().eat(TokenKind::Comma);
        }
    }
    ctx.cursor().expect(TokenKind::RBrace)?;

    Ok(PureClassMappingBody {
        src_class,
        filter,
        property_mappings,
    })
}

// ---------------------------------------------------------------------------
// EnumerationClassMappingBody — Stage 4
// ---------------------------------------------------------------------------

fn parse_enumeration_body(
    ctx: &mut ParserContext<'_>,
) -> Result<EnumerationClassMappingBody, ParseError> {
    ctx.cursor().expect(TokenKind::LBrace)?;
    let mut value_mappings = Vec::new();
    while !ctx.cursor().check(TokenKind::RBrace) && !ctx.cursor().check(TokenKind::Eof) {
        value_mappings.push(parse_enum_value_mapping(ctx)?);
        // Comma-separated, trailing comma OK.
        ctx.cursor().eat(TokenKind::Comma);
    }
    ctx.cursor().expect(TokenKind::RBrace)?;
    Ok(EnumerationClassMappingBody { value_mappings })
}

fn parse_enum_value_mapping(ctx: &mut ParserContext<'_>) -> Result<EnumValueMapping, ParseError> {
    let name_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let start_si = name_tok.source_info.clone();
    let enum_value_name = SmolStr::new(name_tok.text.clone());
    ctx.cursor().expect(TokenKind::Colon)?;

    let source_values = if ctx.cursor().eat(TokenKind::LBracket) {
        // `[v1, v2, …]` multi-value form.
        let mut values = Vec::new();
        if !ctx.cursor().check(TokenKind::RBracket) {
            loop {
                values.push(parse_enum_source_value(ctx)?);
                if !ctx.cursor().eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        ctx.cursor().expect(TokenKind::RBracket)?;
        values
    } else {
        // Single-value form.
        vec![parse_enum_source_value(ctx)?]
    };

    let end_si = ctx.cursor().current_source_info();
    Ok(EnumValueMapping {
        enum_value_name,
        source_values,
        source_info: merge_si(&start_si, &end_si),
    })
}

fn parse_enum_source_value(ctx: &mut ParserContext<'_>) -> Result<EnumSourceValue, ParseError> {
    let tok = ctx.cursor().peek().clone();
    match tok.kind {
        TokenKind::StringLiteral => {
            ctx.cursor().advance();
            // Strip the surrounding single quotes the lexer keeps.
            let raw = tok.text.as_str();
            let stripped = raw.strip_prefix('\'').unwrap_or(raw);
            let stripped = stripped.strip_suffix('\'').unwrap_or(stripped);
            Ok(EnumSourceValue::String {
                value: SmolStr::new(stripped),
                source_info: tok.source_info,
            })
        }
        TokenKind::IntegerLiteral => {
            ctx.cursor().advance();
            let value: i64 = tok.text.parse().map_err(|_| ParseError::Unexpected {
                message: format!("Invalid integer literal '{}'", tok.text),
                source_info: tok.source_info.clone(),
            })?;
            Ok(EnumSourceValue::Integer {
                value,
                source_info: tok.source_info,
            })
        }
        TokenKind::Identifier => {
            // `pkg::Enum.VALUE` reference. parse_qualified_name reads
            // `pkg::Enum`; then `.VALUE` follows.
            let (package, name, name_si) = ctx.parse_qualified_name()?;
            ctx.cursor().expect(TokenKind::Dot)?;
            let value_tok = ctx.cursor().expect(TokenKind::Identifier)?;
            let end_si = value_tok.source_info.clone();
            Ok(EnumSourceValue::EnumRef {
                enumeration: PackageableElementPtr {
                    package,
                    name,
                    source_info: name_si.clone(),
                },
                value_name: SmolStr::new(value_tok.text.clone()),
                source_info: merge_si(&name_si, &end_si),
            })
        }
        _ => Err(ParseError::Unexpected {
            message: format!(
                "Expected enum source value (string literal, integer literal, or qualified \
                 enum-value reference), found '{}'",
                tok.text
            ),
            source_info: tok.source_info,
        }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_packageable_ptr(ctx: &mut ParserContext<'_>) -> Result<PackageableElementPtr, ParseError> {
    let (package, name, si) = ctx.parse_qualified_name()?;
    Ok(PackageableElementPtr {
        package,
        name,
        source_info: si,
    })
}

fn merge_si(start: &SourceInfo, end: &SourceInfo) -> SourceInfo {
    SourceInfo::new(
        start.source.clone(),
        start.start_line,
        start.start_column,
        end.end_line,
        end.end_column,
    )
}

/// Skip tokens until the next `Mapping` keyword, section header, or
/// EOF — so a malformed declaration doesn't poison the rest of the
/// file.
fn skip_to_recovery_point(cursor: &mut Cursor) {
    while !cursor.check(TokenKind::Eof) && !cursor.check(TokenKind::SectionHeader) {
        if cursor.check(TokenKind::Identifier) && cursor.peek().text == "Mapping" {
            return;
        }
        cursor.advance();
    }
}
