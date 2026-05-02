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

//! `.purem` reader — bytes → `PureModelSlice`.

use super::header::read_header;
use super::slice::PureModelSlice;
use super::writer::ReadError;

/// Parse a `.purem` blob, validate its header, and deserialize the
/// payload into a [`PureModelSlice`].
///
/// **Eager Tier-1 read in v0.** The full slice (including any function
/// bodies) is materialized in memory at this point. v1 will split bodies
/// into a Tier-2 region and return a lazy handle; the public API of this
/// function won't change.
///
/// # Errors
///
/// - [`ReadError::Header`] for any header validation failure (magic /
///   version / schema_hash / length mismatch).
/// - [`ReadError::Postcard`] for any payload deserialization failure.
pub fn read_repo(blob: &[u8]) -> Result<PureModelSlice, ReadError> {
    let payload = read_header(blob)?;
    let slice: PureModelSlice = postcard::from_bytes(payload)?;
    Ok(slice)
}
