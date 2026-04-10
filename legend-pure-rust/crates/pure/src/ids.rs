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

//! Typed index wrappers for the Pure semantic graph.
//!
//! Every node in the graph is referenced by a lightweight, `Copy` index.
//! The segmented [`ElementId`] enables zero-rewrite chunk merging — when a
//! new chunk is added, all existing IDs remain valid.

use std::fmt;

// ---------------------------------------------------------------------------
// ElementId — the universal element reference
// ---------------------------------------------------------------------------

/// A segmented index into the chunked element arenas.
///
/// `chunk_id` identifies which [`ModelChunk`](crate::model::ModelChunk) owns
/// the element, and `local_idx` is the position within that chunk's arenas.
///
/// This design enables O(1) model merging: push a new chunk, link its elements
/// into the global package tree, and rebuild derived indexes — no ID rewriting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId {
    /// Which chunk owns this element (0 = bootstrap).
    pub chunk_id: u16,
    /// Index within the chunk's `nodes` and `elements` arenas.
    pub local_idx: u32,
}

impl fmt::Display for ElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.chunk_id, self.local_idx)
    }
}

// ---------------------------------------------------------------------------
// PackageId — index into the global package tree
// ---------------------------------------------------------------------------

/// An index into the global package arena on [`PureModel`](crate::model::PureModel).
///
/// Unlike [`ElementId`], packages are not chunked — they live in a single
/// global arena because a package like `meta::pure` can span multiple chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageId(pub u32);

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pkg:{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// RelationId — index into the structural type interner (Phase 5)
// ---------------------------------------------------------------------------

/// An index into the relation type interner on `PureModel`.
///
/// Relation types are anonymous structural types (column bags) that are
/// interned for deduplication. This ID is used by `TypeExpr::Relation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelationId(pub u32);

impl fmt::Display for RelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rel:{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_id_is_copy_and_eq() {
        let a = ElementId {
            chunk_id: 0,
            local_idx: 5,
        };
        let b = a; // Copy
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "0:5");
    }

    #[test]
    fn element_id_hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ElementId {
            chunk_id: 0,
            local_idx: 0,
        });
        set.insert(ElementId {
            chunk_id: 0,
            local_idx: 1,
        });
        set.insert(ElementId {
            chunk_id: 0,
            local_idx: 0,
        }); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn element_id_different_chunks_not_equal() {
        let a = ElementId {
            chunk_id: 0,
            local_idx: 0,
        };
        let b = ElementId {
            chunk_id: 1,
            local_idx: 0,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn package_id_is_copy_and_eq() {
        let a = PackageId(3);
        let b = a;
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "pkg:3");
    }

    #[test]
    fn relation_id_display() {
        let r = RelationId(42);
        assert_eq!(r.to_string(), "rel:42");
    }
}
