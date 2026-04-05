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

//! Diagnostic rendering for CLI error output.
//!
//! Provides user-friendly, colored error messages for all CLI failure modes.

use std::path::PathBuf;

use owo_colors::OwoColorize;

/// Errors that can occur during CLI execution.
#[derive(Debug)]
pub enum CliError {
    /// A file was not found at the specified path.
    FileNotFound(PathBuf),

    /// No `.pure` files were found in the specified location.
    NoFilesFound,

    /// An I/O error occurred while reading/writing a file.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// One or more files had parse errors.
    ParseErrors(usize),

    /// JSON serialization error.
    Serialization(serde_json::Error),

    /// A command is not yet implemented.
    NotImplemented {
        command: &'static str,
        description: &'static str,
        reason: &'static str,
    },

    /// A custom error message.
    Custom(String),

    /// Protocol conversion error (e.g., from JSON → AST).
    Protocol(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound(path) => {
                write!(f, "file not found: {}", path.display())
            }
            Self::NoFilesFound => {
                write!(f, "no .pure files found")
            }
            Self::Io { path, source } => {
                write!(f, "I/O error on '{}': {}", path.display(), source)
            }
            Self::ParseErrors(count) => {
                write!(f, "{count} file(s) had parse errors")
            }
            Self::Serialization(e) => {
                write!(f, "JSON serialization error: {e}")
            }
            Self::NotImplemented {
                command,
                description,
                reason,
            } => {
                write!(
                    f,
                    "command '{command}' is not yet implemented: {description}. {reason}"
                )
            }
            Self::Custom(msg) => write!(f, "{msg}"),
            Self::Protocol(msg) => write!(f, "protocol conversion error: {msg}"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialization(e) => Some(e),
            _ => None,
        }
    }
}

/// Print a [`CliError`] to stderr with colored formatting.
pub fn print_error(error: &CliError) {
    match error {
        CliError::NotImplemented {
            command,
            description,
            reason,
        } => {
            eprintln!();
            eprintln!(
                "  {} `legend {}` — {}",
                "🚧".dimmed(),
                command.yellow().bold(),
                description
            );
            eprintln!();
            eprintln!("  {}", reason.dimmed());
            eprintln!();
            eprintln!(
                "  This feature is under development. Track progress at:"
            );
            eprintln!(
                "  {}",
                "https://github.com/finos/legend-engine".cyan().underline()
            );
            eprintln!();
        }
        CliError::ParseErrors(_) => {
            // Parse errors are already printed inline during processing.
            // Just print the summary here.
            eprintln!(
                "\n{} {}",
                "error:".red().bold(),
                error
            );
        }
        _ => {
            eprintln!(
                "{} {}",
                "error:".red().bold(),
                error
            );
        }
    }
}

/// Renders a source code snippet with line numbers and underline carets
/// pointing to the error location.
///
/// Produces output like:
/// ```text
///     |
///   6 |   name: String[1]
///   7 |   age: Integer[1]
///     |   ^^^ Expected ';' here
///   8 |   active: Boolean[1]
///     |
/// ```
pub fn render_source_snippet(
    source: &str,
    error: &legend_pure_parser_parser::ParseError,
) {
    let Some(si) = error.source_info() else {
        return;
    };

    let lines: Vec<&str> = source.lines().collect();
    let error_line = si.start_line as usize;
    let error_col_start = si.start_column as usize;
    let error_col_end = si.end_column as usize;

    if error_line == 0 || error_line > lines.len() + 1 {
        return;
    }

    // Determine context window: 1 line before and 1 line after the error
    let ctx_start = error_line.saturating_sub(1).max(1);
    let ctx_end = (error_line + 1).min(lines.len());

    // Width of the largest line number for alignment
    let gutter_width = format!("{ctx_end}").len();
    let gutter_pad = " ".repeat(gutter_width);

    // Blank gutter line
    eprintln!("     {gutter_pad}{}", "|".cyan());

    for line_num in ctx_start..=ctx_end {
        let line_content = lines.get(line_num - 1).unwrap_or(&"");
        let num_str = format!("{line_num:>gutter_width$}");

        if line_num == error_line {
            // The error line — highlight it
            eprintln!(
                "     {} {} {}",
                num_str.cyan().bold(),
                "|".cyan(),
                line_content
            );

            // Underline carets
            let caret_start = if error_col_start > 0 {
                error_col_start - 1
            } else {
                0
            };
            let caret_len = if error_col_end >= error_col_start {
                (error_col_end - error_col_start + 1).max(1)
            } else {
                1
            };

            let padding = " ".repeat(caret_start);
            let carets = "^".repeat(caret_len);

            eprintln!(
                "     {} {} {}{}",
                gutter_pad,
                "|".cyan(),
                padding,
                format!("{carets} {}", error.message()).red().bold()
            );
        } else {
            // Context line
            eprintln!(
                "     {} {} {}",
                num_str.cyan(),
                "|".cyan(),
                line_content.dimmed()
            );
        }
    }

    // Blank gutter line
    eprintln!("     {gutter_pad}{}", "|".cyan());
}

