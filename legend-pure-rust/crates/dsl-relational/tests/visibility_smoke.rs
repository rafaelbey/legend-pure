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

//! Phase D: repo-boundary visibility smoke tests for the relational DSL.
//!
//! Each test stages a multi-repo fixture by parsing two source files
//! with crafted paths (`/repoA/...`, `/repoB/...`), populates the
//! bootstrap model's `repo_visibility` map directly, and runs the
//! relational extension's `validate()` lifecycle. The matcher checks
//! whether any `NotVisible` error fired against the relational
//! cross-repo reference (`include other::Db`, `[db]` qualifier).

use std::collections::BTreeSet;

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::CompilerExtension;
use smol_str::SmolStr;

fn parse_at(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        path,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .expect("source must parse")
}

fn run_with_visibility(
    files: Vec<SourceFile>,
    visibility: &[(&str, &[&str])],
) -> Vec<CompilationError> {
    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    for (name, deps) in visibility {
        let mut set: BTreeSet<SmolStr> = BTreeSet::new();
        set.insert(SmolStr::new(*name));
        for d in *deps {
            set.insert(SmolStr::new(*d));
        }
        bootstrap.repo_visibility.insert(SmolStr::new(*name), set);
    }

    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.declare(&mut declare_ctx);

    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: None,
    };
    extension.validate(&mut validate_ctx);
    errors
}

fn collect_not_visible(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::NotVisible { .. }))
        .collect()
}

// ---------------------------------------------------------------------------
// `include other::Db` cross-repo
// ---------------------------------------------------------------------------

#[test]
fn include_from_undeclared_repo_emits_not_visible() {
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::baseDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/other/base.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              include other::baseDb
              Table local (id INT PRIMARY KEY)
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(
        vec![other, main],
        &[("other", &[]), ("app", &[])], // app does NOT declare 'other'
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter()
            .any(|h| h.message.contains("other::baseDb") && h.message.contains("/app/main.pure")),
        "expected NotVisible for include cross-repo; got: {errors:#?}"
    );
}

#[test]
fn include_from_declared_dep_passes() {
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::baseDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/other/base.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              include other::baseDb
              Table local (id INT PRIMARY KEY)
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(
        vec![other, main],
        &[("other", &[]), ("app", &["other"])], // declared
    );
    let hits = collect_not_visible(&errors);
    assert!(hits.is_empty(), "expected no NotVisible; got: {errors:#?}");
}

#[test]
fn include_within_same_repo_passes() {
    let base = parse_at(
        indoc! {r"
            ###Relational
            Database app::baseDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/app/base.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              include app::baseDb
              Table local (id INT PRIMARY KEY)
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(vec![base, main], &[("app", &[])]);
    let hits = collect_not_visible(&errors);
    assert!(
        hits.is_empty(),
        "expected no NotVisible for same-repo include; got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// `[db]` qualifier inside a Filter / op-body
// ---------------------------------------------------------------------------

#[test]
fn db_qualifier_in_op_body_from_undeclared_repo_emits_not_visible() {
    // app::Db references [other::otherDb] inside a Filter body.
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::otherDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/other/other.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              Table local (id INT PRIMARY KEY)
              Filter f ([other::otherDb]t.id = local.id)
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(
        vec![other, main],
        &[("other", &[]), ("app", &[])], // app missing 'other' dep
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter()
            .any(|h| h.message.contains("other::otherDb") && h.message.contains("/app/main.pure")),
        "expected NotVisible for [db] qualifier; got: {errors:#?}"
    );
}

#[test]
fn db_qualifier_in_op_body_from_declared_dep_passes() {
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::otherDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/other/other.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              Table local (id INT PRIMARY KEY)
              Filter f ([other::otherDb]t.id = local.id)
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(vec![other, main], &[("other", &[]), ("app", &["other"])]);
    let hits = collect_not_visible(&errors);
    assert!(
        hits.is_empty(),
        "expected no NotVisible (other in deps); got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// `[db]` qualifier inside a View column value
// ---------------------------------------------------------------------------

#[test]
fn db_qualifier_in_view_column_from_undeclared_repo_emits_not_visible() {
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::otherDb
            (
              Table foreign (id INT PRIMARY KEY)
            )
        "},
        "/other/other.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              Schema s
              (
                Table local (id INT PRIMARY KEY)
                View v (id : [other::otherDb]foreign.id)
              )
            )
        "},
        "/app/main.pure",
    );

    let errors = run_with_visibility(
        vec![other, main],
        &[("other", &[]), ("app", &[])], // missing dep
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter()
            .any(|h| h.message.contains("other::otherDb") && h.message.contains("/app/main.pure")),
        "expected NotVisible for [db] in view column; got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// Empty repo_visibility — validator is a no-op
// ---------------------------------------------------------------------------

#[test]
fn empty_visibility_map_disables_all_checks() {
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::otherDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "/other/other.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              include other::otherDb
              Table local (id INT PRIMARY KEY)
            )
        "},
        "/app/main.pure",
    );

    // No visibility map populated.
    let errors = run_with_visibility(vec![other, main], &[]);
    let hits = collect_not_visible(&errors);
    assert!(
        hits.is_empty(),
        "expected no NotVisible when map is empty; got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// Source path with no extractable repo (synthetic / bootstrap fixtures)
// ---------------------------------------------------------------------------

#[test]
fn source_without_repo_prefix_skips_check() {
    // The synthetic path `validator_smoke.pure` (no leading slash)
    // matches `source_repo_name -> None`, so the validator must not
    // emit NotVisible regardless of map contents.
    let other = parse_at(
        indoc! {r"
            ###Relational
            Database other::otherDb
            (
              Table t (id INT PRIMARY KEY)
            )
        "},
        "validator_smoke.pure",
    );
    let main = parse_at(
        indoc! {r"
            ###Relational
            Database app::mainDb
            (
              include other::otherDb
              Table local (id INT PRIMARY KEY)
            )
        "},
        "validator_smoke.pure",
    );

    let errors = run_with_visibility(
        vec![other, main],
        &[("app", &[])], // strict map but no extractable use-site repo
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.is_empty(),
        "expected no NotVisible for repo-less source; got: {errors:#?}"
    );
}
