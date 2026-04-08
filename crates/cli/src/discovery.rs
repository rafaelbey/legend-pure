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

//! File discovery for `.pure` source files.
//!
//! Walks directories recursively to find all `.pure` files,
//! respecting ignore patterns.

use std::path::{Path, PathBuf};

use crate::diagnostics::CliError;

/// Recursively find all `.pure` files in a directory.
///
/// Files are returned in sorted order for deterministic output.
///
/// # Errors
///
/// Returns an error if the directory doesn't exist or can't be read.
pub fn find_pure_files(dir: &Path) -> Result<Vec<PathBuf>, CliError> {
    if !dir.exists() {
        return Err(CliError::FileNotFound(dir.to_path_buf()));
    }
    if !dir.is_dir() {
        return Err(CliError::Custom(format!(
            "'{}' is not a directory",
            dir.display()
        )));
    }

    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "pure")
        })
        .filter(|entry| !is_ignored(entry.path()))
        .map(walkdir::DirEntry::into_path)
        .collect();

    files.sort();
    Ok(files)
}

/// Resolves input paths to a flat list of `.pure` files.
///
/// Files are kept as-is. Directories are recursively walked for `.pure` files.
///
/// # Errors
///
/// Returns `FileNotFound` if any specified path doesn't exist.
pub fn resolve_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, CliError> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            files.extend(find_pure_files(path)?);
        } else if path.exists() {
            files.push(path.clone());
        } else {
            return Err(CliError::FileNotFound(path.clone()));
        }
    }
    Ok(files)
}

/// Extracts the file name from a path as a string.
///
/// Falls back to `"<unknown>"` if the path has no file name component.
/// This is used as the `source_id` when parsing `.pure` files.
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "<unknown>".to_string(),
        |n| n.to_string_lossy().to_string(),
    )
}

/// Checks if a path should be ignored.
///
/// Currently uses a hardcoded set of patterns. In the future, this will
/// read from `.legendignore` files.
fn is_ignored(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    // Skip common non-source directories
    let ignored_dirs = ["target/", ".git/", ".idea/", ".vscode/", "node_modules/"];
    for dir in &ignored_dirs {
        if path_str.contains(dir) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ignored() {
        assert!(is_ignored(Path::new("project/target/debug/test.pure")));
        assert!(is_ignored(Path::new("project/.git/hooks/test.pure")));
        assert!(!is_ignored(Path::new("project/src/model.pure")));
    }
}
