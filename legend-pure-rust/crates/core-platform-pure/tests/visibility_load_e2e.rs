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

//! End-to-end visibility test through the public `repo::load` path.
//!
//! Builds two filesystem repos with crafted descriptor JSONs (one
//! depending on the other in the **opposite** direction of the
//! cross-reference written in source), runs `repo::load`, and asserts
//! that a `NotVisible` error fires. Closes the gap left by the in-pure
//! smoke tests, which call `compile_repo_slice` directly and could
//! mask a bug in `populate_repo_visibility`.

use std::fs;
use std::path::Path;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_parser_pure::error::CompilationErrorKind;

fn write_descriptor(dir: &Path, name: &str, deps: &[&str]) {
    let dependencies = deps
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let json = format!(
        "{{\n  \"name\": \"{name}\",\n  \"pattern\": \".*\",\n  \"dependencies\": [{dependencies}]\n}}\n"
    );
    fs::write(dir.join(format!("{name}.json")), json).expect("write descriptor");
}

fn write_source(repo_root: &Path, rel: &str, content: &str) {
    let target = repo_root.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).expect("mkdirs");
    }
    fs::write(target, content).expect("write source");
}

#[test]
fn repo_load_emits_not_visible_for_undeclared_cross_repo_ref() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Repo "model": defines model::domain::TestClass2.
    let model_root = root.join("model");
    fs::create_dir_all(&model_root).expect("mkdirs");
    write_source(
        &model_root,
        "TestClass2.pure",
        "Class model::domain::TestClass2 {}\n",
    );
    write_descriptor(root, "model", &[]);

    // Repo "system": extends model::domain::TestClass2 but declares no deps.
    let system_root = root.join("system");
    fs::create_dir_all(&system_root).expect("mkdirs");
    write_source(
        &system_root,
        "Sub.pure",
        "Class system::pkg::Sub extends model::domain::TestClass2 {}\n",
    );
    write_descriptor(root, "system", &[]);

    // Build repos via the production from_descriptor path.
    let model_repo = Repo::from_descriptor(&root.join("model.json")).expect("model from desc");
    let system_repo = Repo::from_descriptor(&root.join("system.json")).expect("system from desc");

    // load() topo-sorts and finalize-validates. Expect the NotVisible
    // error in the partial errors list.
    let result = repo::load(&[model_repo, system_repo], &[]);
    let errors = match result {
        Ok(_) => panic!("expected partial model with NotVisible error, got Ok"),
        Err(p) => p.errors,
    };

    let hits: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::NotVisible { .. }))
        .collect();
    assert!(
        hits.iter().any(|h| h.message
            == "model::domain::TestClass2 is not visible in the file /system/Sub.pure"),
        "expected NotVisible diagnostic; got: {errors:?}"
    );
}

#[test]
fn repo_load_passes_when_dependency_declared() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let model_root = root.join("model");
    fs::create_dir_all(&model_root).expect("mkdirs");
    write_source(
        &model_root,
        "TestClass2.pure",
        "Class model::domain::TestClass2 {}\n",
    );
    write_descriptor(root, "model", &[]);

    let system_root = root.join("system");
    fs::create_dir_all(&system_root).expect("mkdirs");
    write_source(
        &system_root,
        "Sub.pure",
        "Class system::pkg::Sub extends model::domain::TestClass2 {}\n",
    );
    // System now declares "model" as a dependency.
    write_descriptor(root, "system", &["model"]);

    let model_repo = Repo::from_descriptor(&root.join("model.json")).expect("model from desc");
    let system_repo = Repo::from_descriptor(&root.join("system.json")).expect("system from desc");

    let errors = match repo::load(&[model_repo, system_repo], &[]) {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };

    let hits: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::NotVisible { .. }))
        .collect();
    assert!(
        hits.is_empty(),
        "declared dependency should suppress NotVisible; got: {hits:?}"
    );
}
