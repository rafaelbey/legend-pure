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

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn main() {
    if let Err(e) = generate() {
        panic!("build script failed: {e}");
    }
}

fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = env::var("OUT_DIR")?;
    let dest_path = Path::new(&out_dir).join("generated_sources.rs");

    // Platform directory from legend-pure Java module
    let platform_dir = PathBuf::from(
        "../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure",
    );

    let mut generated_code = String::new();
    generated_code.push_str("/// Array containing all embedded platform Pure files\n");
    generated_code.push_str("pub const PLATFORM_FILES: &[PureSourceFile] = &[\n");

    if platform_dir.exists() {
        for entry in WalkDir::new(&platform_dir) {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "pure") {
                let relative_path = path
                    .strip_prefix(&platform_dir)
                    .map_err(|e| format!("strip_prefix failed for {}: {e}", path.display()))?;
                let relative_path_str = relative_path
                    .to_str()
                    .ok_or_else(|| format!("non-UTF-8 path: {}", relative_path.display()))?
                    .replace('\\', "/");

                // Exclusions
                if relative_path_str == "grammar/m3.pure" {
                    continue;
                }

                let absolute_path = fs::canonicalize(path)
                    .map_err(|e| format!("canonicalize failed for {}: {e}", path.display()))?;
                let abs_path_str = absolute_path
                    .to_str()
                    .ok_or_else(|| format!("non-UTF-8 path: {}", absolute_path.display()))?
                    .replace('\\', "/");

                let _ = write!(
                    generated_code,
                    "    PureSourceFile {{\n        path: \"{relative_path_str}\",\n        content: include_str!(\"{abs_path_str}\"),\n    }},\n"
                );

                println!("cargo:rerun-if-changed={abs_path_str}");
            }
        }
    }

    println!("cargo:rerun-if-changed={}", platform_dir.display());

    generated_code.push_str("];\n");

    fs::write(&dest_path, generated_code)
        .map_err(|e| format!("failed to write {}: {e}", dest_path.display()))?;

    Ok(())
}
