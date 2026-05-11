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

//! `ProtocolPureInstanceClassMapping` + `ProtocolPurePropertyMapping` —
//! the model-to-model body kind (`_type = "pureInstance"`).
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.store.modelToModel.mapping.{PureInstanceClassMapping, PurePropertyMapping}`.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::multiplicity::Multiplicity;
use legend_pure_parser_protocol::v1::source_info::SourceInformation;
use legend_pure_parser_protocol::v1::value_spec::LambdaFunction;

use crate::protocol::class_mapping::ProtocolClassMappingHeader;

/// Protocol shape for a `: Pure { … }` class mapping body.
///
/// Mirrors Java's `PureInstanceClassMapping` (which extends `ClassMapping`):
/// the abstract base's fields flatten alongside the body's
/// `srcClass`, `propertyMappings`, `filter`. We use
/// `#[serde(flatten)]` on the header struct so the JSON output puts
/// `id`, `class`, `root`, etc. at the top level next to body fields —
/// matching Java's `extends ClassMapping` layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolPureInstanceClassMapping {
    /// Inherited `ClassMapping` fields (id, class, root, …) — flattened
    /// into this struct's JSON. Matches Java's Jackson layout for
    /// `class extends ClassMapping`.
    #[serde(flatten)]
    pub header: ProtocolClassMappingHeader,
    /// `~src pkg::Class` — the source class FQN. `None` when the body
    /// omits the `~src` clause (rare; Java accepts it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_class: Option<String>,
    /// Span of the source-class reference (Java's
    /// `sourceClassSourceInformation`). `None` when `src_class` is
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_class_source_information: Option<SourceInformation>,
    /// Property → transform mappings, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_mappings: Vec<ProtocolPropertyMapping>,
    /// `~filter <expr>` wrapped as a no-parameter `LambdaFunction` —
    /// Java parity at
    /// `PureInstanceClassMappingParseTreeWalker.visitLambda:111-117`.
    /// `None` when the body omits the `~filter` clause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<LambdaFunction>,
}

/// Discriminated union of property-mapping variants by `_type`.
///
/// Maps to Java's `PropertyMapping` abstract base + `@JsonSubTypes` at
/// `PropertyMapping.java:25-31`. The full set is:
///
/// | Variant                            | `_type` discriminator             |
/// |------------------------------------|-----------------------------------|
/// | `PurePropertyMapping`              | `purePropertyMapping`             |
/// | `XStorePropertyMapping`            | `xStorePropertyMapping`           |
/// | `AggregationAwarePropertyMapping`  | `AggregationAwarePropertyMapping` |
/// | `RelationFunctionPropertyMapping`  | `relationFunctionPropertyMapping` |
///
/// Note the capital `A` on the AggregationAware discriminator — Java
/// quirk that must round-trip exactly.
///
/// **c2 status**: only `PurePropertyMapping` is implemented; other
/// variants land in c5/c6/c7.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ProtocolPropertyMapping {
    /// Pure-DSL property mapping (`_type = "purePropertyMapping"`).
    #[serde(rename = "purePropertyMapping")]
    PurePropertyMapping(ProtocolPurePropertyMapping),
}

/// Java `PurePropertyMapping` — the model-to-model property body.
///
/// Header fields (`property`, `source`, `target`, `localMappingProperty`,
/// `sourceInformation`) live on Java's abstract `PropertyMapping`
/// base; body fields (`enumMappingId`, `transform`, `explodeProperty`)
/// live on the concrete subclass. Both flatten into one JSON object
/// next to the `_type` discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolPurePropertyMapping {
    /// Target property pointer (`{class, property, sourceInformation}`).
    pub property: ProtocolPropertyPointer,
    /// Source class-mapping ID — typically the class FQN. `None` when
    /// the body doesn't carry one explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Target class-mapping ID — typically the property type FQN.
    /// `None` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `+name : Type[mult]` declaration. `None` for non-local
    /// references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mapping_property: Option<ProtocolLocalMappingPropertyInfo>,
    /// Enumeration-mapping transformer reference — the
    /// `EnumerationMapping <name> :` prefix on the value side. Java's
    /// `PurePropertyMapping.enumMappingId` is a plain string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_mapping_id: Option<String>,
    /// Transform expression wrapped as a no-parameter `LambdaFunction`
    /// (Java parity:
    /// `PureInstanceClassMappingParseTreeWalker.visitLambda:111-117`).
    pub transform: LambdaFunction,
    /// `explode` flag for the `name *: …` shape. Java types this as
    /// `Boolean` (nullable), so we serialize `None` as missing rather
    /// than `false` — preserves the Java-side default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode_property: Option<bool>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Java's `PropertyPointer` — a class FQN + property name pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolPropertyPointer {
    /// `::`-joined FQN of the property's owning class. Serialized as
    /// `"class"` (Java `@JsonProperty(value = "class")`).
    #[serde(rename = "class")]
    pub class: String,
    /// Property simple name on the owning class.
    pub property: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Java's `LocalMappingPropertyInfo` — declared type + multiplicity
/// for an inline `+name : Type[m]` property on a property mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolLocalMappingPropertyInfo {
    /// `::`-joined FQN of the declared type (e.g. `"String"` or
    /// `"my::pkg::Foo"`).
    #[serde(rename = "type")]
    pub type_path: String,
    /// Declared multiplicity (e.g. `[1]`, `[0..1]`, `[*]`).
    pub multiplicity: Multiplicity,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}
