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

//! `SectionParser` for `###Diagram` section bodies.
//!
//! Recognises a stream of `Diagram pkg::Name(width=W, height=H) { … }`
//! declarations whose body holds zero or more `TypeView`,
//! `AssociationView`, `PropertyView`, or `GeneralizationView`
//! declarations.
//!
//! Mirrors `legend-pure-dsl-diagram/.../DiagramAntlrParser.g4` for the
//! common shapes; less-common features (e.g. bare `#FFFFCC` color
//! literals) require lexer support that core doesn't yet expose, so
//! colours come through as quoted string literals
//! (`color = '#FFFFCC'`) for now. That divergence is documented and
//! tracked as future work.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::SpannedString;
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::type_ref::{Identifier, Package, TypeReference};
use legend_pure_parser_lexer::TokenKind;
use legend_pure_parser_parser::cursor::Cursor;
use legend_pure_parser_parser::error::ParseError;
use legend_pure_parser_parser::section_parser::SectionParser;
use smol_str::SmolStr;

use crate::ast::{
    AssociationView, DiagramDef, DiagramGeometry, DiagramView, GeneralizationView, Point,
    PropertyRef, PropertyView, SECTION_KIND, TypeView,
};

/// Builds the standard "Expected X, found Y" parse error. Uses the
/// public `Unexpected` variant since `ParseError`'s constructor
/// helpers are crate-private.
fn err_unexpected(expected: &str, found: &str, source_info: SourceInfo) -> ParseError {
    ParseError::Unexpected {
        message: format!("Expected {expected}, found '{found}'"),
        source_info,
    }
}

/// Plug-in that owns the `###Diagram` section grammar.
///
/// Register with the core parser via
/// [`legend_pure_parser_parser::parse_with_sections`].
pub struct DiagramSectionParser;

impl SectionParser for DiagramSectionParser {
    fn kind(&self) -> &str {
        SECTION_KIND
    }

    fn parse_body(
        &self,
        cursor: &mut Cursor,
        errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>> {
        let mut out: Vec<Box<dyn DSLElement>> = Vec::new();
        while !cursor.check(TokenKind::SectionHeader) && !cursor.check(TokenKind::Eof) {
            // Each iteration must make progress on either Ok or Err to
            // avoid infinite-loop on malformed input — snapshot the
            // current span and force an advance if neither happened.
            let pos_before = cursor.peek().source_info.clone();
            match parse_diagram(cursor) {
                Ok(d) => out.push(Box::new(d)),
                Err(e) => {
                    errors.push(e);
                    skip_to_recovery_point(cursor);
                }
            }
            if cursor.peek().source_info == pos_before {
                // Defensive — never seen in practice, but guarantees
                // termination if a recovery path forgets to advance.
                cursor.advance();
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Diagram
// ---------------------------------------------------------------------------

fn parse_diagram(cursor: &mut Cursor) -> Result<DiagramDef, ParseError> {
    let start = cursor.current_source_info();
    let kw = cursor.expect(TokenKind::Identifier)?;
    if kw.text != "Diagram" {
        return Err(err_unexpected("Diagram", &kw.text, kw.source_info.clone()));
    }

    let (package, name, name_si) = parse_qualified_name(cursor)?;

    // Optional geometry header `(width=N, height=N)` or `(height=N, width=N)`.
    let geometry = if cursor.check(TokenKind::LParen) {
        Some(parse_geometry(cursor)?)
    } else {
        None
    };

    cursor.expect(TokenKind::LBrace)?;
    let mut views = Vec::new();
    while !cursor.check(TokenKind::RBrace) && !cursor.check(TokenKind::Eof) {
        views.push(parse_view(cursor)?);
    }
    let close = cursor.expect(TokenKind::RBrace)?;

    let source_info = merge_si(&start, &close.source_info);

    Ok(DiagramDef {
        package,
        name: SpannedString {
            value: name,
            source_info: name_si,
        },
        geometry,
        views,
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
        source_info,
    })
}

fn parse_geometry(cursor: &mut Cursor) -> Result<DiagramGeometry, ParseError> {
    let open = cursor.expect(TokenKind::LParen)?;
    let mut width: Option<f64> = None;
    let mut height: Option<f64> = None;
    loop {
        let key_tok = cursor.expect(TokenKind::Identifier)?;
        cursor.expect(TokenKind::Equals)?;
        let value = parse_float(cursor)?;
        match key_tok.text.as_str() {
            "width" => width = Some(value),
            "height" => height = Some(value),
            other => {
                return Err(err_unexpected(
                    "width or height",
                    other,
                    key_tok.source_info,
                ));
            }
        }
        if !cursor.eat(TokenKind::Comma) {
            break;
        }
    }
    let close = cursor.expect(TokenKind::RParen)?;
    Ok(DiagramGeometry {
        width: width.unwrap_or(0.0),
        height: height.unwrap_or(0.0),
        source_info: merge_si(&open.source_info, &close.source_info),
    })
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

fn parse_view(cursor: &mut Cursor) -> Result<DiagramView, ParseError> {
    let kw = cursor.expect(TokenKind::Identifier)?;
    let kw_text = kw.text.clone();
    let kw_si = kw.source_info.clone();
    let id = cursor.expect(TokenKind::Identifier)?;
    let id_text = id.text.clone();
    cursor.expect(TokenKind::LParen)?;

    let view = match kw_text.as_str() {
        "TypeView" => DiagramView::Type(parse_type_view(cursor, &id_text, &kw_si)?),
        "AssociationView" => {
            DiagramView::Association(parse_association_view(cursor, &id_text, &kw_si)?)
        }
        "PropertyView" => DiagramView::Property(parse_property_view(cursor, &id_text, &kw_si)?),
        "GeneralizationView" => {
            DiagramView::Generalization(parse_generalization_view(cursor, &id_text, &kw_si)?)
        }
        other => {
            return Err(err_unexpected(
                "TypeView | AssociationView | PropertyView | GeneralizationView",
                other,
                kw_si,
            ));
        }
    };

    cursor.expect(TokenKind::RParen)?;
    Ok(view)
}

fn parse_type_view(
    cursor: &mut Cursor,
    id: &str,
    start: &SourceInfo,
) -> Result<TypeView, ParseError> {
    let mut tv = TypeView {
        id: SmolStr::new(id),
        type_ref: stub_type_ref(start),
        stereotypes_visible: None,
        attributes_visible: None,
        attribute_stereotypes_visible: None,
        attribute_types_visible: None,
        color: None,
        line_width: None,
        position: None,
        width: None,
        height: None,
        source_info: start.clone(),
    };
    let mut type_seen = false;
    parse_property_pairs(cursor, |key, key_si, cur| {
        match key {
            "type" => {
                tv.type_ref = parse_type_reference(cur)?;
                type_seen = true;
            }
            "stereotypesVisible" => tv.stereotypes_visible = Some(parse_bool(cur)?),
            "attributesVisible" => tv.attributes_visible = Some(parse_bool(cur)?),
            "attributeStereotypesVisible" => {
                tv.attribute_stereotypes_visible = Some(parse_bool(cur)?);
            }
            "attributeTypesVisible" => tv.attribute_types_visible = Some(parse_bool(cur)?),
            "color" => tv.color = Some(parse_string(cur)?),
            "lineWidth" => tv.line_width = Some(parse_float(cur)?),
            "position" => tv.position = Some(parse_point(cur)?),
            "width" => tv.width = Some(parse_float(cur)?),
            "height" => tv.height = Some(parse_float(cur)?),
            other => {
                return Err(err_unexpected(
                    "TypeView property name",
                    other,
                    key_si.clone(),
                ));
            }
        }
        Ok(())
    })?;
    if !type_seen {
        return Err(err_unexpected(
            "TypeView property 'type='",
            "missing",
            start.clone(),
        ));
    }
    Ok(tv)
}

fn parse_association_view(
    cursor: &mut Cursor,
    id: &str,
    start: &SourceInfo,
) -> Result<AssociationView, ParseError> {
    let mut av = AssociationView {
        id: SmolStr::new(id),
        association: stub_type_ref(start),
        stereotypes_visible: None,
        name_visible: None,
        color: None,
        line_width: None,
        label: None,
        line_style: None,
        points: Vec::new(),
        source: None,
        target: None,
        source_prop_position: None,
        source_mult_position: None,
        target_prop_position: None,
        target_mult_position: None,
        source_info: start.clone(),
    };
    let mut assoc_seen = false;
    parse_property_pairs(cursor, |key, key_si, cur| {
        match key {
            "association" => {
                av.association = parse_type_reference(cur)?;
                assoc_seen = true;
            }
            "stereotypesVisible" => av.stereotypes_visible = Some(parse_bool(cur)?),
            "nameVisible" => av.name_visible = Some(parse_bool(cur)?),
            "color" => av.color = Some(parse_string(cur)?),
            "lineWidth" => av.line_width = Some(parse_float(cur)?),
            "label" => av.label = Some(parse_string(cur)?),
            "lineStyle" => av.line_style = Some(parse_identifier(cur)?),
            "points" => av.points = parse_points_array(cur)?,
            "source" => av.source = Some(parse_identifier(cur)?),
            "target" => av.target = Some(parse_identifier(cur)?),
            "sourcePropertyPosition" => av.source_prop_position = Some(parse_point(cur)?),
            "sourceMultiplicityPosition" => av.source_mult_position = Some(parse_point(cur)?),
            "targetPropertyPosition" => av.target_prop_position = Some(parse_point(cur)?),
            "targetMultiplicityPosition" => av.target_mult_position = Some(parse_point(cur)?),
            other => {
                return Err(err_unexpected(
                    "AssociationView property name",
                    other,
                    key_si.clone(),
                ));
            }
        }
        Ok(())
    })?;
    if !assoc_seen {
        return Err(err_unexpected(
            "AssociationView property 'association='",
            "missing",
            start.clone(),
        ));
    }
    Ok(av)
}

fn parse_property_view(
    cursor: &mut Cursor,
    id: &str,
    start: &SourceInfo,
) -> Result<PropertyView, ParseError> {
    let mut pv = PropertyView {
        id: SmolStr::new(id),
        property: PropertyRef {
            class: stub_type_ref(start),
            property: SmolStr::new(""),
            source_info: start.clone(),
        },
        stereotypes_visible: None,
        name_visible: None,
        color: None,
        line_width: None,
        label: None,
        line_style: None,
        points: Vec::new(),
        source: None,
        target: None,
        prop_position: None,
        mult_position: None,
        source_info: start.clone(),
    };
    let mut property_seen = false;
    parse_property_pairs(cursor, |key, key_si, cur| {
        match key {
            "property" => {
                pv.property = parse_property_ref(cur)?;
                property_seen = true;
            }
            "stereotypesVisible" => pv.stereotypes_visible = Some(parse_bool(cur)?),
            "nameVisible" => pv.name_visible = Some(parse_bool(cur)?),
            "color" => pv.color = Some(parse_string(cur)?),
            "lineWidth" => pv.line_width = Some(parse_float(cur)?),
            "label" => pv.label = Some(parse_string(cur)?),
            "lineStyle" => pv.line_style = Some(parse_identifier(cur)?),
            "points" => pv.points = parse_points_array(cur)?,
            "source" => pv.source = Some(parse_identifier(cur)?),
            "target" => pv.target = Some(parse_identifier(cur)?),
            "propertyPosition" => pv.prop_position = Some(parse_point(cur)?),
            "multiplicityPosition" => pv.mult_position = Some(parse_point(cur)?),
            other => {
                return Err(err_unexpected(
                    "PropertyView property name",
                    other,
                    key_si.clone(),
                ));
            }
        }
        Ok(())
    })?;
    if !property_seen {
        return Err(err_unexpected(
            "PropertyView property 'property='",
            "missing",
            start.clone(),
        ));
    }
    Ok(pv)
}

fn parse_generalization_view(
    cursor: &mut Cursor,
    id: &str,
    start: &SourceInfo,
) -> Result<GeneralizationView, ParseError> {
    let mut gv = GeneralizationView {
        id: SmolStr::new(id),
        color: None,
        line_width: None,
        label: None,
        line_style: None,
        points: Vec::new(),
        source: None,
        target: None,
        source_info: start.clone(),
    };
    parse_property_pairs(cursor, |key, key_si, cur| {
        match key {
            "color" => gv.color = Some(parse_string(cur)?),
            "lineWidth" => gv.line_width = Some(parse_float(cur)?),
            "label" => gv.label = Some(parse_string(cur)?),
            "lineStyle" => gv.line_style = Some(parse_identifier(cur)?),
            "points" => gv.points = parse_points_array(cur)?,
            "source" => gv.source = Some(parse_identifier(cur)?),
            "target" => gv.target = Some(parse_identifier(cur)?),
            other => {
                return Err(err_unexpected(
                    "GeneralizationView property name",
                    other,
                    key_si.clone(),
                ));
            }
        }
        Ok(())
    })?;
    Ok(gv)
}

// ---------------------------------------------------------------------------
// Property-pair / value parsers
// ---------------------------------------------------------------------------

/// Parse `key = value (',' key = value)*` until the next `)`. The
/// `handler` callback is invoked once per pair; the closing paren is
/// not consumed (the caller handles it).
fn parse_property_pairs<F>(cursor: &mut Cursor, mut handler: F) -> Result<(), ParseError>
where
    F: FnMut(&str, &SourceInfo, &mut Cursor) -> Result<(), ParseError>,
{
    if cursor.check(TokenKind::RParen) {
        return Ok(());
    }
    loop {
        let key_tok = cursor.expect(TokenKind::Identifier)?;
        let key_text = key_tok.text.clone();
        let key_si = key_tok.source_info.clone();
        cursor.expect(TokenKind::Equals)?;
        handler(&key_text, &key_si, cursor)?;
        if !cursor.eat(TokenKind::Comma) {
            break;
        }
    }
    Ok(())
}

fn parse_qualified_name(
    cursor: &mut Cursor,
) -> Result<(Option<Package>, Identifier, SourceInfo), ParseError> {
    let first = cursor.expect(TokenKind::Identifier)?;
    let mut current_pkg: Option<Package> = None;
    let mut current_name = SmolStr::new(first.text.clone());
    let mut current_si = first.source_info.clone();
    while cursor.eat(TokenKind::PathSep) {
        let next = cursor.expect(TokenKind::Identifier)?;
        // Promote the previous component to a package segment.
        current_pkg = Some(match current_pkg.take() {
            None => Package::root(current_name.clone(), current_si.clone()),
            Some(parent) => Package::new(
                current_name.clone(),
                Some(Box::new(parent)),
                current_si.clone(),
            ),
        });
        current_name = SmolStr::new(next.text.clone());
        current_si = next.source_info.clone();
    }
    Ok((current_pkg, current_name, current_si))
}

fn parse_type_reference(cursor: &mut Cursor) -> Result<TypeReference, ParseError> {
    let (package, name, si) = parse_qualified_name(cursor)?;
    Ok(TypeReference {
        package,
        name,
        type_arguments: Vec::new(),
        multiplicity_arguments: Vec::new(),
        type_variable_values: Vec::new(),
        source_info: si,
    })
}

fn parse_property_ref(cursor: &mut Cursor) -> Result<PropertyRef, ParseError> {
    // `Class` or `pkg::Class` followed by `.propName`.
    let class = parse_type_reference(cursor)?;
    cursor.expect(TokenKind::Dot)?;
    let prop_tok = cursor.expect(TokenKind::Identifier)?;
    let si = merge_si(&class.source_info, &prop_tok.source_info);
    Ok(PropertyRef {
        class,
        property: SmolStr::new(prop_tok.text.clone()),
        source_info: si,
    })
}

fn parse_point(cursor: &mut Cursor) -> Result<Point, ParseError> {
    let open = cursor.expect(TokenKind::LParen)?;
    let x = parse_float(cursor)?;
    cursor.expect(TokenKind::Comma)?;
    let y = parse_float(cursor)?;
    let close = cursor.expect(TokenKind::RParen)?;
    Ok(Point {
        x,
        y,
        source_info: merge_si(&open.source_info, &close.source_info),
    })
}

fn parse_points_array(cursor: &mut Cursor) -> Result<Vec<Point>, ParseError> {
    cursor.expect(TokenKind::LBracket)?;
    let mut out = Vec::new();
    if !cursor.check(TokenKind::RBracket) {
        loop {
            out.push(parse_point(cursor)?);
            if !cursor.eat(TokenKind::Comma) {
                break;
            }
        }
    }
    cursor.expect(TokenKind::RBracket)?;
    Ok(out)
}

fn parse_float(cursor: &mut Cursor) -> Result<f64, ParseError> {
    // Accept floats, integers (treated as floats), and signed numbers
    // via a leading `-`.
    let mut sign = 1.0;
    if cursor.eat(TokenKind::Minus) {
        sign = -1.0;
    }
    let tok = cursor.peek().clone();
    match tok.kind {
        TokenKind::FloatLiteral | TokenKind::IntegerLiteral => {
            cursor.advance();
            let parsed: f64 = tok
                .text
                .parse()
                .map_err(|_| err_unexpected("number", &tok.text, tok.source_info.clone()))?;
            Ok(sign * parsed)
        }
        _ => Err(err_unexpected("number", &tok.text, tok.source_info)),
    }
}

fn parse_bool(cursor: &mut Cursor) -> Result<bool, ParseError> {
    let tok = cursor.peek().clone();
    match tok.kind {
        TokenKind::True => {
            cursor.advance();
            Ok(true)
        }
        TokenKind::False => {
            cursor.advance();
            Ok(false)
        }
        _ => Err(err_unexpected("true or false", &tok.text, tok.source_info)),
    }
}

fn parse_string(cursor: &mut Cursor) -> Result<SpannedString, ParseError> {
    let tok = cursor.expect(TokenKind::StringLiteral)?;
    // The lexer keeps the surrounding single quotes; strip them.
    let raw = tok.text.as_str();
    let stripped = raw.strip_prefix('\'').unwrap_or(raw);
    let stripped = stripped.strip_suffix('\'').unwrap_or(stripped);
    Ok(SpannedString {
        value: SmolStr::new(stripped),
        source_info: tok.source_info,
    })
}

fn parse_identifier(cursor: &mut Cursor) -> Result<Identifier, ParseError> {
    let tok = cursor.expect(TokenKind::Identifier)?;
    Ok(SmolStr::new(tok.text.clone()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stub_type_ref(span: &SourceInfo) -> TypeReference {
    TypeReference {
        package: None,
        name: SmolStr::new(""),
        type_arguments: Vec::new(),
        multiplicity_arguments: Vec::new(),
        type_variable_values: Vec::new(),
        source_info: span.clone(),
    }
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

/// Skip tokens until we see what looks like the next Diagram boundary
/// or section boundary, so a malformed declaration doesn't poison the
/// rest of the file.
fn skip_to_recovery_point(cursor: &mut Cursor) {
    while !cursor.check(TokenKind::Eof) && !cursor.check(TokenKind::SectionHeader) {
        // `Diagram` identifier at top level is a recovery point.
        if cursor.check(TokenKind::Identifier) && cursor.peek().text == "Diagram" {
            return;
        }
        cursor.advance();
    }
}
