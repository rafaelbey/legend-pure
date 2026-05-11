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

//! `ProtocolOperationClassMapping` + `ProtocolMergeOperationClassMapping`
//! — the function-driven combinator body kinds (`_type = "operation"`
//! and `_type = "mergeOperation"`).
//!
//! Maps to Java's
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.{OperationClassMapping, MergeOperationClassMapping}`.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::value_spec::LambdaFunction;

use crate::protocol::class_mapping::ProtocolClassMappingHeader;

/// Java's `MappingOperation` enum — the four well-known router ops
/// that the Java protocol recognizes.
///
/// Reference: `MappingOperation.java:17-23`. When the operation
/// function FQN doesn't match one of the four router ops, Java's
/// `funcToOps.get(...)` returns `null`, so the JSON `operation`
/// field serializes as absent. We mirror that with an `Option<…>`
/// on the parent struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MappingOperation {
    /// `meta::pure::router::operations::union_OperationSetImplementation_1__SetImplementation_MANY_`.
    #[serde(rename = "STORE_UNION")]
    StoreUnion,
    /// `meta::pure::router::operations::special_union_OperationSetImplementation_1__SetImplementation_MANY_`.
    #[serde(rename = "ROUTER_UNION")]
    RouterUnion,
    /// `meta::pure::router::operations::inheritance_OperationSetImplementation_1__SetImplementation_MANY_`.
    #[serde(rename = "INHERITANCE")]
    Inheritance,
    /// `meta::pure::router::operations::merge_OperationSetImplementation_1__SetImplementation_MANY_`.
    #[serde(rename = "MERGE")]
    Merge,
}

impl MappingOperation {
    /// Map a function FQN (typically a mangled platform router-op
    /// FQN) to the corresponding `MappingOperation` enum value. Returns
    /// `None` for any FQN outside the four well-known router ops —
    /// matches Java's `funcToOps.get(...)` returning `null` for
    /// non-router FQNs.
    ///
    /// Reference: `OperationClassMapping.java:27-31` (the
    /// `opsToFunc` map; we walk it in reverse).
    #[must_use]
    pub fn from_function_fqn(fqn: &str) -> Option<Self> {
        match fqn {
            "meta::pure::router::operations::union_OperationSetImplementation_1__SetImplementation_MANY_" => Some(Self::StoreUnion),
            "meta::pure::router::operations::special_union_OperationSetImplementation_1__SetImplementation_MANY_" => Some(Self::RouterUnion),
            "meta::pure::router::operations::inheritance_OperationSetImplementation_1__SetImplementation_MANY_" => Some(Self::Inheritance),
            "meta::pure::router::operations::merge_OperationSetImplementation_1__SetImplementation_MANY_" => Some(Self::Merge),
            _ => None,
        }
    }
}

/// Protocol shape for an `: Operation { … }` class mapping body.
///
/// Mirrors Java's `OperationClassMapping` extending `ClassMapping`:
/// flattens the header alongside `parameters` (List<String>) and
/// `operation` (MappingOperation enum, nullable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolOperationClassMapping {
    /// Inherited `ClassMapping` fields, flattened.
    #[serde(flatten)]
    pub header: ProtocolClassMappingHeader,
    /// Sibling class-mapping IDs combined by the operation, in source
    /// order. Java's `parameters: List<String>` default-empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<String>,
    /// One of the four well-known router operations the function FQN
    /// maps to. `None` when the FQN doesn't match a router op — Java
    /// parity with `funcToOps.get(fqn)` returning `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<MappingOperation>,
}

/// Protocol shape for an `: Operation { merge(...) }` class mapping
/// body — the merge form with a validation lambda.
///
/// Mirrors Java's `MergeOperationClassMapping` extending
/// `OperationClassMapping`: inherits `parameters` + `operation`, adds
/// `validationFunction: LambdaFunction`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolMergeOperationClassMapping {
    /// Inherited `ClassMapping` fields, flattened.
    #[serde(flatten)]
    pub header: ProtocolClassMappingHeader,
    /// Parameters (sibling class-mapping IDs); inherited from
    /// `OperationClassMapping`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<String>,
    /// Well-known router op or `None` (typically `MERGE` here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<MappingOperation>,
    /// Validation lambda applied to the merged rows. Always populated
    /// for merge-form bodies — the parser only constructs a
    /// `ProtocolMergeOperationClassMapping` when the AST carries a
    /// `validation_function: Some(...)`.
    pub validation_function: LambdaFunction,
}
