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

//! Embedded platform Pure sources.

/// A single platform Pure source file.
pub struct PureSourceFile {
    /// Relative path within the platform directory (e.g., "essential/tests/assert.pure").
    pub path: &'static str,
    /// Source content.
    pub content: &'static str,
}

// Include the generated file from build.rs
include!(concat!(env!("OUT_DIR"), "/generated_sources.rs"));

/// Returns all embedded platform Pure source files.
#[must_use]
pub fn platform_sources() -> &'static [PureSourceFile] {
    PLATFORM_FILES
}
