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

//! [`Backend`] — the [`LanguageServer`] trait implementation.
//!
//! Each request handler grabs the workspace mutex, delegates to a pure
//! function in [`crate::handlers`], and emits whatever the function
//! produced. Compile is synchronous within the request — fine for
//! tier-1; `notification`-class events (`did_change`, `did_save`) drop
//! the lock before publishing diagnostics so the client doesn't see a
//! stale model.

use std::sync::Arc;

use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result as JsonResult;
use tower_lsp_server::ls_types::{
    CodeLens, CodeLensOptions, CodeLensParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, ExecuteCommandOptions,
    ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, LSPAny,
    MessageType, OneOf, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tower_lsp_server::{Client, LanguageServer};

use crate::config::LspConfig;
use crate::handlers;
use crate::workspace::Workspace;

/// LSP backend. Owns the workspace state and the `Client` handle for
/// sending notifications.
pub struct Backend {
    client: Client,
    workspace: Arc<Mutex<Workspace>>,
}

impl Backend {
    /// Construct a backend for the given client + initial config.
    #[must_use]
    pub fn new(client: Client, config: LspConfig) -> Self {
        let workspace = Workspace::new(config.repos, config.auto_imports);
        Self {
            client,
            workspace: Arc::new(Mutex::new(workspace)),
        }
    }

    async fn recompile_and_publish(&self) {
        let mut ws = self.workspace.lock().await;
        let outcome = ws.compile();
        // Snapshot the per-source diagnostics + any open URLs so we
        // can drop the lock before doing the (potentially many)
        // notification round-trips.
        let diags_by_source: Vec<(String, Vec<_>)> = ws
            .diagnostics
            .iter()
            .map(|(path, errs)| (path.clone(), errs.clone()))
            .collect();
        let open_uris: Vec<Uri> = ws.open_buffers.keys().cloned().collect();
        // Build a lookup from canonical path → URL so we can publish
        // diagnostics for files the user has open.
        let mut canonical_to_uri: std::collections::HashMap<String, Uri> =
            std::collections::HashMap::new();
        for uri in &open_uris {
            if let Some(path) = ws.canonical_path_for(uri) {
                canonical_to_uri.insert(path, uri.clone());
            }
        }
        drop(ws);

        // Always publish empty diagnostic lists for currently-open
        // files that have no errors — otherwise stale diagnostics
        // would linger after a fix.
        for uri in &open_uris {
            self.client
                .publish_diagnostics(uri.clone(), Vec::new(), None)
                .await;
        }
        for (canonical, errors) in diags_by_source {
            let uri = canonical_to_uri.get(&canonical).cloned().or_else(|| {
                // Best-effort URL from canonical: file:///<canonical>
                format!("file://{canonical}").parse::<Uri>().ok()
            });
            if let Some(uri) = uri {
                let diagnostics = handlers::diagnostics_for(&errors);
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
        }
        tracing::info!(error_count = outcome.error_count, "recompile complete");
        // Echo into the client's log so the LSP4IJ "Logs" tab shows
        // activity each time the workspace recompiles. Helps users
        // tell whether the LSP is alive and producing output.
        let level = if outcome.error_count == 0 {
            MessageType::INFO
        } else {
            MessageType::WARNING
        };
        self.client
            .log_message(
                level,
                format!("recompile complete: {} error(s)", outcome.error_count),
            )
            .await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> JsonResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                // `▶ Run` / `▶ Run test` gutter clicks dispatch
                // through `workspace/executeCommand`. The IDE-side
                // gutter contributor (`PureRunLineMarkerContributor`
                // in the IntelliJ plugin) sends one of these two
                // command names with the function FQN as
                // `arguments[0]`; the server evaluates the function
                // against its in-memory `PureModel` and returns the
                // rendered value. Goes through the LSP instead of a
                // separate CLI subprocess so we hit the workspace's
                // classpath + open buffers, not just the embedded
                // platform.
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        "legend.run".into(),
                        "legend.runTest".into(),
                        // `<<PCT.test>>` execution — takes `[fqn,
                        // adapterFqn]` so the IDE-side popup can
                        // pre-resolve which adapter to use.
                        "legend.runPCT".into(),
                        // Adapter discovery — returns `[{name,
                        // fqn}]` from the compiled model. Backs
                        // the PCT-adapter popup.
                        "legend.listPctAdapters".into(),
                    ],
                    work_done_progress_options: Default::default(),
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "legend-pure-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            // Negotiated text-encoding for `Position` columns. `Utf16`
            // matches the LSP spec default — the only encoding the
            // current handlers compute against. Required by the
            // tower-lsp-server fork as an explicit field; the
            // upstream `tower-lsp` 0.20 defaulted it implicitly.
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        tracing::info!("initialized notification received");
        // Surface the resolved repo set so a user staring at "no
        // diagnostics, no hover" can immediately tell whether their
        // project sources are in scope.
        let (repo_summary, has_filesystem) = {
            let ws = self.workspace.lock().await;
            let mut filesystem = 0usize;
            let mut purem = 0usize;
            let mut other = 0usize;
            let mut total_files = 0usize;
            for repo in &ws.base_repos {
                use legend_pure_core_platform::repo::Repo;
                match repo {
                    Repo::Filesystem { files, .. } => {
                        filesystem += 1;
                        total_files += files.iter().filter(|f| f.path.ends_with(".pure")).count();
                    }
                    Repo::Purem { .. } => purem += 1,
                    _ => other += 1,
                }
            }
            (
                format!(
                    "legend-pure-lsp initialized: {filesystem} filesystem repo(s) ({total_files} \
                     .pure files), {purem} purem repo(s), {other} other"
                ),
                filesystem > 0,
            )
        };
        self.client
            .log_message(MessageType::INFO, &repo_summary)
            .await;
        if !has_filesystem {
            // Without a filesystem repo, hover/goto/outline silently
            // return None for any user file — that confused the first
            // smoke run. Surface it as a warning popup rather than
            // letting the LSP look broken.
            self.client
                .show_message(
                    MessageType::WARNING,
                    "No filesystem repos in classpath. Hover, go-to-definition, and \
                     diagnostics on user `.pure` files will not be available until a \
                     `legend-pure-classpath.toml` declares a filesystem repo.",
                )
                .await;
        }
        self.recompile_and_publish().await;
    }

    async fn shutdown(&self) -> JsonResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        tracing::info!(uri = params.text_document.uri.as_str(), "did_open received");
        let uri = params.text_document.uri;
        let content = params.text_document.text;
        {
            let mut ws = self.workspace.lock().await;
            ws.set_open_buffer(uri.clone(), content);
        }
        self.recompile_and_publish().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // FULL text-document sync — last change wins.
        let uri = params.text_document.uri;
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        {
            let mut ws = self.workspace.lock().await;
            ws.set_open_buffer(uri.clone(), change.text);
        }
        self.recompile_and_publish().await;
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        // Save flushes to disk but the open buffer already shadows
        // it, so a recompile is enough.
        self.recompile_and_publish().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut ws = self.workspace.lock().await;
            ws.close_buffer(&uri);
        }
        // Clear diagnostics for the closed file so they don't linger.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> JsonResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let ws = self.workspace.lock().await;
        let Some(model) = ws.model.as_ref() else {
            return Ok(None);
        };
        let Some(canonical) = ws.canonical_path_for(&uri) else {
            return Ok(None);
        };
        Ok(handlers::hover_for_position(model, &canonical, position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonResult<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let ws = self.workspace.lock().await;
        let Some(model) = ws.model.as_ref() else {
            return Ok(None);
        };
        let Some(canonical) = ws.canonical_path_for(&uri) else {
            return Ok(None);
        };
        let resolver = |c: &str| ws.file_uri_for_canonical(c);
        let references = ws.references.as_deref();
        // `Link` (LocationLink[]) instead of `Scalar` (Location):
        // origin_selection_range tells IntelliJ which source range to
        // underline on ⌘-hover. Without it the request flows but the
        // visual affordance never renders.
        Ok(
            handlers::definition_for_position(
                model, references, &canonical, position, &uri, &resolver,
            )
            .map(|link| GotoDefinitionResponse::Link(vec![link])),
        )
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonResult<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let ws = self.workspace.lock().await;
        let Some(model) = ws.model.as_ref() else {
            return Ok(None);
        };
        let Some(canonical) = ws.canonical_path_for(&uri) else {
            return Ok(None);
        };
        let symbols: Vec<DocumentSymbol> = handlers::document_symbols_for(model, &canonical);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonResult<Option<WorkspaceSymbolResponse>> {
        let query = params.query;
        let ws = self.workspace.lock().await;
        let Some(model) = ws.model.as_ref() else {
            tracing::debug!(query = %query, "workspace/symbol: no model yet");
            return Ok(None);
        };
        let resolver = |c: &str| ws.file_uri_for_canonical(c);
        let syms = handlers::workspace_symbols_for(model, &query, &resolver);
        tracing::debug!(query = %query, count = syms.len(), "workspace/symbol");
        // `Nested` is the modern `WorkspaceSymbol[]` shape — the
        // `Flat` variant returns the deprecated `SymbolInformation[]`.
        // Always use Nested so consumers (IDE clients) only need to
        // handle the non-deprecated path.
        Ok(Some(WorkspaceSymbolResponse::Nested(syms)))
    }

    async fn code_lens(&self, params: CodeLensParams) -> JsonResult<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri;
        let ws = self.workspace.lock().await;
        let Some(model) = ws.model.as_ref() else {
            return Ok(None);
        };
        let Some(canonical) = ws.canonical_path_for(&uri) else {
            return Ok(None);
        };
        Ok(Some(handlers::code_lenses_for(model, &canonical)))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> JsonResult<Option<LSPAny>> {
        let command = params.command;
        tracing::info!(
            command = %command,
            arg_count = params.arguments.len(),
            "workspace/executeCommand",
        );
        // Run in the LSP's own runtime, against the workspace's
        // compiled `PureModel`. This is what lets the gutter ▶
        // icon see user-defined functions in the configured
        // classpath — shelling out to `legend run` only ever
        // loads the embedded platform.
        let result = handlers::execute_legend_command(
            self.workspace.clone(),
            command.as_str(),
            &params.arguments,
        )
        .await;
        self.client
            .log_message(
                if result.error.is_some() {
                    MessageType::WARNING
                } else {
                    MessageType::INFO
                },
                result.summary(),
            )
            .await;
        Ok(Some(serde_json::to_value(&result).unwrap_or(serde_json::Value::Null)))
    }
}

// Compile-time assertion that the `Backend` is `Send + Sync` — the
// `tower-lsp` runtime requires this and a missed `Mutex` (use of
// non-`Send` `RefCell` etc.) would only show up as a confusing
// trait-bound error far from the cause.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Backend>();
};
