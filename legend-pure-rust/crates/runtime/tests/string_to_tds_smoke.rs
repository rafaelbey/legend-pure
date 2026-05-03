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

//! End-to-end runtime smoke for the `stringToTDS` native and the
//! `#TDS\n…\n#` literal that lowers to it.
//!
//! Compiles a synthetic Pure source against the platform model (which
//! includes `platform_dsl_tds`'s `tds.pure`), evaluates a function,
//! and asserts:
//!
//! 1. A `stringToTDS('<csv>')` call returns a `TDS` heap instance
//!    whose `csv` slot equals the input.
//! 2. The `#TDS\n…\n#` literal goes through the compile-time lowerer
//!    and produces the same observable `csv` slot at runtime.

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

fn platform_fixture() -> &'static PlatformFixture {
    static FIXTURE: OnceLock<PlatformFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let repos = Repo::default_with_build_snapshots();
        let auto_imports = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect();
        PlatformFixture {
            repos,
            auto_imports,
        }
    })
}

struct PlatformFixture {
    repos: Vec<Repo>,
    auto_imports: Vec<SmolStr>,
}

fn synthetic_user_repo(user_source: &str) -> Repo {
    static USER_DEPS: &[&str] = &[
        "platform",
        "platform_precise_primitives",
        "platform_dsl_store",
        "platform_dsl_mapping",
        "platform_dsl_diagram",
        "platform_dsl_graph",
        "platform_dsl_tds",
        "platform_store_relational",
    ];
    let meta = RepoMeta {
        name: "user_test",
        pattern: ".*",
        dependencies: USER_DEPS,
    };
    Repo::Filesystem {
        prefix: "/user_test".into(),
        files: vec![OwnedSourceFile {
            path: "/user_test/test_source.pure".into(),
            content: user_source.into(),
        }],
        meta: Some(meta),
    }
}

fn compile(user_source: &str) -> PureModel {
    let fixture = platform_fixture();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    match legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports) {
        Ok(model) => model,
        Err(partial) => {
            let user_errs: Vec<_> = partial
                .errors
                .iter()
                .filter(|e| {
                    let src = e.source_info.source.as_str();
                    src.contains("/user_test/") || src.contains("platform_dsl_tds")
                })
                .collect();
            if !user_errs.is_empty() {
                eprintln!(
                    "compile: {} relevant error(s) during repo::load",
                    user_errs.len()
                );
                for e in &user_errs {
                    eprintln!("  - {}: {}", e.source_info.source, e.message);
                }
                panic!("relevant compile errors; see above");
            }
            partial.model
        }
    }
}

fn eval_returning_csv(source: &str, fqn: &str) -> SmolStr {
    let model = compile(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));

    let result = eval
        .call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("Evaluation error: {e}"));
    match result {
        Value::String(s) => s,
        other => panic!("expected Value::String from {fqn}, got {other:?}"),
    }
}

#[test]
fn string_to_tds_populates_csv_slot() {
    // `stringToTDS` lives in `meta::pure::metamodel::relation`, which
    // isn't in PLATFORM_AUTO_IMPORTS — `meta::pure::functions::relation`
    // is. Reference by FQN (or use the `#TDS#` literal which carries the
    // FQN through the lowerer).
    let source = r"
        function test::f(): String[1] {
            meta::pure::metamodel::relation::stringToTDS('a, b\n1, 2').csv
        }
    ";
    let csv = eval_returning_csv(source, "f__String_1_");
    assert_eq!(csv.as_str(), "a, b\n1, 2");
}

#[test]
fn tds_literal_lowers_to_string_to_tds_and_csv_round_trips() {
    // `#TDS\n a, b\n 1, 2\n#` should lower to
    // `stringToTDS('a, b\n1, 2')->cast(@(a:Integer[1], b:Integer[1]))`.
    // The cast doesn't change runtime identity — `.csv` reads through
    // to the same heap slot the native populated.
    let source = r"
        function test::f(): String[1] {
            (#TDS
              a, b
              1, 2
            #).csv
        }
    ";
    let csv = eval_returning_csv(source, "f__String_1_");
    assert_eq!(csv.as_str(), "a, b\n1, 2");
}

#[test]
fn string_to_tds_and_tds_literal_produce_matching_csv() {
    // Belt-and-suspenders: the same content rendered via the two
    // surface forms must produce identical csv slots, locking the
    // "single shared code path" invariant from the runtime side.
    let source = r"
        function test::fLit(): String[1] {
            (#TDS
              x
              42
            #).csv
        }
        function test::fNative(): String[1] {
            meta::pure::metamodel::relation::stringToTDS('x\n42').csv
        }
    ";
    let lit = eval_returning_csv(source, "fLit__String_1_");
    let native = eval_returning_csv(source, "fNative__String_1_");
    assert_eq!(lit, native);
}
