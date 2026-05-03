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
use tower_lsp::jsonrpc::Result as JsonResult;
use tower_lsp::lsp_types::{
    CodeLens, CodeLensOptions, CodeLensParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, MessageType, OneOf, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer};

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
        let open_uris: Vec<Url> = ws.open_buffers.keys().cloned().collect();
        // Build a lookup from canonical path → URL so we can publish
        // diagnostics for files the user has open.
        let mut canonical_to_uri: std::collections::HashMap<String, Url> =
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
                Url::parse(&format!("file://{canonical}")).ok()
            });
            if let Some(uri) = uri {
                let diagnostics = handlers::diagnostics_for(&errors);
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
        }
        tracing::info!(error_count = outcome.error_count, "recompile complete");
    }
}

#[tower_lsp::async_trait]
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
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "legend-pure-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "legend-pure-lsp initialized")
            .await;
        self.recompile_and_publish().await;
    }

    async fn shutdown(&self) -> JsonResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
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
        Ok(
            handlers::definition_for_position(model, &canonical, position, &uri)
                .map(GotoDefinitionResponse::Scalar),
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
}

// Compile-time assertion that the `Backend` is `Send + Sync` — the
// `tower-lsp` runtime requires this and a missed `Mutex` (use of
// non-`Send` `RefCell` etc.) would only show up as a confusing
// trait-bound error far from the cause.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Backend>();
};
