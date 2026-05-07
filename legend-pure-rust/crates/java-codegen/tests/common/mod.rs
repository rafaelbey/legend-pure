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

#![allow(dead_code)]

use std::sync::OnceLock;

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

struct PlatformFixture {
    repos: Vec<Repo>,
    auto_imports: Vec<SmolStr>,
}

fn platform_fixture() -> &'static PlatformFixture {
    static FIXTURE: OnceLock<PlatformFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| PlatformFixture {
        repos: Repo::default_with_build_snapshots(),
        auto_imports: legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect(),
    })
}

/// Compile the platform + an optional synthetic `user_test` repo carrying
/// `user_source`. Returns the model even if compile errors occurred — the
/// codegen tests want to read what *did* compile.
pub fn compile_with_platform(user_source: Option<&str>) -> PureModel {
    let fixture = platform_fixture();
    let mut repos = fixture.repos.clone();
    if let Some(source) = user_source {
        repos.push(synthetic_user_repo(source));
    }
    match legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports) {
        Ok(model) => model,
        Err(partial) => {
            for e in partial.errors.iter().take(10) {
                eprintln!("compile error: {e}");
            }
            partial.model
        }
    }
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
        "platform_dsl_path",
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
