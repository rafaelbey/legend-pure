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

//! CLI-side classpath resolution: file loading, discovery cascade, and
//! synthetic snapshot-dir scanning on top of the shared parser in
//! [`legend_pure_core_platform::classpath`].
//!
//! The parse layer (`[[repo]]` → `Vec<Repo>`, `[extension.<…>]` tables,
//! merge-with-embedded shadow-by-name) lives in
//! `crates/core-platform-pure/src/classpath.rs` so the JNI bridge can
//! reuse it without inheriting `legend-cli`'s dep tree. Everything in
//! this file is CLI-specific: reading the TOML from disk, walking
//! ancestors for `legend-pure-classpath.toml`, building a synthetic
//! classpath from a `snapshots/` directory next to the binary, and
//! threading the seven-step resolution cascade.
//!
//! # Schema (`legend-pure-classpath.toml`)
//!
//! ```toml
//! # Optional. Defaults to the directory containing this file.
//! root = "."
//!
//! # Optional. Auto-imports applied to every repo (extends the platform
//! # default list — these are added on top of it).
//! auto_imports = ["my::custom::package"]
//!
//! [[repo]]
//! name = "platform"
//! kind = "purem"
//! path = "snapshots/platform.purem"
//!
//! [[repo]]
//! name = "my_app"
//! kind = "filesystem"
//! descriptor = "my_app.definition.json"
//!
//! # Reserved for v2: kind = "maven", coordinates = "...".
//!
//! # Engine configs for runtime extensions. Top-level table is
//! # `extension`; next-level key is the extension domain
//! # (`relational`, `lake`, …); then the engine name (`h2`, `duckdb`,
//! # …). Each engine's value is held opaquely as `toml::Value` here and
//! # re-deserialized by the owning extension crate when it's needed.
//! [extension.relational.h2]
//! jar_path = "~/.m2/repository/com/h2database/h2/2.1.214/h2-2.1.214.jar"
//! version  = "2.1.214"
//! pg_port  = 5435
//! java     = "java"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use legend_pure_core_platform::repo::{Repo, RepoMeta};

pub use legend_pure_core_platform::classpath::{
    Classpath, ClasspathError, ResolvedClasspath, merge_with_embedded, parse_classpath_toml,
};

/// Load a classpath TOML from disk.
///
/// Reads the file, then delegates parsing to
/// [`legend_pure_core_platform::classpath::parse_classpath_toml`] with
/// the TOML's parent directory as the base for relative paths.
///
/// # Errors
///
/// Returns [`ClasspathError`] for any read / parse / build failure.
pub fn load_classpath(toml_path: &Path) -> Result<Classpath, ClasspathError> {
    let text = std::fs::read_to_string(toml_path).map_err(|source| ClasspathError::Io {
        path: toml_path.to_path_buf(),
        source,
    })?;
    let parent = toml_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    parse_classpath_toml(&text, &toml_path.display().to_string(), &parent)
}

/// Default classpath filename — looked up alongside the CLI binary if no
/// `--classpath` flag is passed.
pub const DEFAULT_FILENAME: &str = "legend-pure-classpath.toml";

/// Environment variable that, if set, points at a classpath TOML.
/// Higher priority than ancestor / next-to-binary discovery, lower than
/// the explicit `--classpath` flag.
pub const ENV_CLASSPATH: &str = "LEGEND_PURE_CLASSPATH";

/// Build-time-emitted env var that points at the
/// `target/<profile>/snapshots/` directory the embedder produced. Used
/// as the lowest-priority discovery hint for `cargo run -p legend-cli`.
pub const ENV_BUILD_SNAPSHOTS_DIR: &str = "LEGEND_PURE_BUILD_SNAPSHOTS_DIR";

/// Walk up from `start_dir` looking for a [`DEFAULT_FILENAME`] (Cargo
/// style). Returns the first match, or `None`.
#[must_use]
pub fn discover_classpath(start_dir: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start_dir);
    while let Some(d) = cur {
        let candidate = d.join(DEFAULT_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = d.parent();
    }
    None
}

/// Locate `legend-pure-classpath.toml` next to the running binary, or
/// fall back to a synthetic classpath derived from `<exe_dir>/snapshots/*.purem`.
/// Returns `None` if neither is present.
#[must_use]
pub fn discover_next_to_binary() -> Option<NextToBinary> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;

    let toml = dir.join(DEFAULT_FILENAME);
    if toml.is_file() {
        return Some(NextToBinary::Toml(toml));
    }
    let snapshots = dir.join("snapshots");
    if snapshots.is_dir() {
        return Some(NextToBinary::SnapshotsDir(snapshots));
    }
    None
}

/// Result of [`discover_next_to_binary`].
#[derive(Debug, Clone)]
pub enum NextToBinary {
    /// A `legend-pure-classpath.toml` next to the binary.
    Toml(PathBuf),
    /// A `snapshots/` directory containing `.purem` files.
    SnapshotsDir(PathBuf),
}

/// Build a synthetic [`Classpath`] from every `.purem` file in `dir`.
/// Reads each blob's header to extract the repo's `RepoMeta`.
///
/// # Errors
///
/// Returns [`ClasspathError::PathMissing`] if `dir` doesn't exist;
/// [`ClasspathError::RepoBuild`] for any blob whose header fails to
/// validate.
pub fn synthetic_from_snapshots_dir(dir: &Path) -> Result<Classpath, ClasspathError> {
    if !dir.is_dir() {
        return Err(ClasspathError::PathMissing {
            name: "<snapshots>".into(),
            path: dir.to_path_buf(),
        });
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| ClasspathError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "purem"))
        .collect();
    entries.sort();

    let mut repos: Vec<Repo> = Vec::new();
    for path in entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        // Synthesize a meta with empty deps. Topo sort will need real
        // deps — for now, encoding them in the synthetic classpath
        // requires the user to write an explicit TOML.
        let meta = RepoMeta {
            name: Box::leak(stem.clone().into_boxed_str()),
            pattern: ".*",
            dependencies: &[],
        };
        let repo = Repo::from_purem_file(&path, format!("/{stem}"), meta).map_err(|source| {
            ClasspathError::RepoBuild {
                name: stem.clone(),
                source,
            }
        })?;
        repos.push(repo);
    }

    Ok(Classpath {
        root: dir.to_path_buf(),
        extra_auto_imports: Vec::new(),
        repos,
        extension_configs: HashMap::new(),
    })
}

/// Resolution cascade for a CLI invocation:
/// 1. `--classpath <PATH>` flag (highest priority).
/// 2. `LEGEND_PURE_CLASSPATH` env var.
/// 3. `legend-pure-classpath.toml` discovered in cwd ancestors.
/// 4. `<exe_dir>/legend-pure-classpath.toml` next to the binary.
/// 5. `<exe_dir>/snapshots/*.purem` synthetic classpath.
/// 6. `LEGEND_PURE_BUILD_SNAPSHOTS_DIR/*.purem` (build-time hint for
///    `cargo run -p legend-cli`).
/// 7. `Repo::default_embedded()` — every embedded repo. Source-of-truth
///    fallback for builds that didn't ship classpath artifacts.
///
/// **Shadow-by-name semantics:** when both an embedded repo and a
/// classpath entry share a `name`, the classpath entry replaces the
/// embedded copy. Lets local-dev override an embedded `platform` by
/// declaring a `kind = "filesystem"` entry with the same name.
///
/// # Errors
///
/// Returns [`ClasspathError`] for explicit-classpath failures (bad
/// TOML, missing paths, etc.). Auto-discovered classpaths that fail to
/// load fall through to the next step rather than aborting.
pub fn resolve_classpath(
    explicit: Option<&Path>,
    cwd: &Path,
) -> Result<ResolvedClasspath, ClasspathError> {
    // Step 1: explicit flag.
    if let Some(p) = explicit {
        let cp = load_classpath(p)?;
        return Ok(merge_with_embedded(cp, Some(p.to_path_buf())));
    }

    // Step 2: LEGEND_PURE_CLASSPATH env var.
    if let Some(env) = std::env::var_os(ENV_CLASSPATH) {
        let p = PathBuf::from(env);
        let cp = load_classpath(&p)?;
        return Ok(merge_with_embedded(cp, Some(p)));
    }

    // Step 3: ancestor walk for legend-pure-classpath.toml.
    if let Some(p) = discover_classpath(cwd)
        && let Ok(cp) = load_classpath(&p)
    {
        return Ok(merge_with_embedded(cp, Some(p)));
    }

    // Step 4 + 5: next to binary.
    if let Some(found) = discover_next_to_binary() {
        match found {
            NextToBinary::Toml(p) => {
                if let Ok(cp) = load_classpath(&p) {
                    return Ok(merge_with_embedded(cp, Some(p)));
                }
            }
            NextToBinary::SnapshotsDir(dir) => {
                if let Ok(cp) = synthetic_from_snapshots_dir(&dir) {
                    return Ok(merge_with_embedded(cp, Some(dir)));
                }
            }
        }
    }

    // Step 6: build-time hint (cargo run path).
    if let Some(env) = std::env::var_os(ENV_BUILD_SNAPSHOTS_DIR) {
        let dir = PathBuf::from(env);
        if let Ok(cp) = synthetic_from_snapshots_dir(&dir) {
            return Ok(merge_with_embedded(cp, Some(dir)));
        }
    }

    // Step 7: embedded fallback. Build an empty parsed classpath and
    // merge — keeps the shadow-by-name path uniform.
    let cp = Classpath {
        root: PathBuf::from("."),
        extra_auto_imports: Vec::new(),
        repos: Vec::new(),
        extension_configs: HashMap::new(),
    };
    let mut resolved = merge_with_embedded(cp, None);
    // Step-7 fallback intentionally signals "no source" — the
    // merge_with_embedded helper would propagate the `None` we passed
    // already, so this is redundant but kept explicit for clarity.
    resolved.source = None;
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(p: &Path, contents: &[u8]) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, contents).expect("write");
    }

    #[test]
    fn load_classpath_with_one_purem() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("snapshots").join("a.purem");

        // Write a minimal valid purem blob: just the header + 0-len payload.
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        write_file(&purem_path, &blob);

        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "a"
kind = "purem"
path = "snapshots/a.purem"
"#,
        );

        let cp = load_classpath(&toml_path).expect("load");
        assert_eq!(cp.repos.len(), 1);
        let meta = cp.repos[0].meta().expect("meta");
        assert_eq!(meta.name, "a");
    }

    #[test]
    fn load_classpath_rejects_duplicate_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("a.purem");
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        write_file(&purem_path, &blob);

        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "a"
kind = "purem"
path = "a.purem"

[[repo]]
name = "a"
kind = "purem"
path = "a.purem"
"#,
        );

        match load_classpath(&toml_path) {
            Err(ClasspathError::DuplicateName { name }) => assert_eq!(name, "a"),
            other => panic!("expected DuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn load_classpath_rejects_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "ghost"
kind = "purem"
path = "no-such.purem"
"#,
        );

        match load_classpath(&toml_path) {
            Err(ClasspathError::PathMissing { name, .. }) => assert_eq!(name, "ghost"),
            other => panic!("expected PathMissing, got {other:?}"),
        }
    }

    #[test]
    fn synthetic_from_snapshots_dir_picks_up_purem_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("snapshots");
        std::fs::create_dir_all(&dir).expect("mkdir");

        for name in ["one.purem", "two.purem"] {
            let mut blob = Vec::new();
            legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
            write_file(&dir.join(name), &blob);
        }
        write_file(&dir.join("README.txt"), b"not a purem");

        let cp = synthetic_from_snapshots_dir(&dir).expect("synthetic build");
        assert_eq!(cp.repos.len(), 2, "should find both .purem files");
        let names: std::collections::HashSet<_> = cp
            .repos
            .iter()
            .filter_map(|r| r.meta().map(|m| m.name))
            .collect();
        assert!(names.contains("one"), "got names: {names:?}");
        assert!(names.contains("two"), "got names: {names:?}");
    }

    #[test]
    fn resolve_classpath_explicit_flag_includes_platform() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(&toml_path, b"# empty classpath\n");
        let resolved = resolve_classpath(Some(&toml_path), tmp.path()).expect("resolve explicit");
        assert!(resolved.source.is_some());
        assert!(
            resolved
                .repos
                .iter()
                .any(|r| r.meta().is_some_and(|m| m.name == "platform")),
            "embedded platform must remain available when classpath doesn't shadow it"
        );
    }

    #[test]
    fn resolve_classpath_explicit_flag_shadows_embedded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let purem_path = tmp.path().join("platform.purem");
        let mut blob = Vec::new();
        legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
        write_file(&purem_path, &blob);

        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[[repo]]
name = "platform"
kind = "purem"
path = "platform.purem"
"#,
        );

        let resolved = resolve_classpath(Some(&toml_path), tmp.path())
            .expect("explicit classpath should resolve");
        assert!(resolved.source.is_some(), "source path tracked");

        let platform_count = resolved
            .repos
            .iter()
            .filter(|r| r.meta().is_some_and(|m| m.name == "platform"))
            .count();
        assert_eq!(
            platform_count, 1,
            "shadow-by-name: classpath platform replaces embedded one"
        );
    }

    #[test]
    fn load_classpath_parses_extension_configs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_path = tmp.path().join("legend-pure-classpath.toml");
        write_file(
            &toml_path,
            br#"
[extension.relational.h2]
jar_path = "~/.m2/repository/com/h2database/h2/2.1.214/h2-2.1.214.jar"
version  = "2.1.214"
pg_port  = 5435
java     = "java"

[extension.lake.snowflake]
account = "test-acct"
"#,
        );
        let cp = load_classpath(&toml_path).expect("load");
        let relational = cp
            .extension_configs
            .get("relational")
            .expect("relational domain present");
        let h2 = relational.get("h2").expect("h2 engine present");
        let table = h2.as_table().expect("h2 is a table");
        assert_eq!(
            table.get("version").and_then(|v| v.as_str()),
            Some("2.1.214")
        );
        assert_eq!(
            table.get("pg_port").and_then(|v| v.as_integer()),
            Some(5435)
        );
        let lake = cp
            .extension_configs
            .get("lake")
            .expect("lake domain present");
        assert!(lake.contains_key("snowflake"));
    }

    #[test]
    fn discover_classpath_walks_up() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let cp = tmp.path().join("legend-pure-classpath.toml");
        write_file(&cp, b"");

        let found = discover_classpath(&nested).expect("found");
        assert_eq!(found, cp);
    }
}
