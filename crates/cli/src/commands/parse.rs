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

//! `legend parse` — Parse Pure grammar into Protocol JSON.
//!
//! This is the primary entry point for converting `.pure` source files into
//! the Protocol v1 JSON format used by the Legend Engine.
//!
//! # Examples
//!
//! ```bash
//! # Parse a single file
//! legend parse model/Person.pure
//!
//! # Parse a directory
//! legend parse --dir src/main/pure
//!
//! # Write output to a file instead of stdout
//! legend parse model/Person.pure --output person.json
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;

use crate::diagnostics::{self, CliError};
use crate::discovery;

/// Arguments for the `legend parse` command.
#[derive(clap::Args)]
pub struct ParseArgs {
    /// Input `.pure` file(s) or directory to parse.
    pub paths: Vec<PathBuf>,

    /// Write output to a file instead of stdout.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Compact JSON output (no pretty-printing).
    #[arg(long)]
    compact: bool,

    /// Strip source location information from output.
    #[arg(long)]
    no_source_info: bool,

    /// Show source code snippets for errors with line numbers and carets.
    #[arg(long)]
    show_source: bool,
}

/// Execute the `legend parse` command.
#[allow(clippy::needless_pass_by_value)] // clap convention: Args are passed by value
pub fn run(args: ParseArgs) -> Result<(), CliError> {
    let files = discovery::resolve_paths(&args.paths)?;

    if files.is_empty() {
        return Err(CliError::NoFilesFound);
    }

    eprintln!(
        "{} {} .pure file(s)...",
        "Parsing".green().bold(),
        files.len()
    );

    let mut all_elements = Vec::new();
    let mut errors = Vec::new();

    for path in &files {
        let source = std::fs::read_to_string(path).map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?;

        let file_name = discovery::file_name(path);

        match legend_pure_parser_parser::parse(&source, &file_name) {
            Ok(source_file) => {
                let pmcd =
                    legend_pure_parser_protocol::v1::convert::convert_source_file(&source_file)?;
                all_elements.extend(pmcd.elements);
                eprintln!("  {} {}", "✓".green(), path.display().dimmed());
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
                errors.push((path.clone(), e));
            }
        }
    }

    if !errors.is_empty() {
        eprintln!();
        eprintln!(
            "{} {} file(s) had parse errors",
            "Error:".red().bold(),
            errors.len()
        );
        return Err(CliError::ParseErrors(errors.len()));
    }

    // Build the merged PureModelContextData
    let pmcd = legend_pure_parser_protocol::v1::context::PureModelContextData::new(all_elements);

    // Serialize
    let json = if args.compact {
        serde_json::to_string(&pmcd).map_err(CliError::Serialization)?
    } else {
        serde_json::to_string_pretty(&pmcd).map_err(CliError::Serialization)?
    };

    // Output
    match args.output {
        Some(ref path) => {
            std::fs::write(path, &json).map_err(|e| CliError::Io {
                path: path.clone(),
                source: e,
            })?;
            eprintln!(
                "{} wrote {} bytes to {}",
                "Done.".green().bold(),
                json.len(),
                path.display()
            );
        }
        None => {
            println!("{json}");
        }
    }

    Ok(())
}
