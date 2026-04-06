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

use smol_str::SmolStr;

/// A compiled profile definition.
///
/// A profile declares stereotypes and tags that can be applied to
/// elements, properties, and enum values via `<<profile.stereo>>`
/// and `{profile.tag = 'value'}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Stereotype names declared by this profile.
    pub stereotypes: Vec<SmolStr>,
    /// Tag names declared by this profile.
    pub tags: Vec<SmolStr>,
}
