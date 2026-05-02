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

//! Parity test: `Repo::default_embedded()` vs. a hybrid configuration
//! using `Repo::from_descriptor` for the `platform` repo.
//!
//! The two configurations must produce semantically equivalent
//! `PureModel`s — same set of qualified element names, same error
//! count. ElementId equality is *not* asserted (arena allocation order
//! can differ across file-iteration orders).

use std::collections::BTreeSet;
use std::path::PathBuf;

use legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS;
use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

fn auto_imports() -> Vec<SmolStr> {
    PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect()
}

/// Resolve the descriptor path used by build.rs from `CARGO_MANIFEST_DIR`,
/// returning `None` if the slim checkout doesn't include the Java tree.
fn platform_descriptor() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir
        .join("../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json");
    candidate.canonicalize().ok().filter(|p| p.is_file())
}

fn collect_qualified_names(model: &PureModel) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for chunk in &model.chunks {
        for node in chunk.nodes.values() {
            names.insert(node.name.to_string());
        }
    }
    names
}

#[test]
fn embedded_and_filesystem_produce_equivalent_models() {
    let Some(descriptor) = platform_descriptor() else {
        eprintln!(
            "skipping repo_parity: descriptor path not found \
             (slim checkout — no Java tree). This is expected on \
             distribution-only builds."
        );
        return;
    };

    let imports = auto_imports();

    // Path A: every repo embedded (the default).
    let embedded_repos = Repo::default_embedded();
    let model_embedded = match repo::load(&embedded_repos, &imports) {
        Ok(m) => m,
        Err(p) => p.model,
    };
    let embedded_errors = repo::load(&embedded_repos, &imports)
        .err()
        .map_or(0, |p| p.errors.len());

    // Path B: filesystem `platform` + embedded DSL repos.
    let fs_platform = Repo::from_descriptor(&descriptor)
        .expect("Repo::from_descriptor on real platform.json should succeed");
    let hybrid_repos: Vec<Repo> = vec![
        fs_platform,
        Repo::embedded_platform_precise_primitives(),
        Repo::embedded_platform_dsl_store(),
        Repo::embedded_platform_dsl_mapping(),
        Repo::embedded_platform_dsl_diagram(),
        Repo::embedded_platform_dsl_graph(),
        Repo::embedded_platform_dsl_tds(),
    ];

    let model_hybrid = match repo::load(&hybrid_repos, &imports) {
        Ok(m) => m,
        Err(p) => p.model,
    };
    let hybrid_errors = repo::load(&hybrid_repos, &imports)
        .err()
        .map_or(0, |p| p.errors.len());

    // Equivalence: same set of element names, same error count.
    let embedded_names = collect_qualified_names(&model_embedded);
    let hybrid_names = collect_qualified_names(&model_hybrid);

    let only_embedded: Vec<_> = embedded_names.difference(&hybrid_names).take(5).collect();
    let only_hybrid: Vec<_> = hybrid_names.difference(&embedded_names).take(5).collect();

    assert_eq!(
        embedded_names.len(),
        hybrid_names.len(),
        "element-count diverged: embedded={} hybrid={}\n  only-embedded (first 5): {only_embedded:?}\n  only-hybrid (first 5): {only_hybrid:?}",
        embedded_names.len(),
        hybrid_names.len()
    );
    assert_eq!(embedded_names, hybrid_names, "element-name set diverged");
    assert_eq!(
        embedded_errors, hybrid_errors,
        "error count diverged: embedded={embedded_errors} hybrid={hybrid_errors}"
    );
}

#[test]
fn from_descriptor_loads_real_platform_meta() {
    let Some(descriptor) = platform_descriptor() else {
        return;
    };
    let repo = Repo::from_descriptor(&descriptor).expect("from_descriptor");
    let meta = repo.meta().expect("descriptor-loaded repo carries meta");
    assert_eq!(meta.name, "platform");
    assert!(meta.dependencies.is_empty());
    assert!(repo.prefix() == "/platform");

    // Sanity: m3.pure is skipped, but a sibling .pure file exists.
    let has_m3 = repo
        .files()
        .any(|(_, p)| p == "/platform/pure/grammar/m3.pure");
    assert!(!has_m3, "m3.pure must be skipped");

    let any_pure = repo
        .files()
        .any(|(_, p)| p.starts_with("/platform/pure/") && p.ends_with(".pure"));
    assert!(
        any_pure,
        "expected at least one /platform/pure/...pure file from filesystem walk"
    );
}
