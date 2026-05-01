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

//! Live-reload platform source loading and file watching.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::RecursiveMode;

use crate::diagnostics::CliError;

/// A Pure source file loaded from disk at runtime.
pub struct LiveSourceFile {
    /// Relative path within the platform directory (e.g., "essential/tests/assert.pure").
    /// Matches the canonical `/platform/pure/...` form.
    pub path: String,
    /// Source content.
    pub content: String,
}

/// Walk `platform_dir`, read all `.pure` files, return them with
/// canonical `/platform/pure/…` paths. Skips `grammar/m3.pure`.
pub fn load_from_disk(platform_dir: &Path) -> Result<Vec<LiveSourceFile>, CliError> {
    if !platform_dir.exists() || !platform_dir.is_dir() {
        return Err(CliError::Custom(format!(
            "Platform directory not found or not a directory: {}",
            platform_dir.display()
        )));
    }

    let mut sources = Vec::new();

    for entry in walkdir::WalkDir::new(platform_dir) {
        let entry = entry.map_err(|e| CliError::Custom(format!("WalkDir error: {e}")))?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        if ext != "pure" && ext != "json" {
            continue;
        }

        let relative_path = path.strip_prefix(platform_dir).map_err(|e| {
            CliError::Custom(format!("strip_prefix failed for {}: {e}", path.display()))
        })?;

        let relative_path_str = relative_path
            .to_str()
            .ok_or_else(|| {
                CliError::Custom(format!("non-UTF-8 path: {}", relative_path.display()))
            })?
            .replace('\\', "/");

        if ext == "pure" && relative_path_str == "grammar/m3.pure" {
            continue;
        }

        let canonical_path_str = format!("/platform/pure/{relative_path_str}");

        if ext == "pure" {
            let content = fs::read_to_string(path).map_err(|e| CliError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;

            sources.push(LiveSourceFile {
                path: canonical_path_str,
                content,
            });
        }
    }

    Ok(sources)
}

/// Resolve the platform dir from explicit --platform-dir or auto-discovery.
pub fn resolve_platform_dir(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }

    let cwd = std::env::current_dir()
        .map_err(|e| CliError::Custom(format!("Failed to get current directory: {e}")))?;

    if let Some(platform_dir) = discover_platform_dir(&cwd) {
        Ok(platform_dir)
    } else {
        Err(CliError::Custom(
            "Could not auto-discover platform directory (legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure). \
             Try running from within the repository, or use --platform-dir <PATH>.".into()
        ))
    }
}

/// Walk ancestors of `start_dir` looking for `legend-pure-core/`.
/// Returns the platform/pure subdirectory if found.
fn discover_platform_dir(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join("legend-pure-core");
        if candidate.is_dir() {
            let platform_dir =
                candidate.join("legend-pure-m3-core/src/main/resources/platform/pure");
            if platform_dir.is_dir() {
                return Some(platform_dir);
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
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
