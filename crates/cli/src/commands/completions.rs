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

//! `legend completions` — Generate shell completion scripts.
//!
//! Generates tab-completion scripts for bash, zsh, fish, `PowerShell`, and elvish.
//! The generated script should be sourced or installed into your shell's
//! completions directory.
//!
//! # Setup
//!
//! ```bash
//! # Zsh (macOS / Linux)
//! legend completions zsh > ~/.zfunc/_legend
//!
//! # Bash
//! legend completions bash >> ~/.bashrc
//!
//! # Fish
//! legend completions fish > ~/.config/fish/completions/legend.fish
//!
//! # PowerShell
//! legend completions powershell >> $PROFILE
//! ```

use clap::CommandFactory;
use clap_complete::Shell;

use crate::diagnostics::CliError;

/// Arguments for the `legend completions` command.
#[derive(clap::Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    pub shell: Shell,
}

/// Execute the `legend completions` command.
///
/// Writes a shell completion script to stdout.
#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
pub fn run(args: CompletionsArgs) -> Result<(), CliError> {
    let mut cmd = crate::Cli::command();
    clap_complete::generate(args.shell, &mut cmd, "legend", &mut std::io::stdout());
    Ok(())
}
