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

//! Workspace-internal task runner. Invoked via the `cargo dist` alias defined
//! in `.cargo/config.toml`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Workspace task runner")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Stage the release CLI binary plus all `.purem` snapshots into a
    /// distributable directory under `target/dist/`.
    Dist(DistArgs),
}

#[derive(clap::Args)]
struct DistArgs {
    /// Skip `cargo build --release`; assume artifacts already exist.
    #[arg(long)]
    skip_build: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Dist(args) => run_dist(&args),
    }
}

fn run_dist(args: &DistArgs) -> Result<()> {
    let workspace_root = workspace_root()?;
    let version = read_workspace_version(&workspace_root)?;
    let triple = host_triple()?;

    if !args.skip_build {
        eprintln!("==> cargo build --release --workspace");
        let status = Command::new(env!("CARGO"))
            .current_dir(&workspace_root)
            .args(["build", "--release", "--workspace"])
            .status()
            .context("failed to spawn cargo build")?;
        if !status.success() {
            bail!("cargo build --release --workspace failed (exit {status})");
        }
    }

    let target_dir = workspace_root.join("target");
    let release_dir = target_dir.join("release");
    let src_bin = release_dir.join(bin_filename("legend"));
    let src_snaps = release_dir.join("snapshots");

    if !src_bin.is_file() {
        bail!(
            "expected release binary at {}; run without --skip-build, or build manually first",
            src_bin.display()
        );
    }
    if !src_snaps.is_dir() {
        bail!(
            "expected snapshots directory at {}; the build script in core-platform-pure should have produced it",
            src_snaps.display()
        );
    }

    let dist_root = target_dir.join("dist");
    let out_dir = dist_root.join(format!("legend-pure-{version}-{triple}"));

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("failed to wipe {}", out_dir.display()))?;
    }
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    let dst_bin = out_dir.join(bin_filename("legend"));
    fs::copy(&src_bin, &dst_bin)
        .with_context(|| format!("copy {} -> {}", src_bin.display(), dst_bin.display()))?;

    let dst_snaps = out_dir.join("snapshots");
    fs::create_dir_all(&dst_snaps)
        .with_context(|| format!("failed to create {}", dst_snaps.display()))?;

    let mut purem_files: Vec<PathBuf> = fs::read_dir(&src_snaps)
        .with_context(|| format!("read_dir {}", src_snaps.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension() == Some(OsStr::new("purem")))
        .collect();
    purem_files.sort();

    if purem_files.len() < 8 {
        bail!(
            "expected at least 8 .purem snapshots in {}, found {}: {:?}",
            src_snaps.display(),
            purem_files.len(),
            purem_files
                .iter()
                .filter_map(|p| p.file_name())
                .collect::<Vec<_>>()
        );
    }

    // Detect a missing prod slice. `purem-embedded` repos (e.g. `platform`)
    // intentionally have no on-disk prod blob — they're `include_bytes!`'d
    // into the binary; `crates/build/src/lib.rs:678-685` actively removes
    // any stale `<name>.purem` from `snapshots/`. So only flag artifact-shape
    // repos whose prod blob is missing.
    let embedded = embedded_repo_names(&workspace_root)?;
    let names: std::collections::BTreeSet<String> = purem_files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    let mut orphaned_tests: Vec<&str> = names
        .iter()
        .filter_map(|n| n.strip_suffix(".tests.purem"))
        .filter(|stem| !names.contains(&format!("{stem}.purem")))
        .filter(|stem| !embedded.contains(*stem))
        .collect();
    orphaned_tests.sort_unstable();
    for stem in &orphaned_tests {
        eprintln!(
            "WARNING: {stem}.tests.purem present but {stem}.purem is missing — build script may have dropped the prod slice"
        );
    }

    let copied_bytes = copy_snapshots(&purem_files, &dst_snaps)? + fs::metadata(&dst_bin)?.len();
    print_summary(
        &dist_root,
        &out_dir,
        &dst_bin,
        purem_files.len(),
        copied_bytes,
    )?;

    Ok(())
}

fn copy_snapshots(purem_files: &[PathBuf], dst_snaps: &Path) -> Result<u64> {
    let mut copied_bytes: u64 = 0;
    for src in purem_files {
        let name = src
            .file_name()
            .ok_or_else(|| anyhow!("snapshot path has no file name: {}", src.display()))?;
        let dst = dst_snaps.join(name);
        let bytes = fs::copy(src, &dst)
            .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
        copied_bytes += bytes;
    }
    Ok(copied_bytes)
}

fn print_summary(
    dist_root: &Path,
    out_dir: &Path,
    dst_bin: &Path,
    snapshot_count: usize,
    copied_bytes: u64,
) -> Result<()> {
    let bin_bytes = fs::metadata(dst_bin)?.len();
    let dir_name = out_dir.file_name().unwrap_or_default().to_string_lossy();
    eprintln!();
    eprintln!("==> staged distribution");
    eprintln!("    location:  {}", out_dir.display());
    eprintln!("    binary:    legend ({bin_bytes} bytes)");
    eprintln!("    snapshots: {snapshot_count} files in snapshots/");
    eprintln!("    total:     {copied_bytes} bytes");
    eprintln!();
    eprintln!(
        "Zip with: ( cd {} && zip -r {dir_name}.zip {dir_name} )",
        dist_root.display(),
    );
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            anyhow!(
                "cannot derive workspace root from {}",
                manifest_dir.display()
            )
        })
}

fn read_workspace_version(workspace_root: &Path) -> Result<String> {
    let manifest = workspace_root.join("Cargo.toml");
    let text =
        fs::read_to_string(&manifest).with_context(|| format!("read {}", manifest.display()))?;

    let mut in_workspace_package = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if in_workspace_package
            && let Some(rest) = line.strip_prefix("version")
            && let Some(rest) = rest.trim_start().strip_prefix('=')
        {
            let value = rest.trim().trim_matches('"');
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    bail!(
        "could not find [workspace.package] version in {}",
        manifest.display()
    );
}

/// Returns the set of repo names whose `shape = "purem-embedded"` in
/// `core-platform-pure/Cargo.toml`. Embedded repos have their prod `.purem`
/// blob `include_bytes!`'d into the binary, so they intentionally don't
/// appear in `target/<profile>/snapshots/`.
///
/// The descriptor field encodes the repo name (e.g.
/// `.../platform.json` → `platform`,
/// `.../platform_dsl_store.definition.json` → `platform_dsl_store`).
fn embedded_repo_names(workspace_root: &Path) -> Result<std::collections::BTreeSet<String>> {
    let manifest = workspace_root.join("crates/core-platform-pure/Cargo.toml");
    let text =
        fs::read_to_string(&manifest).with_context(|| format!("read {}", manifest.display()))?;

    let mut out = std::collections::BTreeSet::new();
    let mut in_repo_block = false;
    let mut current_descriptor: Option<String> = None;
    let mut current_shape: Option<String> = None;

    let flush = |out: &mut std::collections::BTreeSet<String>,
                 desc: &Option<String>,
                 shape: &Option<String>| {
        if shape.as_deref() == Some("purem-embedded")
            && let Some(d) = desc.as_deref()
            && let Some(name) = repo_name_from_descriptor(d)
        {
            out.insert(name);
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line == "[[package.metadata.legend-pure.repos]]" {
            flush(&mut out, &current_descriptor, &current_shape);
            in_repo_block = true;
            current_descriptor = None;
            current_shape = None;
            continue;
        }
        if line.starts_with('[') {
            flush(&mut out, &current_descriptor, &current_shape);
            in_repo_block = false;
            current_descriptor = None;
            current_shape = None;
            continue;
        }
        if !in_repo_block {
            continue;
        }
        if let Some(rest) = line.strip_prefix("descriptor") {
            if let Some(value) = parse_toml_string(rest) {
                current_descriptor = Some(value);
            }
        } else if let Some(rest) = line.strip_prefix("shape")
            && let Some(value) = parse_toml_string(rest)
        {
            current_shape = Some(value);
        }
    }
    flush(&mut out, &current_descriptor, &current_shape);
    Ok(out)
}

fn parse_toml_string(rest: &str) -> Option<String> {
    let after_eq = rest.trim_start().strip_prefix('=')?.trim();
    let stripped = after_eq.trim_matches('"');
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn repo_name_from_descriptor(descriptor: &str) -> Option<String> {
    let file = Path::new(descriptor).file_name()?.to_str()?;
    // `<name>.json` (platform.json) or `<name>.definition.json` (others).
    let stem = file.strip_suffix(".json")?;
    Some(stem.strip_suffix(".definition").unwrap_or(stem).to_string())
}

fn host_triple() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("failed to run rustc -vV")?;
    if !out.status.success() {
        bail!("rustc -vV exited {}", out.status);
    }
    let text = String::from_utf8(out.stdout).context("rustc -vV output not UTF-8")?;
    for line in text.lines() {
        if let Some(triple) = line.strip_prefix("host: ") {
            return Ok(triple.trim().to_string());
        }
    }
    bail!("rustc -vV did not report a host triple")
}

fn bin_filename(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}
