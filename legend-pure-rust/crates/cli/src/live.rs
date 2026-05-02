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

//! Live-reload platform descriptor discovery and file watching.
//!
//! The `--live` CLI flag swaps the embedded `platform` repo for a
//! filesystem-backed [`legend_pure_core_platform::repo::Repo`] built
//! from the on-disk `platform.json` descriptor. The DSL repos remain
//! embedded.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use legend_pure_core_platform::repo::Repo;
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::RecursiveMode;

use crate::diagnostics::CliError;

/// Resolve the platform descriptor JSON from explicit `--platform-dir`
/// or auto-discovery.
///
/// `--platform-dir` may point at:
/// - the descriptor JSON itself (`.../platform.json`)
/// - the resources directory (`.../resources/`) — `platform.json` inside
///   it is used
/// - the legacy platform source directory (`.../resources/platform/pure/`
///   or `.../resources/platform/`) — the `platform.json` two/one levels
///   up is used
///
/// Auto-discovery walks ancestors of the cwd looking for
/// `legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json`.
pub fn resolve_platform_descriptor(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        // Probe sibling and ancestor `platform.json` for legacy
        // `--platform-dir <pure-source-dir>` invocations.
        let mut probe = p.to_path_buf();
        for _ in 0..3 {
            let candidate = probe.join("platform.json");
            if candidate.is_file() {
                return Ok(candidate);
            }
            if !probe.pop() {
                break;
            }
        }
        return Err(CliError::Custom(format!(
            "could not locate platform.json relative to: {}",
            p.display()
        )));
    }

    let cwd = std::env::current_dir()
        .map_err(|e| CliError::Custom(format!("Failed to get current directory: {e}")))?;

    discover_platform_descriptor(&cwd).ok_or_else(|| {
        CliError::Custom(
            "Could not auto-discover platform descriptor \
             (legend-pure-core/legend-pure-m3-core/src/main/resources/platform.json). \
             Try running from within the repository, or use --platform-dir <PATH>."
                .into(),
        )
    })
}

/// Walk ancestors of `start_dir` looking for `legend-pure-core/`.
/// Returns the platform descriptor JSON if found.
fn discover_platform_descriptor(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current
            .join("legend-pure-core")
            .join("legend-pure-m3-core")
            .join("src/main/resources/platform.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Build a `Vec<Repo>` mixing a filesystem-backed `platform` repo with
/// every DSL `.purem` artifact discovered next to the binary (or in
/// the build-time-emitted snapshots dir, for `cargo run` invocations).
/// Used by `legend test --live` and `legend repl --live`.
///
/// After Phase 3b the binary only embeds `platform`; DSLs ship as
/// build-script-emitted `.purem` artifacts in
/// `target/<profile>/snapshots/` (dev) or `<exe_dir>/snapshots/`
/// (deployed). This helper assembles both ends.
///
/// # Errors
///
/// Returns [`CliError`] if the platform descriptor cannot be loaded.
pub fn live_repos(platform_descriptor: &Path) -> Result<Vec<Repo>, CliError> {
    let platform = Repo::from_descriptor(platform_descriptor).map_err(|e| {
        CliError::Custom(format!(
            "failed to load platform repo from {}: {e}",
            platform_descriptor.display()
        ))
    })?;
    let mut repos = vec![platform];

    // Pull DSL repos from the build-time-emitted snapshots dir.
    if let Some(env) = std::env::var_os(crate::classpath::ENV_BUILD_SNAPSHOTS_DIR) {
        let dir = std::path::PathBuf::from(env);
        if let Ok(synthetic) = crate::classpath::synthetic_from_snapshots_dir(&dir) {
            for r in synthetic.repos {
                // Skip a duplicate `platform` — we already have it from
                // the live filesystem descriptor.
                if r.meta().is_some_and(|m| m.name == "platform") {
                    continue;
                }
                repos.push(r);
            }
        }
    }

    Ok(repos)
}

/// Filesystem source root for the live `platform` repo — the directory
/// walked by [`watch_dir`]. Mirrors the descriptor-relative inference
/// rule used by [`Repo::from_descriptor`]: `<dir_of_descriptor>/platform/`.
pub fn platform_source_root(descriptor: &Path) -> Result<PathBuf, CliError> {
    descriptor
        .parent()
        .map(|d| d.join("platform"))
        .ok_or_else(|| {
            CliError::Custom(format!(
                "descriptor {} has no parent dir",
                descriptor.display()
            ))
        })
}

/// Spawns a notify file watcher on `dir`. Returns a shared atomic boolean
/// that is set to true when any `.pure` file is created/modified/deleted.
/// Uses debounced events (~200ms) to coalesce rapid saves.
pub fn watch_dir(
    dir: &Path,
) -> Result<
    (
        notify_debouncer_mini::Debouncer<notify_debouncer_mini::notify::RecommendedWatcher>,
        Arc<AtomicBool>,
    ),
    CliError,
> {
    let changed = Arc::new(AtomicBool::new(false));
    let changed_clone = changed.clone();

    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        move |res: Result<
            Vec<notify_debouncer_mini::DebouncedEvent>,
            notify_debouncer_mini::notify::Error,
        >| match res {
            Ok(events) => {
                let mut should_reload = false;
                for event in events {
                    let path = event.path;
                    if let Some(ext) = path.extension()
                        && (ext == "pure" || ext == "json")
                    {
                        should_reload = true;
                        break;
                    }
                }
                if should_reload {
                    changed_clone.store(true, Ordering::SeqCst);
                }
            }
            Err(e) => {
                eprintln!("Watch error: {e:?}");
            }
        },
    )
    .map_err(|e| CliError::Custom(format!("Failed to create file watcher: {e}")))?;

    debouncer
        .watcher()
        .watch(dir, RecursiveMode::Recursive)
        .map_err(|e| CliError::Custom(format!("Failed to watch directory: {e}")))?;

    Ok((debouncer, changed))
}
