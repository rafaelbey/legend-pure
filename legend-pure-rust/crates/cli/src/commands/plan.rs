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

//! `legend plan` — Generate an execution plan for a function or query.
//!
//! **Status: Not yet implemented.**
//!
//! This command will compile a Pure function, generate an execution plan,
//! and output it as JSON. Execution plans describe how the Legend Engine
//! will execute a query against one or more data stores.
//!
//! # Future Usage
//!
//! ```bash
//! legend plan "my::package::myFunction()"
//! legend plan --element "my::package::myFunction" --runtime my-runtime.json
//! ```

use crate::diagnostics::CliError;

/// Arguments for the `legend plan` command.
#[derive(clap::Args)]
pub struct PlanArgs {
    /// Fully qualified function/query to generate a plan for.
    pub element: String,

    /// Path to a runtime configuration file.
    #[arg(long, value_name = "FILE")]
    pub runtime: Option<std::path::PathBuf>,

    /// Output format (json or text).
    #[arg(long, default_value = "json")]
    pub format: String,
}

/// Execute the `legend plan` command.
///
/// # Status
///
/// This command is not yet implemented. Execution plan generation requires
/// the Pure semantic layer, connection management, and the plan generator.
pub fn run(_args: PlanArgs) -> Result<(), CliError> {
    Err(CliError::NotImplemented {
        command: "plan",
        description: "Generate an execution plan for a function or query",
        reason: "Plan generation requires the Pure semantic layer, connection management, and the plan generator.",
    })
}
