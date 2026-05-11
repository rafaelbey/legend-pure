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

//! Repo `pattern` membership smoke tests.
//!
//! Java parity: `RepositoryPackageValidator` /
//! `CodeRepository.isPackageAllowed`. Each test stages a multi-repo
//! fixture with `compile_repo_slice` per repo, manually populates
//! `model.repo_patterns` to mimic what the snapshot-builder does, then
//! checks the `PackageNotInRepoPattern` diagnostic surface.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{compile_repo_slice, finalize_model, init_bootstrap_model};
use legend_pure_parser_pure::visibility::{RepoPattern, compile_repo_pattern};
use smol_str::SmolStr;

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn make_patterns(model: &mut PureModel, entries: &[(&str, &str)]) {
    for (name, pattern) in entries {
        let compiled = compile_repo_pattern(pattern).expect("test pattern must compile");
        model.repo_patterns.insert(
            SmolStr::new(*name),
            RepoPattern {
                source: SmolStr::new(*pattern),
                compiled,
            },
        );
    }
}

fn collect_pattern_violations(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::PackageNotInRepoPattern { .. }))
        .collect()
}

#[test]
fn class_outside_repo_pattern_emits_violation() {
    // Repo `platform` is allowed to ship `meta::*`, `system::*`, or
    // `apps::pure::*` packages — `abc::Class1` is outside that language.
    let sf = parse("Class abc::Class1 {}", "/platform/abc/class1.pure");

    let mut model = init_bootstrap_model();
    make_patterns(
        &mut model,
        &[("platform", "((meta)|(system)|(apps::pure))(::.*)?")],
    );

    let (_r, slice_errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let mut all = slice_errs;
    all.extend(final_errs);

    let hits = collect_pattern_violations(&all);
    assert_eq!(hits.len(), 1, "expected one violation; got: {all:?}");
    let hit = hits[0];
    assert_eq!(
        hit.message,
        "Package abc is not allowed in platform; \
         only packages matching ((meta)|(system)|(apps::pure))(::.*)? are allowed",
    );
    let CompilationErrorKind::PackageNotInRepoPattern {
        package,
        repo,
        pattern,
    } = &hit.kind
    else {
        panic!("expected PackageNotInRepoPattern, got {:?}", hit.kind);
    };
    assert_eq!(package.as_str(), "abc");
    assert_eq!(repo.as_str(), "platform");
    assert_eq!(pattern.as_str(), "((meta)|(system)|(apps::pure))(::.*)?");
}

#[test]
fn class_inside_repo_pattern_passes() {
    let sf = parse("Class meta::pure::demo::Foo {}", "/platform/meta/foo.pure");

    let mut model = init_bootstrap_model();
    make_patterns(
        &mut model,
        &[("platform", "((meta)|(system)|(apps::pure))(::.*)?")],
    );

    let (_r, slice_errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let mut all = slice_errs;
    all.extend(final_errs);
    let hits = collect_pattern_violations(&all);
    assert!(hits.is_empty(), "expected no violations; got: {hits:?}");
}

#[test]
fn empty_pattern_map_disables_check() {
    let sf = parse("Class abc::Class1 {}", "/platform/abc/class1.pure");
    let mut model = init_bootstrap_model();
    // Deliberately leave repo_patterns empty.
    let (_r, _) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);
    let hits = collect_pattern_violations(&final_errs);
    assert!(
        hits.is_empty(),
        "empty pattern map must short-circuit; got: {hits:?}"
    );
}

#[test]
fn use_site_with_unregistered_repo_passes() {
    // Pattern map has `platform` but the use site is `/other/...`.
    // Mirrors visibility behaviour: unknown use-site repos pass.
    let sf = parse("Class abc::Class1 {}", "/other/abc/class1.pure");

    let mut model = init_bootstrap_model();
    make_patterns(&mut model, &[("platform", "(meta)(::.*)?")]);

    let (_r, _) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);
    let hits = collect_pattern_violations(&final_errs);
    assert!(
        hits.is_empty(),
        "unknown use-site repo must skip pattern check; got: {hits:?}"
    );
}

#[test]
fn anchored_match_rejects_prefix_extension() {
    // `(meta)(::.*)?` anchored must NOT match `metadata` — the wrap as
    // `^(?:…)$` is what enforces this. Without anchoring, Rust's
    // `is_match` returns true on prefix hits, diverging from Java's
    // `Matcher.matches()`.
    let sf = parse(
        "Class metadata::Class1 {}",
        "/platform/metadata/class1.pure",
    );

    let mut model = init_bootstrap_model();
    make_patterns(&mut model, &[("platform", "(meta)(::.*)?")]);

    let (_r, slice_errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let mut all = slice_errs;
    all.extend(final_errs);
    let hits = collect_pattern_violations(&all);
    assert!(
        !hits.is_empty(),
        "anchored pattern must reject prefix-extension `metadata`; got: {all:?}"
    );
    assert!(
        hits[0].message.contains("Package metadata is not allowed"),
        "wrong message: {}",
        hits[0].message
    );
}

#[test]
fn function_outside_repo_pattern_emits_violation() {
    // The validator is per-element via hydrate_element_signature, so
    // every PackageableElement kind is gated — not just Class.
    let sf = parse(
        "function abc::testFn(): Boolean[1] { true; }",
        "/platform/abc/test.pure",
    );

    let mut model = init_bootstrap_model();
    make_patterns(&mut model, &[("platform", "(meta)(::.*)?")]);

    let (_r, slice_errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    let final_errs = finalize_model(&mut model, &[], &[]);

    let mut all = slice_errs;
    all.extend(final_errs);
    let hits = collect_pattern_violations(&all);
    assert!(
        !hits.is_empty(),
        "function declaration outside pattern must violate; got: {all:?}"
    );
}
