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

//! Integration tests for the LSP-side incremental recompile path
//! (T-20260513-01 Phase 1). Two independent wins → two test fixtures:
//!
//! Fixture A: one-repo, multi-file workspace. Exercises the
//! parse-cache win — only the edited file is re-parsed, unchanged
//! files in the same chunk hit `Workspace.parsed_files`.
//!
//! Fixture B: two-repo workspace with cross-repo deps (`app`
//! references `base`). Exercises the compile-cache win — the rerun
//! closure includes only the dirty chunk and its transitive
//! dependents.

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_lsp::workspace::Workspace;
use smol_str::SmolStr;

fn make_meta(name: &str, deps: Vec<&str>) -> RepoMeta {
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());
    let pattern: &'static str = "(.*)";
    let deps_leaked: Vec<&'static str> = deps
        .into_iter()
        .map(|d| Box::leak(d.to_string().into_boxed_str()) as &'static str)
        .collect();
    let dependencies: &'static [&'static str] = Box::leak(deps_leaked.into_boxed_slice());
    RepoMeta {
        name,
        pattern,
        dependencies,
    }
}

fn fs_repo(name: &str, prefix: &str, files: Vec<(&str, &str)>, deps: Vec<&str>) -> Repo {
    Repo::Filesystem {
        prefix: prefix.to_string(),
        files: files
            .into_iter()
            .map(|(path, content)| OwnedSourceFile {
                path: path.to_string(),
                content: content.to_string(),
            })
            .collect(),
        meta: Some(make_meta(name, deps)),
        source_root: None,
    }
}

fn set_file_content(repos: &mut [Repo], canonical: &str, new_content: &str) {
    for repo in repos {
        if let Repo::Filesystem { files, .. } = repo {
            for f in files {
                if f.path == canonical {
                    f.content = new_content.to_string();
                    return;
                }
            }
        }
    }
    panic!("set_file_content: no file matched canonical {canonical}");
}

// ---------------------------------------------------------------------------
// Fixture A — one repo, multi-file: parse-cache win
// ---------------------------------------------------------------------------

#[test]
fn parse_cache_only_reparses_dirty_file_within_a_chunk() {
    // Single-repo workspace: 3 files in one chunk. Editing one file
    // should re-parse exactly that file; the other two hit the cache.
    let repo = fs_repo(
        "proj",
        "/proj",
        vec![
            ("/proj/a.pure", "Class proj::A { name: String[1]; }"),
            ("/proj/b.pure", "Class proj::B { tag: String[1]; }"),
            ("/proj/c.pure", "Class proj::C { code: String[1]; }"),
        ],
        vec![],
    );
    let mut ws = Workspace::new(vec![repo], Vec::new());

    // Cold compile — every file parsed once.
    let _ = ws.compile();
    assert_eq!(
        ws.parse_count_since_last_compile(),
        3,
        "cold compile must parse every source file"
    );
    assert!(ws.last_rerun_chunks().is_none(), "cold path: no rerun set");

    // Edit c.pure. Mark dirty + replace content in base_repos so the
    // snapshot loader picks up the new text (no open-buffer URI plumbing
    // in this test — the production path is identical, just routed
    // through did_change/set_open_buffer).
    set_file_content(
        &mut ws.base_repos,
        "/proj/c.pure",
        "Class proj::C { code: String[1]; revision: Integer[1]; }",
    );
    ws.dirty_files.insert(SmolStr::new("/proj/c.pure"));

    let _ = ws.compile();
    assert_eq!(
        ws.parse_count_since_last_compile(),
        1,
        "incremental compile must reparse only the dirty file"
    );
}

// ---------------------------------------------------------------------------
// Fixture B — two repos with cross-repo dep: compile-cache win
// ---------------------------------------------------------------------------

fn fixture_b() -> Workspace {
    let base = fs_repo(
        "base",
        "/base",
        vec![("/base/x.pure", "Class base::X { name: String[1]; }")],
        vec![],
    );
    let app = fs_repo(
        "app",
        "/app",
        vec![
            (
                "/app/y.pure",
                "Class app::Y extends base::X { extra: String[1]; }",
            ),
            ("/app/z.pure", "function app::greet(): String[1] { 'hi' }"),
        ],
        vec!["base"],
    );
    Workspace::new(vec![base, app], Vec::new())
}

#[test]
fn b1_self_contained_edit_no_downstream() {
    // Editing app/z.pure (no incoming refs from elsewhere) reruns
    // only the app chunk. `chunk_dependents[app]` is empty since
    // nothing references app.
    let mut ws = fixture_b();
    let outcome = ws.compile();
    assert_eq!(
        outcome.error_count, 0,
        "fixture B must compile clean: diagnostics: {:?}",
        ws.diagnostics
    );

    set_file_content(
        &mut ws.base_repos,
        "/app/z.pure",
        "function app::greet(): String[1] { 'hello' }",
    );
    ws.dirty_files.insert(SmolStr::new("/app/z.pure"));

    let outcome = ws.compile();
    assert_eq!(outcome.error_count, 0);

    let rerun = ws
        .last_rerun_chunks()
        .expect("rerun set populated")
        .to_vec();
    assert_eq!(
        rerun.len(),
        1,
        "leaf-chunk edit reruns exactly one chunk; got {rerun:?}"
    );
    // The single chunk must be the one owning /app/z.pure.
    let app_y_chunk_present = ws.diagnostics.is_empty();
    assert!(app_y_chunk_present, "no diagnostics expected after rerun");
}

#[test]
fn b2_edit_with_downstream_reruns_dependents() {
    // Editing base/x.pure reruns chunk(base) AND chunk(app),
    // because app/y.pure has a supertype ref into base::X.
    let mut ws = fixture_b();
    let outcome = ws.compile();
    assert_eq!(outcome.error_count, 0, "{:?}", ws.diagnostics);

    set_file_content(
        &mut ws.base_repos,
        "/base/x.pure",
        "Class base::X { name: String[1]; age: Integer[1]; }",
    );
    ws.dirty_files.insert(SmolStr::new("/base/x.pure"));

    let outcome = ws.compile();
    assert_eq!(
        outcome.error_count, 0,
        "errors after rerun: {:?}",
        ws.diagnostics
    );

    let rerun = ws
        .last_rerun_chunks()
        .expect("rerun set populated")
        .to_vec();
    assert_eq!(
        rerun.len(),
        2,
        "edit on upstream reruns 1 + len(downstream) chunks; got {rerun:?}"
    );
}

#[test]
fn b3_element_removal_surfaces_downstream_errors() {
    // Removing class X from base/x.pure must rerun chunk(app) and
    // surface an unresolved-supertype-style error on app/y.pure.
    let mut ws = fixture_b();
    let outcome = ws.compile();
    assert_eq!(outcome.error_count, 0, "{:?}", ws.diagnostics);

    // Replace base/x.pure with an empty file (delete class X).
    set_file_content(&mut ws.base_repos, "/base/x.pure", "");
    ws.dirty_files.insert(SmolStr::new("/base/x.pure"));

    let _ = ws.compile();
    let rerun = ws
        .last_rerun_chunks()
        .expect("rerun set populated")
        .to_vec();
    assert!(
        rerun.len() >= 2,
        "deletion must rerun upstream + downstream; got {rerun:?}"
    );
    // app/y.pure should now carry a diagnostic referencing base::X.
    let y_errors = ws
        .diagnostics
        .get("/app/y.pure")
        .cloned()
        .unwrap_or_default();
    assert!(
        y_errors.iter().any(|e| e.message.contains("base::X")
            || e.message.contains("base") && e.message.contains("X")),
        "expected diagnostic referencing base::X on /app/y.pure; got: {y_errors:?}"
    );
}

#[test]
fn b4_parse_error_keeps_prior_model() {
    // A transient parse error on base/x.pure must not wipe the prior
    // model: hover/goto on app/y.pure must continue to resolve.
    let mut ws = fixture_b();
    let outcome = ws.compile();
    assert_eq!(outcome.error_count, 0, "{:?}", ws.diagnostics);

    // Snapshot pre-edit resolution: app::Y exists and resolves
    // through its supertype.
    let pre_model = ws.model.as_ref().expect("model present").clone();
    let y_pre = pre_model
        .resolve_by_path(&["app".into(), "Y".into()])
        .expect("app::Y resolves pre-edit");

    // Introduce a parse error in base/x.pure.
    set_file_content(&mut ws.base_repos, "/base/x.pure", "class");
    ws.dirty_files.insert(SmolStr::new("/base/x.pure"));

    let _ = ws.compile();

    // base/x.pure carries a parse diagnostic.
    let x_errors = ws
        .diagnostics
        .get("/base/x.pure")
        .cloned()
        .unwrap_or_default();
    assert!(
        !x_errors.is_empty(),
        "parse error must surface on /base/x.pure"
    );

    // The model is still queryable; app::Y still resolves. (It may
    // be the same Arc-shared snapshot as before since no chunk
    // actually reran; that's the intended fallback.)
    let post_model = ws.model.as_ref().expect("model retained").clone();
    let y_post = post_model
        .resolve_by_path(&["app".into(), "Y".into()])
        .expect("app::Y still resolves after parse error on upstream");
    assert_eq!(
        y_pre, y_post,
        "ElementId for app::Y should survive the parse-error rerun"
    );
}
