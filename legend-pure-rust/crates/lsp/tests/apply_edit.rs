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

//! Integration tests for the LSP-side apply-edit path
//! (`handlers::apply_edit_through_workspace`). Builds a real
//! filesystem repo under a tmp dir, drives the helper, asserts:
//!
//! - the on-disk file content is rewritten,
//! - `Workspace.base_repos` carries the new in-memory content,
//! - `Workspace.dirty_files` is populated **before** the next compile
//!   clears it,
//! - the subsequent `Workspace::compile()` sees the new content
//!   (verified by checking the parse + diagnostic state).
//!
//! Driving the helper directly (rather than the full `Backend` +
//! tokio runtime) keeps the test deterministic — there's no client to
//! mock and no network round-trip to wait on.

use legend_pure_core_platform::edit::{Position, Range, TextEdit};
use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_lsp::handlers;
use legend_pure_lsp::workspace::Workspace;
use tower_lsp_server::ls_types::Uri;

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

fn pos(line: u32, character: u32) -> Position {
    Position { line, character }
}

fn rng(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
    Range {
        start: pos(sl, sc),
        end: pos(el, ec),
    }
}

fn edit(rng: Range, text: &str) -> TextEdit {
    TextEdit {
        range: rng,
        new_text: text.to_string(),
    }
}

/// Build a [`Workspace`] over a tmp filesystem repo containing a
/// single `.pure` source with the supplied initial content. Returns
/// the workspace plus the disk path so the caller can verify
/// post-write content.
///
/// The repo is constructed with an explicit [`RepoMeta`] (rather
/// than via [`Repo::from_filesystem`]) so the cross-repo topo-sort
/// inside `WorkspaceSnapshot::compile` accepts it. Without the
/// meta the test workspace's compile path errors out before it
/// can reach the parse stage, which would mask the very behaviour
/// these tests exist to verify.
fn workspace_with_one_file(initial: &str) -> (Workspace, std::path::PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir(&proj_dir).expect("mkdir proj");
    let file_path = proj_dir.join("foo.pure");
    std::fs::write(&file_path, initial).expect("write");
    let repo = Repo::Filesystem {
        prefix: "/proj".to_string(),
        files: vec![OwnedSourceFile {
            path: "/proj/foo.pure".to_string(),
            content: initial.to_string(),
        }],
        meta: Some(make_meta("proj", Vec::new())),
        source_root: Some(proj_dir),
    };
    let ws = Workspace::new(vec![repo], Vec::new());
    (ws, file_path, tmp)
}

#[test]
fn apply_edit_rewrites_disk_and_in_memory_content() {
    let initial = "// comment\nfunction proj::greet(): String[1]\n{\n  'hi'\n}\n";
    let (mut ws, file_path, _tmp) = workspace_with_one_file(initial);

    // Replace 'hi' with 'HELLO' (line 3, chars 2..6).
    let edits = vec![edit(rng(3, 2, 3, 6), "'HELLO'")];
    let applied = handlers::apply_edit_through_workspace(&mut ws, "/proj/foo.pure", &edits)
        .expect("apply_edit_through_workspace");

    // 1) On-disk content rewritten.
    let on_disk = std::fs::read_to_string(&file_path).expect("read");
    assert!(on_disk.contains("'HELLO'"), "on-disk: {on_disk}");
    assert!(!on_disk.contains("'hi'"), "on-disk should not contain 'hi'");
    assert_eq!(on_disk, applied.new_content);

    // 2) In-memory base_repos carries the new content.
    if let Repo::Filesystem { files, .. } = &ws.base_repos[0] {
        let f = files
            .iter()
            .find(|f| f.path == "/proj/foo.pure")
            .expect("file present in base_repos");
        assert_eq!(f.content, applied.new_content);
    } else {
        panic!("expected Filesystem repo");
    }

    // 3) dirty_files populated BEFORE we run compile().
    assert!(
        ws.dirty_files.contains(applied.canonical_path.as_str()),
        "dirty_files should hold /proj/foo.pure; got {:?}",
        ws.dirty_files,
    );

    // 4) Subsequent compile() succeeds and observes the new content.
    //    The platform repo doesn't ship in this workspace so we won't
    //    get a full Pure compile — but the file should parse cleanly,
    //    so no parseFailure diagnostics for it. The first compile is
    //    a cold compile (model is None pre-compile); after it
    //    dirty_files is drained.
    ws.compile();
    assert!(
        ws.dirty_files.is_empty(),
        "compile() should drain dirty_files"
    );
}

#[test]
fn compile_after_apply_edit_sees_the_new_content() {
    // Inject a parse error via apply_edit, then verify the next
    // compile() surfaces a diagnostic for the canonical path. This
    // is the direct check that the chain — mutate base_repos ->
    // dirty_files insert -> compile pulls fresh content via
    // snapshot_repos -> diagnostics keyed by canonical path —
    // works end to end. Without this we'd only know the disk + the
    // in-memory copy changed, not that the compiler honoured it.
    let initial = "Class proj::Person { name: String[1]; }\n";
    let (mut ws, _file_path, _tmp) = workspace_with_one_file(initial);

    // Force the workspace into a known-good baseline (no diagnostics
    // for our file) so the post-edit assertion is unambiguous.
    ws.compile();
    assert!(
        !ws.diagnostics.contains_key("/proj/foo.pure")
            || ws
                .diagnostics
                .get("/proj/foo.pure")
                .is_some_and(Vec::is_empty),
        "baseline must be diagnostic-free for the edited file; got: {:?}",
        ws.diagnostics.get("/proj/foo.pure"),
    );

    // Now apply an edit that swaps the class body for invalid syntax.
    let edits = vec![edit(
        rng(0, 0, 0, 39),
        "Class proj::Person { name: $$$INVALID$$$ }",
    )];
    handlers::apply_edit_through_workspace(&mut ws, "/proj/foo.pure", &edits)
        .expect("apply_edit_through_workspace");

    // The recompile MUST pull the freshly-mutated content and emit
    // a parse error keyed by the canonical path.
    ws.compile();
    let diags = ws
        .diagnostics
        .get("/proj/foo.pure")
        .expect("post-edit diagnostics for /proj/foo.pure")
        .clone();
    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic after the malformed edit; got: {diags:?}",
    );
}

#[test]
fn apply_edit_overrides_stale_open_buffer() {
    // Regression test for the open-buffer overlay clobber:
    // `Workspace::snapshot_repos` overlays every `open_buffers` entry
    // onto `base_repos` before compile. If `apply_edit_through_workspace`
    // mutates `base_repos` but leaves a matching `open_buffers` entry
    // alone, the next compile re-clobbers the file back to the stale
    // buffer content — so returned diagnostics describe pre-edit
    // source, not the edit the agent just made.
    //
    // To make the failure mode unambiguous, we deliberately set the
    // open buffer to source that would parse CLEANLY, while the edit
    // we apply introduces a parse error. If the overlay clobber bug
    // returned, the post-edit compile would see the clean buffer and
    // produce no diagnostic — the assertion below would fail.
    let on_disk = "Class proj::Person { name: String[1]; }\n";
    let (mut ws, file_path, _tmp) = workspace_with_one_file(on_disk);

    // Establish a clean baseline so the post-edit assertion is
    // unambiguous (no pre-existing diagnostics for this file).
    ws.compile();

    // Pretend the IDE opened the file with content that would also
    // compile cleanly. Crucially this is the source the broken
    // snapshot_repos overlay would re-apply over the edit.
    let clean_buffer = "Class proj::Person { age: Integer[1]; }\n";
    let uri = Uri::from_file_path(&file_path).expect("Uri::from_file_path");
    ws.set_open_buffer(uri.clone(), clean_buffer.to_string());

    // Apply an edit that breaks parsing. Replace line 0 plus its
    // trailing newline (range end is (1, 0)) so the new text can
    // supply its own newline — keeps the position math obviously
    // correct regardless of the original line's exact length.
    let edits = vec![edit(
        rng(0, 0, 1, 0),
        "Class proj::Person { name: $$$INVALID$$$ }\n",
    )];
    let applied = handlers::apply_edit_through_workspace(&mut ws, "/proj/foo.pure", &edits)
        .expect("apply_edit_through_workspace");

    // The matching open_buffer entry must have been updated to the
    // post-edit content — that's the fix's contract.
    assert_eq!(
        ws.open_buffers.get(&uri).map(String::as_str),
        Some(applied.new_content.as_str()),
        "open_buffers entry must reflect the post-edit content",
    );

    // And the recompile MUST surface a parse-error diagnostic for
    // the canonical path, proving the compiler saw the edited
    // source rather than the stale clean buffer.
    ws.compile();
    let diags = ws
        .diagnostics
        .get("/proj/foo.pure")
        .expect("post-edit diagnostics for /proj/foo.pure")
        .clone();
    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic after the broken edit; got: {diags:?}",
    );
}

#[test]
fn apply_edit_rejects_unknown_path() {
    let (mut ws, _file_path, _tmp) = workspace_with_one_file("a\n");
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    let err = handlers::apply_edit_through_workspace(&mut ws, "/proj/no-such.pure", &edits)
        .expect_err("unknown path must error");
    assert!(
        matches!(
            err,
            legend_pure_core_platform::edit::EditError::FileNotInWorkspace { .. }
        ),
        "got: {err:?}"
    );
}

#[test]
fn apply_edit_rejects_overlap_without_disk_write() {
    let (mut ws, file_path, _tmp) = workspace_with_one_file("abcdef\n");
    let edits = vec![edit(rng(0, 0, 0, 4), "X"), edit(rng(0, 2, 0, 5), "Y")];
    let err = handlers::apply_edit_through_workspace(&mut ws, "/proj/foo.pure", &edits)
        .expect_err("overlap must error");
    assert!(
        matches!(
            err,
            legend_pure_core_platform::edit::EditError::OverlappingEdits { .. }
        ),
        "got: {err:?}"
    );

    // The error path must not have written to disk OR mutated the
    // in-memory content.
    assert_eq!(
        std::fs::read_to_string(&file_path).expect("read"),
        "abcdef\n",
        "disk content must be unchanged after a rejected edit"
    );
    if let Repo::Filesystem { files, .. } = &ws.base_repos[0] {
        let f = files
            .iter()
            .find(|f| f.path == "/proj/foo.pure")
            .expect("present");
        assert_eq!(f.content, "abcdef\n", "in-memory content must be unchanged");
    }
}
