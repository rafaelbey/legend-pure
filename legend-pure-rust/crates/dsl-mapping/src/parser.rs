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
//! `parserName` token. Currently supports `Pure` (model-to-model),
//! `EnumerationMapping` (Stage 4), `Operation` (Stage 5, simple
//! parameters form), `AggregationAware` (Stage 6 — recurses into
//! `Pure`/`Operation` bodies for the nested
//! `~mainMapping`/`~aggregateMapping` clauses), and `XStore`
//! (Stage 7 — per-association-property cross expressions binding
//! `$this`/`$that`). Other parser names produce an
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
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::ParserContext;
use legend_pure_parser_parser::cursor::Cursor;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::section_parser::SectionParser;
use smol_str::SmolStr;

use crate::ast::{
    AggregateSpecification, AggregateView, AggregationAwareClassMappingBody,
    AggregationFunctionSpec, ClassMapping, ClassMappingBody, EnumSourceValue, EnumValueMapping,
    EnumerationClassMappingBody, ForeignClassMappingBody, LocalPropertyDecl, MappingDef,
    MappingInclude, NestedClassMapping, OperationClassMappingBody, OperationParameter,
    PureClassMappingBody, PurePropertyMapping, SECTION_KIND, StoreSubstitution,
    XStoreClassMappingBody, XStorePropertyMapping,
};

fn err_unexpected(expected: &str, found: &str, source_info: SourceInfo) -> ParseError {
    ParseError::Unexpected {
        message: format!("Expected {expected}, found '{found}'"),
        source_info,
    }
}

/// Plug-in trait for foreign class-mapping body sub-grammars (Stage 0b).
///
/// Mirrors the [`SectionParser`] / `IslandParser` plug-in pattern at the
/// class-mapping body level. Built-in body sub-grammars (Pure,
/// Enumeration, Operation, AggregationAware, XStore) are dispatched
/// inside `dsl-mapping`; foreign DSLs (Relational, future stores)
/// register a parser here whose [`kind()`](Self::kind) matches the
/// `parserName` token in `Class : <parserName> { … }` and whose
/// [`parse()`](Self::parse) consumes the surrounding braces and body
/// content, returning a [`ForeignClassMappingBody`] trait object that
/// lands in [`ClassMappingBody::Foreign`](crate::ast::ClassMappingBody::Foreign).
///
/// Implementations must be `Send + Sync` so they can be shared across
/// parallel file parsers.
pub trait ClassMappingBodyParser: Send + Sync {
    /// The `parserName` token this parser handles (e.g. `"Relational"`).
    /// Compared verbatim against the user-source token; case-sensitive.
    fn kind(&self) -> &str;

    /// Parse the body block — the cursor is positioned **at** the
    /// opening `{`. Implementations consume through the matching `}`
    /// and return the parsed AST.
    fn parse(
        &self,
        ctx: &mut ParserContext<'_>,
    ) -> Result<Box<dyn ForeignClassMappingBody>, ParseError>;
}

/// Plug-in that owns the `###Mapping` section grammar.
///
/// Built-in body sub-grammars (Pure / Enumeration / Operation /
/// AggregationAware / XStore) are always available. Foreign body
/// sub-grammars (e.g. Relational) plug in via
/// [`with_body_parsers`](Self::with_body_parsers); see
/// `tests/foreign_body_parser.rs` for the registration shape.
///
/// Register with the core parser via
/// [`legend_pure_parser_parser::parse_with_sections`].
pub struct MappingSectionParser {
    foreign_body_parsers: Vec<Box<dyn ClassMappingBodyParser>>,
}

impl MappingSectionParser {
    /// Construct an empty registration — built-in body sub-grammars
    /// only, no foreign DSLs.
    #[must_use]
    pub fn new() -> Self {
        Self {
            foreign_body_parsers: Vec::new(),
        }
    }

    /// Construct with foreign body parsers registered. Pass one parser
    /// per `parserName` token your DSL contributes; the section parser
    /// dispatches on [`ClassMappingBodyParser::kind()`] before falling
    /// through to the built-in switch.
    #[must_use]
    pub fn with_body_parsers(parsers: Vec<Box<dyn ClassMappingBodyParser>>) -> Self {
        Self {
            foreign_body_parsers: parsers,
        }
    }
}

impl Default for MappingSectionParser {
    fn default() -> Self {
        Self::new()
    }
}

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
            match parse_mapping(ctx, &self.foreign_body_parsers) {
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

fn parse_mapping(
    ctx: &mut ParserContext<'_>,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<MappingDef, ParseError> {
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
            class_mappings.push(parse_class_mapping(ctx, foreign_parsers)?);
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

fn parse_class_mapping(
    ctx: &mut ParserContext<'_>,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<ClassMapping, ParseError> {
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

    let body =
        parse_class_mapping_body(ctx, parser_name.as_str(), &parser_name_si, foreign_parsers)?;

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

/// Dispatch a `parserName` token to the matching body sub-grammar.
///
/// Factored out so the recursive AggregationAware body parsing can
/// re-use it for `~mainMapping` / `~aggregateMapping` nested clauses
/// — those re-enter the same dispatch with a fresh `parserName`.
fn parse_class_mapping_body(
    ctx: &mut ParserContext<'_>,
    parser_name: &str,
    parser_name_si: &SourceInfo,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<ClassMappingBody, ParseError> {
    Ok(match parser_name {
        "Pure" => ClassMappingBody::Pure(Box::new(parse_pure_body(ctx)?)),
        "EnumerationMapping" => ClassMappingBody::Enumeration(parse_enumeration_body(ctx)?),
        "Operation" => ClassMappingBody::Operation(parse_operation_body(ctx)?),
        "AggregationAware" => ClassMappingBody::AggregationAware(Box::new(
            parse_aggregation_aware_body(ctx, foreign_parsers)?,
        )),
        "XStore" => ClassMappingBody::XStore(parse_xstore_body(ctx)?),
        other => {
            // Foreign body parser registered for this `parserName`?
            if let Some(p) = foreign_parsers.iter().find(|p| p.kind() == other) {
                ClassMappingBody::Foreign(p.parse(ctx)?)
            } else {
                return Err(ParseError::Unexpected {
                    message: format!(
                        "Mapping sub-parser '{other}' is not supported \
                         (built-ins: 'Pure', 'EnumerationMapping', 'Operation', \
                         'AggregationAware', 'XStore'; foreign DSLs register via \
                         `MappingSectionParser::with_body_parsers`)"
                    ),
                    source_info: parser_name_si.clone(),
                });
            }
        }
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
            // `(+)? propertyName (: Type[mult])? : (EnumerationMapping <name> :)? transform`
            //
            // The `+` prefix marks a *local* property declaration:
            // the user is adding a new property to the target class
            // visible only from this mapping. Mirrors the M3 grammar
            // alternative `(PLUS qualifiedName COLON type multiplicity)`.
            let local_start = ctx.cursor().current_source_info();
            let is_local = ctx.cursor().eat(TokenKind::Plus);
            let prop_tok = ctx.cursor().expect(TokenKind::Identifier)?;
            let prop_si = prop_tok.source_info.clone();
            let prop_name = SmolStr::new(prop_tok.text.clone());
            // Java M3 grammar (M3CoreParser.g4:84) allows an optional
            // `*` between the property header and the value colon —
            // `propName *: transform` marks the transform as
            // exploding into multiple property values. The flag is
            // stored on `PurePropertyMapping.explode` and round-trips
            // through the composer. It's mutually exclusive with the
            // local-property `+name : Type[mult]` form syntactically
            // (the local form consumes its own `: Type[mult] : `
            // chunk first), so the check sits before any local-form
            // parsing.
            let is_explode = !is_local && ctx.cursor().eat(TokenKind::Star);
            ctx.cursor().expect(TokenKind::Colon)?;

            // Local-property type+mult declaration consumes the
            // `Type[mult]` chunk before the next `:` separator.
            // `parse_multiplicity` expects to start *inside* the
            // brackets — the `[` and `]` are caller-managed.
            let local_property = if is_local {
                let type_ref = ctx.parse_type_reference()?;
                ctx.cursor().expect(TokenKind::LBracket)?;
                let multiplicity = ctx.parse_multiplicity()?;
                ctx.cursor().expect(TokenKind::RBracket)?;
                let local_end = ctx.cursor().current_source_info();
                ctx.cursor().expect(TokenKind::Colon)?;
                Some(LocalPropertyDecl {
                    type_ref,
                    multiplicity,
                    source_info: merge_si(&local_start, &local_end),
                })
            } else {
                None
            };

            // Optional `EnumerationMapping <name> :` transformer
            // prefix. Mirrors the M3 grammar's
            // `(ENUMERATION_MAPPING identifier COLON)?` between the
            // property colon and the combinedExpression. Two-token
            // lookahead resolves the ambiguity vs. a transform that
            // happens to start with the bare identifier
            // `EnumerationMapping`: the prefix is only consumed when
            // followed by another identifier *and* a colon.
            let transformer = if ctx.cursor().check(TokenKind::Identifier)
                && ctx.cursor().peek().text == "EnumerationMapping"
                && ctx.cursor().peek_kind_at(1) == TokenKind::Identifier
                && ctx.cursor().peek_kind_at(2) == TokenKind::Colon
            {
                ctx.cursor().advance(); // EnumerationMapping
                let name_tok = ctx.cursor().expect(TokenKind::Identifier)?;
                ctx.cursor().expect(TokenKind::Colon)?;
                Some(SmolStr::new(name_tok.text.clone()))
            } else {
                None
            };

            let transform = ctx.parse_expression()?;
            // Capture span up to the end of the transform — for the
            // entry-level source_info.
            let end_si = ctx.cursor().current_source_info();
            property_mappings.push(PurePropertyMapping {
                property_name: prop_name,
                transform,
                transformer,
                explode: is_explode,
                local_property,
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
// OperationClassMappingBody — Stage 5 (simple parameters form)
// ---------------------------------------------------------------------------

fn parse_operation_body(
    ctx: &mut ParserContext<'_>,
) -> Result<OperationClassMappingBody, ParseError> {
    ctx.cursor().expect(TokenKind::LBrace)?;

    let operation = parse_packageable_ptr(ctx)?;
    ctx.cursor().expect(TokenKind::LParen)?;

    let mut parameters = Vec::new();
    if !ctx.cursor().check(TokenKind::RParen) {
        // Reject the `mergeParameters` form (`[id, ...], { lambda }`)
        // up-front rather than mis-parsing it as an empty parameter
        // list. The simple form's first token is an identifier
        // (parameter ID); the merge form's first token is `[`.
        if ctx.cursor().check(TokenKind::LBracket) {
            let tok = ctx.cursor().peek().clone();
            return Err(ParseError::Unexpected {
                message:
                    "Operation `mergeParameters` form (`[id, …], { lambda }`) is not supported \
                     yet — Stage 5 ships only the simple `(id, …)` parameter form; merge \
                     arrives in a follow-up sub-stage."
                        .to_string(),
                source_info: tok.source_info,
            });
        }
        loop {
            let id_tok = ctx.cursor().expect(TokenKind::Identifier)?;
            parameters.push(OperationParameter {
                id: SmolStr::new(id_tok.text.clone()),
                source_info: id_tok.source_info,
            });
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    ctx.cursor().expect(TokenKind::RParen)?;
    // Optional `;` line terminator (matches Java `END_LINE?` in the
    // grammar). Some users write it, some don't.
    ctx.cursor().eat(TokenKind::Semicolon);
    ctx.cursor().expect(TokenKind::RBrace)?;

    Ok(OperationClassMappingBody {
        operation,
        parameters,
    })
}

// ---------------------------------------------------------------------------
// AggregationAwareClassMappingBody — Stage 6
// ---------------------------------------------------------------------------

fn parse_aggregation_aware_body(
    ctx: &mut ParserContext<'_>,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<AggregationAwareClassMappingBody, ParseError> {
    ctx.cursor().expect(TokenKind::LBrace)?;

    // `Views : [ aggSpec (, aggSpec)* ]`
    expect_keyword(ctx, "Views")?;
    ctx.cursor().expect(TokenKind::Colon)?;
    ctx.cursor().expect(TokenKind::LBracket)?;
    let mut views = Vec::new();
    if !ctx.cursor().check(TokenKind::RBracket) {
        loop {
            views.push(parse_aggregate_view(ctx, foreign_parsers)?);
            if !ctx.cursor().eat(TokenKind::Comma) {
                break;
            }
        }
    }
    ctx.cursor().expect(TokenKind::RBracket)?;
    // The `Views: [...]` clause and `~mainMapping` clause are
    // comma-separated in the Java grammar; some user samples omit
    // the comma. Accept either.
    ctx.cursor().eat(TokenKind::Comma);

    // `~mainMapping : <parserName> { ... }`
    let main_mapping = parse_nested_class_mapping(ctx, "mainMapping", foreign_parsers)?;
    ctx.cursor().expect(TokenKind::RBrace)?;

    Ok(AggregationAwareClassMappingBody {
        views,
        main_mapping,
    })
}

fn parse_aggregate_view(
    ctx: &mut ParserContext<'_>,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<AggregateView, ParseError> {
    let start = ctx.cursor().current_source_info();
    ctx.cursor().expect(TokenKind::LParen)?;
    let model_operation = parse_model_operation(ctx)?;
    ctx.cursor().expect(TokenKind::Comma)?;
    let aggregate_mapping = parse_nested_class_mapping(ctx, "aggregateMapping", foreign_parsers)?;
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(AggregateView {
        model_operation,
        aggregate_mapping,
        source_info: merge_si(&start, &close.source_info),
    })
}

fn parse_model_operation(
    ctx: &mut ParserContext<'_>,
) -> Result<AggregateSpecification, ParseError> {
    let start = ctx.cursor().current_source_info();
    expect_tilde_keyword(ctx, "modelOperation")?;
    ctx.cursor().expect(TokenKind::Colon)?;
    ctx.cursor().expect(TokenKind::LBrace)?;

    // The three slots (~canAggregate, ~groupByFunctions,
    // ~aggregateValues) are written in fixed source order in every
    // example we've seen, but the grammar tolerates any order; mirror
    // that by dispatching on which `~name` we hit next.
    let mut can_aggregate: Option<bool> = None;
    let mut group_by_functions: Option<Vec<Expression>> = None;
    let mut aggregate_values: Option<Vec<AggregationFunctionSpec>> = None;
    while !ctx.cursor().check(TokenKind::RBrace) && !ctx.cursor().check(TokenKind::Eof) {
        ctx.cursor().expect(TokenKind::Tilde)?;
        let name_tok = ctx.cursor().expect(TokenKind::Identifier)?;
        match name_tok.text.as_str() {
            "canAggregate" => {
                let bool_tok = ctx.cursor().peek().clone();
                let val = match bool_tok.kind {
                    TokenKind::True => true,
                    TokenKind::False => false,
                    _ => {
                        return Err(err_unexpected(
                            "true or false (after ~canAggregate)",
                            &bool_tok.text,
                            bool_tok.source_info,
                        ));
                    }
                };
                ctx.cursor().advance();
                if can_aggregate.replace(val).is_some() {
                    return Err(err_unexpected(
                        "single ~canAggregate clause",
                        "duplicate ~canAggregate",
                        name_tok.source_info,
                    ));
                }
            }
            "groupByFunctions" => {
                if group_by_functions.is_some() {
                    return Err(err_unexpected(
                        "single ~groupByFunctions clause",
                        "duplicate ~groupByFunctions",
                        name_tok.source_info,
                    ));
                }
                ctx.cursor().expect(TokenKind::LParen)?;
                let mut exprs = Vec::new();
                if !ctx.cursor().check(TokenKind::RParen) {
                    loop {
                        exprs.push(ctx.parse_expression()?);
                        if !ctx.cursor().eat(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                ctx.cursor().expect(TokenKind::RParen)?;
                group_by_functions = Some(exprs);
            }
            "aggregateValues" => {
                if aggregate_values.is_some() {
                    return Err(err_unexpected(
                        "single ~aggregateValues clause",
                        "duplicate ~aggregateValues",
                        name_tok.source_info,
                    ));
                }
                ctx.cursor().expect(TokenKind::LParen)?;
                let mut entries = Vec::new();
                if !ctx.cursor().check(TokenKind::RParen) {
                    loop {
                        entries.push(parse_aggregate_value(ctx)?);
                        if !ctx.cursor().eat(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                ctx.cursor().expect(TokenKind::RParen)?;
                aggregate_values = Some(entries);
            }
            other => {
                return Err(err_unexpected(
                    "canAggregate, groupByFunctions, or aggregateValues",
                    other,
                    name_tok.source_info,
                ));
            }
        }
        // Inter-slot commas (Java grammar requires them between
        // slots) — accept absent at the very end.
        ctx.cursor().eat(TokenKind::Comma);
    }
    let close = ctx.cursor().expect(TokenKind::RBrace)?;

    let can_aggregate = can_aggregate.ok_or_else(|| ParseError::Unexpected {
        message: "~modelOperation block missing required ~canAggregate clause".to_string(),
        source_info: start.clone(),
    })?;
    Ok(AggregateSpecification {
        can_aggregate,
        group_by_functions: group_by_functions.unwrap_or_default(),
        aggregate_values: aggregate_values.unwrap_or_default(),
        source_info: merge_si(&start, &close.source_info),
    })
}

fn parse_aggregate_value(
    ctx: &mut ParserContext<'_>,
) -> Result<AggregationFunctionSpec, ParseError> {
    let start = ctx.cursor().current_source_info();
    ctx.cursor().expect(TokenKind::LParen)?;
    expect_tilde_keyword(ctx, "mapFn")?;
    ctx.cursor().expect(TokenKind::Colon)?;
    let map_fn = ctx.parse_expression()?;
    ctx.cursor().expect(TokenKind::Comma)?;
    expect_tilde_keyword(ctx, "aggregateFn")?;
    ctx.cursor().expect(TokenKind::Colon)?;
    let aggregate_fn = ctx.parse_expression()?;
    let close = ctx.cursor().expect(TokenKind::RParen)?;
    Ok(AggregationFunctionSpec {
        map_fn,
        aggregate_fn,
        source_info: merge_si(&start, &close.source_info),
    })
}

/// Parse a `~<keyword> : <parserName> { … }` nested clause used by
/// `~mainMapping` and per-view `~aggregateMapping`. Re-enters the
/// dispatch (`parse_class_mapping_body`) so the nested body can be
/// any supported sub-grammar.
fn parse_nested_class_mapping(
    ctx: &mut ParserContext<'_>,
    expected_keyword: &str,
    foreign_parsers: &[Box<dyn ClassMappingBodyParser>],
) -> Result<NestedClassMapping, ParseError> {
    let start = ctx.cursor().current_source_info();
    expect_tilde_keyword(ctx, expected_keyword)?;
    ctx.cursor().expect(TokenKind::Colon)?;
    let pn_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let parser_name = SmolStr::new(pn_tok.text.clone());
    let pn_si = pn_tok.source_info.clone();
    let body = parse_class_mapping_body(ctx, parser_name.as_str(), &pn_si, foreign_parsers)?;
    let end_si = ctx.cursor().current_source_info();
    Ok(NestedClassMapping {
        parser_name,
        body,
        source_info: merge_si(&start, &end_si),
    })
}

fn expect_keyword(ctx: &mut ParserContext<'_>, expected: &str) -> Result<(), ParseError> {
    let tok = ctx.cursor().expect(TokenKind::Identifier)?;
    if tok.text != expected {
        return Err(err_unexpected(expected, &tok.text, tok.source_info));
    }
    Ok(())
}

fn expect_tilde_keyword(ctx: &mut ParserContext<'_>, expected: &str) -> Result<(), ParseError> {
    ctx.cursor().expect(TokenKind::Tilde)?;
    expect_keyword(ctx, expected)
}

// ---------------------------------------------------------------------------
// XStoreClassMappingBody — Stage 7
// ---------------------------------------------------------------------------

fn parse_xstore_body(ctx: &mut ParserContext<'_>) -> Result<XStoreClassMappingBody, ParseError> {
    ctx.cursor().expect(TokenKind::LBrace)?;
    let mut property_mappings = Vec::new();
    while !ctx.cursor().check(TokenKind::RBrace) && !ctx.cursor().check(TokenKind::Eof) {
        property_mappings.push(parse_xstore_property_mapping(ctx)?);
        // Comma-separated, trailing comma OK.
        ctx.cursor().eat(TokenKind::Comma);
    }
    ctx.cursor().expect(TokenKind::RBrace)?;
    Ok(XStoreClassMappingBody { property_mappings })
}

fn parse_xstore_property_mapping(
    ctx: &mut ParserContext<'_>,
) -> Result<XStorePropertyMapping, ParseError> {
    let prop_tok = ctx.cursor().expect(TokenKind::Identifier)?;
    let start_si = prop_tok.source_info.clone();
    let property_name = SmolStr::new(prop_tok.text.clone());

    let (source_set_impl_id, target_set_impl_id) = if ctx.cursor().eat(TokenKind::LBracket) {
        let src_tok = ctx.cursor().expect(TokenKind::Identifier)?;
        let src = Some(SmolStr::new(src_tok.text.clone()));
        let tgt = if ctx.cursor().eat(TokenKind::Comma) {
            let t = ctx.cursor().expect(TokenKind::Identifier)?;
            Some(SmolStr::new(t.text.clone()))
        } else {
            None
        };
        ctx.cursor().expect(TokenKind::RBracket)?;
        (src, tgt)
    } else {
        (None, None)
    };

    ctx.cursor().expect(TokenKind::Colon)?;
    let cross_expression = ctx.parse_expression()?;
    let end_si = ctx.cursor().current_source_info();

    Ok(XStorePropertyMapping {
        property_name,
        source_set_impl_id,
        target_set_impl_id,
        cross_expression,
        source_info: merge_si(&start_si, &end_si),
    })
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
