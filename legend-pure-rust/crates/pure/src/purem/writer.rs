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

//! `.purem` writer — `PureModelSlice` → bytes.
//!
//! v0 format: hand-rolled magic header + Postcard-encoded payload.
//! Postcard's encoding is deterministic by construction (no field
//! reordering, fixed varint discipline), so two `write_repo` calls on the
//! same input produce byte-identical output. The determinism gate in
//! Phase J asserts this twice in a row.

use thiserror::Error;

use super::header::{HEADER_LEN, HeaderError, write_header};
use super::slice::PureModelSlice;

/// Errors raised by [`write_repo`].
#[derive(Debug, Error)]
pub enum WriteError {
    /// Postcard serialization failed. Never expected for well-typed
    /// inputs, but propagated rather than panicking.
    #[error("postcard serialization failed: {0}")]
    Postcard(#[from] postcard::Error),
}

/// Errors raised by [`crate::purem::reader::read_repo`].
#[derive(Debug, Error)]
pub enum ReadError {
    /// Header parse / validation failed.
    #[error(transparent)]
    Header(#[from] HeaderError),
    /// Payload deserialization failed (corrupt bytes, format drift).
    #[error("postcard deserialization failed: {0}")]
    Postcard(#[from] postcard::Error),
}

/// Serialize a [`PureModelSlice`] into a length-prefixed `.purem` blob.
///
/// Output layout: `[22-byte header][postcard-encoded slice]`. See
/// [`super::header`] for the header layout.
///
/// # Errors
///
/// Returns [`WriteError::Postcard`] if Postcard fails to encode the slice.
/// In practice this only happens if a `Serialize` impl returns an error
/// — none of our types do today.
pub fn write_repo(slice: &PureModelSlice) -> Result<Vec<u8>, WriteError> {
    let payload = postcard::to_allocvec(slice)?;
    let mut blob = Vec::with_capacity(HEADER_LEN + payload.len());
    write_header(&mut blob, payload.len() as u64);
    blob.extend_from_slice(&payload);
    Ok(blob)
}
