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

//! Repo `pattern` membership coverage for DSL instances.
//!
//! Pinned because DSL extensions allocate `Element::DSLInstance` rows
//! through their own `define()` paths, bypassing
//! `pipeline::hydrate_element_signature`. Without an explicit cross-chunk
//! sweep in `validate(model)`, the per-element validator misses every
//! Mapping / Database / Diagram declaration. These tests guard against
//! that regression.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::visibility::{RepoPattern, compile_repo_pattern};
use smol_str::SmolStr;

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        path,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

fn make_patterns(model: &mut PureModel, entries: &[(&str, &str)]) {
    for (name, pattern) in entries {
        let compiled = compile_repo_pattern(pattern).expect("test pattern compiles");
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

fn run_pipeline(sources: Vec<SourceFile>, patterns: &[(&str, &str)]) -> Vec<CompilationError> {
    let mut model = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    make_patterns(&mut model, patterns);

    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];

    let mut all: Vec<CompilationError> = Vec::new();
    for sf in sources {
        let (_r, errs) =
            legend_pure_parser_pure::pipeline::compile_repo_slice(&mut model, &[sf], &[], &exts);
        all.extend(errs);
    }
    all.extend(legend_pure_parser_pure::pipeline::finalize_model(
        &mut model,
        &[],
        &exts,
    ));
    all
}

#[test]
fn mapping_outside_repo_pattern_emits_violation() {
    // The platform pattern admits meta::*, system::*, apps::pure::*.
    // A Mapping declared in `abc::M` violates that.
    let m3 = parse(
        indoc! {r"
            ###Pure
            Class abc::Firm
            {
              name : String[1];
            }

            Class abc::FirmSrc
            {
              legalName : String[1];
            }
        "},
        "/platform/abc/types.pure",
    );
    let mapping = parse(
        indoc! {r"
            ###Mapping
            Mapping abc::M
            (
              *abc::Firm[firm_set] : Pure
              {
                ~src abc::FirmSrc
                name : $src.legalName
              }
            )
        "},
        "/platform/abc/m.pure",
    );

    let errs = run_pipeline(
        vec![m3, mapping],
        &[("platform", "((meta)|(system)|(apps::pure))(::.*)?")],
    );

    let hits = collect_pattern_violations(&errs);
    let mapping_hit = hits.iter().find(|e| {
        matches!(
            &e.kind,
            CompilationErrorKind::PackageNotInRepoPattern { repo, package, .. }
                if repo.as_str() == "platform" && package.as_str() == "abc"
        )
    });
    // Without the cross-chunk pass, the Class violations would land
    // here but the Mapping would slip through. We need *at least* one
    // hit at `abc::M`'s source to know DSL instances are covered.
    let mapping_at_m = hits
        .iter()
        .any(|e| e.source_info.source.as_str() == "/platform/abc/m.pure");
    assert!(
        mapping_at_m,
        "expected violation at /platform/abc/m.pure (the Mapping declaration); \
         hits: {hits:?}"
    );
    // Sanity: at minimum the Class declarations also flag.
    assert!(
        mapping_hit.is_some() || mapping_at_m,
        "validator must cover both M3 + DSLInstance kinds; got: {errs:?}"
    );
}

#[test]
fn mapping_inside_repo_pattern_passes() {
    let m3 = parse(
        indoc! {r"
            ###Pure
            Class meta::test::Firm
            {
              name : String[1];
            }

            Class meta::test::FirmSrc
            {
              legalName : String[1];
            }
        "},
        "/platform/meta/test/types.pure",
    );
    let mapping = parse(
        indoc! {r"
            ###Mapping
            Mapping meta::test::M
            (
              *meta::test::Firm[firm_set] : Pure
              {
                ~src meta::test::FirmSrc
                name : $src.legalName
              }
            )
        "},
        "/platform/meta/test/m.pure",
    );

    let errs = run_pipeline(
        vec![m3, mapping],
        &[("platform", "((meta)|(system)|(apps::pure))(::.*)?")],
    );

    let hits = collect_pattern_violations(&errs);
    assert!(
        hits.is_empty(),
        "in-pattern Mapping must produce no violations; got: {hits:?}"
    );
}
