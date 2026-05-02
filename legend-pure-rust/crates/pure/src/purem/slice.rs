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

//! Per-repo slices of a [`PureModel`].
//!
//! [`slice_by_repo`] partitions a compiled model by chunk-id range,
//! cloning the chunks and rewriting every cross-range [`ElementId`]
//! reference into an FQN-sentinel that survives serialization.
//! [`merge_slice`] reverses the process: it appends a slice's chunks to a
//! running model, rewrites internal IDs to the model's new chunk numbering,
//! and resolves FQN sentinels against the model's package tree.
//!
//! Together they form the in-memory primitive that the FlatBuffers wire
//! format (Phase B-D) sits on top of — the file format never sees raw
//! [`ElementId`]s, only FQN strings + slice-local indices.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::ElementId;
use crate::model::{ModelChunk, PureModel};

use super::fqn_path::{FqnPath, build_fqn_index, fqn_path_to_string, package_path};
use super::walk::walk_element_ids;

/// Sentinel chunk id used inside a slice's chunks to flag external
/// references. The local_idx of the sentinel ElementId indexes into
/// [`PureModelSlice::external_refs`].
///
/// `u16::MAX` is reserved (chunk_id is `u16`, real chunk ids count up
/// from 0). Any code that reads chunks from a slice must check for this
/// sentinel before treating chunk_id as an array index.
pub const EXTERNAL_REF_SENTINEL: u16 = u16::MAX;

/// A per-repo slice of a [`PureModel`]: clone of one chunk-id range, plus
/// the side tables needed to relocate it into a fresh model.
///
/// **Invariant:** every [`ElementId::InstanceId`] inside `chunks` has
/// either:
/// - `chunk_id ∈ [0, chunks.len())` — an internal reference using
///   slice-local numbering, OR
/// - `chunk_id == EXTERNAL_REF_SENTINEL` and `local_idx` is an index into
///   `external_refs` — a cross-repo reference encoded as an FQN string.
///
/// Plus every `ElementNode.parent_package` is replaced with `PackageId(0)`
/// (a tombstone) — the real package is reconstructed at merge time from
/// the parallel `element_packages` table. This keeps `PackageId`s, which
/// are arena indices, out of the wire format entirely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PureModelSlice {
    /// Chunk-id range this slice covered in the source model. Informational.
    pub source_chunk_range: Range<u16>,

    /// Cloned chunks. Internal ElementIds use slice-local chunk indices;
    /// external ones use [`EXTERNAL_REF_SENTINEL`].
    pub chunks: Vec<ModelChunk>,

    /// FQN path for every external reference, indexed by sentinel
    /// `local_idx`.
    pub external_refs: Vec<FqnPath>,

    /// Package paths (FQN segments) for every element, parallel to chunks.
    /// `element_packages[chunk_idx][local_idx]` is the package path for
    /// the element at `chunks[chunk_idx].nodes[local_idx]`.
    pub element_packages: Vec<Vec<FqnPath>>,
}

/// Errors raised by [`merge_slice`].
#[derive(Debug, Error)]
pub enum SliceError {
    /// An external reference's FQN didn't resolve in the running model.
    /// Almost always means the dependency repo wasn't loaded first.
    #[error("unresolved external reference '{fqn}' (slice expected dependency loaded first)")]
    UnresolvedExternal {
        /// The FQN string that failed to resolve.
        fqn: String,
    },
}

/// Partition a compiled [`PureModel`] by chunk-id range into a
/// [`PureModelSlice`].
///
/// The slice is a *standalone* representation: every reference into chunks
/// outside `chunk_range` is captured as an FQN string in `external_refs`,
/// and every package reference is captured as an FQN path. The original
/// `model` is not modified.
///
/// `chunk_range.start..chunk_range.end` must be in-bounds. Empty ranges
/// produce an empty slice.
#[must_use]
pub fn slice_by_repo(model: &PureModel, chunk_range: Range<u16>) -> PureModelSlice {
    let chunks_in_range: Vec<ModelChunk> = (chunk_range.start..chunk_range.end)
        .map(|cid| model.chunks[cid as usize].clone())
        .collect();

    // Build chunk-id → slice-local-index map (chunk ids in range are
    // contiguous, but we keep this explicit for clarity).
    let in_range = |cid: u16| -> Option<u16> {
        if cid >= chunk_range.start && cid < chunk_range.end {
            Some(cid - chunk_range.start)
        } else {
            None
        }
    };

    // Translate every ElementId in every chunk's elements: internal refs
    // get rebased to slice-local chunk ids; external refs get replaced
    // with a sentinel pointing into external_refs.
    //
    // Pre-build the FQN index once: O(packages × children) up-front, then
    // O(1) per external ref. Critical for platform-scale slices with many
    // hundreds of external refs.
    let fqn_index = build_fqn_index(model);
    let mut external_refs: Vec<FqnPath> = Vec::new();

    let mut translated_chunks: Vec<ModelChunk> = Vec::with_capacity(chunks_in_range.len());
    for (slice_idx, mut chunk) in chunks_in_range.into_iter().enumerate() {
        // Re-stamp the chunk's own chunk_id to the slice-local index. The
        // merge step rewrites this again to the model's new numbering.
        #[allow(clippy::cast_possible_truncation)]
        let new_chunk_id = slice_idx as u16;
        chunk.chunk_id = new_chunk_id;

        for local_idx in 0..chunk.elements.len() {
            let element = chunk.elements.get_mut(local_idx);
            walk_element_ids(element, |id: &mut ElementId| {
                if let ElementId::InstanceId {
                    chunk_id,
                    local_idx,
                } = *id
                {
                    if let Some(local_chunk) = in_range(chunk_id) {
                        *id = ElementId::InstanceId {
                            chunk_id: local_chunk,
                            local_idx,
                        };
                    } else {
                        // External: capture FQN from index, replace with
                        // sentinel.
                        let original = ElementId::InstanceId {
                            chunk_id,
                            local_idx,
                        };
                        let fqn = fqn_index.get(&original).cloned().unwrap_or_else(|| {
                            // Fall back to the per-call walk only when
                            // the index doesn't have an entry — never
                            // expected for a well-formed model, but
                            // defensive against rare edge cases.
                            super::fqn_path::element_fqn_path(model, original)
                        });
                        let ref_idx = external_refs.len();
                        external_refs.push(fqn);
                        #[allow(clippy::cast_possible_truncation)]
                        let ref_idx_u32 = ref_idx as u32;
                        *id = ElementId::InstanceId {
                            chunk_id: EXTERNAL_REF_SENTINEL,
                            local_idx: ref_idx_u32,
                        };
                    }
                }
                // Package(_) variants stay as-is here. Per the slice
                // invariants, we tombstone parent_package in the loop
                // below; ad-hoc Package(_) refs inside Element data are
                // not currently produced by the compiler.
            });
        }

        translated_chunks.push(chunk);
    }

    // Capture parent_package paths and tombstone the PackageIds. Done in
    // a second pass so the visitor above doesn't see partially-rewritten
    // state.
    let mut element_packages: Vec<Vec<FqnPath>> = Vec::with_capacity(translated_chunks.len());
    for (chunk_idx, chunk) in translated_chunks.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let source_chunk_id = chunk_range.start + chunk_idx as u16;
        let original_chunk = &model.chunks[source_chunk_id as usize];
        let n_nodes = chunk.nodes.len();
        let mut paths: Vec<FqnPath> = Vec::with_capacity(n_nodes as usize);
        for local_idx in 0..n_nodes {
            let original_node = original_chunk.nodes.get(local_idx);
            let path = package_path(model, original_node.parent_package);
            paths.push(path);
            // Tombstone: the merge step always replaces this with a real
            // PackageId looked up against the merge target. We use
            // PackageId(0) (the root) as a deterministic sentinel.
            chunk.nodes.get_mut(local_idx).parent_package = crate::ids::PackageId(0);
        }
        element_packages.push(paths);
    }

    PureModelSlice {
        source_chunk_range: chunk_range,
        chunks: translated_chunks,
        external_refs,
        element_packages,
    }
}

/// Append a [`PureModelSlice`] to a running [`PureModel`].
///
/// Re-allocates chunk ids to the model's current numbering, rewrites every
/// internal ElementId accordingly, resolves external FQN refs against the
/// model's package tree, and reconstructs each element's `parent_package`
/// (creating packages as needed).
///
/// # Errors
///
/// Returns [`SliceError::UnresolvedExternal`] if any external FQN doesn't
/// resolve in `model` — the dependency was almost certainly loaded out of
/// order. The caller is expected to topo-sort by [`crate::extension`]-aware
/// repo metadata before merging.
pub fn merge_slice(model: &mut PureModel, slice: PureModelSlice) -> Result<(), SliceError> {
    #[allow(clippy::cast_possible_truncation)]
    let chunk_offset = model.chunks.len() as u16;

    // Pre-resolve every external FQN to the running model's ElementId.
    // Bailing early on the first miss gives the cleanest diagnostic.
    let mut resolved_external: Vec<ElementId> = Vec::with_capacity(slice.external_refs.len());
    for path in &slice.external_refs {
        let id = model
            .resolve_by_path(path)
            .ok_or_else(|| SliceError::UnresolvedExternal {
                fqn: fqn_path_to_string(path),
            })?;
        resolved_external.push(id);
    }

    // Rewrite ElementIds in-place: internal chunk ids get bumped by
    // chunk_offset; external sentinels get swapped for the resolved ID.
    let mut chunks = slice.chunks;
    let element_packages = slice.element_packages;

    for (slice_chunk_idx, chunk) in chunks.iter_mut().enumerate() {
        chunk.chunk_id = chunk_offset + slice_chunk_idx as u16;
        for local_idx in 0..chunk.elements.len() {
            let element = chunk.elements.get_mut(local_idx);
            walk_element_ids(element, |id: &mut ElementId| {
                if let ElementId::InstanceId {
                    chunk_id,
                    local_idx: lidx,
                } = *id
                {
                    if chunk_id == EXTERNAL_REF_SENTINEL {
                        *id = resolved_external[lidx as usize];
                    } else {
                        *id = ElementId::InstanceId {
                            chunk_id: chunk_id + chunk_offset,
                            local_idx: lidx,
                        };
                    }
                }
            });
        }

        // Reconstruct parent_package and register elements in their
        // packages. Done while we still know the chunk's slice-local
        // index so we can index into element_packages correctly.
        for (local_idx, path) in element_packages[slice_chunk_idx].iter().enumerate() {
            let pkg_id = if path.is_empty() {
                model.root_package
            } else {
                model.get_or_create_package(path)
            };
            #[allow(clippy::cast_possible_truncation)]
            let local_idx_u32 = local_idx as u32;
            chunk.nodes.get_mut(local_idx_u32).parent_package = pkg_id;
            let new_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx: local_idx_u32,
            };
            model.register_element(pkg_id, new_id);
        }
    }

    // Push merged chunks to the model.
    for chunk in chunks {
        model.chunks.push(chunk);
    }

    // Refresh derived indexes so subsequent queries see the new elements.
    model.rebuild_derived_indexes();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};
    use smol_str::SmolStr;

    fn parse(s: &str, p: &str) -> legend_pure_parser_ast::section::SourceFile {
        legend_pure_parser_parser::parse(s, p).expect("parse should succeed")
    }

    #[test]
    fn round_trip_one_class() {
        // Compile a class against a fresh bootstrap model. Slice it out,
        // then merge into a fresh model. Element should still resolve.
        let sf = parse("Class repo_a::Foo { x: Integer[1]; }", "foo.pure");
        let mut original = init_bootstrap_model();
        let (range, errs) = compile_repo_slice(&mut original, &[sf], &[], &[]);
        assert!(errs.is_empty(), "compile errors: {errs:?}");
        assert_eq!(range, 1..2);

        // Slice the user chunk only.
        let slice = slice_by_repo(&original, 1..2);
        assert_eq!(slice.chunks.len(), 1);
        // Foo has no cross-repo refs *except* its property type Integer,
        // which lives in chunk 0 (bootstrap). So we expect at least one
        // external ref.
        assert!(
            !slice.external_refs.is_empty(),
            "Foo should have external Integer reference"
        );

        // Merge into a fresh model.
        let mut fresh = init_bootstrap_model();
        merge_slice(&mut fresh, slice).expect("merge should succeed");

        // Foo must resolve in the merged model.
        let foo = fresh
            .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Foo")])
            .expect("Foo should resolve in merged model");
        // And Foo's name should match.
        assert_eq!(fresh.get_node(foo).name, SmolStr::new("Foo"));
    }

    #[test]
    fn round_trip_cross_slice_supertype() {
        // Two slices where one extends the other. After round-trip, the
        // supertype should still resolve.
        let mut model = init_bootstrap_model();
        let sf_animal = parse("Class zoo::Animal { name: String[1]; }", "a.pure");
        let sf_dog = parse("Class kennel::Dog extends zoo::Animal {}", "d.pure");

        let (_r1, e1) = compile_repo_slice(&mut model, &[sf_animal], &[], &[]);
        assert!(e1.is_empty());
        let (_r2, e2) = compile_repo_slice(&mut model, &[sf_dog], &[], &[]);
        assert!(e2.is_empty());

        let animal_slice = slice_by_repo(&model, 1..2);
        let dog_slice = slice_by_repo(&model, 2..3);

        // Dog slice must reference Animal as an external FQN.
        assert!(
            dog_slice
                .external_refs
                .iter()
                .any(|p| p.last() == Some(&SmolStr::new("Animal"))),
            "Dog slice should carry zoo::Animal as an external FQN"
        );

        // Merge into a fresh model in dependency order.
        let mut fresh = init_bootstrap_model();
        merge_slice(&mut fresh, animal_slice).expect("animal merge");
        merge_slice(&mut fresh, dog_slice).expect("dog merge");
        finalize_model(&mut fresh, &[], &[]);

        let dog_id = fresh
            .resolve_by_path(&[SmolStr::new("kennel"), SmolStr::new("Dog")])
            .expect("Dog should resolve");
        let animal_id = fresh
            .resolve_by_path(&[SmolStr::new("zoo"), SmolStr::new("Animal")])
            .expect("Animal should resolve");

        // Verify the supertype rewired correctly.
        let crate::model::Element::Class(dog) = fresh.get_element(dog_id) else {
            panic!("Dog should be a class");
        };
        assert_eq!(dog.super_types.len(), 1);
        let super_id = match &dog.super_types[0] {
            crate::types::TypeExpr::Named { element, .. } => *element,
            other => panic!("expected Named, got {other:?}"),
        };
        assert_eq!(super_id, animal_id);
    }

    #[test]
    fn merge_fails_on_missing_dependency() {
        // Slice Dog, which references Animal, but merge into a fresh
        // model that lacks Animal. Should error cleanly.
        let mut model = init_bootstrap_model();
        let sf_animal = parse("Class zoo::Animal { name: String[1]; }", "a.pure");
        let sf_dog = parse("Class kennel::Dog extends zoo::Animal {}", "d.pure");
        let (_r1, _e1) = compile_repo_slice(&mut model, &[sf_animal], &[], &[]);
        let (_r2, _e2) = compile_repo_slice(&mut model, &[sf_dog], &[], &[]);

        let dog_slice = slice_by_repo(&model, 2..3);

        // Merge into a model that lacks Animal — should fail.
        let mut fresh = init_bootstrap_model();
        let err = merge_slice(&mut fresh, dog_slice).expect_err("should fail");
        match err {
            SliceError::UnresolvedExternal { fqn } => {
                assert!(fqn.contains("Animal"), "expected Animal in fqn, got {fqn}");
            }
        }
    }
}
