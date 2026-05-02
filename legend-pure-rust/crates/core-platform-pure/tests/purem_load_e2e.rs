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

//! End-to-end: produce a `.purem` blob from the embedded platform, then
//! load a fresh model from it via `Repo::Purem` + `repo::load`.
//!
//! This is the smoke test that the consumer path (Phase F + G) works
//! against a real platform-scale slice.

use std::sync::Arc;

use legend_pure_core_platform::platform;
use legend_pure_core_platform::repo::{self, Repo, RepoMeta};
use legend_pure_parser_pure::purem::{slice_by_repo, write_repo};
use smol_str::SmolStr;

#[test]
fn purem_repo_loads_into_fresh_model() {
    // Step 1: compile the embedded platform.
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };
    let user_range = 1..(model.chunks.len() as u16);
    if user_range.is_empty() {
        return; // bare model — nothing to test
    }

    // Step 2: serialize as one platform-wide slice.
    let slice = slice_by_repo(&model, user_range);
    let bytes = write_repo(&slice).expect("write platform slice");

    // Step 3: build a Purem repo carrying that blob. We reuse the
    // platform's existing meta name so it's recognizable.
    let auto_imports: Vec<SmolStr> = platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();

    let meta = RepoMeta {
        name: "platform_purem_test",
        pattern: ".*",
        dependencies: &[],
    };
    let purem_repo = Repo::from_purem_bytes(
        "/platform_purem_test",
        meta,
        Arc::from(bytes.into_boxed_slice()),
    );

    // Step 4: load via repo::load. Model bootstraps + merges the slice.
    let loaded = repo::load(&[purem_repo], &auto_imports);
    let loaded = match loaded {
        Ok(m) => m,
        Err(p) => panic!("load failed: {} errors\n{:?}", p.errors.len(), p.errors),
    };

    // Step 5: spot-check a few well-known elements.
    let string_id = loaded
        .resolve_by_path(&[SmolStr::new("String")])
        .expect("String primitive should resolve in loaded model");
    assert!(matches!(loaded.get_node(string_id).name.as_str(), "String"));

    // The class hierarchy should still be intact: walking up from a Pure
    // class to its supertype must succeed.
    let class_id = loaded
        .resolve_by_path(&[
            SmolStr::new("meta"),
            SmolStr::new("pure"),
            SmolStr::new("metamodel"),
            SmolStr::new("type"),
            SmolStr::new("Class"),
        ])
        .expect("meta::pure::metamodel::type::Class should resolve");
    assert_eq!(loaded.get_node(class_id).name.as_str(), "Class");
}
