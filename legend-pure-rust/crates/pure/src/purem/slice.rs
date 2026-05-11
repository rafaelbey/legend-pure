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

use std::collections::{HashMap, HashSet};
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
    slice_by_repo_with_filter(model, chunk_range, None)
}

/// Partition a compiled [`PureModel`] by chunk-id range, optionally
/// keeping only the elements in `include`.
///
/// When `include` is `None`, every element in `chunk_range` is retained
/// and the slice matches what [`slice_by_repo`] produces. When `include`
/// is `Some(set)`, only elements whose [`ElementId::InstanceId`] is in
/// the set survive; everything else is dropped from the slice. References
/// inside survivors that point at dropped elements get rewritten to
/// external FQN sentinels — the dual-slice path uses
/// [`crate::purem::filter::assert_no_dangling_refs`] to confirm the
/// partition is consistent before writing.
///
/// Internal references between survivors are rebased to slice-local
/// `(chunk_id, local_idx)`. External references (across `chunk_range`)
/// follow the same FQN-sentinel path as the wide-cover overload.
#[must_use]
pub fn slice_by_repo_with_filter(
    model: &PureModel,
    chunk_range: Range<u16>,
    include: Option<&HashSet<ElementId>>,
) -> PureModelSlice {
    let fqn_index = build_fqn_index(model);
    let mut external_refs: Vec<FqnPath> = Vec::new();

    // Build the survivor remap (orig ElementId → slice-local ElementId)
    // as we clone each chunk's nodes/elements. Empty-but-present chunks
    // are kept so the slice's chunk count matches the source range; this
    // preserves the existing invariant that `source_chunk_range.len() ==
    // slice.chunks.len()`.
    let mut remap: HashMap<ElementId, (u16, u32)> =
        HashMap::with_capacity(estimate_survivor_count(model, &chunk_range, include));
    let mut translated_chunks: Vec<ModelChunk> = Vec::with_capacity(chunk_range.len());

    for source_chunk_id in chunk_range.start..chunk_range.end {
        let source_chunk = &model.chunks[source_chunk_id as usize];
        let slice_local_chunk_id = source_chunk_id - chunk_range.start;
        let mut new_chunk = ModelChunk::new(slice_local_chunk_id);
        let len = source_chunk.elements.len();
        for orig_local_idx in 0..len {
            let orig_eid = ElementId::InstanceId {
                chunk_id: source_chunk_id,
                local_idx: orig_local_idx,
            };
            let kept = match include {
                Some(set) => set.contains(&orig_eid),
                None => true,
            };
            if !kept {
                continue;
            }
            let new_idx = new_chunk.alloc_element(
                source_chunk.nodes.get(orig_local_idx).clone(),
                source_chunk.elements.get(orig_local_idx).clone(),
            );
            remap.insert(orig_eid, (slice_local_chunk_id, new_idx));
        }
        translated_chunks.push(new_chunk);
    }

    // Translate every ElementId in every survivor. Internal refs get
    // the (slice_chunk_id, new_local_idx) from `remap`; refs to dropped
    // elements, refs into chunks outside `chunk_range`, AND every
    // `ElementId::Package(_)` reference get an external FQN sentinel —
    // PackageIds are global-arena indices that never survive a slice
    // round-trip into a different model where the package-allocation
    // order can drift.
    //
    // The merger uses `resolve_package_or_element` to recover the right
    // variant: an FQN that resolves to an element-in-package becomes
    // `InstanceId`; an FQN that resolves to a package node becomes
    // `Package(_)`.
    for chunk in &mut translated_chunks {
        let len = chunk.elements.len();
        for local_idx in 0..len {
            let element = chunk.elements.get_mut(local_idx);
            walk_element_ids(element, |id: &mut ElementId| match *id {
                ElementId::InstanceId {
                    chunk_id,
                    local_idx: lidx,
                } => {
                    let original = ElementId::InstanceId {
                        chunk_id,
                        local_idx: lidx,
                    };
                    if let Some(&(sc, ni)) = remap.get(&original) {
                        *id = ElementId::InstanceId {
                            chunk_id: sc,
                            local_idx: ni,
                        };
                    } else {
                        let fqn = fqn_index
                            .get(&original)
                            .cloned()
                            .unwrap_or_else(|| super::fqn_path::element_fqn_path(model, original));
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
                ElementId::Package(pkg_id) => {
                    let fqn = package_path(model, pkg_id);
                    let ref_idx = external_refs.len();
                    external_refs.push(fqn);
                    #[allow(clippy::cast_possible_truncation)]
                    let ref_idx_u32 = ref_idx as u32;
                    *id = ElementId::InstanceId {
                        chunk_id: EXTERNAL_REF_SENTINEL,
                        local_idx: ref_idx_u32,
                    };
                }
            });
        }
    }

    // Capture parent_package paths from the source model (the cloned
    // node still carries the source PackageId), then tombstone. Walks the
    // survivor's source location via the remap key.
    let mut element_packages: Vec<Vec<FqnPath>> = Vec::with_capacity(translated_chunks.len());
    for (slice_chunk_idx, chunk) in translated_chunks.iter_mut().enumerate() {
        let n_nodes = chunk.nodes.len();
        let mut paths: Vec<FqnPath> = Vec::with_capacity(n_nodes as usize);
        for local_idx in 0..n_nodes {
            let node = chunk.nodes.get_mut(local_idx);
            let path = package_path(model, node.parent_package);
            paths.push(path);
            node.parent_package = crate::ids::PackageId(0);
        }
        element_packages.push(paths);
        let _ = slice_chunk_idx;
    }

    PureModelSlice {
        source_chunk_range: chunk_range,
        chunks: translated_chunks,
        external_refs,
        element_packages,
    }
}

/// Resolve an FQN path, preferring an element-in-package match but
/// falling back to a pure-package walk if no element with that name
/// lives in the parent package.
///
/// Returns `Some(ElementId::InstanceId(_))` for elements registered in
/// `pkg.children_elements`, `Some(ElementId::Package(_))` for paths
/// that name a package node directly (including the root for an empty
/// path), or `None` if no segment matches.
fn resolve_package_or_element(model: &PureModel, path: &[smol_str::SmolStr]) -> Option<ElementId> {
    if path.is_empty() {
        return Some(ElementId::Package(model.root_package));
    }
    if let Some(eid) = model.resolve_by_path(path) {
        return Some(eid);
    }
    // Fall back: walk the package tree exactly, returning the leaf as
    // an `ElementId::Package`. This is the path the slice writer takes
    // for `ExprKind::PackageableElementRef { element: Package(_) }`.
    let mut current = model.root_package;
    for segment in path {
        let pkg = model.get_package(current);
        let next = pkg
            .children_packages
            .iter()
            .find(|&&cid| model.global_packages.get(cid.0).name == *segment)
            .copied()?;
        current = next;
    }
    Some(ElementId::Package(current))
}

fn estimate_survivor_count(
    model: &PureModel,
    chunk_range: &Range<u16>,
    include: Option<&HashSet<ElementId>>,
) -> usize {
    if let Some(set) = include {
        return set.len();
    }
    let mut total = 0usize;
    for cid in chunk_range.start..chunk_range.end {
        total += model.chunks[cid as usize].elements.len() as usize;
    }
    total
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
    let mut chunks = slice.chunks;
    let element_packages = slice.element_packages;

    // Step 1: pre-create every package this slice needs so that
    // package-typed external refs resolve.
    //
    // The slice's `element_packages` lists the parent-package FQN of
    // every survivor; instantiating those up front guarantees that any
    // `Package(_)` reference embedded in a body can resolve via
    // `resolve_package_or_element` even though it's a self-reference
    // into the slice's own scope (e.g. `meta::pure::functions::meta::tests`
    // appears in a test body before any element is registered there).
    for chunk_paths in &element_packages {
        for path in chunk_paths {
            if !path.is_empty() {
                model.get_or_create_package(path);
            }
        }
    }

    // Step 2: pre-resolve every external FQN to the running model's
    // ElementId. The slice writer collapses element-in-package refs and
    // pure-package refs into the same `external_refs` table; we try
    // element resolution first and fall back to a package walk if that
    // misses. Bailing early on the first miss gives the cleanest
    // diagnostic.
    let mut resolved_external: Vec<ElementId> = Vec::with_capacity(slice.external_refs.len());
    for path in &slice.external_refs {
        let id = resolve_package_or_element(model, path).ok_or_else(|| {
            SliceError::UnresolvedExternal {
                fqn: fqn_path_to_string(path),
            }
        })?;
        resolved_external.push(id);
    }

    // Step 3: rewrite ElementIds in-place — internal chunk ids get
    // bumped by `chunk_offset`; external sentinels get swapped for the
    // resolved ID.

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
        let finalize_errs = finalize_model(&mut fresh, &[], &[]);
        assert!(finalize_errs.is_empty(), "{finalize_errs:?}");

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
    fn slice_with_filter_drops_excluded_elements() {
        // Compile two classes in one chunk; slice with a filter that
        // keeps only one. The dropped class must vanish from the slice's
        // chunks and any reference to it must surface as an external
        // sentinel (which the dangling-ref gate would later flag).
        let mut model = init_bootstrap_model();
        let sf = parse(
            "Class repo_a::Keep { x: Integer[1]; }\n\
             Class repo_a::Drop { y: String[1]; }",
            "two.pure",
        );
        let chunks_before = model.chunks.len();
        let (_range, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        #[allow(clippy::cast_possible_truncation)]
        let user_range: Range<u16> = (chunks_before as u16)..(model.chunks.len() as u16);
        assert_eq!(user_range.end - user_range.start, 1);

        let keep_id = model
            .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Keep")])
            .expect("Keep resolves");
        // Confirm Drop exists in the pre-slice model — we'll assert it
        // disappears from the slice below.
        let _ = model
            .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Drop")])
            .expect("Drop resolves");

        let mut include: HashSet<ElementId> = HashSet::new();
        include.insert(keep_id);
        let slice = slice_by_repo_with_filter(&model, user_range.clone(), Some(&include));

        // Slice retains exactly one element across the range's one chunk.
        let total_elements: u32 = slice.chunks.iter().map(|c| c.elements.len()).sum();
        assert_eq!(total_elements, 1);

        // Dropped Drop class is not present in the slice's chunks.
        let mut found_drop_name = false;
        for chunk in &slice.chunks {
            for (idx, _) in chunk.nodes.iter() {
                if chunk.nodes.get(idx).name.as_str() == "Drop" {
                    found_drop_name = true;
                }
            }
        }
        assert!(!found_drop_name, "Drop should be excluded from slice");

        // Round-trip the slice into a fresh model — Keep resolves, Drop
        // does not.
        let mut fresh = init_bootstrap_model();
        merge_slice(&mut fresh, slice).expect("merge with filtered slice");
        assert!(
            fresh
                .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Keep")])
                .is_some()
        );
        assert!(
            fresh
                .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Drop")])
                .is_none()
        );
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
