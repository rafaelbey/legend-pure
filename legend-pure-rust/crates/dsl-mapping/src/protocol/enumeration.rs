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

//! `ProtocolEnumerationMapping` + `ProtocolEnumValueMapping` + source-
//! value discriminated union.
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.{EnumerationMapping, EnumValueMapping}`.
//!
//! **Important layering**: Java's `EnumerationMapping` lives in
//! `Mapping.enumerationMappings` (sibling list to `classMappings`),
//! NOT under the `ClassMapping` `_type` enum. Our Rust AST encodes
//! `Enumeration` as one of the `ClassMappingBody` variants for parser
//! convenience; protocol conversion splits it out into the right
//! Java list.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::source_info::SourceInformation;
use legend_pure_parser_protocol::v1::value_spec::ProtocolPackageableElementPtr;

/// Java `EnumerationMapping` — a top-level enumeration source-value
/// → target-value table.
///
/// Lives in `Mapping.enumerationMappings`. Not under
/// `ProtocolClassMapping`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumerationMapping {
    /// Mapping ID — typically the class-mapping declaration's `[id]`
    /// or the target enumeration's simple name as a default.
    pub id: String,
    /// Target enumeration FQN, wrapped as a `PackageableElementPointer`
    /// (matches the v1 protocol layout — Java's `enumeration` field
    /// is the same type).
    pub enumeration: ProtocolPackageableElementPtr,
    /// Per-target-value mappings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_value_mappings: Vec<ProtocolEnumValueMapping>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Java `EnumValueMapping` — one target-value's source list.
///
/// `sourceValues` carries 1–N entries; each is one of the three
/// `EnumValueMappingSourceValue` discriminated variants
/// (`stringSourceValue` / `integerSourceValue` / `enumSourceValue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumValueMapping {
    /// Target enumeration value name (e.g. `"ACTIVE"`).
    pub enum_value: String,
    /// Sequence of source values that map to `enum_value`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_values: Vec<ProtocolEnumValueMappingSourceValue>,
}

/// Discriminated union of enum-mapping source-value variants.
///
/// `_type` discriminators (Java `EnumValueMappingSourceValue.java:21-25`):
///
/// | Variant            | `_type`             |
/// |--------------------|---------------------|
/// | string literal     | `stringSourceValue` |
/// | integer literal    | `integerSourceValue` |
/// | enum reference     | `enumSourceValue`   |
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ProtocolEnumValueMappingSourceValue {
    /// String-literal source value (e.g. `"abc"`).
    #[serde(rename = "stringSourceValue")]
    String(ProtocolEnumValueMappingStringSourceValue),
    /// Integer-literal source value (e.g. `123`).
    #[serde(rename = "integerSourceValue")]
    Integer(ProtocolEnumValueMappingIntegerSourceValue),
    /// Enum-reference source value (e.g. `pkg::Other.FOO`).
    #[serde(rename = "enumSourceValue")]
    Enum(ProtocolEnumValueMappingEnumSourceValue),
}

/// String-literal source value (`_type = "stringSourceValue"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumValueMappingStringSourceValue {
    /// The string value.
    pub value: String,
}

/// Integer-literal source value (`_type = "integerSourceValue"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumValueMappingIntegerSourceValue {
    /// The integer value. Java uses `Integer` (32-bit signed);
    /// we mirror with `i32` so JSON round-trips numerically clean.
    pub value: i32,
}

/// Enum-reference source value (`_type = "enumSourceValue"`).
///
/// References a value `value` declared on the enumeration FQN
/// `enumeration`. Java uses two plain strings (not a
/// `PackageableElementPointer`) for the source-side enumeration —
/// `EnumValueMappingEnumSourceValue.java:18-20`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumValueMappingEnumSourceValue {
    /// `::`-joined FQN of the source enumeration.
    pub enumeration: String,
    /// The source enumeration's value name.
    pub value: String,
}
