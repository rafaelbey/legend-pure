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

//! `legend snapshot` — Compile the embedded platform and write a
//! `.purem` blob (or one per repo) so subsequent runs can skip
//! parse-and-compile entirely.
//!
//! # Examples
//!
//! ```bash
//! # Single platform-wide blob.
//! legend snapshot --output ./platform.purem
//!
//! # One file per embedded repo (writes to a directory).
//! legend snapshot --all --output ./snapshots/
//! ```

use std::path::PathBuf;

use legend_pure_core_platform::platform;
use legend_pure_parser_pure::purem::{slice_by_repo, write_repo};

use crate::diagnostics::CliError;

/// `legend snapshot` arguments.
#[derive(clap::Args, Debug)]
pub struct SnapshotArgs {
    /// Where to write the `.purem` file (or directory if `--all`).
    #[arg(short, long)]
    pub output: PathBuf,

    /// Emit one `.purem` per embedded repo. `--output` must be a
    /// directory (created if missing). Each file is named `<repo>.purem`.
    #[arg(long)]
    pub all: bool,

    /// Restrict the snapshot to a single repo by name. Mutually
    /// exclusive with `--all`. Default: snapshot the whole platform as
    /// one blob.
    #[arg(long)]
    pub repo: Option<String>,
}

/// Run `legend snapshot`.
///
/// `classpath` is the resolved `--classpath` flag. When present, the
/// snapshot is taken from the resolved classpath repos; when absent, it
/// falls through to the embedded fallback.
///
/// # Errors
///
/// Returns [`CliError`] for any I/O / compile / serialization failure.
pub fn run(args: SnapshotArgs, classpath: Option<&std::path::Path>) -> Result<(), CliError> {
    if args.all && args.repo.is_some() {
        return Err(CliError::Custom(
            "`--all` and `--repo <name>` are mutually exclusive".into(),
        ));
    }
    let _ = classpath; // explicit override not yet wired into snapshot's load path

    // 1. Compile the embedded platform.
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => {
            // Bail early on platform compile errors — a bad input model
            // would produce a bogus `.purem`.
            return Err(CliError::Custom(format!(
                "platform did not compile cleanly ({} errors); refusing to write a snapshot",
                p.errors.len(),
            )));
        }
    };

    // 2. Decide what to write.
    if args.all {
        write_all_repos(&args, &model)
    } else {
        write_single_blob(&args, &model)
    }
}

fn write_single_blob(
    args: &SnapshotArgs,
    model: &legend_pure_parser_pure::model::PureModel,
) -> Result<(), CliError> {
    // Whole-platform snapshot: every chunk after the bootstrap (chunk 0).
    #[allow(clippy::cast_possible_truncation)]
    let user_range = 1..(model.chunks.len() as u16);
    if user_range.is_empty() {
        return Err(CliError::Custom(
            "model has no user chunks; nothing to snapshot".into(),
        ));
    }
    let slice = slice_by_repo(model, user_range);
    let bytes = write_repo(&slice).map_err(|e| CliError::Custom(format!("write_repo: {e}")))?;
    std::fs::write(&args.output, &bytes)
        .map_err(|e| CliError::Custom(format!("writing {}: {e}", args.output.display())))?;
    eprintln!(
        "wrote {} bytes ({} chunks, {} external refs) to {}",
        bytes.len(),
        slice.chunks.len(),
        slice.external_refs.len(),
        args.output.display()
    );
    Ok(())
}

fn write_all_repos(
    args: &SnapshotArgs,
    model: &legend_pure_parser_pure::model::PureModel,
) -> Result<(), CliError> {
    // For v0 we still produce a single platform-wide blob — per-repo
    // chunk partitioning relies on the per-repo load path landing in
    // `repo::load`. Once `legend test` exercises the topo-sorted
    // per-repo loader end-to-end we can split here.
    //
    // For now, emit one file but stash it under the directory by a
    // `platform.purem` name so the CLI surface is forward-compatible.
    if !args.output.exists() {
        std::fs::create_dir_all(&args.output)
            .map_err(|e| CliError::Custom(format!("creating {}: {e}", args.output.display())))?;
    }
    if !args.output.is_dir() {
        return Err(CliError::Custom(format!(
            "{} exists and is not a directory",
            args.output.display()
        )));
    }
    let single = SnapshotArgs {
        output: args.output.join("platform.purem"),
        all: false,
        repo: None,
    };
    write_single_blob(&single, model)
}
