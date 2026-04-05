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

//! `legend publish` — Package and publish to the depot repository.
//!
//! **Status: Not yet implemented.**
//!
//! This command performs everything `legend package` does (parse, compile,
//! generate artifacts), and then publishes the resulting artifacts to a
//! depot repository. This is how Legend models are shared across projects
//! and teams.
//!
//! This is the Legend equivalent of `cargo publish` — it builds and uploads
//! your model to a shared registry.
//!
//! # Future Usage
//!
//! ```bash
//! legend publish                         # Package + publish to default depot
//! legend publish --depot https://depot.example.com
//! legend publish --dry-run               # Build artifacts without publishing
//! ```

use std::path::PathBuf;

use crate::diagnostics::CliError;

/// Arguments for the `legend publish` command.
#[derive(clap::Args)]
pub struct PublishArgs {
    /// Input `.pure` file(s) or directory to publish.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Depot repository URL to publish to.
    #[arg(long, value_name = "URL")]
    pub depot: Option<String>,

    /// Perform a dry run — package artifacts but don't publish.
    #[arg(long)]
    pub dry_run: bool,

    /// Output format for packaged artifacts.
    #[arg(long, default_value = "json", value_parser = ["json", "bincode"])]
    pub format: String,
}

/// Execute the `legend publish` command.
///
/// # Status
///
/// This command is not yet implemented. Publishing requires packaging
/// (parse + compile + artifact generation) and a depot repository client.
pub fn run(_args: PublishArgs) -> Result<(), CliError> {
    Err(CliError::NotImplemented {
        command: "publish",
        description: "Package and publish artifacts to the depot repository",
        reason: "Publishing requires packaging (parse + compile + artifact generation) and a depot repository client.",
    })
}
