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

/// Returns all embedded platform JSON manifests (PCT manifests, etc.).
///
/// Path field uses the same `/platform/pure/<relative>` resource-URL form
/// as [`platform_sources`] — `loadPCTManifest` matches by suffix so callers
/// can supply either the bare filename (`pct_essential_native.json`) or
/// the full canonical path.
#[must_use]
pub fn platform_manifests() -> &'static [PureSourceFile] {
    PLATFORM_MANIFESTS
}

/// Look up an embedded platform manifest by suffix match against its
/// canonical `/platform/pure/...` path. Returns the file content if any
/// manifest's path ends with `suffix`, otherwise `None`.
///
/// Matches whole path segments — `"essential.json"` doesn't match
/// `"pct_essential_native.json"` because the character preceding the
/// match must be a path separator (or the start of the string).
#[must_use]
pub fn find_manifest(suffix: &str) -> Option<&'static str> {
    PLATFORM_MANIFESTS
        .iter()
        .find(|f| {
            if f.path == suffix {
                return true;
            }
            if !f.path.ends_with(suffix) {
                return false;
            }
            let prefix_len = f.path.len() - suffix.len();
            prefix_len == 0
                || f.path[..prefix_len]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c == '/' || c == '\\')
        })
        .map(|f| f.content)
}
