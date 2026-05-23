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

//! Unit + integration tests for the shared apply-edit helper that
//! both the LSP and MCP servers consume.

use std::path::PathBuf;

use legend_pure_core_platform::edit::{
    self, EditError, Position, Range, TextEdit, apply_text_edits, write_to_disk,
};
use legend_pure_core_platform::repo::{OwnedSourceFile, Repo};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fs_repo(prefix: &str, root: PathBuf, files: Vec<(&str, &str)>) -> Repo {
    Repo::Filesystem {
        prefix: prefix.to_string(),
        files: files
            .into_iter()
            .map(|(p, c)| OwnedSourceFile {
                path: p.to_string(),
                content: c.to_string(),
            })
            .collect(),
        meta: None,
        source_root: Some(root),
    }
}

fn fs_repo_no_root(prefix: &str, files: Vec<(&str, &str)>) -> Repo {
    Repo::Filesystem {
        prefix: prefix.to_string(),
        files: files
            .into_iter()
            .map(|(p, c)| OwnedSourceFile {
                path: p.to_string(),
                content: c.to_string(),
            })
            .collect(),
        meta: None,
        source_root: None,
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

// ---------------------------------------------------------------------------
// Position translation
// ---------------------------------------------------------------------------

#[test]
fn position_translation_ascii() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "alpha beta gamma\n")],
    )];
    // Replace "beta" with "BETA"
    let edits = vec![edit(rng(0, 6, 0, 10), "BETA")];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "alpha BETA gamma\n");
}

#[test]
fn position_translation_multi_byte_utf8() {
    // `é` is 2 bytes UTF-8, 1 UTF-16 code unit.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "café latte\n")],
    )];
    // Replace " latte" with "!" — `café` is 4 UTF-16 code units even
    // though `é` takes 2 UTF-8 bytes.
    let edits = vec![edit(rng(0, 4, 0, 10), "!")];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "café!\n");
}

#[test]
fn position_translation_three_byte_utf8() {
    // `日` is 3 bytes UTF-8, 1 UTF-16 code unit.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "日本語 text\n")],
    )];
    // Replace " text" (5 UTF-16 cu after `日本語`) with "!!"
    let edits = vec![edit(rng(0, 3, 0, 8), "!!")];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "日本語!!\n");
}

#[test]
fn position_translation_surrogate_pair_emoji() {
    // `🎉` is U+1F389 — 4 bytes UTF-8, 2 UTF-16 code units (surrogate pair).
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "🎉 party\n")],
    )];
    // Replace "🎉 " (3 UTF-16 cu: 2 for the surrogate pair + 1 for space) with "HI "
    let edits = vec![edit(rng(0, 0, 0, 3), "HI ")];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "HI party\n");
}

// ---------------------------------------------------------------------------
// Multi-edit descending application
// ---------------------------------------------------------------------------

#[test]
fn multi_edit_applied_descending_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let content = "line one\nline two\nline three\n";
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", content)],
    )];
    let edits = vec![
        // Replace "one" -> "1"
        edit(rng(0, 5, 0, 8), "1"),
        // Replace "two" -> "2"
        edit(rng(1, 5, 1, 8), "2"),
        // Replace "three" -> "3"
        edit(rng(2, 5, 2, 10), "3"),
    ];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "line 1\nline 2\nline 3\n");
}

#[test]
fn touching_edits_allowed() {
    // a.end == b.start is allowed: edits can be adjacent without overlap.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "abcdef\n")],
    )];
    let edits = vec![edit(rng(0, 0, 0, 3), "XYZ"), edit(rng(0, 3, 0, 6), "WVU")];
    let result = apply_text_edits(&mut repos, "/p/x.pure", &edits).expect("apply");
    assert_eq!(result.new_content, "XYZWVU\n");
}

// ---------------------------------------------------------------------------
// Validation: overlap, out-of-range, unknown path, embedded/purem
// ---------------------------------------------------------------------------

#[test]
fn overlap_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "abcdef\n")],
    )];
    let edits = vec![edit(rng(0, 0, 0, 4), "X"), edit(rng(0, 2, 0, 5), "Y")];
    match apply_text_edits(&mut repos, "/p/x.pure", &edits) {
        Err(EditError::OverlappingEdits { .. }) => {}
        other => panic!("expected OverlappingEdits, got {other:?}"),
    }
}

#[test]
fn out_of_range_line_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "one\ntwo\n")],
    )];
    // File has 2 lines (after the trailing newline, line_starts = [0,
    // 4, 8]); line 5 is way past EOF.
    let edits = vec![edit(rng(5, 0, 5, 1), "X")];
    match apply_text_edits(&mut repos, "/p/x.pure", &edits) {
        Err(EditError::OutOfRangePosition { line: 5, .. }) => {}
        other => panic!("expected OutOfRangePosition, got {other:?}"),
    }
}

#[test]
fn out_of_range_character_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "abc\n")],
    )];
    // Line 0 is "abc" (3 chars); character 10 is past EOL.
    let edits = vec![edit(rng(0, 10, 0, 11), "X")];
    match apply_text_edits(&mut repos, "/p/x.pure", &edits) {
        Err(EditError::OutOfRangePosition { character: 10, .. }) => {}
        other => panic!("expected OutOfRangePosition character=10, got {other:?}"),
    }
}

#[test]
fn unknown_canonical_path_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut repos = vec![fs_repo(
        "/p",
        tmp.path().to_path_buf(),
        vec![("/p/x.pure", "a\n")],
    )];
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    match apply_text_edits(&mut repos, "/p/other.pure", &edits) {
        Err(EditError::FileNotInWorkspace { .. }) => {}
        other => panic!("expected FileNotInWorkspace, got {other:?}"),
    }
}

#[test]
fn embedded_repo_rejected() {
    // The embedded platform repo is always a `Repo::Embedded` carrying
    // a static slice. Constructing one ad-hoc requires the same shape;
    // we cheat by leaking a static file table out of a `Box`.
    use legend_pure_core_platform::repo::RepoMeta;
    use legend_pure_core_platform::sources::PureSourceFile;
    let files: &'static [PureSourceFile] = Box::leak(Box::new([PureSourceFile {
        path: "/embed/x.pure",
        content: "abc\n",
    }]));
    let meta: &'static RepoMeta = Box::leak(Box::new(RepoMeta {
        name: "embed",
        pattern: "(.*)",
        dependencies: &[],
    }));
    let mut repos = vec![Repo::Embedded {
        prefix: "/embed",
        files,
        meta,
    }];
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    match apply_text_edits(&mut repos, "/embed/x.pure", &edits) {
        Err(EditError::RepoNotWritable { kind: "embedded" }) => {}
        other => panic!("expected RepoNotWritable(embedded), got {other:?}"),
    }
}

#[test]
fn purem_repo_rejected() {
    use legend_pure_core_platform::repo::RepoMeta;
    use std::sync::Arc;
    let mut repos = vec![Repo::from_purem_bytes(
        "/p_purem",
        RepoMeta {
            name: "p_purem",
            pattern: "(.*)",
            dependencies: &[],
        },
        Arc::from(Vec::<u8>::new().into_boxed_slice()),
    )];
    // Purem repos carry empty manifests by default — file lookup
    // simply fails. That's still the "no match" path, which we want
    // to report as FileNotInWorkspace (consistent with "the file
    // simply isn't there"). The RepoNotWritable("purem") path is
    // exercised when a purem repo's `manifests` table happens to
    // surface a matching canonical path; covered by the next test.
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    match apply_text_edits(&mut repos, "/p_purem/x.pure", &edits) {
        Err(EditError::FileNotInWorkspace { .. }) => {}
        other => panic!("expected FileNotInWorkspace for empty-manifest purem, got {other:?}"),
    }

    // Now construct a purem with a static manifest entry and verify
    // the RepoNotWritable path.
    let manifests: &'static [(&'static str, &'static str)] =
        Box::leak(Box::new([("/p_purem2/data.json", "{}")]));
    let meta: &'static RepoMeta = Box::leak(Box::new(RepoMeta {
        name: "p_purem2",
        pattern: "(.*)",
        dependencies: &[],
    }));
    let mut with_manifest = vec![Repo::from_purem_static_with_manifests(
        "/p_purem2",
        meta,
        &[],
        manifests,
    )];
    match apply_text_edits(&mut with_manifest, "/p_purem2/data.json", &edits) {
        Err(EditError::RepoNotWritable { kind: "purem" }) => {}
        other => panic!("expected RepoNotWritable(purem), got {other:?}"),
    }
}

#[test]
fn no_source_root_rejected() {
    let mut repos = vec![fs_repo_no_root("/p", vec![("/p/x.pure", "a\n")])];
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    match apply_text_edits(&mut repos, "/p/x.pure", &edits) {
        Err(EditError::RepoMissingSourceRoot { .. }) => {}
        other => panic!("expected RepoMissingSourceRoot, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// End-to-end round trip
// ---------------------------------------------------------------------------

#[test]
fn end_to_end_round_trip_through_filesystem_repo() {
    // Use a real on-disk file under a tmp dir so write_to_disk has
    // somewhere to land. The Repo::from_filesystem walk discovers it
    // and the suffix-match resolver can find it by canonical URL.
    let tmp = tempfile::tempdir().expect("tempdir");
    let file_path = tmp.path().join("hello.pure");
    std::fs::write(&file_path, "function hello(): String[1]\n{\n  'old'\n}\n").expect("write");
    let repo = Repo::from_filesystem(tmp.path(), "/proj").expect("from_filesystem");
    let mut repos = vec![repo];

    // Replace 'old' with 'new'. Inner string is on line 2,
    // characters 2..7 (0-indexed: `  'old'` — two spaces then the
    // 5-char `'old'` literal at columns 2..7).
    let edits = vec![edit(rng(2, 2, 2, 7), "'NEW'")];

    let applied = apply_text_edits(&mut repos, "/proj/hello.pure", &edits).expect("apply");
    assert!(applied.new_content.contains("'NEW'"));
    assert!(!applied.new_content.contains("'old'"));

    // Disk write.
    write_to_disk(&applied).expect("write_to_disk");
    let on_disk = std::fs::read_to_string(&file_path).expect("read");
    assert_eq!(on_disk, applied.new_content);

    // In-memory mutation reflected too.
    if let Repo::Filesystem { files, .. } = &repos[0] {
        let f = files.iter().find(|f| f.path == "/proj/hello.pure").unwrap();
        assert_eq!(f.content, applied.new_content);
    } else {
        panic!("expected Filesystem repo");
    }

    // The disk path resolves cleanly under tmp dir.
    assert_eq!(applied.disk_path, file_path);
    assert_eq!(applied.canonical_path.as_str(), "/proj/hello.pure");
}

#[test]
fn end_to_end_absolute_disk_path_match() {
    // Verify that we can locate the file by passing its absolute disk
    // path (the MCP plan calls out this match shape explicitly).
    //
    // For the suffix-match to fire on the absolute path the on-disk
    // layout has to mirror the canonical's "/{repo}/{rel}" tail —
    // drop the file under a `proj/` subdir of the tmp root and point
    // the repo's source_root at that subdir.
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir(&proj_dir).expect("mkdir proj");
    let file_path = proj_dir.join("hello.pure");
    std::fs::write(&file_path, "abc\n").expect("write");
    let repo = Repo::from_filesystem(&proj_dir, "/proj").expect("from_filesystem");
    let mut repos = vec![repo];
    let edits = vec![edit(rng(0, 0, 0, 1), "X")];
    let abs_path = file_path.to_string_lossy().into_owned();
    let applied = apply_text_edits(&mut repos, &abs_path, &edits).expect("apply by abs path");
    assert_eq!(applied.new_content, "Xbc\n");
}

// ---------------------------------------------------------------------------
// JSON wire-shape regression
// ---------------------------------------------------------------------------

#[test]
fn text_edit_wire_uses_newtext_camel_case() {
    let value = serde_json::json!({
        "range": {
            "start": {"line": 0, "character": 0},
            "end":   {"line": 0, "character": 3}
        },
        "newText": "abc"
    });
    let parsed: edit::TextEdit = serde_json::from_value(value).expect("from_value");
    assert_eq!(parsed.new_text, "abc");
    assert_eq!(parsed.range.start.line, 0);
    assert_eq!(parsed.range.end.character, 3);
}
