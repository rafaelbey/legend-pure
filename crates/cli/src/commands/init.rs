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

//! `legend init` — Initialize a new Legend project.
//!
//! Scaffolds a directory structure with starter `.pure` files
//! to get a new Legend project off the ground quickly.
//!
//! # Examples
//!
//! ```bash
//! # Create a new project
//! legend init my-model
//!
//! # Creates:
//! # my-model/
//! # ├── src/
//! # │   └── main/
//! # │       └── pure/
//! # │           └── model.pure
//! # └── .legendignore
//! ```

use std::path::{Path, PathBuf};

use owo_colors::OwoColorize;

use crate::diagnostics::CliError;

/// Arguments for the `legend init` command.
#[derive(clap::Args)]
pub struct InitArgs {
    /// Name of the project to create (creates a directory with this name).
    pub name: String,

    /// Group identifier (e.g., `org.finos.legend`).
    #[arg(long, default_value = "org.example")]
    pub group: String,
}

/// Execute the `legend init` command.
#[allow(clippy::needless_pass_by_value)] // clap convention: Args are passed by value
pub fn run(args: InitArgs) -> Result<(), CliError> {
    let project_dir = PathBuf::from(&args.name);

    if project_dir.exists() {
        return Err(CliError::Custom(format!(
            "directory '{}' already exists",
            args.name
        )));
    }

    eprintln!(
        "{} project '{}'...",
        "Creating".green().bold(),
        args.name,
    );

    // Create directory structure
    let pure_dir = project_dir.join("src").join("main").join("pure");
    std::fs::create_dir_all(&pure_dir).map_err(|e| CliError::Io {
        path: pure_dir.clone(),
        source: e,
    })?;

    // Create starter model.pure
    let model_path = pure_dir.join("model.pure");
    write_starter_file(&model_path, &args.name, &args.group)?;

    // Create .legendignore
    let ignore_path = project_dir.join(".legendignore");
    write_ignore_file(&ignore_path)?;

    // Print summary
    eprintln!();
    eprintln!("  {} {}/", "📁".dimmed(), args.name);
    eprintln!("  {}   src/main/pure/", "  ".dimmed());
    eprintln!("  {}     {} (starter model)", "  ".dimmed(), "model.pure".cyan());
    eprintln!("  {}   {}", "  ".dimmed(), ".legendignore".dimmed());
    eprintln!();
    eprintln!("{}", "Get started:".bold());
    eprintln!("  cd {}", args.name);
    eprintln!("  legend parse src/main/pure/model.pure");
    eprintln!("  legend check src/main/pure/");

    Ok(())
}

fn write_starter_file(path: &Path, project_name: &str, group: &str) -> Result<(), CliError> {
    let content = format!(
        r"// {project_name} — Legend Pure Model
//
// Get started by defining your data model below.
// Run `legend check .` to validate your Pure files.
// Run `legend parse <file>` to convert to Protocol JSON.

Class {group}::{project_name}::MyFirstClass
{{
  name: String[1];
  description: String[0..1];
  createdDate: DateTime[1];
}}

Enum {group}::{project_name}::Status
{{
  Active,
  Inactive,
  Pending
}}

// Example function
function {group}::{project_name}::greet(name: String[1]): String[1]
{{
  'Hello, ' + $name + '!'
}}
"
    );
    std::fs::write(path, content).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

fn write_ignore_file(path: &Path) -> Result<(), CliError> {
    let content = "# Legend ignore patterns\n# Files and directories matching these patterns are excluded from parsing.\n\n# Build output\ntarget/\n\n# IDE files\n.idea/\n.vscode/\n*.iml\n\n# OS files\n.DS_Store\n";
    std::fs::write(path, content).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}
