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
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::sync::Arc;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::types::{Expression, Multiplicity, Parameter, TypeExpr};

// ---------------------------------------------------------------------------
// Variance
// ---------------------------------------------------------------------------

/// Variance of a class type-parameter slot. Mirrors Java's
/// `TypeParameter.contravariant` flag (the metamodel-level form) and
/// the surface syntax `<-T>` / `<+T>` (the class-level form). Default
/// `Invariant`.
///
/// Java declares `Property<U[contravariant], V>`,
/// `Column<U[contravariant], V>`, and
/// `NewPropertyRouteNodeFunctionDefinition<U[contravariant], V>` via
/// the metamodel-level `^TypeParameter{contravariant: true}` syntax —
/// `<-U>` prefix syntax is a separate path used only by `Path<-U,V|m>`
/// in path.pure. Both forms collapse onto the same compiled
/// `Variance` here so consumers (`bind_type_with_mode`,
/// `is_type_compatible`) can dispatch uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Variance {
    /// Default: invariant. `Container<Integer>` is neither sub- nor
    /// super-type of `Container<Number>`.
    #[default]
    Invariant,
    /// Covariant `<+T>`. `Producer<Integer>` is a subtype of
    /// `Producer<Number>` (output position).
    Covariant,
    /// Contravariant `<-T>`. `Consumer<Number>` is a subtype of
    /// `Consumer<Integer>` (input position). Pure's metamodel uses
    /// this for `Property`'s owner slot, `Column`'s row slot, and
    /// `NewPropertyRouteNodeFunctionDefinition`.
    Contravariant,
}

// ---------------------------------------------------------------------------
// Class
// ---------------------------------------------------------------------------

/// A compiled class definition.
///
/// Mirrors `ast::ClassDef` with all references resolved to `ElementId`s.
/// Stores only canonical, unidirectional data — no specializations,
/// no association-injected properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Class {
    /// Type parameters (e.g., `["T", "U"]`).
    pub type_parameters: Vec<SmolStr>,
    /// Variance of each type parameter, position-aligned with
    /// `type_parameters`. Default `Invariant` for empty / pre-existing
    /// serialized blobs that don't carry this field
    /// (`#[serde(default)]`). Populated from m3's
    /// `^TypeParameter{contravariant: true}` instance form (Property /
    /// Column / NewPropertyRouteNodeFunctionDefinition) and from the
    /// surface `<-T>` / `<+T>` prefix syntax (path.pure's
    /// `Path<-U,V|m>`).
    #[serde(default)]
    pub type_parameter_variances: Vec<Variance>,
    /// Multiplicity parameters declared on a parametric class —
    /// `Class Holder<T|m>` carries one parameter named `m`. Position-
    /// aligned with use-site `TypeExpr::Named.multiplicity_arguments`
    /// so `compute_type_arg_bindings` can map declared name → use-site
    /// value when substituting on property access.
    pub multiplicity_parameters: Vec<SmolStr>,
    /// Type-variable VALUE parameters declared on a parametric class —
    /// `Class C(x:Integer[1]) [...]` carries one parameter named `x`.
    /// Bound at construction time from `^C(10)(props)` and threaded
    /// through to constraints / qualified properties so `$x` resolves
    /// against the receiver instance.
    pub type_variable_parameters: Vec<crate::types::Parameter>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualifiedProperty {
    /// Property name.
    pub name: SmolStr,
    /// Source location.
    pub source_info: SourceInfo,
    /// Parameters. Stored as `Arc<[Parameter]>` so that QP dispatch
    /// shares the parameter list across invocations instead of
    /// deep-cloning per call (mirrors `Function.parameters`). `Arc`
    /// (not `Rc`) keeps `PureModel: Send + Sync` for the LSP.
    pub parameters: Arc<[Parameter]>,
    /// Return type.
    pub return_type: TypeExpr,
    /// Return multiplicity.
    pub return_multiplicity: Multiplicity,
    /// Body expressions. Stored as `Arc<[Expression]>` for the same
    /// reason as `Function.body` — eliminates the per-call deep clone
    /// at `eval_qualified_property` and `apply_qualified_property`.
    pub body: Arc<[Expression]>,
    /// Stereotypes on this qualified property.
    pub stereotypes: Vec<StereotypeRef>,
    /// Tagged values on this qualified property.
    pub tagged_values: Vec<TaggedValueRef>,
}

// ---------------------------------------------------------------------------
// Constraint
// ---------------------------------------------------------------------------

/// A compiled class constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
