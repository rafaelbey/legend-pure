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

//! Integration tests for the MCP `find_references` tool. Stands up a
//! real [`LegendMcpServer`] over a two-file tmp filesystem repo (a
//! target function and a caller) and asserts that
//! `server.find_references(...)` returns the caller's source location.
//!
//! Mirrors the helper layout of `apply_edit.rs` rather than extracting
//! a shared module — two test files is below the abstraction
//! threshold.

use std::sync::{Arc, Mutex};

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_mcp::{
    LegendMcpServer, WorkspaceSnapshot,
    server::{FindReferencesArgs, SearchSymbolsArgs},
};
use rmcp::handler::server::wrapper::Parameters;

/// Build a `RepoMeta` whose `&'static` fields are leaked once per
/// call. Same shape as the helper in `apply_edit.rs`.
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

/// Pull the inner JSON string from a `CallToolResult` whose only
/// content block is the `text` payload built by `json_result`.
fn extract_text_content(result: &rmcp::model::CallToolResult) -> String {
    match &result.content[0].raw {
        rmcp::model::RawContent::Text(t) => t.text.clone(),
        other => panic!("unexpected content block: {other:?}"),
    }
}

/// Stand up an MCP server over a tmp filesystem repo containing the
/// given `(canonical_path, content)` files.
fn build_server(files: &[(&str, &str)]) -> (LegendMcpServer, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir(&proj_dir).expect("mkdir proj");
    let mut owned: Vec<OwnedSourceFile> = Vec::with_capacity(files.len());
    for (canonical, content) in files {
        // Strip the leading "/proj/" so the file lands inside proj_dir
        // and the on-disk layout matches the canonical paths.
        let rel = canonical
            .strip_prefix("/proj/")
            .expect("canonical must start with /proj/");
        let disk_path = proj_dir.join(rel);
        std::fs::write(&disk_path, content).expect("write");
        owned.push(OwnedSourceFile {
            path: (*canonical).to_string(),
            content: (*content).to_string(),
        });
    }
    let repo = Repo::Filesystem {
        prefix: "/proj".to_string(),
        files: owned,
        meta: Some(make_meta("proj", Vec::new())),
        source_root: Some(proj_dir),
    };
    let repos = Arc::new(Mutex::new(vec![repo]));
    let auto_imports = Arc::new(Vec::new());
    let initial_repos = repos
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let snapshot = Arc::new(WorkspaceSnapshot::compile(&initial_repos, &auto_imports));
    drop(initial_repos);
    let server = LegendMcpServer::new(snapshot, repos, auto_imports);
    (server, tmp)
}

/// Run `search_symbols` for the given substring and return the first
/// hit's FQN. We use this to resolve the mangled FQN of `target`
/// without hard-coding the mangling scheme — if it ever changes,
/// this still finds the right element.
async fn first_symbol_fqn(server: &LegendMcpServer, query: &str) -> String {
    let result = server
        .search_symbols(Parameters(SearchSymbolsArgs {
            query: query.to_string(),
            limit: Some(50),
        }))
        .await
        .expect("search_symbols ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse search_symbols json");
    let arr = parsed.as_array().expect("search_symbols returns array");
    assert!(
        !arr.is_empty(),
        "search_symbols returned no hits for {query:?}: {json}"
    );
    arr[0]["fqn"].as_str().expect("fqn is a string").to_string()
}

#[tokio::test]
async fn find_references_returns_callsite_for_called_function() {
    // Two files: target() in a.pure, caller() in b.pure that calls
    // pkg::target(). `find_references` on target's FQN should return
    // the call site inside caller's body.
    let a = "function pkg::target(): String[1] { 'ok' }\n";
    let b = "function pkg::caller(): String[1] { pkg::target() }\n";
    let (server, _tmp) = build_server(&[("/proj/a.pure", a), ("/proj/b.pure", b)]);

    let target_fqn = first_symbol_fqn(&server, "pkg::target").await;

    let result = server
        .find_references(Parameters(FindReferencesArgs {
            fqn: target_fqn.clone(),
        }))
        .await
        .expect("find_references ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("parse find_references json");
    let arr = parsed.as_array().expect("find_references returns array");

    assert_eq!(
        arr.len(),
        1,
        "expected exactly one reference site, got {arr:#?} (target_fqn={target_fqn})"
    );
    let row = &arr[0];
    assert_eq!(
        row["source"].as_str().expect("source string"),
        "/proj/b.pure",
        "reference should live in b.pure: {row:#?}"
    );
    assert_eq!(
        row["line"].as_u64().expect("line"),
        1,
        "caller body is on line 1"
    );
    // Column is wherever `pkg::target()` starts inside the body —
    // we don't pin the exact value (depends on lexer span shape for
    // qualified names); only that it's past the opening brace.
    let col = row["column"].as_u64().expect("column");
    assert!(col > 30, "column should land past the '{{' (got {col})");
}

#[tokio::test]
async fn find_references_unknown_fqn_returns_error() {
    let a = "function pkg::target(): String[1] { 'ok' }\n";
    let (server, _tmp) = build_server(&[("/proj/a.pure", a)]);

    let err = server
        .find_references(Parameters(FindReferencesArgs {
            fqn: "pkg::does_not_exist".to_string(),
        }))
        .await
        .expect_err("unknown fqn should error");
    let msg = err.message.to_string();
    assert!(
        msg.contains("element not found"),
        "error message should call out missing element: {msg}"
    );
    assert!(
        msg.contains("pkg::does_not_exist"),
        "error message should echo the bad FQN: {msg}"
    );
}

#[tokio::test]
async fn find_references_empty_for_unused_function() {
    // target() exists but nothing calls it; find_references should
    // return [] (success), not an error.
    let a = "function pkg::target(): String[1] { 'ok' }\n";
    let (server, _tmp) = build_server(&[("/proj/a.pure", a)]);

    let target_fqn = first_symbol_fqn(&server, "pkg::target").await;
    let result = server
        .find_references(Parameters(FindReferencesArgs {
            fqn: target_fqn.clone(),
        }))
        .await
        .expect("find_references ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let arr = parsed.as_array().expect("returns array");
    assert!(
        arr.is_empty(),
        "expected zero references for unused function, got {arr:#?}"
    );
}
