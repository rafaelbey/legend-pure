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

//! Compiled Profile node.

use legend_pure_parser_ast::SourceInfo;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// A compiled profile definition.
///
/// A profile declares stereotypes and tags that can be applied to
/// elements, properties, and enum values via `<<profile.stereo>>`
/// and `{profile.tag = 'value'}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Stereotype names declared by this profile.
    pub stereotypes: Vec<SmolStr>,
    /// Tag names declared by this profile.
    pub tags: Vec<SmolStr>,
    /// Source spans of each stereotype declaration, position-aligned
    /// with `stereotypes`. Populated by the parser; `None` per slot
    /// for snapshot-loaded or m3-bootstrap profiles whose
    /// declarations didn't come through the parser. Used by IDE
    /// goto-def to navigate from a `<<P.stereo>>` use-site to the
    /// `stereo` line within the Profile body.
    #[serde(default)]
    pub stereotype_source_infos: Vec<Option<SourceInfo>>,
    /// Source spans of each tag declaration, position-aligned with
    /// `tags`. Same shape and rationale as `stereotype_source_infos`.
    #[serde(default)]
    pub tag_source_infos: Vec<Option<SourceInfo>>,
}
