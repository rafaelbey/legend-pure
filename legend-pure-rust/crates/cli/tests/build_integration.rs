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

//! End-to-end fixture tests for `legend build`.
//!
//! Each test stands up a synthetic classpath in a tempdir with two
//! filesystem-kind repos (`b` and `a`, where `a` depends on `b`),
//! drives the real `legend` binary at it via `assert_cmd`, and
//! verifies cache behavior by inspecting the materialized
//! `target/legend/<repo>/{source.purem,cache.toml}` artifacts.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::Command;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    classpath: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();

        // Descriptor JSONs at <root>/<name>.definition.json, source
        // roots at <root>/<name>/ — matches `Repo::from_descriptor`'s
        // contract (descriptor parent + name → source root).
        write(
            &root.join("b.definition.json"),
            br#"{"name":"b","pattern":".*","dependencies":[]}"#,
        );
        write(
            &root.join("b").join("x.pure"),
            b"Class b::Foo\n{\n   name: String[1];\n}\n",
        );

        write(
            &root.join("a.definition.json"),
            br#"{"name":"a","pattern":".*","dependencies":["b"]}"#,
        );
        write(
            &root.join("a").join("x.pure"),
            b"Class a::Bar\n{\n   tag: String[1];\n}\n",
        );

        let classpath = root.join("legend-pure-classpath.toml");
        write(
            &classpath,
            br#"
[[repo]]
name = "b"
kind = "filesystem"
descriptor = "b.definition.json"

[[repo]]
name = "a"
kind = "filesystem"
descriptor = "a.definition.json"
"#,
        );

        Self {
            _dir: dir,
            root,
            classpath,
        }
    }

    fn cache_dir(&self) -> PathBuf {
        self.root.join("target").join("legend")
    }

    fn purem(&self, repo: &str) -> PathBuf {
        self.cache_dir().join(repo).join("source.purem")
    }

    fn stamp(&self, repo: &str) -> PathBuf {
        self.cache_dir().join(repo).join("cache.toml")
    }

    fn build(&self) -> Command {
        let mut cmd = Command::cargo_bin("legend").expect("legend bin");
        cmd.arg("--classpath")
            .arg(&self.classpath)
            .arg("build")
            .arg("--skip-tests");
        cmd
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, bytes).expect("write");
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

fn stamp_field(stamp_path: &Path, key: &str) -> String {
    let text = std::fs::read_to_string(stamp_path).expect("read stamp");
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&format!("{key} = ")) {
            return rest.trim_matches('"').to_string();
        }
    }
    panic!("key {key} not found in stamp {}", stamp_path.display());
}

#[test]
fn cold_build_produces_purem_and_stamp_per_repo() {
    let fx = Fixture::new();

    fx.build().assert().success();

    assert!(fx.purem("b").is_file(), "b/source.purem must exist");
    assert!(fx.purem("a").is_file(), "a/source.purem must exist");
    assert!(fx.stamp("b").is_file(), "b/cache.toml must exist");
    assert!(fx.stamp("a").is_file(), "a/cache.toml must exist");

    // Sentinel marker dropped so --clean is permitted on the next run.
    assert!(
        fx.cache_dir().join(".legend-cache").is_file(),
        "sentinel must be written on first build"
    );

    let b_fp = stamp_field(&fx.stamp("b"), "fingerprint");
    assert_eq!(b_fp.len(), 64, "fingerprint must be 64 hex chars");
}

#[test]
fn warm_rerun_is_a_no_op_for_purem_files() {
    let fx = Fixture::new();
    fx.build().assert().success();
    let b_before = mtime(&fx.purem("b"));
    let a_before = mtime(&fx.purem("a"));

    // Ensure enough delay that any rewrite would tick the mtime.
    std::thread::sleep(std::time::Duration::from_millis(50));

    fx.build().assert().success();

    assert_eq!(
        mtime(&fx.purem("b")),
        b_before,
        "b/source.purem must not be rewritten on warm rerun"
    );
    assert_eq!(
        mtime(&fx.purem("a")),
        a_before,
        "a/source.purem must not be rewritten on warm rerun"
    );
}

#[test]
fn upstream_source_change_invalidates_downstream() {
    let fx = Fixture::new();
    fx.build().assert().success();
    let b_fp_before = stamp_field(&fx.stamp("b"), "fingerprint");
    let a_fp_before = stamp_field(&fx.stamp("a"), "fingerprint");
    let b_purem_before = mtime(&fx.purem("b"));
    let a_purem_before = mtime(&fx.purem("a"));

    std::thread::sleep(std::time::Duration::from_millis(50));

    // Append a trailing newline to b's source — content changes,
    // fingerprint must change, and a (which depends on b) must
    // also be rebuilt because its dep_fingerprints rolls forward.
    std::fs::write(
        fx.root.join("b").join("x.pure"),
        b"Class b::Foo\n{\n   name: String[1];\n   note: String[1];\n}\n",
    )
    .expect("rewrite");

    fx.build().assert().success();

    let b_fp_after = stamp_field(&fx.stamp("b"), "fingerprint");
    let a_fp_after = stamp_field(&fx.stamp("a"), "fingerprint");
    assert_ne!(b_fp_before, b_fp_after, "b's fingerprint must change");
    assert_ne!(
        a_fp_before, a_fp_after,
        "a's fingerprint must cascade from b's invalidation"
    );
    assert!(
        mtime(&fx.purem("b")) > b_purem_before,
        "b purem must be rewritten"
    );
    assert!(
        mtime(&fx.purem("a")) > a_purem_before,
        "a purem must be rewritten (dep cascade)"
    );
}

#[test]
fn clean_flag_removes_cache_then_rebuilds() {
    let fx = Fixture::new();
    fx.build().assert().success();
    let b_before = mtime(&fx.purem("b"));
    assert!(fx.purem("b").is_file());

    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut cmd = Command::cargo_bin("legend").expect("legend bin");
    cmd.arg("--classpath")
        .arg(&fx.classpath)
        .arg("build")
        .arg("--skip-tests")
        .arg("--clean")
        .assert()
        .success();

    let b_after = mtime(&fx.purem("b"));
    assert!(
        b_after > b_before,
        "--clean must force rebuild (mtime should advance)"
    );
}

#[test]
fn clean_refuses_unmarked_cache_dir() {
    let fx = Fixture::new();
    // Point --cache-dir at a directory that doesn't carry the
    // sentinel, then try --clean. Must fail without deleting
    // anything.
    let unrelated = fx.root.join("scratch");
    std::fs::create_dir_all(&unrelated).expect("mkdir");
    std::fs::write(unrelated.join("important.txt"), b"data").expect("write");

    let mut cmd = Command::cargo_bin("legend").expect("legend bin");
    cmd.arg("--classpath")
        .arg(&fx.classpath)
        .arg("build")
        .arg("--skip-tests")
        .arg("--clean")
        .arg("--cache-dir")
        .arg(&unrelated)
        .assert()
        .failure();

    assert!(
        unrelated.join("important.txt").exists(),
        "important file must survive a refused --clean"
    );
}

#[test]
fn parse_error_in_upstream_blocks_downstream() {
    let fx = Fixture::new();

    // Replace b's source with invalid Pure that the parser rejects.
    std::fs::write(
        fx.root.join("b").join("x.pure"),
        b"Class b::Bro {\n   <<>>this is not pure grammar\n}\n",
    )
    .expect("rewrite");

    fx.build().assert().failure();

    // b's purem must NOT have been written.
    assert!(
        !fx.purem("b").exists(),
        "b/source.purem must not be materialized on parse failure"
    );
    // a's purem must also not be present — it was blocked.
    assert!(
        !fx.purem("a").exists(),
        "a/source.purem must not be materialized when its dep failed"
    );
}

#[test]
fn no_write_purem_skips_materialization() {
    let fx = Fixture::new();

    let mut cmd = Command::cargo_bin("legend").expect("legend bin");
    cmd.arg("--classpath")
        .arg(&fx.classpath)
        .arg("build")
        .arg("--skip-tests")
        .arg("--no-write-purem")
        .assert()
        .success();

    assert!(
        !fx.purem("b").exists(),
        "--no-write-purem must skip purem materialization"
    );
    assert!(
        !fx.stamp("b").exists(),
        "--no-write-purem must also skip stamp writes (no stable artifact to record)"
    );
}
