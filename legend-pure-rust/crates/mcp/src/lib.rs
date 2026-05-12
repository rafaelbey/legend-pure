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

//! Model Context Protocol server for Legend Pure.
//!
//! Sibling to [`legend-pure-lsp`](../legend_pure_lsp/index.html) and
//! [`legend-pure-dap`](../legend_pure_dap/index.html). The MCP server
//! speaks JSON-RPC 2.0 over stdio (the MCP standard transport) and
//! exposes the compiled workspace as a curated set of typed tools so
//! AI coding agents (Claude Code, Claude Desktop, custom runners)
//! can search symbols, run functions, and surface diagnostics without
//! grepping the filesystem or shelling out to ad-hoc commands.
//!
//! # MVP tool surface
//!
//! Read:
//! - `search_symbols { query, limit? }`
//! - `get_diagnostics { file? }`
//!
//! Execute:
//! - `run_function { fqn }`
//! - `run_test { fqn }`
//! - `run_pct { test_fqn, adapter_fqn }`
//! - `list_pct_adapters`
//!
//! Introspect:
//! - `read_element { fqn }`
//! - `list_packages { prefix? }`
//! - `list_tests { package_prefix? }`
//!
//! Behind the scenes every execute tool routes through
//! [`legend_pure_runtime::runner`], the same module the LSP's
//! `workspace/executeCommand` flow uses — so the IDE ▶ Run button
//! and the MCP `run_function` tool produce byte-identical typed
//! results.
//!
//! # Workspace lifetime
//!
//! The workspace is compiled **once** at server startup. Agents
//! shouldn't typically mutate sources through the MCP, so a single
//! up-front compile keeps the implementation simple. A future
//! `reload_workspace` tool can be added when concurrent agent-write
//! workflows arise.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod config;
mod error;
mod server;
mod state;

pub use config::McpConfig;
pub use error::McpError;
pub use server::LegendMcpServer;
pub use state::WorkspaceSnapshot;

use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::stdio;

/// Run the MCP server over stdio with the given configuration.
///
/// Blocks until the client closes the stream. This is the entry
/// point for the `legend mcp` CLI subcommand.
///
/// # Errors
///
/// - [`McpError::Runtime`] if tokio fails to build the runtime.
/// - [`McpError::Service`] if the rmcp service setup or wait loop
///   fails.
pub fn run(config: McpConfig) -> Result<(), McpError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let repos = Arc::new(config.repos);
        let auto_imports = Arc::new(config.auto_imports);
        let snapshot = Arc::new(WorkspaceSnapshot::compile(&repos, &auto_imports));
        tracing::info!(
            error_count = snapshot.error_count,
            chunks = snapshot.model.chunks.len(),
            compiled_at = %snapshot.compiled_at,
            "legend mcp server starting",
        );
        let server = LegendMcpServer::new(snapshot, repos, auto_imports);
        let service = server
            .serve(stdio())
            .await
            .map_err(|e| McpError::Service(e.to_string()))?;
        service
            .waiting()
            .await
            .map_err(|e| McpError::Service(e.to_string()))?;
        Ok(())
    })
}
