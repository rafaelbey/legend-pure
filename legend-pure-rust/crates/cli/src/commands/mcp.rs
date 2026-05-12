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

//! `legend mcp` — Run the Pure Model Context Protocol server over
//! stdio.
//!
//! Resolves the classpath using the same cascade as
//! `legend check`/`compile`/`lsp`/`dap`, feeds the resulting
//! `Vec<Repo>` into the [`legend_pure_mcp`] crate, and serves until
//! the MCP client closes stdin.
//!
//! Designed to be spawned as a subprocess by an AI coding agent
//! (Claude Code, Claude Desktop, custom runner). Example MCP config:
//!
//! ```json
//! {
//!   "legend": {
//!     "command": "legend",
//!     "args": ["mcp", "--classpath", "./legend-pure-classpath.toml"]
//!   }
//! }
//! ```
//!
//! # Examples
//!
//! ```bash
//! legend mcp                                    # auto-discover classpath
//! legend mcp --classpath ./legend-pure-classpath.toml
//! ```

use std::path::PathBuf;

use legend_pure_mcp::McpConfig;

use crate::diagnostics::CliError;

/// Arguments for the `legend mcp` command.
#[derive(clap::Args)]
pub struct McpArgs {
    /// Log level for the embedded server (overrides global `-v`).
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

/// Execute the `legend mcp` command.
///
/// `classpath_override` is the global `--classpath` flag passed
/// through from `Cli`; it takes priority over the discovery cascade.
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: McpArgs, classpath_override: Option<&std::path::Path>) -> Result<(), CliError> {
    let cwd: PathBuf = std::env::current_dir()
        .map_err(|e| CliError::Custom(format!("failed to read current directory: {e}")))?;
    let resolved = crate::classpath::resolve_classpath(classpath_override, &cwd)
        .map_err(|e| CliError::Custom(format!("failed to resolve classpath: {e}")))?;

    let mut config = McpConfig::from_repos(resolved.repos);
    config
        .auto_imports
        .extend(resolved.extra_auto_imports.iter().cloned());
    config.log_level = args.log_level;

    legend_pure_mcp::run(config).map_err(|e| CliError::Custom(format!("mcp server error: {e}")))
}
