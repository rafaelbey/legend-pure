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

//! `legend lsp` — Run the Pure Language Server Protocol server over
//! stdio.
//!
//! Resolves the classpath using the same cascade as `legend
//! check`/`compile` (explicit `--classpath`, env var, ancestor walk,
//! next-to-binary), feeds the resulting `Vec<Repo>` into the
//! [`legend_pure_lsp`] crate, and serves until the client closes
//! stdin.
//!
//! # Examples
//!
//! ```bash
//! legend lsp                                    # auto-discover classpath
//! legend lsp --classpath ./legend-pure-classpath.toml
//! ```

use std::path::PathBuf;

use legend_pure_lsp::LspConfig;

use crate::diagnostics::CliError;

/// Arguments for the `legend lsp` command.
#[derive(clap::Args)]
pub struct LspArgs {
    /// Log level for the embedded server (overrides global `-v`).
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

/// Execute the `legend lsp` command.
///
/// `classpath_override` is the global `--classpath` flag passed
/// through from `Cli`; it takes priority over the discovery cascade.
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: LspArgs, classpath_override: Option<&std::path::Path>) -> Result<(), CliError> {
    let cwd: PathBuf = std::env::current_dir()
        .map_err(|e| CliError::Custom(format!("failed to read current directory: {e}")))?;
    let resolved = crate::classpath::resolve_classpath(classpath_override, &cwd)
        .map_err(|e| CliError::Custom(format!("failed to resolve classpath: {e}")))?;

    let mut config = LspConfig::from_repos(resolved.repos);
    // Append any classpath-declared extra auto-imports on top of the
    // platform defaults (mirrors what `legend compile` does).
    config
        .auto_imports
        .extend(resolved.extra_auto_imports.iter().cloned());
    config.log_level = args.log_level;

    legend_pure_lsp::run(config).map_err(|e| CliError::Custom(format!("lsp server error: {e}")))
}
