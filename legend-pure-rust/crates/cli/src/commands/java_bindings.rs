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

//! `legend java-bindings` — Generate Java wrappers around the JNI evaluator.
//!
//! For each requested Pure function FQN, emits typed Java static-method
//! facades plus interfaces for every reachable user-defined class. The
//! generated sources target the JNI runtime support library at
//! `org.finos.legend.pure.rust.proxy`.
//!
//! Reachability is seeded from the requested functions' parameter and
//! return types. Use `--classes` and `--associations` to add more seeds
//! for elements that aren't referenced by any wrapper function — e.g.
//! when generating bindings for a model the user will populate via
//! `evaluate(...)` returning generic values, or for an association whose
//! endpoint classes wouldn't otherwise be pulled in.
//!
//! For Maven-style "bootstrap an evaluator from a manifest" workflows,
//! `--bindings-file <PATH>` accepts a single file containing one FQN per
//! line; the CLI resolves each entry against the model and dispatches
//! it to the right slot (function / class / association) based on its
//! actual element kind. Blank lines and `#`-prefixed comments are
//! ignored.
//!
//! # Usage
//!
//! ```bash
//! legend java-bindings \
//!     --output ./gen-java \
//!     --java-package com.example.gen \
//!     --functions meta::pure::functions::math::plus_Integer_MANY__Integer_1_ \
//!     --classes user_test::Person \
//!     --associations user_test::PersonAddress
//!
//! # Or, bootstrap from a curated list:
//! legend java-bindings \
//!     --output target/generated-sources/java-bindings \
//!     --java-package org.finos.legend.pure.rust.generated \
//!     --bindings-file src/main/pure-bindings/m3-bindings.txt
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;
use smol_str::SmolStr;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_java_codegen::{
    FqnInput, Options, dispatch_bindings_by_kind, generate, parse_manifest,
};

use crate::diagnostics::CliError;

/// Arguments for the `legend java-bindings` command.
#[derive(clap::Args)]
pub struct JavaBindingsArgs {
    /// Output directory. Each generated `.java` file is written under
    /// the directory layout that matches its Java package.
    #[arg(short, long, value_name = "DIR")]
    pub output: PathBuf,

    /// Java root package — every emitted class lives under this prefix.
    #[arg(long, value_name = "PKG", default_value = "com.example.legend.gen")]
    pub java_package: String,

    /// Mangled FQN of a Pure function to wrap. Repeat for multiple
    /// functions. Combine with `--functions-file` to provide a list.
    #[arg(long = "functions", value_name = "FQN", action = clap::ArgAction::Append)]
    pub functions: Vec<String>,

    /// File of FQNs (one per line, blank lines and `#`-prefixed comments
    /// ignored).
    #[arg(long, value_name = "FILE")]
    pub functions_file: Option<PathBuf>,

    /// FQN of a Pure `Class` to emit a Java interface for, beyond what
    /// is reachable from `--functions`. Repeatable.
    #[arg(long = "classes", value_name = "FQN", action = clap::ArgAction::Append)]
    pub classes: Vec<String>,

    /// FQN of a Pure `Association` to seed the reachability walk with.
    /// Both participating classes are added to the seed set so the
    /// association-injected properties surface on their generated
    /// interfaces. Repeatable.
    #[arg(long = "associations", value_name = "FQN", action = clap::ArgAction::Append)]
    pub associations: Vec<String>,

    /// Single file listing FQNs of Pure elements to bind, one per line.
    /// The CLI resolves each entry and dispatches it by element kind
    /// (Function / Class / Association). Blank lines and `#`-prefixed
    /// comments are ignored. Suitable for Maven-style bootstrap
    /// manifests that mix functions, classes, and associations.
    #[arg(long, value_name = "FILE")]
    pub bindings_file: Option<PathBuf>,

    /// Simple class name for the generated static-functions facade.
    /// Defaults to `PureFunctions`.
    #[arg(long, value_name = "NAME")]
    pub functions_class: Option<String>,
}

/// Execute the `legend java-bindings` command.
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: JavaBindingsArgs) -> Result<(), CliError> {
    eprintln!(
        "{} Java bindings into {}...",
        "Generating".cyan().bold(),
        args.output.display()
    );

    let mut requested: Vec<FqnInput> = args
        .functions
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();

    if let Some(file) = args.functions_file.as_deref() {
        let content = std::fs::read_to_string(file).map_err(|e| CliError::Io {
            path: file.to_path_buf(),
            source: e,
        })?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            requested.push(FqnInput::new(trimmed));
        }
    }

    let mut extra_classes: Vec<FqnInput> = args
        .classes
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();
    let mut extra_associations: Vec<FqnInput> = args
        .associations
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();

    // Read the bindings manifest, parsing `@pkg` / `@functions-class`
    // / `@import` directives out of the body. The CLI is the
    // standalone codegen path; `@import` requires classpath
    // resolution which only the annotation processor can do, so we
    // surface a clear error if a CLI-supplied manifest tries to use
    // them.
    let mut manifest_pkg: Option<String> = None;
    let mut manifest_functions_class: Option<String> = None;
    let bindings_lines = if let Some(file) = args.bindings_file.as_deref() {
        let content = std::fs::read_to_string(file).map_err(|e| CliError::Io {
            path: file.to_path_buf(),
            source: e,
        })?;
        let raw_lines: Vec<String> = content.lines().map(str::to_owned).collect();
        let parsed = parse_manifest(&raw_lines).map_err(|e| CliError::Custom(e.to_string()))?;
        if !parsed.imports.is_empty() {
            return Err(CliError::Custom(format!(
                "manifest `{}` uses `@import:` which only the annotation processor can resolve. \
                 The CLI is for standalone codegen — emit each manifest separately or invoke \
                 the AP via `mvn compile`.",
                file.display()
            )));
        }
        manifest_pkg = parsed.pkg;
        manifest_functions_class = parsed.functions_class;
        parsed.fqns
    } else {
        Vec::new()
    };

    if requested.is_empty()
        && extra_classes.is_empty()
        && extra_associations.is_empty()
        && bindings_lines.is_empty()
    {
        return Err(CliError::Custom(
            "nothing to generate — pass --functions, --functions-file, --classes, \
             --associations, or --bindings-file"
                .to_owned(),
        ));
    }

    eprintln!("  {} loading platform model...", "•".dimmed());
    let repos = Repo::default_with_build_snapshots();
    let auto_imports: Vec<SmolStr> = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    let model = match repo::load(&repos, &auto_imports) {
        Ok(m) => m,
        Err(partial) => {
            eprintln!(
                "{} platform model loaded with {} compile error(s); continuing",
                "warning:".yellow().bold(),
                partial.errors.len()
            );
            partial.model
        }
    };

    // Dispatch every entry from the bindings file by its element kind.
    if !bindings_lines.is_empty() {
        let dispatched = dispatch_bindings_by_kind(&model, &bindings_lines)
            .map_err(|e| CliError::Custom(e.to_string()))?;
        let fns_added = dispatched.functions.len();
        let classes_added = dispatched.classes.len();
        let assocs_added = dispatched.associations.len();
        requested.extend(dispatched.functions);
        extra_classes.extend(dispatched.classes);
        extra_associations.extend(dispatched.associations);
        eprintln!(
            "  {} bindings file: {} function(s), {} class(es), {} association(s)",
            "•".dimmed(),
            fns_added,
            classes_added,
            assocs_added,
        );
    }

    // Manifest's `@pkg:` directive overrides the `--java-package`
    // command-line default; same for `--functions-class`.
    let java_package = manifest_pkg.unwrap_or(args.java_package);
    let mut opts = Options::new(&java_package);
    if let Some(name) = args.functions_class.or(manifest_functions_class) {
        opts.functions_class_name = Some(name);
    }

    let files = generate(
        &model,
        &requested,
        &extra_classes,
        &extra_associations,
        &opts,
    )
    .map_err(|e| CliError::Custom(e.to_string()))?;

    // Idempotent writes: only touch a file when its content actually
    // changes. Maven incremental builds and Develocity remote caches
    // key on mtime, so unconditionally rewriting every file flushes
    // those caches even when nothing the codegen produces is
    // different.
    let mut written = 0usize;
    let mut unchanged = 0usize;
    for file in &files {
        let dest = args.output.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let did_write = write_if_changed(&dest, &file.contents).map_err(|e| CliError::Io {
            path: dest.clone(),
            source: e,
        })?;
        if did_write {
            written += 1;
        } else {
            unchanged += 1;
        }
    }

    eprintln!(
        "  {} {} written, {} unchanged under {}",
        "✓".green(),
        written.to_string().bold(),
        unchanged.to_string().dimmed(),
        args.output.display()
    );
    Ok(())
}

/// Write `contents` to `dest` only if `dest` doesn't already contain
/// exactly that byte sequence. Returns `true` when a write happened,
/// `false` when the existing file was byte-identical and was left
/// untouched.
///
/// Read-then-compare is cheap relative to the cargo + codegen run that
/// produced `contents`, and avoids the IO-error trap of
/// "permission-denied to stat before write": we only suppress the
/// write when we positively confirmed the existing bytes match.
fn write_if_changed(dest: &std::path::Path, contents: &str) -> std::io::Result<bool> {
    let existing = std::fs::read(dest).ok();
    if existing.as_deref() == Some(contents.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(dest, contents)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::write_if_changed;

    #[test]
    fn write_if_changed_creates_missing_file() {
        let tmp = tempdir();
        let path = tmp.join("a.java");
        let did = write_if_changed(&path, "alpha").expect("write");
        assert!(did, "first write must report did_write=true");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
    }

    #[test]
    fn write_if_changed_skips_byte_identical() {
        let tmp = tempdir();
        let path = tmp.join("b.java");
        std::fs::write(&path, "alpha").unwrap();
        let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Sleep so a real write would tick the mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let did = write_if_changed(&path, "alpha").expect("write");
        assert!(!did, "byte-identical content must not rewrite");
        let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "mtime must be preserved");
    }

    #[test]
    fn write_if_changed_overwrites_when_diff() {
        let tmp = tempdir();
        let path = tmp.join("c.java");
        std::fs::write(&path, "alpha").unwrap();
        let did = write_if_changed(&path, "beta").expect("write");
        assert!(did);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "beta");
    }

    fn tempdir() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("legend-write-if-changed-{nanos}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
