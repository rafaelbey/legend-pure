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

//! Platform-scale smoke test for the `.purem` wire format.
//!
//! Loads the full embedded platform (all repos), slices it into one big
//! `PureModelSlice` covering every user chunk, writes it to bytes, reads
//! it back, and verifies:
//!
//! 1. `write_repo` is idempotent on the platform-scale slice (determinism
//!    gate — Phase J).
//! 2. The reader produces a structurally-equivalent slice
//!    (`chunks.len()`, `external_refs.len()`, etc. match).
//! 3. Re-serializing the recovered slice yields byte-identical output to
//!    the original.

use legend_pure_core_platform::platform;
use legend_pure_parser_pure::purem::{read_repo, slice_by_repo, write_repo};

#[test]
fn platform_slice_round_trips_through_wire_format() {
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model, // partial model is fine for the wire-format test
    };

    // Bootstrap is chunk 0; everything else is user-defined.
    let user_range = 1..(model.chunks.len() as u16);
    if user_range.is_empty() {
        // No user chunks → skip; the test is meaningless on a bare model.
        return;
    }
    let slice = slice_by_repo(&model, user_range);

    // Realistic scale: hundreds of elements + thousands of refs.
    assert!(
        slice.chunks.iter().map(|c| c.elements.len()).sum::<u32>() > 100,
        "platform slice should contain >100 elements"
    );

    // Determinism gate: idempotent writes.
    let a = write_repo(&slice).expect("write a should succeed");
    let b = write_repo(&slice).expect("write b should succeed");
    assert_eq!(a, b, "two writes of platform slice must be byte-identical");

    // Round-trip stability.
    let recovered = read_repo(&a).expect("read should succeed");
    assert_eq!(slice.chunks.len(), recovered.chunks.len());
    assert_eq!(slice.external_refs, recovered.external_refs);
    assert_eq!(
        slice.element_packages.len(),
        recovered.element_packages.len()
    );
    assert_eq!(slice.source_chunk_range, recovered.source_chunk_range);

    let c = write_repo(&recovered).expect("write c should succeed");
    assert_eq!(
        a, c,
        "round-trip through wire format must be byte-identical"
    );

    // Sanity: the platform .purem blob should not be enormous (a sign of
    // accidental string repetition, missing dedup, etc.). Adjust the
    // ceiling if real-world platform growth justifies it. Today's
    // platform is ~1338 elements; back-of-envelope: a few MB max.
    assert!(
        a.len() < 50_000_000,
        "platform .purem should fit comfortably under 50 MB, got {} bytes",
        a.len()
    );

    eprintln!(
        "platform .purem is {} bytes ({} chunks, {} external refs)",
        a.len(),
        slice.chunks.len(),
        slice.external_refs.len()
    );
}
