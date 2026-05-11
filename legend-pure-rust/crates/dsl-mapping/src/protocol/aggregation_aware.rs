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

//! `ProtocolAggregationAwareClassMapping` + nested aggregate
//! specifications.
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.aggregationAware.*`.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::value_spec::LambdaFunction;

use crate::protocol::class_mapping::{ProtocolClassMapping, ProtocolClassMappingHeader};
use crate::protocol::pure::ProtocolPropertyMapping;

/// Protocol shape for an `: AggregationAware { … }` class mapping body.
///
/// Mirrors Java's `AggregationAwareClassMapping`
/// (`AggregationAwareClassMapping.java:23-33`):
///
/// - `mainSetImplementation: ClassMapping` — the fall-through main
///   mapping used when no aggregate view matches.
/// - `propertyMappings: List<PropertyMapping>` — `AggregationAwarePropertyMapping`
///   instances summarising the main mapping's properties. Java's
///   post-processor synthesises these from `mainSetImplementation`;
///   our protocol layer emits an empty list since the AST doesn't
///   carry the synthesised list (consumers walk
///   `mainSetImplementation` directly for property data).
/// - `aggregateSetImplementations: List<AggregateSetImplementationContainer>`
///   — per-view aggregate spec + nested set implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAggregationAwareClassMapping {
    /// Inherited `ClassMapping` fields, flattened.
    #[serde(flatten)]
    pub header: ProtocolClassMappingHeader,
    /// The fall-through main set implementation. Boxed because
    /// `ProtocolClassMapping` is a sized discriminated union that
    /// recursively contains this struct.
    pub main_set_implementation: Box<ProtocolClassMapping>,
    /// Property mappings synthesised by Java's post-processor —
    /// empty in our protocol output (see struct doc).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_mappings: Vec<ProtocolPropertyMapping>,
    /// Per-view aggregate containers. The Java grammar requires at
    /// least one entry; we don't enforce that at the protocol layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregate_set_implementations: Vec<ProtocolAggregateSetImplementationContainer>,
}

/// One `(index, setImplementation, aggregateSpecification)` per-view
/// container.
///
/// Maps to Java
/// `AggregateSetImplementationContainer.java:19-24`. The `index` is
/// the view's position in the parent list (used by the plan
/// generator to disambiguate views).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAggregateSetImplementationContainer {
    /// Java's `Long index`. Our 0-based source order.
    pub index: i64,
    /// Nested aggregate set implementation. Boxed for the same
    /// recursion reason as `main_set_implementation`.
    pub set_implementation: Box<ProtocolClassMapping>,
    /// `~modelOperation` block: canAggregate flag + groupBy + aggregate
    /// values.
    pub aggregate_specification: ProtocolAggregateSpecification,
}

/// `~modelOperation` aggregate specification.
///
/// Maps to Java `AggregateSpecification.java:19-24`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAggregateSpecification {
    /// `~canAggregate true|false` — declares whether this view
    /// covers exact aggregation or a pre-aggregated approximation.
    pub can_aggregate: bool,
    /// `~groupByFunctions ( expr, expr, … )` — one entry per
    /// grouping key, each wrapped as a `LambdaFunction` with `$this`
    /// bound to the outer class type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by_functions: Vec<ProtocolGroupByFunction>,
    /// `~aggregateValues ( ( ~mapFn:…, ~aggregateFn:… ), … )` — one
    /// pair per aggregated property.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregate_values: Vec<ProtocolAggregateFunction>,
}

/// One `~groupByFunctions` entry, wrapping a single lambda.
///
/// Maps to Java `GroupByFunction.java:19-22`. Java wraps even a
/// single-expression group-by key as a `LambdaFunction` — same
/// idiom as `~filter` / `~transform`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolGroupByFunction {
    /// Group-by expression wrapped as a no-parameter
    /// `LambdaFunction` (Java parity).
    pub group_by_fn: LambdaFunction,
}

/// One `~aggregateValues` `(~mapFn:…, ~aggregateFn:…)` pair.
///
/// Maps to Java `AggregateFunction.java:19-23`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAggregateFunction {
    /// `~mapFn` — projects a row to the value being aggregated.
    pub map_fn: LambdaFunction,
    /// `~aggregateFn` — folds many `$mapped` values into one.
    pub aggregate_fn: LambdaFunction,
}
