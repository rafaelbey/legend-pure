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

//! AST → text round-trip for the Diagram DSL.
//!
//! [`compose_diagram`] emits a single [`DiagramDef`] in canonical
//! `###Diagram`-section form. Re-parsing the output through
//! [`DiagramSectionParser`](crate::parser::DiagramSectionParser) is
//! expected to yield an AST equal to the input (modulo `source_info`).

use std::fmt::Write as _;

use legend_pure_parser_ast::annotation::SpannedString;
use legend_pure_parser_ast::type_ref::{Identifier, TypeReference};

use crate::ast::{
    AssociationView, DiagramDef, DiagramView, GeneralizationView, Point, PropertyRef, PropertyView,
    TypeView,
};

/// Render a single `DiagramDef` to canonical Pure source text.
///
/// Output does *not* carry the `###Diagram` section header — emit
/// that yourself once before composing the first diagram of a file.
#[must_use]
pub fn compose_diagram(d: &DiagramDef) -> String {
    let mut out = String::new();
    write_diagram(&mut out, d);
    out
}

/// Render a section of zero or more diagrams, prefixed with the
/// `###Diagram` header.
#[must_use]
pub fn compose_diagram_section(diagrams: &[&DiagramDef]) -> String {
    let mut out = String::from("###Diagram\n");
    for (i, d) in diagrams.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        write_diagram(&mut out, d);
    }
    out
}

fn write_diagram(out: &mut String, d: &DiagramDef) {
    out.push_str("Diagram ");
    if let Some(pkg) = d.package.as_ref() {
        out.push_str(&pkg.to_string());
        out.push_str("::");
    }
    out.push_str(d.name.value.as_str());
    if let Some(g) = d.geometry.as_ref() {
        let _ = write!(
            out,
            "(width={}, height={})",
            fmt_f64(g.width),
            fmt_f64(g.height)
        );
    }
    out.push_str("\n{\n");
    for v in &d.views {
        write_view(out, v);
    }
    out.push_str("}\n");
}

fn write_view(out: &mut String, v: &DiagramView) {
    match v {
        DiagramView::Type(t) => write_type_view(out, t),
        DiagramView::Association(a) => write_association_view(out, a),
        DiagramView::Property(p) => write_property_view(out, p),
        DiagramView::Generalization(g) => write_generalization_view(out, g),
    }
}

fn write_type_view(out: &mut String, t: &TypeView) {
    let _ = write!(out, "    TypeView {}(", t.id);
    let mut props: Vec<(String, String)> = Vec::new();
    props.push(("type".into(), fmt_type_ref(&t.type_ref)));
    if let Some(v) = t.stereotypes_visible {
        props.push(("stereotypesVisible".into(), fmt_bool(v)));
    }
    if let Some(v) = t.attributes_visible {
        props.push(("attributesVisible".into(), fmt_bool(v)));
    }
    if let Some(v) = t.attribute_stereotypes_visible {
        props.push(("attributeStereotypesVisible".into(), fmt_bool(v)));
    }
    if let Some(v) = t.attribute_types_visible {
        props.push(("attributeTypesVisible".into(), fmt_bool(v)));
    }
    if let Some(c) = t.color.as_ref() {
        props.push(("color".into(), fmt_string(c)));
    }
    if let Some(w) = t.line_width {
        props.push(("lineWidth".into(), fmt_f64(w)));
    }
    if let Some(p) = t.position.as_ref() {
        props.push(("position".into(), fmt_point(p)));
    }
    if let Some(w) = t.width {
        props.push(("width".into(), fmt_f64(w)));
    }
    if let Some(h) = t.height {
        props.push(("height".into(), fmt_f64(h)));
    }
    write_props(out, &props);
    out.push_str(")\n");
}

fn write_association_view(out: &mut String, a: &AssociationView) {
    let _ = write!(out, "    AssociationView {}(", a.id);
    let mut props: Vec<(String, String)> = Vec::new();
    props.push(("association".into(), fmt_type_ref(&a.association)));
    if let Some(v) = a.stereotypes_visible {
        props.push(("stereotypesVisible".into(), fmt_bool(v)));
    }
    if let Some(v) = a.name_visible {
        props.push(("nameVisible".into(), fmt_bool(v)));
    }
    if let Some(c) = a.color.as_ref() {
        props.push(("color".into(), fmt_string(c)));
    }
    if let Some(w) = a.line_width {
        props.push(("lineWidth".into(), fmt_f64(w)));
    }
    if let Some(l) = a.label.as_ref() {
        props.push(("label".into(), fmt_string(l)));
    }
    if let Some(s) = a.line_style.as_ref() {
        props.push(("lineStyle".into(), s.to_string()));
    }
    if !a.points.is_empty() {
        props.push(("points".into(), fmt_points(&a.points)));
    }
    if let Some(s) = a.source.as_ref() {
        props.push(("source".into(), s.to_string()));
    }
    if let Some(t) = a.target.as_ref() {
        props.push(("target".into(), t.to_string()));
    }
    if let Some(p) = a.source_prop_position.as_ref() {
        props.push(("sourcePropertyPosition".into(), fmt_point(p)));
    }
    if let Some(p) = a.source_mult_position.as_ref() {
        props.push(("sourceMultiplicityPosition".into(), fmt_point(p)));
    }
    if let Some(p) = a.target_prop_position.as_ref() {
        props.push(("targetPropertyPosition".into(), fmt_point(p)));
    }
    if let Some(p) = a.target_mult_position.as_ref() {
        props.push(("targetMultiplicityPosition".into(), fmt_point(p)));
    }
    write_props(out, &props);
    out.push_str(")\n");
}

fn write_property_view(out: &mut String, p: &PropertyView) {
    let _ = write!(out, "    PropertyView {}(", p.id);
    let mut props: Vec<(String, String)> = Vec::new();
    props.push(("property".into(), fmt_property_ref(&p.property)));
    if let Some(v) = p.stereotypes_visible {
        props.push(("stereotypesVisible".into(), fmt_bool(v)));
    }
    if let Some(v) = p.name_visible {
        props.push(("nameVisible".into(), fmt_bool(v)));
    }
    if let Some(c) = p.color.as_ref() {
        props.push(("color".into(), fmt_string(c)));
    }
    if let Some(w) = p.line_width {
        props.push(("lineWidth".into(), fmt_f64(w)));
    }
    if let Some(l) = p.label.as_ref() {
        props.push(("label".into(), fmt_string(l)));
    }
    if let Some(s) = p.line_style.as_ref() {
        props.push(("lineStyle".into(), s.to_string()));
    }
    if !p.points.is_empty() {
        props.push(("points".into(), fmt_points(&p.points)));
    }
    if let Some(s) = p.source.as_ref() {
        props.push(("source".into(), s.to_string()));
    }
    if let Some(t) = p.target.as_ref() {
        props.push(("target".into(), t.to_string()));
    }
    if let Some(pp) = p.prop_position.as_ref() {
        props.push(("propertyPosition".into(), fmt_point(pp)));
    }
    if let Some(mp) = p.mult_position.as_ref() {
        props.push(("multiplicityPosition".into(), fmt_point(mp)));
    }
    write_props(out, &props);
    out.push_str(")\n");
}

fn write_generalization_view(out: &mut String, g: &GeneralizationView) {
    let _ = write!(out, "    GeneralizationView {}(", g.id);
    let mut props: Vec<(String, String)> = Vec::new();
    if let Some(c) = g.color.as_ref() {
        props.push(("color".into(), fmt_string(c)));
    }
    if let Some(w) = g.line_width {
        props.push(("lineWidth".into(), fmt_f64(w)));
    }
    if let Some(l) = g.label.as_ref() {
        props.push(("label".into(), fmt_string(l)));
    }
    if let Some(s) = g.line_style.as_ref() {
        props.push(("lineStyle".into(), s.to_string()));
    }
    if !g.points.is_empty() {
        props.push(("points".into(), fmt_points(&g.points)));
    }
    if let Some(s) = g.source.as_ref() {
        props.push(("source".into(), s.to_string()));
    }
    if let Some(t) = g.target.as_ref() {
        props.push(("target".into(), t.to_string()));
    }
    write_props(out, &props);
    out.push_str(")\n");
}

fn write_props(out: &mut String, props: &[(String, String)]) {
    for (i, (k, v)) in props.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{k}={v}");
    }
}

fn fmt_type_ref(t: &TypeReference) -> String {
    if let Some(pkg) = t.package.as_ref() {
        format!("{pkg}::{}", t.name)
    } else {
        t.name.to_string()
    }
}

fn fmt_property_ref(p: &PropertyRef) -> String {
    format!("{}.{}", fmt_type_ref(&p.class), p.property)
}

fn fmt_point(p: &Point) -> String {
    format!("({}, {})", fmt_f64(p.x), fmt_f64(p.y))
}

fn fmt_points(points: &[Point]) -> String {
    let inner: Vec<String> = points.iter().map(fmt_point).collect();
    format!("[{}]", inner.join(", "))
}

fn fmt_bool(b: bool) -> String {
    (if b { "true" } else { "false" }).to_string()
}

fn fmt_string(s: &SpannedString) -> String {
    format!("'{}'", s.value)
}

fn fmt_f64(x: f64) -> String {
    // Always keep at least one decimal so the parser sees a
    // FloatLiteral rather than an IntegerLiteral. Both are
    // accepted as numeric on the parse side, but composer output
    // staying in float form preserves the source intent.
    if x.fract() == 0.0 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

#[allow(dead_code)] // exposed via re-export only
fn _unused(_: &Identifier) {}
