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
//
//! Repo-boundary visibility smoke tests for the M3 path.
//!
//! Mirrors the structural shape of Java's
//! `m3.tests.validation.TestVisibility` (the unit-level cousin of
//! `m2.dsl.mapping.test.TestVisibility`). The Mapping-DSL ports live in
//! `dsl-mapping` and follow on Phase 2.
//!
//! Each test stages a multi-repo fixture by calling
//! `compile_repo_slice` per repo with crafted source paths, then
//! manually populates `model.repo_visibility` to mimic what
//! `core_platform_pure::repo::load` does. The shared finalize step then
//! either emits or does not emit a `NotVisible` error.

use std::collections::BTreeSet;

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};
use smol_str::SmolStr;

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn make_visibility(model: &mut PureModel, entries: &[(&str, &[&str])]) {
    for (name, deps) in entries {
        let mut set: BTreeSet<SmolStr> = BTreeSet::new();
        set.insert(SmolStr::new(*name));
        for d in *deps {
            set.insert(SmolStr::new(*d));
        }
        model.repo_visibility.insert(SmolStr::new(*name), set);
    }
}

fn collect_not_visible(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::NotVisible { .. }))
        .collect()
}

#[test]
fn class_extending_undeclared_repo_emits_not_visible() {
    // model defines TestClass2; system declares deps [platform] only.
    // A class in /system/... that extends model::domain::TestClass2 must
    // emit NotVisible.
    let sf_model = parse(
        "Class model::domain::TestClass2 {}",
        "/model/testFile2.pure",
    );
    let sf_system = parse(
        "Class system::pkg::Sub extends model::domain::TestClass2 {}",
        "/system/testFile.pure",
    );

    let mut model = init_bootstrap_model();
    make_visibility(
        &mut model,
        &[
            ("model", &["platform"]),
            ("system", &["platform"]),
            ("platform", &[]),
        ],
    );

    let (_r, errs) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    assert!(errs.is_empty(), "model slice errors: {errs:?}");
    let (_r, errs) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    assert!(errs.is_empty(), "system slice errors: {errs:?}");

    let final_errs = finalize_model(&mut model, &[], &[]);
    let hits = collect_not_visible(&final_errs);
    assert!(
        !hits.is_empty(),
        "expected at least one NotVisible error, got: {final_errs:?}"
    );

    let hit = hits[0];
    assert_eq!(
        hit.message,
        "model::domain::TestClass2 is not visible in the file /system/testFile.pure",
    );
    if let CompilationErrorKind::NotVisible {
        target_fqn,
        source_id,
    } = &hit.kind
    {
        assert_eq!(target_fqn.as_str(), "model::domain::TestClass2");
        assert_eq!(source_id.as_str(), "/system/testFile.pure");
    } else {
        panic!("expected NotVisible kind, got {:?}", hit.kind);
    }
}

#[test]
fn class_extending_declared_dep_passes() {
    // system declares deps [platform, model], so model::domain::TestClass2
    // is visible — finalize must produce zero NotVisible errors.
    let sf_model = parse(
        "Class model::domain::TestClass2 {}",
        "/model/testFile2.pure",
    );
    let sf_system = parse(
        "Class system::pkg::Sub extends model::domain::TestClass2 {}",
        "/system/testFile.pure",
    );

    let mut model = init_bootstrap_model();
    make_visibility(
        &mut model,
        &[
            ("model", &["platform"]),
            ("system", &["platform", "model"]),
            ("platform", &[]),
        ],
    );

    let (_r, errs) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    assert!(errs.is_empty());
    let (_r, errs) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    assert!(errs.is_empty());

    let final_errs = finalize_model(&mut model, &[], &[]);
    let hits = collect_not_visible(&final_errs);
    assert!(
        hits.is_empty(),
        "expected zero NotVisible errors, got: {hits:?}"
    );
}

#[test]
fn property_typed_with_undeclared_repo_emits_not_visible() {
    let sf_model = parse("Class model::domain::Helper {}", "/model/helper.pure");
    let sf_system = parse(
        "Class system::pkg::User { h: model::domain::Helper[1]; }",
        "/system/userFile.pure",
    );

    let mut model = init_bootstrap_model();
    make_visibility(
        &mut model,
        &[
            ("model", &["platform"]),
            ("system", &["platform"]),
            ("platform", &[]),
        ],
    );

    let (_r, _) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    let (_r, _) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let hits = collect_not_visible(&final_errs);
    assert!(
        hits.iter().any(|h| h.message
            == "model::domain::Helper is not visible in the file /system/userFile.pure"),
        "expected NotVisible for property type ref; got: {final_errs:?}"
    );
}

#[test]
fn empty_visibility_map_disables_all_checks() {
    // Same negative scenario as #1 but with an empty repo_visibility
    // map: validator must produce no NotVisible errors.
    let sf_model = parse(
        "Class model::domain::TestClass2 {}",
        "/model/testFile2.pure",
    );
    let sf_system = parse(
        "Class system::pkg::Sub extends model::domain::TestClass2 {}",
        "/system/testFile.pure",
    );

    let mut model = init_bootstrap_model();
    // Deliberately do NOT populate repo_visibility.

    let (_r, _) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    let (_r, _) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let hits = collect_not_visible(&final_errs);
    assert!(
        hits.is_empty(),
        "empty visibility map should disable checks; got: {hits:?}"
    );
}

#[test]
fn association_property_across_repos_emits_not_visible() {
    // Association A binds a property typed model::Foo from /system/...,
    // and system doesn't depend on model. Hits the walk_property path
    // inside walk_class for an Association.
    let sf_model = parse("Class model::Foo {}", "/model/foo.pure");
    let sf_system = parse(
        "Class system::Bar {}\n\
         Association system::FooBar { foo: model::Foo[1]; bar: system::Bar[1]; }",
        "/system/assoc.pure",
    );

    let mut model = init_bootstrap_model();
    make_visibility(
        &mut model,
        &[
            ("model", &["platform"]),
            ("system", &["platform"]),
            ("platform", &[]),
        ],
    );

    let (_r, _) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    let (_r, _) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let hits = collect_not_visible(&final_errs);
    assert!(
        hits.iter()
            .any(|h| h.message == "model::Foo is not visible in the file /system/assoc.pure"),
        "expected NotVisible for association property type ref; got: {final_errs:?}"
    );
}

#[test]
fn use_site_with_unregistered_repo_passes() {
    // Visibility is populated for some repos but the use-site repo
    // ("system") isn't registered. Java's getSourceRepoName would still
    // return "system" but the lookup against codeRepositories returns
    // null, and isVisibleInSource returns true. Match that.
    let sf_model = parse(
        "Class model::domain::TestClass2 {}",
        "/model/testFile2.pure",
    );
    let sf_system = parse(
        "Class system::pkg::Sub extends model::domain::TestClass2 {}",
        "/system/testFile.pure",
    );

    let mut model = init_bootstrap_model();
    make_visibility(&mut model, &[("model", &["platform"]), ("platform", &[])]);

    let (_r, _) = compile_repo_slice(&mut model, &[sf_model], &[], &[]);
    let (_r, _) = compile_repo_slice(&mut model, &[sf_system], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let hits = collect_not_visible(&final_errs);
    assert!(
        hits.is_empty(),
        "use-site repo not in map should not produce errors; got: {hits:?}"
    );
}
