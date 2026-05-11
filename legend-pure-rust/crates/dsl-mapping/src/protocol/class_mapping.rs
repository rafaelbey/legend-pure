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

//! `ProtocolClassMapping` — discriminated union of class-mapping body
//! kinds.
//!
//! Maps to Java
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.ClassMapping`
//! (abstract base + 5 `@JsonSubTypes`).

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::source_info::SourceInformation;

use crate::protocol::aggregation_aware::ProtocolAggregationAwareClassMapping;
use crate::protocol::operation::{
    ProtocolMergeOperationClassMapping, ProtocolOperationClassMapping,
};
use crate::protocol::pure::ProtocolPureInstanceClassMapping;

/// Discriminated union over the 5 Java class-mapping body kinds.
///
/// The `_type` JSON tag distinguishes variants — `pureInstance` /
/// `operation` / `mergeOperation` / `aggregationAware` / `relation`
/// (`ClassMapping.java:25-32`).
///
/// Each variant carries a body struct that `#[serde(flatten)]`s the
/// shared header fields (id, class, root, …) alongside body-specific
/// data — matching Java's `extends ClassMapping` layout where every
/// concrete class has the abstract base's fields at the top JSON level.
///
/// **Status by variant**:
/// - `PureInstance` — body landed in c2.
/// - `Operation` / `MergeOperation` — body landed in c3.
/// - `AggregationAware` — body landed in c5.
/// - `Relation` — header-only stub (c6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ProtocolClassMapping {
    /// Pure-instance class mapping (`_type = "pureInstance"`).
    #[serde(rename = "pureInstance")]
    PureInstance(ProtocolPureInstanceClassMapping),

    /// Operation class mapping (`_type = "operation"`).
    #[serde(rename = "operation")]
    Operation(ProtocolOperationClassMapping),

    /// Merge operation class mapping (`_type = "mergeOperation"`).
    #[serde(rename = "mergeOperation")]
    MergeOperation(ProtocolMergeOperationClassMapping),

    /// Aggregation-aware class mapping (`_type = "aggregationAware"`).
    #[serde(rename = "aggregationAware")]
    AggregationAware(Box<ProtocolAggregationAwareClassMapping>),

    /// Relation-function class mapping (`_type = "relation"`). Body
    /// data (relation function FQN + property→column mappings) lands
    /// in c6.
    #[serde(rename = "relation")]
    Relation(ProtocolClassMappingHeader),
}

/// Common fields shared across every `ProtocolClassMapping` variant.
///
/// Mirrors Java's `ClassMapping.java` abstract base (lines 33-44):
/// `id`, `_class` (JSON key `"class"`), `root`, `extendsClassMappingId`,
/// `sourceInformation`. The `mappingClass` and `classSourceInformation`
/// fields on the Java base are not yet exercised by any AST consumer
/// in this workspace and are deferred until they appear in a
/// fixture round-trip.
///
/// `c1 deliberate omission`: the body-specific fields (`propertyMappings`,
/// `srcClass`, `filter`, `parameters`, …) are absent; later commits
/// add per-variant flattened structs that supersede this stub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolClassMappingHeader {
    /// Explicit class-mapping ID (`[id]` in grammar). `None` when
    /// the user didn't write one; Java uses the class FQN as the
    /// default, but the protocol JSON leaves it absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// `::`-joined FQN of the target class. Serializes as JSON key
    /// `"class"` (Java's `@JsonProperty(value = "class")`).
    #[serde(rename = "class")]
    pub class: String,

    /// `extends [parentId]` clause — the parent class-mapping ID this
    /// mapping inherits from. `None` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends_class_mapping_id: Option<String>,

    /// `*` root marker on the class-mapping line. Java's default is
    /// `true`; emit only when `false` for a slightly cleaner JSON.
    #[serde(default = "default_root_true", skip_serializing_if = "is_true")]
    pub root: bool,

    /// Span covering the entire class-mapping declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

fn default_root_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(b: &bool) -> bool {
    *b
}
