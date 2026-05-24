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

//! `.purem` — pre-compiled per-repo `PureModel` snapshots.
//!
//! See `crates/core-platform-pure/docs/PUREM_FORMAT.md` for the design.
//!
//! # Layered API
//!
//! - [`slice`](mod@slice) — [`PureModelSlice`] + `slice_by_repo` / `merge_slice`. The
//!   in-memory partition primitive.
//! - [`header`] — magic / version / schema_hash header, hand-rolled.
//! - [`writer`] / [`reader`] — wire format. v0 uses hand-rolled header +
//!   Postcard payload; v1 may upgrade Tier-2 to lazy mmap (FlatBuffers,
//!   etc.) without changing the public API.
//! - `extension` — `PuremExtension` registry for DSL plugin data
//!   (Phase E).

pub mod filter;
pub mod fqn_path;
pub mod header;
pub mod reader;
pub mod slice;
pub mod walk;
pub mod writer;

pub use filter::{FilterError, TestPartition, assert_no_dangling_refs, collect_test_partition};
pub use reader::read_repo;
pub use slice::{
    PureModelSlice, SliceError, merge_slice, slice_by_repo, slice_by_repo_with_filter,
};
pub use writer::{ReadError, WriteError, write_repo};
