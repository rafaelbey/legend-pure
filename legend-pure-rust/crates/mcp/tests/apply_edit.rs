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

//! Integration tests for the MCP `apply_edit` tool. Constructs a real
//! [`LegendMcpServer`] over a tmp filesystem repo, calls
//! `server.apply_edit(...)` directly (the `#[tool]`-decorated method
//! is a regular async fn — no rmcp transport scaffolding needed), and
//! asserts:
//!
//! - the on-disk file content was rewritten,
//! - the `current` snapshot's `compiled_at` advanced past the pre-edit
//!   snapshot's timestamp,
//! - the returned diagnostics reflect the post-edit state (empty in
//!   the happy path since we edit a single trivial file with no
//!   compile errors).
//!
//! The `apply_edit` tool intentionally takes its arguments through
//! the rmcp `Parameters(args)` wrapper. We construct that wrapper
//! directly here.

use std::sync::{Arc, Mutex};

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_mcp::{
    LegendMcpServer, WorkspaceSnapshot,
    server::{ApplyEditArgs, WirePosition, WireRange, WireTextEdit},
};
use rmcp::handler::server::wrapper::Parameters;

/// Build a `RepoMeta` whose `&'static` fields are leaked once per
/// call. Mirrors the helper in `lsp/tests/incremental_recompile.rs`.
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

fn pos(line: u32, character: u32) -> WirePosition {
    WirePosition { line, character }
}

fn rng(sl: u32, sc: u32, el: u32, ec: u32) -> WireRange {
    WireRange {
        start: pos(sl, sc),
        end: pos(el, ec),
    }
}

fn edit(r: WireRange, text: &str) -> WireTextEdit {
    WireTextEdit {
        range: r,
        new_text: text.to_string(),
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

/// Stand up an MCP server over a tmp filesystem repo containing one
/// `.pure` file. Returns the server, the on-disk file path, the tmp
/// dir guard (to keep the dir alive for the test), and the canonical
/// path the edit will target.
fn build_server(
    initial: &str,
) -> (
    LegendMcpServer,
    std::path::PathBuf,
    tempfile::TempDir,
    String,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir(&proj_dir).expect("mkdir proj");
    let file_path = proj_dir.join("foo.pure");
    std::fs::write(&file_path, initial).expect("write");
    // Build the repo directly so we can stamp a RepoMeta on it
    // (Repo::from_filesystem leaves meta=None, which trips the
    // topo-sort guard that requires every repo to identify itself).
    let canonical = "/proj/foo.pure".to_string();
    let repo = Repo::Filesystem {
        prefix: "/proj".to_string(),
        files: vec![OwnedSourceFile {
            path: canonical.clone(),
            content: initial.to_string(),
        }],
        meta: Some(make_meta("proj", Vec::new())),
        source_root: Some(proj_dir.clone()),
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
    (server, file_path, tmp, canonical)
}

#[tokio::test]
async fn apply_edit_rewrites_disk_and_advances_compiled_at() {
    let initial = "// trivial\n";
    let (server, file_path, _tmp, canonical) = build_server(initial);

    // Capture the pre-edit compiled_at timestamp by calling the
    // workspace_status tool (it returns the same status_from).
    // jiff::Timestamp has nanosecond resolution; the apply_edit
    // recompile spans at minimum a few ms, so the new timestamp
    // will strictly exceed the old one.
    let pre_status = server.workspace_status().await.expect("pre status");
    let pre_text = extract_text_content(&pre_status);

    let args = ApplyEditArgs {
        file: canonical.clone(),
        edits: vec![edit(rng(0, 0, 0, 10), "// edited")],
    };
    let result = server
        .apply_edit(Parameters(args))
        .await
        .expect("apply_edit ok");
    let result_text = extract_text_content(&result);

    // 1) On-disk content was updated. The edit replaces line 0 chars
    //    0..10 ("// trivial") with "// edited"; the trailing newline
    //    is preserved.
    let on_disk = std::fs::read_to_string(&file_path).expect("read");
    assert_eq!(on_disk, "// edited\n", "on-disk content: {on_disk:?}");

    // 2) The post-edit response carries a status object with a new
    //    compiled_at timestamp.
    let parsed: serde_json::Value = serde_json::from_str(&result_text).expect("parse json");
    let post_compiled_at = parsed["status"]["compiled_at"]
        .as_str()
        .expect("status.compiled_at present")
        .to_string();
    assert!(
        !post_compiled_at.is_empty(),
        "post compiled_at: {post_compiled_at}"
    );
    // Compare to the pre snapshot status. The whole-JSON comparison
    // is enough: even if the model is unchanged structurally, the
    // compiled_at timestamp differs (jiff::Timestamp::now() between
    // two compiles is strictly increasing on every supported
    // platform).
    assert_ne!(
        pre_text, result_text,
        "post apply_edit status should differ from pre",
    );

    // 3) The diagnostics for the edited file are returned (and empty
    //    for this trivial edit).
    let diagnostics = parsed["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diagnostics.is_empty(),
        "expected zero post-edit diagnostics; got: {diagnostics:?}",
    );

    // 4) The status's compiled_at is strictly greater than the pre
    //    status's compiled_at — parsed as RFC 3339 jiff::Timestamp.
    let pre_parsed: serde_json::Value = serde_json::from_str(&pre_text).expect("parse pre status");
    let pre_compiled_at = pre_parsed["compiled_at"]
        .as_str()
        .expect("pre compiled_at present")
        .to_string();
    let pre_ts: jiff::Timestamp = pre_compiled_at.parse().expect("parse pre ts");
    let post_ts: jiff::Timestamp = post_compiled_at.parse().expect("parse post ts");
    assert!(post_ts > pre_ts, "post {post_ts} must exceed pre {pre_ts}",);
}

#[tokio::test]
async fn apply_edit_injects_parse_error_visible_in_diagnostics() {
    // Inject a deliberately broken edit (turn a valid file into one
    // with a parse error) and assert the post-edit diagnostics
    // surface a parseFailure for the edited file. This is the direct
    // check that the recompile chain — mutate `repos` → clone →
    // `WorkspaceSnapshot::compile` — actually sees the new content,
    // not a cached snapshot.
    let initial = "Class proj::Person { name: String[1]; }\n";
    let (server, file_path, _tmp, canonical) = build_server(initial);

    // Replace the entire class declaration with a deliberately
    // malformed Pure source. `@@@` is not a valid Pure section
    // header / element, so the parser must surface an error on
    // line 0.
    let args = ApplyEditArgs {
        file: canonical.clone(),
        edits: vec![edit(
            rng(0, 0, 0, 39),
            "Class proj::Person { name: $$$INVALID$$$ }",
        )],
    };
    let result = server
        .apply_edit(Parameters(args))
        .await
        .expect("apply_edit ok");

    // Disk sanity: the new on-disk content should contain our
    // injected garbage token.
    let on_disk = std::fs::read_to_string(&file_path).expect("read");
    assert!(
        on_disk.contains("$$$INVALID$$$"),
        "expected injected garbage in file content; got: {on_disk:?}"
    );

    let text = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("parse json");
    let diagnostics = parsed["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        !diagnostics.is_empty(),
        "post-edit diagnostics must surface at least one error for the malformed file; \
         got: {diagnostics:?}"
    );
    let all_for_file = diagnostics
        .iter()
        .all(|d| d["source"].as_str().is_some_and(|s| s == canonical));
    assert!(
        all_for_file,
        "every diagnostic in the apply_edit response must scope to the edited file; got: {diagnostics:?}"
    );
    // Workspace-level error_count agrees (the parse failure is the
    // single error in this tiny workspace).
    let total = parsed["status"]["error_count"].as_u64();
    assert_eq!(total, Some(1), "expected exactly one workspace error");
}

#[tokio::test]
async fn apply_edit_unknown_file_returns_error() {
    let (server, _file_path, _tmp, _canonical) = build_server("a\n");
    let args = ApplyEditArgs {
        file: "/proj/no-such.pure".to_string(),
        edits: vec![edit(rng(0, 0, 0, 1), "X")],
    };
    let err = server
        .apply_edit(Parameters(args))
        .await
        .expect_err("unknown path must error");
    // The error variant is rmcp's invalid_params with the underlying
    // `FileNotInWorkspace` message. We assert on the message
    // substring rather than the variant since rmcp's ErrorData
    // surface keeps the structured error opaque.
    let msg = err.message.to_string();
    assert!(msg.contains("file not found in workspace"), "got: {msg}");
}
