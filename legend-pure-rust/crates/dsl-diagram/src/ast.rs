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

//! Diagram DSL AST.
//!
//! Top-level shape: a `###Diagram` section contains zero or more
//! [`DiagramDef`]s. Each definition has an optional geometry header
//! `(width=…, height=…)` and a body of `TypeView`, `AssociationView`,
//! `PropertyView`, and `GeneralizationView` declarations.
//!
//! Mirrors the upstream Java grammar at
//! `legend-pure-dsl-diagram/.../DiagramAntlrParser.g4`. Each view kind
//! has a fixed set of optional `key=value` properties; the parser fills
//! only the keys the user actually wrote, leaving the rest as `None` /
//! empty `Vec`.

use std::any::Any;

use legend_pure_parser_ast::annotation::{SpannedString, StereotypePtr, TaggedValue};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::{Annotated, PackageableElement};
use legend_pure_parser_ast::source_info::{SourceInfo, Spanned};
use legend_pure_parser_ast::type_ref::{Identifier, Package, TypeReference};

/// Section kind string this DSL claims (`###Diagram`).
pub const SECTION_KIND: &str = "Diagram";

// ---------------------------------------------------------------------------
// DiagramDef
// ---------------------------------------------------------------------------

/// A `Diagram` definition: `Diagram pkg::MyDiagram(width=…, height=…) { … }`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagramDef {
    /// Package qualifier on the diagram name.
    pub package: Option<Package>,
    /// The diagram's simple name plus its name token's span.
    pub name: SpannedString,
    /// Optional geometry header `(width=…, height=…)`.
    pub geometry: Option<DiagramGeometry>,
    /// Body items in source order. Mixing of view kinds is allowed.
    pub views: Vec<DiagramView>,
    /// Stereotypes on the diagram element.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values on the diagram element.
    pub tagged_values: Vec<TaggedValue>,
    /// Span of the entire `Diagram … { … }` declaration.
    pub source_info: SourceInfo,
}

impl Spanned for DiagramDef {
    fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }
}

impl Annotated for DiagramDef {
    fn stereotypes(&self) -> &[StereotypePtr] {
        &self.stereotypes
    }
    fn tagged_values(&self) -> &[TaggedValue] {
        &self.tagged_values
    }
}

impl PackageableElement for DiagramDef {
    fn package(&self) -> Option<&Package> {
        self.package.as_ref()
    }
    fn name(&self) -> &Identifier {
        &self.name.value
    }
}

impl DSLElement for DiagramDef {
    fn kind(&self) -> &str {
        SECTION_KIND
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn DSLElement> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn DSLElement) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

/// Optional `(width=…, height=…)` header on a Diagram. The parser
/// accepts width-first or height-first ordering; both lower here.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagramGeometry {
    /// Width in diagram coordinates.
    pub width: f64,
    /// Height in diagram coordinates.
    pub height: f64,
    /// Span of the parenthesised header.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// DiagramView
// ---------------------------------------------------------------------------

/// One body item inside a Diagram. Variants correspond to the four
/// view-kind keywords: `TypeView`, `AssociationView`, `PropertyView`,
/// `GeneralizationView`.
#[derive(Debug, Clone, PartialEq)]
pub enum DiagramView {
    /// `TypeView ID(type=…, …)` — a class drawn as a box.
    Type(TypeView),
    /// `AssociationView ID(association=…, source=…, target=…, …)` — an association edge.
    Association(AssociationView),
    /// `PropertyView ID(property=Class.prop, source=…, target=…, …)` — a property edge.
    Property(PropertyView),
    /// `GeneralizationView ID(source=…, target=…, …)` — an inheritance edge.
    Generalization(GeneralizationView),
}

// ---------------------------------------------------------------------------
// TypeView
// ---------------------------------------------------------------------------

/// `TypeView identifier(type=…, position=(x,y), width=…, height=…, …)`.
///
/// Every property after `type` is optional; a missing key stays `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeView {
    /// Local identifier (referenced by `source=` / `target=` in edges).
    pub id: Identifier,
    /// The class or type the view represents (`type=`).
    pub type_ref: TypeReference,
    /// Whether stereotypes show in the box (`stereotypesVisible=`).
    pub stereotypes_visible: Option<bool>,
    /// Whether attributes show in the box (`attributesVisible=`).
    pub attributes_visible: Option<bool>,
    /// Whether attribute stereotypes show (`attributeStereotypesVisible=`).
    pub attribute_stereotypes_visible: Option<bool>,
    /// Whether attribute types show (`attributeTypesVisible=`).
    pub attribute_types_visible: Option<bool>,
    /// Fill color, e.g. `#FFFFCC` (`color=`).
    pub color: Option<SpannedString>,
    /// Border line width (`lineWidth=`).
    pub line_width: Option<f64>,
    /// Top-left position (`position=(x, y)`).
    pub position: Option<Point>,
    /// Box width (`width=`).
    pub width: Option<f64>,
    /// Box height (`height=`).
    pub height: Option<f64>,
    /// Span of the entire `TypeView … (…)` declaration.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// AssociationView
// ---------------------------------------------------------------------------

/// `AssociationView identifier(association=…, source=…, target=…, …)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AssociationView {
    /// Local identifier.
    pub id: Identifier,
    /// The association this edge represents (`association=`).
    pub association: TypeReference,
    /// `stereotypesVisible=`.
    pub stereotypes_visible: Option<bool>,
    /// `nameVisible=` — whether the association name is drawn.
    pub name_visible: Option<bool>,
    /// `color=`.
    pub color: Option<SpannedString>,
    /// `lineWidth=`.
    pub line_width: Option<f64>,
    /// `label='…'`.
    pub label: Option<SpannedString>,
    /// `lineStyle=` — references a `LineStyle` enum value (`SIMPLE`, `RIGHT_ANGLE`).
    pub line_style: Option<Identifier>,
    /// `points=[(x,y), (x,y), …]`.
    pub points: Vec<Point>,
    /// `source=` — local `TypeView` id this edge starts from.
    pub source: Option<Identifier>,
    /// `target=` — local `TypeView` id this edge ends at.
    pub target: Option<Identifier>,
    /// `sourcePropertyPosition=(x, y)`.
    pub source_prop_position: Option<Point>,
    /// `sourceMultiplicityPosition=(x, y)`.
    pub source_mult_position: Option<Point>,
    /// `targetPropertyPosition=(x, y)`.
    pub target_prop_position: Option<Point>,
    /// `targetMultiplicityPosition=(x, y)`.
    pub target_mult_position: Option<Point>,
    /// Span of the entire declaration.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// PropertyView
// ---------------------------------------------------------------------------

/// `PropertyView identifier(property=Class.propName, source=…, target=…, …)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyView {
    /// Local identifier.
    pub id: Identifier,
    /// The class-property pair this edge represents (`property=Class.prop`).
    pub property: PropertyRef,
    /// `stereotypesVisible=`.
    pub stereotypes_visible: Option<bool>,
    /// `nameVisible=`.
    pub name_visible: Option<bool>,
    /// `color=`.
    pub color: Option<SpannedString>,
    /// `lineWidth=`.
    pub line_width: Option<f64>,
    /// `label='…'`.
    pub label: Option<SpannedString>,
    /// `lineStyle=`.
    pub line_style: Option<Identifier>,
    /// `points=[…]`.
    pub points: Vec<Point>,
    /// `source=`.
    pub source: Option<Identifier>,
    /// `target=`.
    pub target: Option<Identifier>,
    /// `propertyPosition=(x, y)`.
    pub prop_position: Option<Point>,
    /// `multiplicityPosition=(x, y)`.
    pub mult_position: Option<Point>,
    /// Span of the entire declaration.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// GeneralizationView
// ---------------------------------------------------------------------------

/// `GeneralizationView identifier(source=…, target=…, …)`.
///
/// Generalization edges have no `type=` / `association=` / `property=`
/// — the source and target `TypeViews` fully identify the inheritance.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralizationView {
    /// Local identifier.
    pub id: Identifier,
    /// `color=`.
    pub color: Option<SpannedString>,
    /// `lineWidth=`.
    pub line_width: Option<f64>,
    /// `label='…'`.
    pub label: Option<SpannedString>,
    /// `lineStyle=`.
    pub line_style: Option<Identifier>,
    /// `points=[…]`.
    pub points: Vec<Point>,
    /// `source=`.
    pub source: Option<Identifier>,
    /// `target=`.
    pub target: Option<Identifier>,
    /// Span of the entire declaration.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Shared sub-types
// ---------------------------------------------------------------------------

/// `Class.propertyName` — used as the `property=` value in `PropertyView`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyRef {
    /// The class side (`Class` in `Class.propertyName`).
    pub class: TypeReference,
    /// The property side (`propertyName`).
    pub property: Identifier,
    /// Span of the whole `Class.propertyName` reference.
    pub source_info: SourceInfo,
}

/// `(x, y)` floating-point coordinate pair used in positions and
/// `points=[…]` lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Span of the parenthesised pair.
    pub source_info: SourceInfo,
}
