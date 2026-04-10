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

//! Compiled Class node with properties, constraints, and type parameters.
//!
//! **No bidirectional pointers.** The Class stores only **generalizations**
//! (`super_types`) — the canonical upward direction from the AST `extends`
//! clause. Specializations, association-injected properties, and milestoning
//! synthetic properties are computed as derived indexes on the frozen model.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::types::{Expression, Multiplicity, Parameter, TypeExpr};

// ---------------------------------------------------------------------------
// Class
// ---------------------------------------------------------------------------

/// A compiled class definition.
///
/// Mirrors `ast::ClassDef` with all references resolved to `ElementId`s.
/// Stores only canonical, unidirectional data — no specializations,
/// no association-injected properties.
#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    /// Type parameters (e.g., `["T", "U"]`).
    pub type_parameters: Vec<SmolStr>,
    /// Generalizations only (upward). Never stores specializations.
    /// Uses `TypeExpr` (not plain `ElementId`) because a class can
    /// extend a parameterized type: `extends List<String>`.
    pub super_types: Vec<TypeExpr>,
    /// Declared properties only.
    pub properties: Vec<Property>,
    /// Qualified (derived) properties.
    pub qualified_properties: Vec<QualifiedProperty>,
    /// Constraints.
    pub constraints: Vec<Constraint>,
    /// Stereotypes.
    pub stereotypes: Vec<StereotypeRef>,
    /// Tagged values.
    pub tagged_values: Vec<TaggedValueRef>,
}

// ---------------------------------------------------------------------------
// Property
// ---------------------------------------------------------------------------

/// Aggregation kind for properties.
///
/// Mirrors `ast::AggregationKind`. Most properties have no aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationKind {
    /// No aggregation — `(none)`.
    None,
    /// Shared aggregation — `(shared)`.
    Shared,
    /// Composite aggregation — `(composite)`.
    Composite,
}

/// A compiled class or association property.
///
/// Reused by both Class and Association.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// Property name.
    pub name: SmolStr,
    /// Source location.
    pub source_info: SourceInfo,
    /// Resolved property type.
    pub type_expr: TypeExpr,
    /// Property multiplicity.
    pub multiplicity: Multiplicity,
    /// Aggregation kind (if specified).
    pub aggregation: Option<AggregationKind>,
    /// Default value expression (if specified).
    pub default_value: Option<Expression>,
    /// Stereotypes on this property.
    pub stereotypes: Vec<StereotypeRef>,
    /// Tagged values on this property.
    pub tagged_values: Vec<TaggedValueRef>,
}

// ---------------------------------------------------------------------------
// QualifiedProperty
// ---------------------------------------------------------------------------

/// A compiled qualified (derived) property.
///
/// Reused by both Class and Association.
#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedProperty {
    /// Property name.
    pub name: SmolStr,
    /// Source location.
    pub source_info: SourceInfo,
    /// Parameters.
    pub parameters: Vec<Parameter>,
    /// Return type.
    pub return_type: TypeExpr,
    /// Return multiplicity.
    pub return_multiplicity: Multiplicity,
    /// Body expressions.
    pub body: Vec<Expression>,
    /// Stereotypes on this qualified property.
    pub stereotypes: Vec<StereotypeRef>,
    /// Tagged values on this qualified property.
    pub tagged_values: Vec<TaggedValueRef>,
}

// ---------------------------------------------------------------------------
// Constraint
// ---------------------------------------------------------------------------

/// A compiled class constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct Constraint {
    /// Constraint name (optional — unnamed constraints are allowed).
    pub name: Option<SmolStr>,
    /// Source location.
    pub source_info: SourceInfo,
    /// The constraint function/expression.
    pub function: Expression,
    /// Enforcement level, e.g., `Warn`, `Error`.
    pub enforcement_level: Option<SmolStr>,
    /// External identifier.
    pub external_id: Option<String>,
    /// Message function (evaluated when constraint fails).
    pub message: Option<Expression>,
}
