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

//! `legend emit` — Convert Protocol JSON back to Pure grammar.
//!
//! This command reads a Protocol v1 JSON file (e.g., the output of
//! `legend parse`) and emits formatted Pure grammar text.
//!
//! # Usage
//!
//! ```bash
//! legend emit model.json                    # JSON → grammar on stdout
//! legend emit model.json --output model.pure # JSON → grammar file
//! cat model.json | legend emit -             # Read from stdin
//! ```

use std::io::Read;
use std::path::PathBuf;

use owo_colors::OwoColorize;

use crate::diagnostics::CliError;

/// Arguments for the `legend emit` command.
#[derive(clap::Args)]
pub struct EmitArgs {
    /// Input Protocol JSON file (use `-` for stdin).
    pub file: PathBuf,

    /// Write output to a file instead of stdout.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Execute the `legend emit` command.
///
/// Pipeline: Protocol JSON → AST (via `from_protocol`) → Pure grammar text (via `compose`).
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: EmitArgs) -> Result<(), CliError> {
    eprintln!(
        "{} Protocol JSON to Pure grammar...",
        "Converting".green().bold()
    );

    // Read input
    let json = if args.file.to_string_lossy() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::Io {
                path: PathBuf::from("<stdin>"),
                source: e,
            })?;
        buf
    } else {
        std::fs::read_to_string(&args.file).map_err(|e| CliError::Io {
            path: args.file.clone(),
            source: e,
        })?
    };

    // Deserialize Protocol JSON → PureModelContextData
    let pmcd: legend_pure_parser_protocol::v1::context::PureModelContextData =
        serde_json::from_str(&json).map_err(CliError::Serialization)?;

    // Convert Protocol → AST
    let source_file =
        legend_pure_parser_protocol::v1::from_protocol::convert_context_to_source_file(&pmcd)
            .map_err(|e| CliError::Protocol(e.to_string()))?;

    // Compose AST → Pure grammar text
    let grammar = legend_pure_parser_compose::compose_source_file(&source_file);

    // Output
    match args.output {
        Some(ref path) => {
            std::fs::write(path, &grammar).map_err(|e| CliError::Io {
                path: path.clone(),
                source: e,
            })?;
            eprintln!(
                "{} wrote {} bytes to {}",
                "Done.".green().bold(),
                grammar.len(),
                path.display()
            );
        }
        None => {
            print!("{grammar}");
        }
    }

    Ok(())
}
