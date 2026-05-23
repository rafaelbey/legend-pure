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

//! End-to-end repo `pattern` test through the public `repo::load` path.
//!
//! `pure/tests/repo_pattern_smoke.rs` exercises the validator with a
//! hand-populated `model.repo_patterns` map, which would miss a bug
//! where the load path forgot to populate the map. This test goes
//! through `from_descriptor` + `repo::load`, the same code path the
//! LSP and `legend check` use, so a regression that disables the
//! validator in production fails here.

use std::fs;
use std::path::Path;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_parser_pure::error::CompilationErrorKind;

fn write_descriptor(dir: &Path, name: &str, pattern: &str, deps: &[&str]) {
    let dependencies = deps
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let json = format!(
        "{{\n  \"name\": \"{name}\",\n  \"pattern\": \"{pattern}\",\n  \"dependencies\": [{dependencies}]\n}}\n"
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
fn repo_load_emits_pattern_violation_for_out_of_pattern_package() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let model_root = root.join("model");
    fs::create_dir_all(&model_root).expect("mkdirs");
    // Repo pattern only admits `model::*` packages; the declared class
    // lives in `abc::*`, so the validator must reject it.
    write_source(&model_root, "Bad.pure", "Class abc::Bad {}\n");
    write_descriptor(root, "model", "(model)(::.*)?", &[]);

    let model_repo = Repo::from_descriptor(&root.join("model.json")).expect("model from desc");

    let errors = match repo::load(&[model_repo], &[]) {
        Ok(_) => panic!("expected partial model with PackageNotInRepoPattern, got Ok"),
        Err(p) => p.errors,
    };

    let hits: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::PackageNotInRepoPattern { .. }))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one PackageNotInRepoPattern; got: {errors:?}"
    );
    let CompilationErrorKind::PackageNotInRepoPattern {
        package,
        repo,
        pattern,
    } = &hits[0].kind
    else {
        unreachable!()
    };
    assert_eq!(package.as_str(), "abc");
    assert_eq!(repo.as_str(), "model");
    assert_eq!(pattern.as_str(), "(model)(::.*)?");
}

#[test]
fn repo_load_passes_when_package_matches_pattern() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let model_root = root.join("model");
    fs::create_dir_all(&model_root).expect("mkdirs");
    write_source(&model_root, "Good.pure", "Class model::pkg::Good {}\n");
    write_descriptor(root, "model", "(model)(::.*)?", &[]);

    let model_repo = Repo::from_descriptor(&root.join("model.json")).expect("model from desc");

    let errors = match repo::load(&[model_repo], &[]) {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };
    let hits: Vec<_> = errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::PackageNotInRepoPattern { .. }))
        .collect();
    assert!(
        hits.is_empty(),
        "in-pattern package should not trigger violation; got: {hits:?}"
    );
}

#[test]
fn repo_load_surfaces_invalid_pattern_regex_as_synthetic_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let model_root = root.join("model");
    fs::create_dir_all(&model_root).expect("mkdirs");
    write_source(&model_root, "Foo.pure", "Class model::Foo {}\n");
    // `[` is an unterminated character class — invalid regex.
    write_descriptor(root, "model", "[", &[]);

    let model_repo = Repo::from_descriptor(&root.join("model.json")).expect("model from desc");

    let errors = match repo::load(&[model_repo], &[]) {
        Ok(_) => panic!("expected partial model with descriptor error, got Ok"),
        Err(p) => p.errors,
    };
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("invalid repo pattern regex")
                && e.source_info.source.as_str() == "<descriptor:model>"),
        "expected synthetic descriptor error; got: {errors:?}"
    );
}
