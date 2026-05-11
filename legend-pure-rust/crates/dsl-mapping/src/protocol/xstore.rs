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

//! `ProtocolAssociationMapping` + `ProtocolXStoreAssociationMapping`
//! + `ProtocolXStorePropertyMapping`.
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.{AssociationMapping, xStore.XStoreAssociationMapping, xStore.XStorePropertyMapping}`.
//!
//! **Important layering**: Java's `AssociationMapping` lives in
//! `Mapping.associationMappings` (sibling to `classMappings`).
//! XStore-bodied class mappings route here, not into the
//! `ClassMapping` discriminator. Our Rust AST encodes XStore as one
//! of the `ClassMappingBody` variants; protocol conversion splits
//! it out.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::source_info::SourceInformation;
use legend_pure_parser_protocol::v1::value_spec::{
    LambdaFunction, ProtocolPackageableElementPtr,
};

use crate::protocol::pure::ProtocolPropertyPointer;

/// Discriminated union of `AssociationMapping` variants. Java's
/// abstract base has only one concrete subclass today:
/// `XStoreAssociationMapping` (`_type = "xStore"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ProtocolAssociationMapping {
    /// XStore association mapping (`_type = "xStore"`).
    #[serde(rename = "xStore")]
    XStore(ProtocolXStoreAssociationMapping),
}

/// Java `XStoreAssociationMapping extends AssociationMapping`.
///
/// Mirrors `XStoreAssociationMapping.java:23-26` + the abstract
/// `AssociationMapping.java:29-35`. The abstract base's `id`,
/// `association`, `stores`, `sourceInformation` flatten alongside
/// the concrete `propertyMappings: List<XStorePropertyMapping>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolXStoreAssociationMapping {
    /// Association-mapping ID — typically the association FQN.
    pub id: String,
    /// Target association FQN.
    pub association: ProtocolPackageableElementPtr,
    /// Stores spanned by this association mapping (FQN list). Today
    /// derived from the cross-expression participants; populated
    /// empty when the AST doesn't carry explicit store metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stores: Vec<String>,
    /// Per-property cross mappings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_mappings: Vec<ProtocolXStorePropertyMapping>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Java `XStorePropertyMapping extends PropertyMapping`.
///
/// `XStorePropertyMapping.java:21-23` adds a single body field
/// `crossExpression: LambdaFunction`. The header fields
/// (`property`, `source`, `target`, `localMappingProperty`,
/// `sourceInformation`) come from the abstract `PropertyMapping`
/// base — for XStore, `source` and `target` carry the source/target
/// set-implementation IDs respectively.
///
/// Note: this struct deliberately doesn't appear in the
/// `ProtocolPropertyMapping` enum (which is the discriminated union
/// used by Pure / AggregationAware / RelationFunction). XStore
/// property mappings only show up under XStoreAssociationMapping;
/// keeping them in a sibling struct avoids cluttering the cross-
/// body shared enum with a variant nothing in the class-mapping
/// world consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolXStorePropertyMapping {
    /// Target association property pointer.
    pub property: ProtocolPropertyPointer,
    /// Source set-implementation ID (from the `[src, tgt]` bracket
    /// pair on the AST). `None` when the bracket pair was omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Target set-implementation ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Cross-expression wrapped as a no-parameter `LambdaFunction`
    /// (Java parity: same wrapping idiom as `~filter` / `~transform`).
    pub cross_expression: LambdaFunction,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}
