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

//! `ProtocolRelationFunctionClassMapping` +
//! `ProtocolRelationFunctionPropertyMapping` + `ProtocolBindingTransformer`.
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.relationFunction.{RelationFunctionClassMapping, RelationFunctionPropertyMapping}`
//! and `…externalFormat.BindingTransformer`.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::source_info::SourceInformation;
use legend_pure_parser_protocol::v1::value_spec::ProtocolPackageableElementPtr;
// `SourceInformation` is used by sibling structs that still need
// optional source info. `ProtocolBindingTransformer` omits it
// (Java `@JsonIgnore`) — see struct doc.

use crate::protocol::class_mapping::ProtocolClassMappingHeader;
use crate::protocol::pure::{ProtocolLocalMappingPropertyInfo, ProtocolPropertyPointer};

/// Protocol shape for a `: Relation { ~func … }` class mapping body.
///
/// Mirrors Java's `RelationFunctionClassMapping`
/// (`RelationFunctionClassMapping.java:24-33`):
/// flattens the header alongside `relationFunction:
/// PackageableElementPointer` and `propertyMappings: List<PropertyMapping>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolRelationFunctionClassMapping {
    /// Inherited `ClassMapping` fields, flattened.
    #[serde(flatten)]
    pub header: ProtocolClassMappingHeader,
    /// Function FQN wrapped as a `PackageableElementPointer`.
    /// Java captures the full
    /// `qualifiedName(P1,…):Return[m]` signature via `getText()`;
    /// we pass through the qualified-name portion that the AST
    /// parsed. The signature suffix doesn't survive JSON round-
    /// trip (it's preserved on the AST for composer fidelity).
    pub relation_function: ProtocolPackageableElementPtr,
    /// Per-property column mappings, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_mappings: Vec<crate::protocol::pure::ProtocolPropertyMapping>,
}

/// Java `RelationFunctionPropertyMapping` — extends
/// `PropertyMapping`, adds `column` + optional `bindingTransformer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolRelationFunctionPropertyMapping {
    /// Target property pointer (inherited from `PropertyMapping`).
    pub property: ProtocolPropertyPointer,
    /// Source class-mapping ID (inherited, typically `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Target class-mapping ID (inherited, typically `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `+name : Type[mult]` declaration. `None` for non-local
    /// references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mapping_property: Option<ProtocolLocalMappingPropertyInfo>,
    /// Column name on the function's relation output.
    pub column: String,
    /// Optional `Binding pkg::B :` transformer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_transformer: Option<ProtocolBindingTransformer>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Java's `BindingTransformer` from the external-format package.
///
/// `BindingTransformer.java:20-25`: a single `binding: String` field
/// + a `@JsonIgnore` source-information field. The Rust protocol
/// struct mirrors only what's on the wire; source info lives on the
/// AST `BindingTransformer` for non-serialization consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolBindingTransformer {
    /// FQN of the binding element.
    pub binding: String,
}
