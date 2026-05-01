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

/// One Pure repo to embed: a filesystem root + a virtual path prefix
/// applied to every file's `SourceInformation.source`.
///
/// Until the descriptor-driven loader lands (see deferred backlog
/// item: "Repo descriptors + manifest"), repos are enumerated here
/// directly. The prefix mirrors Java Pure's resource-URL convention:
/// `legend-pure-m3-core/.../platform/pure/<rel>` ↔ `/platform/pure/<rel>`,
/// `legend-pure-dsl-store/.../platform_dsl_store/<rel>` ↔ `/platform_dsl_store/<rel>`.
struct PureRepo {
    root: PathBuf,
    prefix: &'static str,
}

fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = env::var("OUT_DIR")?;
    let dest_path = Path::new(&out_dir).join("generated_sources.rs");

    let repos = [
        PureRepo {
            root: PathBuf::from(
                "../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure",
            ),
            prefix: "/platform/pure",
        },
        PureRepo {
            root: PathBuf::from(
                "../../../legend-pure-dsl/legend-pure-dsl-store/legend-pure-m2-dsl-store-pure/src/main/resources/platform_dsl_store",
            ),
            prefix: "/platform_dsl_store",
        },
        PureRepo {
            root: PathBuf::from(
                "../../../legend-pure-dsl/legend-pure-dsl-diagram/legend-pure-m2-dsl-diagram-pure/src/main/resources/platform_dsl_diagram",
            ),
            prefix: "/platform_dsl_diagram",
        },
        PureRepo {
            root: PathBuf::from(
                "../../../legend-pure-dsl/legend-pure-dsl-tds/legend-pure-m2-dsl-tds-pure/src/main/resources/platform_dsl_tds",
            ),
            prefix: "/platform_dsl_tds",
        },
    ];

    let mut generated_code = String::new();
    generated_code.push_str("/// Array containing all embedded platform Pure files\n");
    generated_code.push_str("pub const PLATFORM_FILES: &[PureSourceFile] = &[\n");

    let mut manifest_entries = String::new();

    for repo in &repos {
        embed_repo(repo, &mut generated_code, &mut manifest_entries)?;
        println!("cargo:rerun-if-changed={}", repo.root.display());
    }

    generated_code.push_str("];\n\n");
    generated_code
        .push_str("/// Array containing all embedded platform JSON manifests (PCT, etc.).\n");
    generated_code.push_str("pub const PLATFORM_MANIFESTS: &[PureSourceFile] = &[\n");
    generated_code.push_str(&manifest_entries);
    generated_code.push_str("];\n");

    fs::write(&dest_path, generated_code)
        .map_err(|e| format!("failed to write {}: {e}", dest_path.display()))?;

    Ok(())
}

fn embed_repo(
    repo: &PureRepo,
    generated_code: &mut String,
    manifest_entries: &mut String,
) -> Result<(), Box<dyn std::error::Error>> {
    if !repo.root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(&repo.root) {
        let entry = entry?;
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
        let relative_path = path
            .strip_prefix(&repo.root)
            .map_err(|e| format!("strip_prefix failed for {}: {e}", path.display()))?;
        let relative_path_str = relative_path
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 path: {}", relative_path.display()))?
            .replace('\\', "/");

        // m3.pure is the bootstrap instance graph and is parsed by
        // a special-purpose reader, not the regular parser.
        if ext == "pure" && relative_path_str == "grammar/m3.pure" {
            continue;
        }

        // Embed paths in the Java Pure "resource URL" form — see the
        // `PureRepo.prefix` field comment for why these are stable.
        let canonical_path_str = format!("{}/{relative_path_str}", repo.prefix);

        let absolute_path = fs::canonicalize(path)
            .map_err(|e| format!("canonicalize failed for {}: {e}", path.display()))?;
        let abs_path_str = absolute_path
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 path: {}", absolute_path.display()))?
            .replace('\\', "/");

        if ext == "pure" {
            let _ = write!(
                generated_code,
                "    PureSourceFile {{\n        path: \"{canonical_path_str}\",\n        content: include_str!(\"{abs_path_str}\"),\n    }},\n"
            );
        } else {
            let _ = write!(
                manifest_entries,
                "    PureSourceFile {{\n        path: \"{canonical_path_str}\",\n        content: include_str!(\"{abs_path_str}\"),\n    }},\n"
            );
        }

        println!("cargo:rerun-if-changed={abs_path_str}");
    }
    Ok(())
}
