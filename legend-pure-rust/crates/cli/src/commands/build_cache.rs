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

//! Content-hash-keyed incremental cache for `legend build`.
//!
//! Each filesystem (or embedded) repo gets a directory
//! `<cache>/<repo_name>/` containing a serialized `.purem` slice
//! (`source.purem`) and a stamp file (`cache.toml`) recording the
//! fingerprint that produced it. A subsequent build computes the same
//! fingerprint from current inputs: if it matches the stamp and the
//! purem file still exists, the cached purem is loaded instead of
//! re-running parse + compile + serialize.
//!
//! The cache directory is gated by a `.legend-cache` sentinel file
//! that [`clean_cache`] requires before removing the directory. This
//! prevents accidental `rm -rf` of unrelated paths supplied via
//! `--cache-dir`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use legend_pure_core_platform::repo::Repo;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Sentinel filename written into the cache directory on first
/// successful write. Required by [`clean_cache`] before any deletion.
pub const SENTINEL: &str = ".legend-cache";

/// Current schema version for the [`Stamp`] file. Bumped when the
/// stamp's TOML shape changes in a backwards-incompatible way.
pub const STAMP_SCHEMA: u32 = 1;

/// blake3 fingerprint of a repo's compile inputs. 32 bytes; serialized
/// to disk as 64-char lowercase hex.
pub type Fingerprint = [u8; 32];

/// Outcome recorded by the previous build's test phase. Drives the
/// "cache test results too" semantics: a `Pass` stamp + matching
/// fingerprint lets the next build skip tests for the repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestResult {
    /// The previous build ran tests for this repo and they all passed.
    Pass,
    /// The previous build ran tests for this repo and at least one
    /// failed or errored.
    Fail,
    /// The previous build did not run tests (e.g. `--skip-tests`).
    Skipped,
}

/// On-disk stamp file (`<cache>/<repo>/cache.toml`). Captures the
/// fingerprint that produced the sibling `source.purem`, the CLI
/// version that built it, and the test outcome recorded for the next
/// run's "cache tests on pass" decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stamp {
    /// [`STAMP_SCHEMA`] at write time.
    pub schema: u32,
    /// 64-char lowercase hex of the blake3 fingerprint.
    pub fingerprint: String,
    /// `CARGO_PKG_VERSION` of the legend-cli binary that wrote this
    /// stamp. Mismatch ⇒ cache miss.
    pub cli_version: String,
    /// `legend_pure_parser_pure::purem::header::SCHEMA_HASH` as 8-char
    /// lowercase hex. Mismatch ⇒ cache miss.
    pub purem_schema: String,
    /// UNIX-epoch seconds at write time. Debug-friendly only; not used
    /// for cache decisions.
    pub generated_at: u64,
    /// Direct-dependency repos → their fingerprint at the time this
    /// repo was compiled. Sorted by repo name for deterministic
    /// serialization.
    pub dep_fingerprints: BTreeMap<String, String>,
    /// Outcome of the previous build's test phase for this repo.
    pub last_test_result: TestResult,
}

/// Convert a binary fingerprint to lowercase hex.
#[must_use]
pub fn fingerprint_to_hex(fp: &Fingerprint) -> String {
    let mut out = String::with_capacity(64);
    for byte in fp {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Parse 64-char lowercase hex back into a binary fingerprint.
/// Returns `None` if the input is not exactly 64 hex chars.
#[must_use]
pub fn fingerprint_from_hex(s: &str) -> Option<Fingerprint> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = char_to_nibble(s.as_bytes()[i * 2])?;
        let lo = char_to_nibble(s.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

fn char_to_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Compute the blake3 fingerprint of a repo's compile inputs.
///
/// Hashes (in order, with length-prefixed framing) the domain
/// separator, CLI version, purem schema hash, sorted auto-imports,
/// sorted dep fingerprints, and sorted source files. Content-hashed
/// rather than mtime-hashed so a git checkout that bumps mtime
/// without changing content stays a cache hit.
///
/// Dep fingerprints are mandatory: a type drift in `b` invalidates
/// `a` even when `a`'s sources are byte-identical.
#[must_use]
pub fn compute_fingerprint(
    repo: &Repo,
    dep_fingerprints: &BTreeMap<&str, Fingerprint>,
    cli_version: &str,
    purem_schema: u32,
    auto_imports: &[SmolStr],
) -> Fingerprint {
    let mut h = blake3::Hasher::new();
    h.update(b"legend-build-v1\0");
    write_len_prefixed(&mut h, cli_version.as_bytes());
    h.update(&purem_schema.to_le_bytes());

    let mut sorted_imports: Vec<&str> = auto_imports.iter().map(SmolStr::as_str).collect();
    sorted_imports.sort_unstable();
    h.update(&(u32::try_from(sorted_imports.len()).unwrap_or(u32::MAX)).to_le_bytes());
    for imp in sorted_imports {
        write_len_prefixed(&mut h, imp.as_bytes());
    }

    h.update(&(u32::try_from(dep_fingerprints.len()).unwrap_or(u32::MAX)).to_le_bytes());
    for (name, fp) in dep_fingerprints {
        write_len_prefixed(&mut h, name.as_bytes());
        h.update(fp);
    }

    let mut sources: Vec<(&str, &str)> = repo.sources().collect();
    sources.sort_unstable_by_key(|(_, path)| *path);
    h.update(&(u32::try_from(sources.len()).unwrap_or(u32::MAX)).to_le_bytes());
    for (content, path) in sources {
        write_len_prefixed(&mut h, path.as_bytes());
        h.update(&(content.len() as u64).to_le_bytes());
        h.update(content.as_bytes());
    }

    let out = h.finalize();
    *out.as_bytes()
}

fn write_len_prefixed(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(u32::try_from(bytes.len()).unwrap_or(u32::MAX)).to_le_bytes());
    h.update(bytes);
}

/// Read a stamp file from disk. Returns `None` on any I/O / parse /
/// schema-version error — the caller treats that uniformly as a cache
/// miss.
#[must_use]
pub fn read_stamp(path: &Path) -> Option<Stamp> {
    let text = std::fs::read_to_string(path).ok()?;
    let stamp: Stamp = toml::from_str(&text).ok()?;
    if stamp.schema != STAMP_SCHEMA {
        return None;
    }
    Some(stamp)
}

/// Write a stamp file to disk, creating parent directories as needed.
///
/// # Errors
///
/// Returns the underlying I/O / serialization error.
pub fn write_stamp(path: &Path, stamp: &Stamp) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(stamp).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, text)
}

/// Default cache location for a resolved classpath.
///
/// When the classpath was resolved from a TOML on disk, the cache
/// goes next to it: `<dir-of-classpath-toml>/target/legend/`. When
/// there is no on-disk source (env / embedded fallback), falls back
/// to `$XDG_CACHE_HOME/legend/` (or `$HOME/.cache/legend/` /
/// `<tempdir>/legend/` on platforms without XDG).
#[must_use]
pub fn default_cache_dir(classpath_source: Option<&Path>) -> PathBuf {
    if let Some(src) = classpath_source {
        let root = src.parent().unwrap_or_else(|| Path::new("."));
        return root.join("target").join("legend");
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("legend");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("legend");
    }
    std::env::temp_dir().join("legend")
}

/// Remove the cache directory after verifying it carries a
/// [`SENTINEL`] marker file written by a previous build. Refuses to
/// touch any directory that lacks the marker — protects users who
/// pass `--cache-dir <unrelated-dir>` from data loss.
///
/// A non-existent directory is treated as success (nothing to clean).
///
/// # Errors
///
/// Returns [`std::io::Error`] if the directory exists but lacks the
/// sentinel, or if the removal itself fails.
pub fn clean_cache(dir: &Path) -> Result<(), std::io::Error> {
    if !dir.exists() {
        return Ok(());
    }
    let sentinel = dir.join(SENTINEL);
    if !sentinel.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing to clean {}: no {} marker (was this directory written by legend build?)",
                dir.display(),
                SENTINEL,
            ),
        ));
    }
    std::fs::remove_dir_all(dir)
}

/// Ensure the cache directory exists and carries a [`SENTINEL`]
/// marker. Safe to call repeatedly.
///
/// # Errors
///
/// Returns [`std::io::Error`] on directory-creation or sentinel-write
/// failure.
pub fn ensure_cache_dir(dir: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    let sentinel = dir.join(SENTINEL);
    if !sentinel.exists() {
        std::fs::write(
            &sentinel,
            b"legend build cache directory. \
              Safe to delete; will be regenerated on the next build.\n",
        )?;
    }
    Ok(())
}

/// `CARGO_PKG_VERSION` of the legend-cli binary. Captured here so
/// build.rs and tests reference a single source of truth.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current UNIX-epoch seconds, or 0 if the system clock is before the
/// epoch (impossible in practice, but `SystemTime::duration_since`
/// returns a `Result`).
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let fp: Fingerprint = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0,
            0xd0, 0xe0, 0xf0, 0x01,
        ];
        let hex = fingerprint_to_hex(&fp);
        assert_eq!(hex.len(), 64);
        assert_eq!(fingerprint_from_hex(&hex), Some(fp));
    }

    #[test]
    fn hex_rejects_short() {
        assert_eq!(fingerprint_from_hex("abc"), None);
    }

    #[test]
    fn hex_rejects_non_hex() {
        assert_eq!(fingerprint_from_hex(&"z".repeat(64)), None);
    }

    #[test]
    fn read_stamp_missing_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(read_stamp(&tmp.path().join("nope.toml")).is_none());
    }

    #[test]
    fn read_stamp_wrong_schema_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("cache.toml");
        std::fs::write(&path, "schema = 999\nfingerprint = \"x\"\n").expect("write");
        assert!(read_stamp(&path).is_none());
    }

    #[test]
    fn stamp_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("repo").join("cache.toml");
        let mut deps = BTreeMap::new();
        deps.insert("b".to_string(), "ff".repeat(32));
        let s = Stamp {
            schema: STAMP_SCHEMA,
            fingerprint: "ab".repeat(32),
            cli_version: "0.1.0".into(),
            purem_schema: "deadbeef".into(),
            generated_at: 1_700_000_000,
            dep_fingerprints: deps,
            last_test_result: TestResult::Pass,
        };
        write_stamp(&path, &s).expect("write");
        let back = read_stamp(&path).expect("read");
        assert_eq!(back.fingerprint, s.fingerprint);
        assert_eq!(back.dep_fingerprints.get("b").map(String::as_str), Some(s.dep_fingerprints["b"].as_str()));
        assert_eq!(back.last_test_result, TestResult::Pass);
    }

    #[test]
    fn clean_cache_refuses_unmarked_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("unrelated");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("important.txt"), b"keep me").expect("write");
        let res = clean_cache(&dir);
        assert!(res.is_err(), "must refuse without sentinel");
        assert!(dir.join("important.txt").exists(), "must not have deleted");
    }

    #[test]
    fn clean_cache_removes_marked_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("cache");
        ensure_cache_dir(&dir).expect("ensure");
        std::fs::write(dir.join("foo.purem"), b"x").expect("write");
        clean_cache(&dir).expect("clean");
        assert!(!dir.exists());
    }

    #[test]
    fn clean_cache_missing_dir_ok() {
        let tmp = tempfile::tempdir().expect("tempdir");
        clean_cache(&tmp.path().join("nope")).expect("clean of missing");
    }

    #[test]
    fn default_cache_dir_with_source_uses_target_legend() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml = tmp.path().join("legend-pure-classpath.toml");
        std::fs::write(&toml, b"").expect("write");
        let dir = default_cache_dir(Some(&toml));
        assert_eq!(dir, tmp.path().join("target").join("legend"));
    }

    #[test]
    fn default_cache_dir_without_source_falls_back() {
        // Just ensure it returns *some* path; the exact value depends on env.
        let dir = default_cache_dir(None);
        assert!(dir.ends_with("legend"));
    }
}
