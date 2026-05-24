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
//! - `find_references { fqn }`
//! - `list_packages { prefix? }`
//! - `list_tests { package_prefix? }`
//!
//! Lifecycle:
//! - `workspace_status`
//! - `reload_workspace`
//!
//! Behind the scenes every execute tool routes through
//! [`legend_pure_runtime::runner`], the same module the LSP's
//! `workspace/executeCommand` flow uses — so the IDE ▶ Run button
//! and the MCP `run_function` tool produce byte-identical typed
//! results.
//!
//! # Intended workflow
//!
//! The server is bound to a single compiled [`WorkspaceSnapshot`]
//! for its lifetime; the only state that changes between tool calls
//! is the `.pure` source on disk and the snapshot's `compiled_at`
//! timestamp. Downstream agents combine the tools as an
//! author–debug–verify loop:
//!
//! 1. **Author** — edit `.pure`, call `reload_workspace`, then
//!    `get_diagnostics { file }` for a scoped error read.
//! 2. **Explore** — `list_packages` → `search_symbols` →
//!    `read_element` to inspect the live compiled model rather than
//!    grep the filesystem.
//! 3. **Verify** — `run_function` for a parameterless eval,
//!    `run_test` on a leaf or package FQN for regression, `run_pct`
//!    for cross-engine parity on platform-style functions.
//!
//! See `README.md` for the longer-form workflow guide and
//! guardrails. `reload_workspace` is required after every `.pure`
//! edit; `workspace_status.compiled_at` is the source of truth for
//! snapshot freshness.
//!
//! # Workspace lifetime
//!
//! Compiled once at server startup; subsequent edits don't
//! auto-refresh. The snapshot lives in `Arc<Mutex<Arc<…>>>`: read
//! tools clone the inner `Arc` under the lock and drop it
//! immediately, while `reload_workspace` swaps the slot under the
//! same `Mutex`. The `Mutex` is never held across an `await`, so
//! tool calls never block one another beyond the swap itself.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod config;
mod error;
/// Public-but-low-traffic surface for the MCP tool argument /
/// result types so integration tests and downstream consumers can
/// call the `#[tool]`-decorated async methods directly without
/// going through the rmcp transport. The bulk of the module is
/// `pub(crate)`-shaped — only the wire types and the
/// [`LegendMcpServer`] re-exported below are intended for external
/// use.
pub mod server;
mod state;

pub use config::McpConfig;
pub use error::McpError;
pub use server::LegendMcpServer;
pub use state::WorkspaceSnapshot;

use std::sync::{Arc, Mutex};

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
        // Wrap repos in a `Mutex` so the `apply_edit` tool can
        // mutate the in-memory `Repo::Filesystem.files[i].content`
        // before recompiling. Reads (snapshot, reload, status)
        // clone the inner `Vec<Repo>` out of the lock and drop the
        // guard before any await — see `LegendMcpServer.repos_clone`.
        let repos = Arc::new(Mutex::new(config.repos));
        let auto_imports = Arc::new(config.auto_imports);
        let snapshot = {
            let initial_repos = repos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            Arc::new(WorkspaceSnapshot::compile(&initial_repos, &auto_imports))
        };
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
