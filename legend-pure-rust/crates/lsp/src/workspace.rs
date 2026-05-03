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
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::model::PureModel;
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
        let result = repo::load(&snapshot, &self.auto_imports);
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
        self.model = Some(Arc::new(model));
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
}

/// Summary of a compile pass; full per-source diagnostics live on
/// [`Workspace::diagnostics`].
#[derive(Debug, Clone, Copy)]
pub struct CompileOutcome {
    /// Total number of errors produced.
    pub error_count: usize,
}
