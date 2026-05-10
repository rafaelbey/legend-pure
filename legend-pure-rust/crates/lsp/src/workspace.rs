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

//! Workspace state: open buffers, base repos, last compiled model.
//!
//! [`Workspace::compile`] is the per-recompile entry point. It clones
//! the base [`Repo`] list, patches in-flight buffer content into any
//! matching [`Repo::Filesystem`] file (URL-tail match against canonical
//! paths), and calls [`legend_pure_core_platform::repo::load`] to
//! produce a fresh [`PureModel`] (or [`PartialPureModel`]).
//!
//! `.purem` repos are cloned by `Arc<[u8]>` reference and never
//! recompiled — a key cost-of-edit constraint that keeps tier-1 LSP
//! latency manageable for downstream projects depending on
//! `platform.purem`.

use std::collections::HashMap;
use std::sync::Arc;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_store::compiler::RelationStoreExtension;
use legend_pure_parser_parser::SectionParser;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::refs::{ReferenceIndex, build_reference_index_with_extensions};
use smol_str::SmolStr;
use tower_lsp::lsp_types::Url;

/// Tracked state for an LSP workspace.
pub struct Workspace {
    /// Repos as configured at startup. Compile-time clones receive
    /// open-buffer overlays; this baseline is preserved for re-runs.
    pub base_repos: Vec<Repo>,
    /// Open buffers keyed by client URL.
    pub open_buffers: HashMap<Url, String>,
    /// Last compiled model, retained between requests so that hover /
    /// goto / outline can serve answers without recompiling on every
    /// keypress.
    pub model: Option<Arc<PureModel>>,
    /// Reference index built alongside `model` after each compile.
    /// Carries every clickable source span and its target — drives
    /// goto-def, find-references, hover, and the future
    /// semanticTokens response. Rebuilt per compile because source
    /// spans move when the user edits.
    pub references: Option<Arc<ReferenceIndex>>,
    /// Diagnostics from the most recent compile, keyed by canonical
    /// source path (as the compiler sees it). The handler turns these
    /// into per-URL `publishDiagnostics`.
    pub diagnostics: HashMap<String, Vec<CompilationError>>,
    /// Auto-import packages forwarded to every compile.
    pub auto_imports: Vec<SmolStr>,
}

impl Workspace {
    /// Construct a fresh workspace from the configured repos.
    #[must_use]
    pub fn new(base_repos: Vec<Repo>, auto_imports: Vec<SmolStr>) -> Self {
        Self {
            base_repos,
            open_buffers: HashMap::new(),
            model: None,
            references: None,
            diagnostics: HashMap::new(),
            auto_imports,
        }
    }

    /// Record an open or modified buffer.
    pub fn set_open_buffer(&mut self, uri: Url, content: String) {
        self.open_buffers.insert(uri, content);
    }

    /// Drop a closed buffer. After this, the next compile reads the
    /// file from disk again (Filesystem repo) or from the embedded
    /// blob (Purem repo).
    pub fn close_buffer(&mut self, uri: &Url) {
        self.open_buffers.remove(uri);
    }

    /// Build a snapshot of [`Repo`]s with open-buffer content patched
    /// into matching `Filesystem` files.
    ///
    /// Match strategy: for each open buffer URL, take its `to_file_path`,
    /// and for every Filesystem repo file, override the content if the
    /// URL's filesystem path ends with the canonical path's tail
    /// (without the leading `/`). This handles the typical single-project
    /// workspace where the canonical path `/myproj/foo.pure` is the
    /// suffix of `/abs/path/myproj/foo.pure`.
    #[must_use]
    pub fn snapshot_repos(&self) -> Vec<Repo> {
        let mut out = self.base_repos.clone();
        if self.open_buffers.is_empty() {
            return out;
        }
        for (uri, content) in &self.open_buffers {
            let Ok(disk_path) = uri.to_file_path() else {
                continue;
            };
            for repo in &mut out {
                if let Repo::Filesystem { files, .. } = repo {
                    for f in files.iter_mut() {
                        let canonical_tail = f.path.trim_start_matches('/');
                        if disk_path.ends_with(canonical_tail) {
                            f.content = content.clone();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Recompile the workspace. Returns the result so the handler can
    /// publish diagnostics; also stores a fresh `Arc<PureModel>` and a
    /// per-source diagnostic index on `self`.
    pub fn compile(&mut self) -> CompileOutcome {
        let snapshot = self.snapshot_repos();
        // Fresh DSL extensions per compile — each carries `RefCell`
        // state populated during `declare`/`define_*` and walked by
        // `walk_references`. Reusing across compiles would accumulate
        // stale entries unless every extension's `declare` started
        // with a clear, which today's implementations don't do.
        let mapping = MappingExtension::new();
        let relational = RelationalExtension::new();
        let store = RelationStoreExtension::new();
        let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];
        let result = repo::load_with_extensions(
            &snapshot,
            &self.auto_imports,
            &extensions,
            // Each repo gets fresh section parsers. `SectionParser`
            // is not `Clone`, so we instantiate inside the closure.
            &mut || -> Vec<Box<dyn SectionParser>> {
                vec![
                    Box::new(MappingSectionParser::new()),
                    Box::new(RelationalSectionParser),
                ]
            },
        );
        let (model, errors) = match result {
            Ok(m) => (m, Vec::<CompilationError>::new()),
            Err(partial) => (partial.model, partial.errors),
        };
        let mut by_source: HashMap<String, Vec<CompilationError>> = HashMap::new();
        for err in &errors {
            by_source
                .entry(err.source_info.source.to_string())
                .or_default()
                .push(err.clone());
        }
        self.diagnostics = by_source;
        let model_arc = Arc::new(model);
        // Build the reference index from the freshly-compiled model,
        // including DSL extension contributions. Cheap (single linear
        // walk over chunks + each extension's RefCell).
        self.references = Some(Arc::new(build_reference_index_with_extensions(
            &model_arc,
            &extensions,
        )));
        self.model = Some(model_arc);
        CompileOutcome {
            error_count: errors.len(),
        }
    }

    /// Map an open-buffer URL to the canonical source path the
    /// compiler uses for it. Returns `None` if the URL doesn't
    /// resolve to any known source path.
    #[must_use]
    pub fn canonical_path_for(&self, uri: &Url) -> Option<String> {
        let disk_path = uri.to_file_path().ok()?;
        for repo in &self.base_repos {
            if let Repo::Filesystem { files, .. } = repo {
                for f in files {
                    let tail = f.path.trim_start_matches('/');
                    if disk_path.ends_with(tail) {
                        return Some(f.path.clone());
                    }
                }
            }
        }
        None
    }

    /// Inverse of [`Self::canonical_path_for`]: given a canonical
    /// source path the compiler emits (e.g. `/user_proj/foo/bar.pure`),
    /// produce a `file://` URL the IDE can navigate to.
    ///
    /// Resolution order:
    /// 1. If an open buffer's URL maps to this canonical (via
    ///    [`Self::canonical_path_for`]), reuse its URL.
    /// 2. Walk [`Self::base_repos`] for a [`Repo::Filesystem`] whose
    ///    `prefix` matches the canonical's leading segment AND whose
    ///    `source_root` was populated. Reconstruct the on-disk path
    ///    as `source_root.join(canonical[prefix.len()..])`.
    ///
    /// Returns `None` for canonicals served from [`Repo::Embedded`] /
    /// [`Repo::Purem`] (no on-disk source) or filesystem repos built
    /// without a `source_root` (synthetic test fixtures).
    #[must_use]
    pub fn file_uri_for_canonical(&self, canonical: &str) -> Option<Url> {
        for uri in self.open_buffers.keys() {
            if self.canonical_path_for(uri).as_deref() == Some(canonical) {
                return Some(uri.clone());
            }
        }
        for repo in &self.base_repos {
            let Repo::Filesystem {
                prefix,
                source_root,
                ..
            } = repo
            else {
                continue;
            };
            let Some(root) = source_root else { continue };
            let Some(rel) = canonical.strip_prefix(prefix.as_str()) else {
                continue;
            };
            // Strip the leading '/' so `join` treats `rel` as relative
            // — otherwise `join("/abs")` discards the root.
            let rel = rel.trim_start_matches('/');
            let disk = root.join(rel);
            if let Ok(url) = Url::from_file_path(&disk) {
                return Some(url);
            }
        }
        None
    }
}

/// Summary of a compile pass; full per-source diagnostics live on
/// [`Workspace::diagnostics`].
#[derive(Debug, Clone, Copy)]
pub struct CompileOutcome {
    /// Total number of errors produced.
    pub error_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_core_platform::repo::OwnedSourceFile;
    use std::path::PathBuf;

    fn fs_repo_with_root(prefix: &str, source_root: PathBuf, files: Vec<&str>) -> Repo {
        let owned = files
            .into_iter()
            .map(|p| OwnedSourceFile {
                path: p.to_string(),
                content: String::new(),
            })
            .collect();
        Repo::Filesystem {
            prefix: prefix.to_string(),
            files: owned,
            meta: None,
            source_root: Some(source_root),
        }
    }

    #[test]
    fn file_uri_for_canonical_resolves_via_source_root() {
        // /tmp/proj/lib.pure is the on-disk file; canonical is /proj/lib.pure
        let repo = fs_repo_with_root(
            "/proj",
            PathBuf::from("/tmp/proj"),
            vec!["/proj/lib.pure", "/proj/sub/foo.pure"],
        );
        let ws = Workspace::new(vec![repo], Vec::new());

        let url = ws
            .file_uri_for_canonical("/proj/lib.pure")
            .expect("filesystem repo with source_root must resolve");
        assert_eq!(url.scheme(), "file");
        assert!(
            url.path().ends_with("/tmp/proj/lib.pure"),
            "got path: {}",
            url.path()
        );

        let nested = ws
            .file_uri_for_canonical("/proj/sub/foo.pure")
            .expect("nested canonical must resolve");
        assert!(nested.path().ends_with("/tmp/proj/sub/foo.pure"));
    }

    #[test]
    fn file_uri_for_canonical_returns_none_without_source_root() {
        // Synthetic test fixture: source_root is None.
        let repo = Repo::Filesystem {
            prefix: "/proj".to_string(),
            files: vec![OwnedSourceFile {
                path: "/proj/x.pure".into(),
                content: String::new(),
            }],
            meta: None,
            source_root: None,
        };
        let ws = Workspace::new(vec![repo], Vec::new());
        assert!(ws.file_uri_for_canonical("/proj/x.pure").is_none());
    }

    #[test]
    fn file_uri_for_canonical_returns_none_for_unknown_canonical() {
        let repo = fs_repo_with_root(
            "/proj",
            PathBuf::from("/tmp/proj"),
            vec!["/proj/lib.pure"],
        );
        let ws = Workspace::new(vec![repo], Vec::new());
        // Canonical's prefix doesn't match any repo.
        assert!(ws.file_uri_for_canonical("/other/lib.pure").is_none());
    }

    #[test]
    fn file_uri_for_canonical_prefers_open_buffer_url() {
        // When a buffer is open for the matching canonical, that URL
        // is returned in preference to the synthesised disk URL.
        let repo = fs_repo_with_root(
            "/proj",
            PathBuf::from("/tmp/proj"),
            vec!["/proj/lib.pure"],
        );
        let mut ws = Workspace::new(vec![repo], Vec::new());
        let buffer_uri = Url::parse("file:///tmp/proj/lib.pure").unwrap();
        ws.set_open_buffer(buffer_uri.clone(), "...".into());
        let resolved = ws.file_uri_for_canonical("/proj/lib.pure").unwrap();
        assert_eq!(resolved, buffer_uri);
    }
}
