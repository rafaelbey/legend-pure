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

//! Test-element / production-element partition for `.purem` slicing.
//!
//! Computes a per-element split that lets the snapshot-builder emit two
//! complementary `.purem` blobs per repo: a production blob with zero
//! test code, plus a tests blob carrying the rest. The two combined
//! cover every non-bootstrap element exactly once.
//!
//! # Formulation
//!
//! ```text
//! T              = elements stereotyped <<test.*>> or <<PCT.test>>
//! ScopedPrivate  = elements in a `tests::*` package WITH <<access.private>>
//! Candidates     = T ∪ ScopedPrivate
//! NonTestRoots   = (AllElements \ chunk0) \ Candidates
//! Reachable      = forward-walk(NonTestRoots) over walk_element_ids
//! prod_slice     = Reachable
//! test_slice     = (AllElements \ chunk0) \ Reachable
//! ```
//!
//! Bootstrap chunk 0 is skipped entirely — M3 metaclasses are never
//! test-tagged and removing one corrupts every model that loads the
//! resulting blob.
//!
//! **Why no broad `tests::*` package heuristic.** A previous version
//! pulled every element under a `tests::*` package into the test
//! candidate set; that turned out unsafe because functions and helper
//! classes can be referenced from external code (other repos, user
//! Pure source) we have no visibility over. We can only confidently
//! strip:
//!
//! 1. Elements explicitly stereotyped as tests (`<<test.Test>>`,
//!    `<<test.TestCollection>>`, `<<test.BeforePackage>>`,
//!    `<<test.AfterPackage>>`, `<<test.ToFix>>`, `<<PCT.test>>`).
//! 2. Elements in a `tests::*` package whose author explicitly marked
//!    them `<<access.private>>` — that stereotype is a contract that
//!    the element is package-scope and not consumed externally.
//!
//! Helper classes / fixtures that lack `access.private` stay in prod
//! to be safe. The real win is from (1) — test functions are the bulk
//! of code volume in test-heavy repos.
//!
//! Test elements that are *referenced from* non-test code (e.g. a
//! production function that calls a test fn — rare but possible) land
//! in `Reachable` and therefore in the prod slice, so the production
//! blob never carries dangling references.

use std::collections::HashSet;

use smol_str::SmolStr;
use thiserror::Error;

use crate::ids::{ElementId, PackageId};
use crate::model::{Element, PureModel};

use super::fqn_path::{build_fqn_index, fqn_path_to_string, package_path};
use super::slice::{EXTERNAL_REF_SENTINEL, PureModelSlice};
use super::walk::walk_element_ids;

/// Stereotype names on the `meta::pure::profiles::test` profile that
/// indicate a runtime test element.
const TEST_STEREOTYPE_NAMES: &[&str] = &[
    "Test",
    "TestCollection",
    "BeforePackage",
    "AfterPackage",
    "ToFix",
];

/// Package segment that conventionally marks test-only code in the
/// platform sources. We require BOTH this segment AND the
/// `<<access.private>>` stereotype before stripping a non-test-tagged
/// element.
const TEST_PACKAGE_SEGMENT: &str = "tests";

/// FQN of the standard test profile.
const TEST_PROFILE_FQN: &[&str] = &["meta", "pure", "profiles", "test"];

/// FQN of the PCT framework profile.
const PCT_PROFILE_FQN: &[&str] = &["meta", "pure", "test", "pct", "PCT"];

/// FQN of the access-modifier profile. `<<access.private>>` is the
/// only stereotype on this profile that signals "scope-private; safe
/// to strip if otherwise unreferenced".
const ACCESS_PROFILE_FQN: &[&str] = &["meta", "pure", "profiles", "access"];

/// Stereotype value on the PCT profile that marks an executable PCT
/// test (as opposed to `<<PCT.adapter>>`, `<<PCT.function>>`, etc.).
const PCT_TEST_STEREOTYPE: &str = "test";

/// Stereotype value on the access profile that confirms an element is
/// scope-private. The slice writer will move such an element to the
/// tests slice if it also lives under a `tests::*` package.
const PRIVATE_ACCESS_STEREOTYPE: &str = "private";

/// Two disjoint sets covering every non-bootstrap element of a
/// [`PureModel`]: those that survive in the production slice, and those
/// that are split off into the tests slice.
#[derive(Debug, Default, Clone)]
pub struct TestPartition {
    /// Elements that the production slice retains. Equals
    /// `forward-walk(NonTestRoots)`.
    pub prod: HashSet<ElementId>,
    /// Elements that move to the tests slice. Equals
    /// `(AllElements \ chunk0) \ prod`.
    pub test: HashSet<ElementId>,
}

/// Errors raised by [`assert_no_dangling_refs`].
#[derive(Debug, Error)]
pub enum FilterError {
    /// A surviving slice element refers to an element that the partition
    /// dropped. Never expected with a correctly computed partition; means
    /// either the partition or the slice generator is broken.
    #[error("production slice references dropped element '{fqn}'")]
    DanglingExternal {
        /// FQN of the dropped element that the slice still pointed at.
        fqn: String,
    },
}

/// Walks the model and returns the prod / test partition.
///
/// Always skips chunk 0 (M3 bootstrap). If the model carries neither
/// `meta::pure::profiles::test` nor `meta::pure::test::pct::PCT` (e.g.
/// minimal test fixtures), `T` is computed purely via the package-segment
/// heuristic — most platform layouts still trip the heuristic so the
/// partition stays meaningful.
#[must_use]
pub fn collect_test_partition(model: &PureModel) -> TestPartition {
    let test_profile = model.resolve_by_path(&fqn_smol(TEST_PROFILE_FQN));
    let pct_profile = model.resolve_by_path(&fqn_smol(PCT_PROFILE_FQN));
    let access_profile = model.resolve_by_path(&fqn_smol(ACCESS_PROFILE_FQN));

    // Phase 1: stereotype-tagged candidates plus elements that pair
    // `<<access.private>>` with a `tests::*` package — that combination
    // is what gives us license to strip a non-test-tagged element. A
    // `tests::*` package alone is NOT enough because functions /
    // classes there might be referenced by external (non-platform)
    // code we can't see.
    let mut candidates: HashSet<ElementId> = HashSet::new();
    for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            let eid = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if is_test_stereotyped(element, test_profile, pct_profile) {
                candidates.insert(eid);
                continue;
            }
            if has_access_private(element, access_profile) {
                let parent_pkg = chunk.nodes.get(local_idx).parent_package;
                if package_has_test_segment(model, parent_pkg) {
                    candidates.insert(eid);
                }
            }
        }
    }

    // Phase 2: BFS forward walk from non-candidates. Every non-candidate
    // is its own root (production code keeps every element it declares,
    // even if no internal caller references it — these are exposed APIs).
    let mut reachable: HashSet<ElementId> = HashSet::new();
    let mut worklist: Vec<ElementId> = Vec::new();
    for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, _) in chunk.elements.iter() {
            let eid = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if !candidates.contains(&eid) && reachable.insert(eid) {
                worklist.push(eid);
            }
        }
    }

    while let Some(eid) = worklist.pop() {
        let element = model.get_element(eid);
        let mut copy = element.clone();
        walk_element_ids(&mut copy, |id: &mut ElementId| {
            if let ElementId::InstanceId { chunk_id, .. } = *id {
                if chunk_id == 0 || chunk_id == EXTERNAL_REF_SENTINEL {
                    return;
                }
                if reachable.insert(*id) {
                    worklist.push(*id);
                }
            }
        });
    }

    // Phase 3: derive the test set as the complement of reachable.
    let mut test: HashSet<ElementId> = HashSet::new();
    for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, _) in chunk.elements.iter() {
            let eid = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            if !reachable.contains(&eid) {
                test.insert(eid);
            }
        }
    }

    TestPartition {
        prod: reachable,
        test,
    }
}

/// Verify that no surviving element in `slice` references an element
/// that landed in `drop_set`.
///
/// `drop_set` should be the complement of the slice's include-set in the
/// original model — typically `partition.test` for a production slice or
/// `partition.prod` for a tests slice.
///
/// # Errors
///
/// Returns [`FilterError::DanglingExternal`] for the first surviving
/// reference whose target FQN resolves into the drop set.
pub fn assert_no_dangling_refs(
    model: &PureModel,
    slice: &PureModelSlice,
    drop_set: &HashSet<ElementId>,
) -> Result<(), FilterError> {
    if drop_set.is_empty() {
        return Ok(());
    }
    let fqn_index = build_fqn_index(model);
    let mut by_path: std::collections::HashMap<Vec<SmolStr>, ElementId> =
        std::collections::HashMap::with_capacity(fqn_index.len());
    for (id, path) in fqn_index {
        by_path.insert(path, id);
    }

    for path in &slice.external_refs {
        if let Some(id) = by_path.get(path)
            && drop_set.contains(id)
        {
            return Err(FilterError::DanglingExternal {
                fqn: fqn_path_to_string(path),
            });
        }
    }
    Ok(())
}

fn is_test_stereotyped(
    element: &Element,
    test_profile: Option<ElementId>,
    pct_profile: Option<ElementId>,
) -> bool {
    let stereotypes = match element {
        Element::Function(f) => &f.stereotypes[..],
        Element::Class(c) => &c.stereotypes[..],
        Element::Association(a) => &a.stereotypes[..],
        Element::Enumeration(e) => &e.stereotypes[..],
        // Profile / Measure / Unit / PrimitiveType / PackageableMultiplicity / Package
        // do not carry the test stereotype in any platform source; skip.
        _ => return false,
    };
    for s in stereotypes {
        if let Some(tp) = test_profile
            && s.profile == tp
            && TEST_STEREOTYPE_NAMES.iter().any(|n| n == &s.value.as_str())
        {
            return true;
        }
        if let Some(pp) = pct_profile
            && s.profile == pp
            && s.value.as_str() == PCT_TEST_STEREOTYPE
        {
            return true;
        }
    }
    false
}

fn package_has_test_segment(model: &PureModel, pkg_id: PackageId) -> bool {
    let path = package_path(model, pkg_id);
    path.iter().any(|seg| seg.as_str() == TEST_PACKAGE_SEGMENT)
}

fn has_access_private(element: &Element, access_profile: Option<ElementId>) -> bool {
    let Some(ap) = access_profile else {
        return false;
    };
    let stereotypes: &[_] = match element {
        Element::Function(f) => &f.stereotypes,
        Element::Class(c) => &c.stereotypes,
        Element::Association(a) => &a.stereotypes,
        Element::Enumeration(e) => &e.stereotypes,
        _ => return false,
    };
    stereotypes
        .iter()
        .any(|s| s.profile == ap && s.value.as_str() == PRIVATE_ACCESS_STEREOTYPE)
}

fn fqn_smol(parts: &[&str]) -> Vec<SmolStr> {
    parts.iter().map(|s| SmolStr::new(*s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ElementId;
    use crate::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};

    fn parse(s: &str, p: &str) -> legend_pure_parser_ast::section::SourceFile {
        legend_pure_parser_parser::parse(s, p).expect("parse should succeed")
    }

    fn count_chunks_above_zero(model: &PureModel) -> usize {
        model.chunks.iter().filter(|c| c.chunk_id != 0).count()
    }

    #[test]
    fn partition_on_empty_model_is_empty() {
        let model = init_bootstrap_model();
        let part = collect_test_partition(&model);
        assert!(part.prod.is_empty());
        assert!(part.test.is_empty());
    }

    #[test]
    fn non_test_class_lands_in_prod() {
        let mut model = init_bootstrap_model();
        let sf = parse("Class repo_a::Foo { x: Integer[1]; }", "foo.pure");
        let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        let part = collect_test_partition(&model);
        let foo_id = model
            .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Foo")])
            .expect("Foo resolves");
        assert!(part.prod.contains(&foo_id));
        assert!(!part.test.contains(&foo_id));
    }

    #[test]
    fn class_in_tests_subpackage_without_access_private_stays_in_prod() {
        // `repo_a::tests::CO_Firm` lives under a `tests::*` subpackage
        // but carries no `<<access.private>>` stereotype. The heuristic
        // therefore leaves it in prod — we can't be certain external
        // callers don't reference it.
        let mut model = init_bootstrap_model();
        let sf = parse(
            "Class repo_a::tests::CO_Firm { name: String[1]; }",
            "co_firm.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        let part = collect_test_partition(&model);
        let firm_id = model
            .resolve_by_path(&[
                SmolStr::new("repo_a"),
                SmolStr::new("tests"),
                SmolStr::new("CO_Firm"),
            ])
            .expect("CO_Firm resolves");
        assert!(
            part.prod.contains(&firm_id),
            "CO_Firm must stay in prod without access.private"
        );
        assert!(!part.test.contains(&firm_id));
        assert_eq!(count_chunks_above_zero(&model), 1);
    }

    #[test]
    fn helper_referenced_by_prod_stays_in_prod() {
        // Even with `<<access.private>>`, a helper referenced from
        // non-test code lands in `Reachable` and stays in prod — that's
        // what keeps the production blob free of dangling refs.
        let mut model = init_bootstrap_model();
        // Declare the access profile inline (the bootstrap model
        // doesn't carry it).
        let access_sf = parse(
            "Profile meta::pure::profiles::access \
             { stereotypes: [public, protected, private, externalizable]; }",
            "access.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[access_sf], &[], &[]);
        assert!(errs.is_empty(), "access profile: {errs:?}");

        let sf = parse(
            "Class <<meta::pure::profiles::access.private>> repo_a::tests::Helper { x: Integer[1]; }\n\
             Class repo_a::Holder { h: repo_a::tests::Helper[1]; }",
            "both.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        let part = collect_test_partition(&model);
        let helper_id = model
            .resolve_by_path(&[
                SmolStr::new("repo_a"),
                SmolStr::new("tests"),
                SmolStr::new("Helper"),
            ])
            .expect("Helper resolves");
        let holder_id = model
            .resolve_by_path(&[SmolStr::new("repo_a"), SmolStr::new("Holder")])
            .expect("Holder resolves");
        assert!(part.prod.contains(&holder_id));
        assert!(
            part.prod.contains(&helper_id),
            "Helper must be promoted to prod since Holder references it"
        );
        assert!(!part.test.contains(&helper_id));
    }

    #[test]
    fn private_class_in_tests_pkg_lands_in_test() {
        // Class with `<<access.private>>` in a `tests::*` package and
        // no non-test reference → confirmed local-scope, drops to test
        // slice.
        let mut model = init_bootstrap_model();
        let access_sf = parse(
            "Profile meta::pure::profiles::access \
             { stereotypes: [public, protected, private, externalizable]; }",
            "access.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[access_sf], &[], &[]);
        assert!(errs.is_empty(), "access profile: {errs:?}");

        let sf = parse(
            "Class <<meta::pure::profiles::access.private>> repo_a::tests::CO_Firm \
             { name: String[1]; }",
            "co_firm.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        let part = collect_test_partition(&model);
        let firm_id = model
            .resolve_by_path(&[
                SmolStr::new("repo_a"),
                SmolStr::new("tests"),
                SmolStr::new("CO_Firm"),
            ])
            .expect("CO_Firm resolves");
        assert!(
            part.test.contains(&firm_id),
            "private CO_Firm under tests::* should drop to test slice"
        );
        assert!(!part.prod.contains(&firm_id));
    }

    #[test]
    fn partition_is_disjoint_and_exhaustive() {
        let mut model = init_bootstrap_model();
        let sf = parse(
            "Class repo_a::Foo { x: Integer[1]; }\n\
             Class repo_a::tests::Bar { y: String[1]; }",
            "mix.pure",
        );
        let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
        assert!(errs.is_empty(), "{errs:?}");
        finalize_model(&mut model, &[], &[]);
        let part = collect_test_partition(&model);

        // Every non-chunk-0 element is in exactly one half.
        let mut total_above_zero = 0usize;
        for chunk in &model.chunks {
            if chunk.chunk_id == 0 {
                continue;
            }
            for (local_idx, _) in chunk.elements.iter() {
                let eid = ElementId::InstanceId {
                    chunk_id: chunk.chunk_id,
                    local_idx,
                };
                let in_prod = part.prod.contains(&eid);
                let in_test = part.test.contains(&eid);
                assert!(in_prod ^ in_test, "{eid:?} should be in exactly one half");
                total_above_zero += 1;
            }
        }
        assert_eq!(total_above_zero, part.prod.len() + part.test.len());
    }
}
