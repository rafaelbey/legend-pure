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

//! `legend package` — Parse, compile, and generate artifacts.
//!
//! **Status: Not yet implemented.**
//!
//! This command performs the full build pipeline: parse `.pure` files,
//! compile them through the semantic layer, and generate distributable
//! artifacts (e.g., serialized `PureModelContextData` bundles, execution
//! plans, generated code).
//!
//! This is the Legend equivalent of `cargo build --release` — it produces
//! artifacts ready for deployment or publication.
//!
//! # Future Usage
//!
//! ```bash
//! legend package                         # Package the current project
//! legend package --output dist/          # Package to a specific directory
//! legend package --format json           # Output format (json, bincode)
//! ```

use std::path::PathBuf;

use crate::diagnostics::CliError;

/// Arguments for the `legend package` command.
#[derive(clap::Args)]
pub struct PackageArgs {
    /// Input `.pure` file(s) or directory to package.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Output directory for generated artifacts.
    #[arg(short, long, value_name = "DIR", default_value = "target/package")]
    pub output: PathBuf,

    /// Output format for packaged artifacts.
    #[arg(long, default_value = "json", value_parser = ["json", "bincode"])]
    pub format: String,
}

/// Execute the `legend package` command.
///
/// # Status
///
/// This command is not yet implemented. Packaging requires the full pipeline:
/// parsing, semantic compilation, and artifact generation.
pub fn run(_args: PackageArgs) -> Result<(), CliError> {
    Err(CliError::NotImplemented {
        command: "package",
        description: "Parse, compile, and generate distributable artifacts",
        reason: "Packaging requires the Pure semantic layer and artifact generation, which are under active development.",
    })
}
