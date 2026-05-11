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

//! Legend Protocol v1 JSON layer for `###Mapping` DSL types.
//!
//! Maps `crates/dsl-mapping/src/ast.rs` AST nodes to Java-parity protocol
//! structs that serialize to the Legend Protocol v1 JSON shape. Mirror
//! of Java's `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping`
//! package.
//!
//! ## Architecture
//!
//! DSL-owned protocol module (lives next to the AST and grammar). The
//! sibling `crates/protocol/` crate handles only core Pure types
//! (Class, Function, Enumeration, …) and intentionally does not depend
//! on DSL crates. Each DSL owns its protocol-conversion path so the
//! layering rule (core → DSL, never reverse) is preserved.
//!
//! ## Conversion pattern
//!
//! AST → protocol struct → JSON, two-step, via `impl From<&ast::*>`
//! blocks in [`convert`]. Same idiom as `crates/protocol/src/v1/convert.rs`.
//!
//! ## JSON discriminators (Java parity)
//!
//! From `ClassMapping.java:25-32`:
//!
//! | Variant                          | `_type` discriminator |
//! |----------------------------------|-----------------------|
//! | `PureInstanceClassMapping`       | `pureInstance`        |
//! | `OperationClassMapping`          | `operation`           |
//! | `MergeOperationClassMapping`     | `mergeOperation`      |
//! | `AggregationAwareClassMapping`   | `aggregationAware`    |
//! | `RelationFunctionClassMapping`   | `relation`            |
//!
//! `EnumerationMapping` lives in a sibling `enumerationMappings` list
//! on the outer `Mapping`, not under `classMappings`. XStore-bodied
//! class mappings route into `associationMappings` since the target
//! is an Association, not a Class. (Our Rust AST encodes all body
//! kinds under a unified `ClassMappingBody` for parser convenience;
//! protocol conversion splits them into the right Java lists.)
//!
//! ## Commit cadence (T3.4)
//!
//! This module is built across seven commits — see
//! `.claude/plans/do-we-have-enought-quiet-swing.md`. **c1** ships
//! the container + the `ProtocolClassMapping` enum stub +
//! `ProtocolMappingInclude`; subsequent commits fill body variants
//! (c2 Pure, c3 Operation/Merge, c4 Enumeration, c5 AggregationAware,
//! c6 RelationFunction, c7 XStore + multi-substitution audit).

pub mod aggregation_aware;
pub mod class_mapping;
pub mod convert;
pub mod enumeration;
pub mod include;
pub mod operation;
pub mod pure;
pub mod relation_function;
pub mod xstore;

use serde::{Deserialize, Serialize};

use crate::protocol::class_mapping::ProtocolClassMapping;
use crate::protocol::enumeration::ProtocolEnumerationMapping;
use crate::protocol::include::ProtocolMappingInclude;
use crate::protocol::xstore::ProtocolAssociationMapping;
use legend_pure_parser_protocol::v1::source_info::SourceInformation;

/// Protocol representation of a top-level `Mapping` (the `###Mapping`
/// section's outer element).
///
/// Maps to Java
/// `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.Mapping`.
///
/// JSON shape:
///
/// ```json
/// {
///   "package": "my::test",
///   "name": "M",
///   "classMappings": [ … ],
///   "includedMappings": [ … ]
/// }
/// ```
///
/// The Java `Mapping` carries additional sibling lists
/// (`associationMappings`, `enumerationMappings`, `tests`,
/// `testSuites`) that this Rust port adds in later commits (c4/c7).
/// Missing-on-the-wire fields deserialize as empty per Java
/// Jackson's `default` behaviour, so the c1 shape forward-compatible
/// with Java JSON that includes those fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolMapping {
    /// Package path, `::`-joined (e.g. `"my::test"`).
    #[serde(rename = "package")]
    pub package_path: String,
    /// Element name (e.g. `"FirmMapping"`).
    pub name: String,
    /// Class mappings: every body kind whose target is a Class
    /// (`PureInstanceClassMapping`, `OperationClassMapping`,
    /// `MergeOperationClassMapping`, `AggregationAwareClassMapping`,
    /// `RelationFunctionClassMapping`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub class_mappings: Vec<ProtocolClassMapping>,
    /// Enumeration mappings — class mappings whose target is an
    /// Enumeration route here (Java parity:
    /// `Mapping.enumerationMappings`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enumeration_mappings: Vec<ProtocolEnumerationMapping>,
    /// Association mappings — XStore-bodied class mappings route
    /// here (Java parity: `Mapping.associationMappings`). The target
    /// is an `Association`, not a `Class`, so they don't belong in
    /// `classMappings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub association_mappings: Vec<ProtocolAssociationMapping>,
    /// Mapping include declarations with optional store substitution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_mappings: Vec<ProtocolMappingInclude>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}
