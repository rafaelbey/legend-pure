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

//! Language Server Protocol server for Legend Pure.
//!
//! Speaks LSP 3.17 over stdio. The CLI's `legend lsp` subcommand wraps
//! this crate; an IDE plugin can also embed it directly.
//!
//! # Capabilities (tier-1)
//!
//! - `textDocument/publishDiagnostics` — driven by
//!   [`legend_pure_core_platform::repo::load`] error output.
//! - `textDocument/hover` — renders inferred type from
//!   `ValueSpec::type_info` via [`legend_pure_parser_pure::locate`].
//! - `textDocument/definition` — resolves identifier under cursor to
//!   the defining element's source span.
//! - `textDocument/documentSymbol` — outline of classes / functions /
//!   associations / etc. in a file.
//! - `textDocument/codeLens` — emits `legend.runTest` lenses for
//!   functions tagged with `<<test::Test>>`. Stub for the planned
//!   IntelliJ test runner integration.
//!
//! # Workspace model
//!
//! The LSP consumes a pre-resolved [`Vec<Repo>`][legend_pure_core_platform::repo::Repo]
//! supplied by the caller (typically [`crate::config::LspConfig`]
//! built from `legend lsp --classpath <toml>`). Filesystem repos are
//! shadowed by open buffers via canonical-path tail matching against
//! the file URL — sufficient for a single-project workspace.
//! `.purem` repos are consumed read-only and are never recompiled.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod config;
pub mod convert;
pub mod diagnostics;
pub mod handlers;
pub mod server;
pub mod workspace;

pub use config::LspConfig;
pub use server::Backend;

use tower_lsp::{LspService, Server};

/// Run the LSP over stdio with the given configuration.
///
/// Blocks until the client closes the stream. This is the entry point
/// for the `legend lsp` CLI subcommand.
///
/// # Errors
///
/// Returns the underlying tokio runtime error if the runtime fails to
/// build or the server task panics.
pub fn run(config: LspConfig) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::build(|client| Backend::new(client, config)).finish();
        Server::new(stdin, stdout, socket).serve(service).await;
    });
    Ok(())
}
