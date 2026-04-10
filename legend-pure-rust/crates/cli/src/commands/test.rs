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

//! `legend test` — Run Pure tests on testable elements.
//!
//! **Status: Not yet implemented.**
//!
//! This command will discover testable elements (functions with `tests {}` blocks),
//! compile them, execute the test cases, and report results.
//!
//! # Future Usage
//!
//! ```bash
//! legend test                          # Run all tests
//! legend test --filter "myFunction"    # Run matching tests
//! legend test src/test/pure/           # Run tests in a directory
//! ```

use std::path::PathBuf;

use crate::diagnostics::CliError;

/// Arguments for the `legend test` command.
#[derive(clap::Args)]
pub struct TestArgs {
    /// Input `.pure` file(s) or directory containing tests.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Filter tests by name pattern.
    #[arg(long, short)]
    pub filter: Option<String>,

    /// Show verbose test output.
    #[arg(long)]
    pub show_output: bool,
}

/// Execute the `legend test` command.
///
/// # Status
///
/// This command is not yet implemented. Test execution requires the
/// Pure semantic layer and an execution engine.
pub fn run(_args: TestArgs) -> Result<(), CliError> {
    Err(CliError::NotImplemented {
        command: "test",
        description: "Run Pure tests on testable elements",
        reason: "Test execution requires the Pure semantic layer and an execution engine.",
    })
}
