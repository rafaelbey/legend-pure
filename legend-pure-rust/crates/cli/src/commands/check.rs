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

//! `legend check` — Validate Pure files (parse-only, no semantic analysis).
//!
//! Parses all specified `.pure` files and reports any syntax errors without
//! performing semantic analysis or compilation. This is the fastest way to
//! catch grammar errors.
//!
//! # Examples
//!
//! ```bash
//! # Check a single file
//! legend check model/Person.pure
//!
//! # Check a directory
//! legend check src/main/pure
//!
//! # Show source code snippets on errors
//! legend check --show-source src/main/pure
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;

use crate::diagnostics::{self, CliError};
use crate::discovery;

/// Arguments for the `legend check` command.
#[derive(clap::Args)]
pub struct CheckArgs {
    /// Input `.pure` file(s) or directory to check.
    pub paths: Vec<PathBuf>,

    /// Show source code snippets for errors with line numbers and carets.
    #[arg(long)]
    pub show_source: bool,
}

/// Execute the `legend check` command.
#[allow(clippy::needless_pass_by_value)] // clap convention: Args are passed by value
pub fn run(args: CheckArgs) -> Result<(), CliError> {
    let files = discovery::resolve_paths(&args.paths)?;

    if files.is_empty() {
        return Err(CliError::NoFilesFound);
    }

    eprintln!(
        "{} {} .pure file(s)...",
        "Checking".cyan().bold(),
        files.len()
    );

    let mut error_count = 0;
    let mut ok_count = 0;

    for path in &files {
        let source = std::fs::read_to_string(path).map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?;

        let file_name = discovery::file_name(path);

        match legend_pure_parser_parser::parse(&source, &file_name) {
            Ok(source_file) => {
                let count = source_file.element_count();
                eprintln!(
                    "  {} {} ({} element{})",
                    "✓".green(),
                    path.display().dimmed(),
                    count,
                    if count == 1 { "" } else { "s" }
                );
                ok_count += 1;
            }
            Err(e) => {
                eprintln!(
                    "  {} {} — {}",
                    "✗".red(),
                    path.display().dimmed(),
                    diagnostics::format_error_with_path(path, &e).red()
                );
                if args.show_source {
                    diagnostics::render_source_snippet(&source, path, &e);
                }
                error_count += 1;
            }
        }
    }

    eprintln!();
    if error_count > 0 {
        eprintln!(
            "{} {} passed, {} failed",
            "Result:".bold(),
            ok_count.to_string().green(),
            error_count.to_string().red()
        );
        Err(CliError::ParseErrors(error_count))
    } else {
        eprintln!(
            "{} all {} file(s) are valid ✓",
            "Result:".bold(),
            ok_count.to_string().green()
        );
        Ok(())
    }
}
