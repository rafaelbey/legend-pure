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

//! `ProtocolMappingInclude` — JSON representation of a `MappingInclude`.
//!
//! Maps to Java
//! `org.finos.legend.engine.protocol.pure.v1.model.packageableElement.mapping.MappingInclude`
//! (abstract) + the single concrete `MappingIncludeMapping` subclass.

use serde::{Deserialize, Serialize};

use legend_pure_parser_protocol::v1::source_info::SourceInformation;

/// Discriminated union of `MappingInclude` JSON variants.
///
/// Today Java declares exactly one concrete subclass —
/// `MappingIncludeMapping` (`_type = "mappingIncludeMapping"`). We
/// keep the enum shape to forward-match Java's
/// `@JsonSubTypes` extensibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ProtocolMappingInclude {
    /// The canonical `include pkg::M [src -> tgt]?` shape.
    #[serde(rename = "mappingIncludeMapping")]
    MappingIncludeMapping(ProtocolMappingIncludeMapping),
}

/// Java's `MappingIncludeMapping` shape — single substitution per
/// include.
///
/// **Java-parity quirk on multi-substitution**: Java's grammar allows
/// `[A -> B, C -> D, …]` (per
/// `MappingParserGrammar.g4:42` —
/// `(storeSubPath (COMMA storeSubPath)*)?`) but the protocol JSON
/// only carries a single `(source, target)` pair on
/// `MappingIncludeMapping`. Java's parser
/// (`CorePureGrammarParser.parseMappingInclude:472-484`) populates
/// `source/target` only when the grammar produced exactly one
/// `storeSubPath`; with more than one it sets both to `null` — the
/// JSON loses the extra substitutions.
///
/// Our T2.6 work covered the multi-substitution AST + validators —
/// the AST keeps `store_substitutions: Vec<StoreSubstitution>`. This
/// protocol struct mirrors Java's lossy single-substitution JSON
/// shape exactly so the wire format round-trips Java-side. Callers
/// who need to preserve every substitution should use the AST
/// directly or the `.purem` binary snapshot, neither of which is
/// limited by the Java protocol's design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolMappingIncludeMapping {
    /// FQN of the included mapping (e.g. `"my::test::Inner"`).
    pub included_mapping: String,

    /// Source database / store FQN of the single substitution.
    /// `None` when the grammar declared zero or more-than-one
    /// substitutions (Java-parity: see struct doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_database_path: Option<String>,

    /// Target database / store FQN of the single substitution.
    /// `None` when the grammar declared zero or more-than-one
    /// substitutions (Java-parity: see struct doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_database_path: Option<String>,

    /// Source location of the include declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}
