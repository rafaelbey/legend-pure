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

//! Phase 2: repo-boundary visibility smoke tests for the Mapping DSL.
//!
//! Mirrors `crates/dsl-relational/tests/visibility_smoke.rs` (Phase D
//! for relational). Each fixture stages a multi-repo source set with
//! crafted paths (`/repoA/...`, `/repoB/...`), populates
//! `model.repo_visibility` directly, and runs the full
//! `compile_repo_slice` + `finalize_model` pipeline with the
//! `MappingExtension` registered. The matcher checks whether
//! `NotVisible` fires for the Mapping-DSL cross-repo reference under
//! test (`include other::Mapping`, `ClassMapping.class`, store
//! substitution endpoint).

use std::collections::BTreeSet;

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

fn parse(source: &str, path: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        path,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
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

/// Stage a multi-source compile through `compile_repo_slice` +
/// `finalize_model` so that each source's `SourceFile.path` becomes
/// the use-site for visibility checks. `repo_setup` is a list of
/// `(repo_name, deps)` tuples for `make_visibility`.
fn run_pipeline(sources: Vec<SourceFile>, repo_setup: &[(&str, &[&str])]) -> Vec<CompilationError> {
    let mut model = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    make_visibility(&mut model, repo_setup);

    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];

    let mut all_errors: Vec<CompilationError> = Vec::new();
    for sf in sources {
        let (_range, errs) =
            legend_pure_parser_pure::pipeline::compile_repo_slice(&mut model, &[sf], &[], &exts);
        all_errors.extend(errs);
    }
    all_errors.extend(legend_pure_parser_pure::pipeline::finalize_model(
        &mut model,
        &[],
        &exts,
    ));
    all_errors
}

fn collect_not_visible(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::NotVisible { .. }))
        .collect()
}

// ---------------------------------------------------------------------------
// `include other::Mapping` cross-repo
// ---------------------------------------------------------------------------

#[test]
fn mapping_include_from_undeclared_repo_emits_not_visible() {
    let other = parse(
        indoc! {r"
            ###Pure
            Class other::Foo { x : String[1]; }

            ###Mapping
            Mapping other::FooMap
            (
              other::Foo : Pure { x : 'a' }
            )
        "},
        "/other/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Pure
            Class app::Bar { y : String[1]; }

            ###Mapping
            Mapping app::BarMap
            (
              include other::FooMap

              app::Bar : Pure { y : 'b' }
            )
        "},
        "/app/bar.pure",
    );
    let errors = run_pipeline(
        vec![other, main],
        &[("other", &[]), ("app", &[])], // app does NOT declare 'other'
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter()
            .any(|h| h.message.contains("other::FooMap") && h.message.contains("/app/bar.pure")),
        "expected NotVisible for cross-repo include; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn mapping_include_from_declared_dep_passes() {
    let other = parse(
        indoc! {r"
            ###Pure
            Class other::Foo { x : String[1]; }

            ###Mapping
            Mapping other::FooMap
            (
              other::Foo : Pure { x : 'a' }
            )
        "},
        "/other/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Pure
            Class app::Bar { y : String[1]; }

            ###Mapping
            Mapping app::BarMap
            (
              include other::FooMap

              app::Bar : Pure { y : 'b' }
            )
        "},
        "/app/bar.pure",
    );
    let errors = run_pipeline(vec![other, main], &[("other", &[]), ("app", &["other"])]);
    let hits = collect_not_visible(&errors);
    assert!(
        !hits.iter().any(|h| h.message.contains("other::FooMap")),
        "expected no NotVisible for declared dep; got: {:#?}",
        hits.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// `ClassMapping.class` reference cross-repo
// ---------------------------------------------------------------------------

#[test]
fn class_mapping_class_from_undeclared_repo_emits_not_visible() {
    let model = parse(
        indoc! {r"
            ###Pure
            Class model::Foo { x : String[1]; }
        "},
        "/model/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Mapping
            Mapping app::FooMap
            (
              model::Foo : Pure { x : 'literal' }
            )
        "},
        "/app/foomap.pure",
    );
    let errors = run_pipeline(
        vec![model, main],
        &[("model", &[]), ("app", &[])], // app missing 'model'
    );
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter()
            .any(|h| h.message.contains("model::Foo") && h.message.contains("/app/foomap.pure")),
        "expected NotVisible for class-mapping target class; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn class_mapping_class_from_declared_dep_passes() {
    let model = parse(
        indoc! {r"
            ###Pure
            Class model::Foo { x : String[1]; }
        "},
        "/model/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Mapping
            Mapping app::FooMap
            (
              model::Foo : Pure { x : 'literal' }
            )
        "},
        "/app/foomap.pure",
    );
    let errors = run_pipeline(vec![model, main], &[("model", &[]), ("app", &["model"])]);
    let hits = collect_not_visible(&errors);
    assert!(
        !hits.iter().any(|h| h.message.contains("model::Foo")),
        "expected no NotVisible (declared dep); got: {:#?}",
        hits.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// `~src` cross-repo
// ---------------------------------------------------------------------------

#[test]
fn pure_body_src_class_from_undeclared_repo_emits_not_visible() {
    let model = parse(
        indoc! {r"
            ###Pure
            Class model::SrcFoo { x : String[1]; }
            Class model::TgtFoo { x : String[1]; }
        "},
        "/model/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Mapping
            Mapping app::FooMap
            (
              model::TgtFoo : Pure
              {
                ~src model::SrcFoo
                x : $src.x
              }
            )
        "},
        "/app/foomap.pure",
    );
    // Declare `model` as a dep so `model::TgtFoo` resolves but
    // demote it to a non-dep variant: actually we want a setup where
    // `~src model::SrcFoo` is the violation. Easier: make app's deps
    // empty, so both TgtFoo AND SrcFoo refs error. Filter for the
    // specific ~src ref's error to avoid coupling to the class-ref
    // error (already covered by the previous test).
    let errors = run_pipeline(vec![model, main], &[("model", &[]), ("app", &[])]);
    let hits = collect_not_visible(&errors);
    assert!(
        hits.iter().any(|h| h.message.contains("model::SrcFoo")),
        "expected NotVisible for ~src class; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Empty visibility map — validator no-ops
// ---------------------------------------------------------------------------

#[test]
fn empty_visibility_map_disables_all_checks() {
    let other = parse(
        indoc! {r"
            ###Pure
            Class other::Foo { x : String[1]; }

            ###Mapping
            Mapping other::FooMap
            (
              other::Foo : Pure { x : 'a' }
            )
        "},
        "/other/foo.pure",
    );
    let main = parse(
        indoc! {r"
            ###Pure
            Class app::Bar { y : String[1]; }

            ###Mapping
            Mapping app::BarMap
            (
              include other::FooMap
              app::Bar : Pure { y : 'b' }
            )
        "},
        "/app/bar.pure",
    );
    // No visibility map populated — validator must produce no
    // NotVisible diagnostics.
    let errors = run_pipeline(vec![other, main], &[]);
    let hits = collect_not_visible(&errors);
    assert!(
        hits.is_empty(),
        "expected no NotVisible when map is empty; got: {:#?}",
        hits.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
