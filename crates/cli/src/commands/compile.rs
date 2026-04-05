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

//! `legend compile` — Compile Pure files (parse + semantic analysis).
//!
//! **Status: Not yet implemented.**
//!
//! This command will perform full compilation: parse `.pure` files into an AST,
//! run the semantic layer to build a `PureModel`, and report any compilation
//! errors (type mismatches, unresolved references, constraint violations, etc.).
//!
//! This requires the `pure` crate (semantic layer), which is under development.
//!
//! # Future Usage
//!
//! ```bash
//! legend compile src/main/pure         # Compile a directory
//! legend compile model/Person.pure     # Compile specific files
//! legend compile                       # Compile project (using config)
//! ```

use std::path::PathBuf;

use crate::diagnostics::CliError;

/// Arguments for the `legend compile` command.
#[derive(clap::Args)]
pub struct CompileArgs {
    /// Input `.pure` file(s) or directory to compile.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Fail on warnings (strict mode).
    #[arg(long)]
    pub strict: bool,
}

/// Execute the `legend compile` command.
///
/// # Status
///
/// This command is not yet implemented. The Pure semantic layer (`PureModel`)
/// is under active development.
pub fn run(_args: CompileArgs) -> Result<(), CliError> {
    Err(CliError::NotImplemented {
        command: "compile",
        description: "Compile Pure files with semantic analysis",
        reason: "The Pure semantic layer (PureModel) is under active development.",
    })
}
