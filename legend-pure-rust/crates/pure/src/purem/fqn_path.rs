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

//! Translate between [`ElementId`] / [`PackageId`] and FQN string paths.
//!
//! At slice time, every cross-repo `ElementId` reference must be encoded
//! as an FQN string so it can survive a round-trip into a fresh
//! [`PureModel`] (where arena indices are necessarily different). This
//! module is the one place that conversion lives.

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::ids::{ElementId, PackageId};
use crate::model::PureModel;

/// FQN segments for an element: `["meta", "pure", "metamodel", "Type"]`.
///
/// The last segment is the element's own name; preceding segments form the
/// package path. Empty `Vec` means the root-level (no package) — only
/// possible for elements registered directly on the root package.
pub type FqnPath = Vec<SmolStr>;

/// Compute the FQN path for an [`ElementId`] in the given model.
///
/// The path includes the element's name as its final segment.
///
/// **Canonical-package preference.** `ElementNode.parent_package` is set
/// to the package the element was *originally allocated under*, which for
/// the M3 bootstrap is always the root. The same elements are then
/// registered (via [`crate::model::PureModel::register_element`]) under
/// their canonical M3 packages — `meta::pure::metamodel::*`. For
/// serialization-stable FQNs we want the canonical path, so this function
/// scans the package tree for the longest path that contains the element
/// and returns that. Falls back to the `parent_package` walk only when
/// the element is registered in exactly one package (the common case for
/// user-defined elements).
///
/// # Panics
///
/// Panics if the `ElementId` is out of range.
#[must_use]
pub fn element_fqn_path(model: &PureModel, id: ElementId) -> FqnPath {
    if let ElementId::Package(pkg_id) = id {
        return package_path(model, pkg_id);
    }

    // Scan every package for one whose children_elements contains this
    // ElementId. Prefer the longest (most-specific) path. This handles
    // M3 bootstrap aliases — `Class` is registered both at the root and
    // at `meta::pure::metamodel::type` — by always picking the canonical
    // M3 path. For user-defined elements there's exactly one match.
    let node = model.get_node(id);
    let mut best: Option<FqnPath> = None;
    for pkg_idx in 0..model.global_packages.len() {
        let pkg_id = PackageId(pkg_idx);
        let pkg = model.get_package(pkg_id);
        if !pkg.children_elements.contains(&id) {
            continue;
        }
        let mut path = package_path(model, pkg_id);
        path.push(node.name.clone());
        match &best {
            None => best = Some(path),
            Some(cur) if path.len() > cur.len() => best = Some(path),
            _ => {}
        }
    }

    best.unwrap_or_else(|| {
        // Element not registered in any package (defensive fallback).
        // Fall back to parent_package walk + element name.
        let mut path = package_path(model, node.parent_package);
        path.push(node.name.clone());
        path
    })
}

/// Build a lookup table from [`ElementId`] to its canonical FQN path.
///
/// O(packages × children) build cost. Use this when serializing many
/// external references — amortizes the per-element scan in
/// [`element_fqn_path`].
#[must_use]
pub fn build_fqn_index(model: &PureModel) -> HashMap<ElementId, FqnPath> {
    let mut out: HashMap<ElementId, FqnPath> = HashMap::new();
    for pkg_idx in 0..model.global_packages.len() {
        let pkg_id = PackageId(pkg_idx);
        let pkg = model.get_package(pkg_id);
        let pkg_path = package_path(model, pkg_id);
        for &child in &pkg.children_elements {
            let mut path = pkg_path.clone();
            path.push(model.element_name(child).clone());
            // Prefer the longest path (canonical M3 over root aliases).
            match out.get(&child) {
                Some(existing) if existing.len() >= path.len() => {}
                _ => {
                    out.insert(child, path);
                }
            }
        }
    }
    out
}

/// Compute the FQN path for a [`PackageId`].
///
/// Returns an empty `Vec` for the root package; otherwise the segment
/// names from the root down to the target package.
#[must_use]
pub fn package_path(model: &PureModel, pkg_id: PackageId) -> FqnPath {
    let mut segments: Vec<SmolStr> = Vec::new();
    let mut cur = Some(pkg_id);
    while let Some(p) = cur {
        let pkg = model.get_package(p);
        match pkg.parent {
            Some(parent_id) => {
                segments.push(pkg.name.clone());
                cur = Some(parent_id);
            }
            None => {
                // Root package — its name is "" and we don't push it.
                cur = None;
            }
        }
    }
    segments.reverse();
    segments
}

/// Look up an FQN path against the model's package tree → element id.
///
/// Returns `None` if the path is empty (refers to the root package, which
/// is not a real element) or if any segment fails to resolve.
#[must_use]
pub fn resolve_fqn_path(model: &PureModel, path: &[SmolStr]) -> Option<ElementId> {
    if path.is_empty() {
        return None;
    }
    model.resolve_by_path(path)
}

/// Render an FQN path back to its `::`-separated string form (for error
/// messages and debug output).
#[must_use]
pub fn fqn_path_to_string(path: &[SmolStr]) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        if i > 0 {
            out.push_str("::");
        }
        out.push_str(seg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::init_bootstrap_model;

    #[test]
    fn package_path_for_root_is_empty() {
        let model = init_bootstrap_model();
        let path = package_path(&model, model.root_package);
        assert!(path.is_empty());
    }

    #[test]
    fn element_fqn_path_for_string_primitive() {
        let model = init_bootstrap_model();
        let string_id = model
            .resolve_by_path(&[SmolStr::new("String")])
            .expect("String primitive should resolve");
        let path = element_fqn_path(&model, string_id);
        assert_eq!(path, vec![SmolStr::new("String")]);
    }

    #[test]
    fn fqn_path_round_trips_through_resolve() {
        let model = init_bootstrap_model();
        let m3_class_id = model
            .resolve_by_path(&[
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("type"),
                SmolStr::new("Class"),
            ])
            .expect("M3 Class should resolve after bootstrap");

        let path = element_fqn_path(&model, m3_class_id);
        assert_eq!(
            path,
            vec![
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("type"),
                SmolStr::new("Class"),
            ]
        );

        let back = resolve_fqn_path(&model, &path).expect("path should round-trip");
        assert_eq!(back, m3_class_id);
    }

    #[test]
    fn fqn_path_string_form() {
        let path = vec![
            SmolStr::new("meta"),
            SmolStr::new("pure"),
            SmolStr::new("Class"),
        ];
        assert_eq!(fqn_path_to_string(&path), "meta::pure::Class");
    }
}
