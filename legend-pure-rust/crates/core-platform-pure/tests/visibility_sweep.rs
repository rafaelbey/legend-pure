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

//! Platform-compile safety gate for repo-boundary visibility.
//!
//! Walks every cross-repo `ElementId` reference produced by a real
//! `load_platform()` run and asserts that the target's home repo is in
//! the use-site repo's declared dependencies. If this test ever fails,
//! either the platform has a real layering bug or a descriptor's
//! `dependencies` list is missing a required entry — fix the source of
//! truth, don't relax the gate.

use legend_pure_core_platform::platform;
use legend_pure_core_platform::repo;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::Element;
use legend_pure_parser_pure::purem::walk::walk_element_ids;
use legend_pure_parser_pure::visibility::{check_element_visible, source_repo_name};

#[test]
fn embedded_platform_has_zero_visibility_violations() {
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    assert!(
        !model.repo_visibility.is_empty(),
        "load_platform should populate repo_visibility from descriptor metadata"
    );

    // Walk every chunk's elements (skip chunk 0 — bootstrap is repo-less).
    let mut violations: Vec<String> = Vec::new();
    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, _element) in chunk.elements.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let node = chunk.nodes.get(local_idx);
            let use_site = &node.source_info.source;

            // Skip elements whose source has no extractable repo
            // (defensive — bootstrap is filtered above).
            if source_repo_name(use_site).is_none() {
                continue;
            }

            // Clone the element so we can use the mutable visitor without
            // touching the live model.
            let mut owned: Element = chunk.elements.get(local_idx).clone();
            walk_element_ids(&mut owned, |target| {
                if let Some(violation) = check_element_visible(&model, use_site, *target) {
                    violations.push(format!(
                        "use_site={use_site} (element {fqn}) → target={target_fqn}",
                        fqn = node.name,
                        target_fqn = violation.target_fqn,
                    ));
                }
                let _ = id; // suppress unused-id warning when no violation
            });
        }
    }

    assert!(
        violations.is_empty(),
        "platform should be visibility-clean; got {} violation(s):\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

#[test]
fn descriptor_visibility_map_matches_meta() {
    // Sanity: visible-set always contains the repo itself.
    let repos = repo::Repo::default_embedded();
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    for repo in &repos {
        let Some(meta) = repo.meta() else { continue };
        let visible = model
            .repo_visibility
            .get(meta.name)
            .unwrap_or_else(|| panic!("missing visibility entry for {}", meta.name));
        assert!(
            visible.contains(meta.name),
            "{} should be in its own visible set",
            meta.name
        );
        for dep in meta.dependencies {
            assert!(
                visible.contains(*dep),
                "{} should declare {} as visible",
                meta.name,
                dep
            );
        }
    }
}
