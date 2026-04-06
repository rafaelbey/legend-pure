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
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A file was not found at the specified path.
    #[error("file not found: {}", .0.display())]
    FileNotFound(PathBuf),

    /// No `.pure` files were found in the specified location.
    #[error("no .pure files found")]
    NoFilesFound,

    /// An I/O error occurred while reading/writing a file.
    #[error("I/O error on '{}': {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// One or more files had parse errors.
    #[error("{0} file(s) had parse errors")]
    ParseErrors(usize),

    /// JSON serialization error.
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A command is not yet implemented.
    #[error("command '{command}' is not yet implemented: {description}. {reason}")]
    NotImplemented {
        command: &'static str,
        description: &'static str,
        reason: &'static str,
    },

    /// A custom error message.
    #[error("{0}")]
    Custom(String),

    /// Protocol conversion error (e.g., from JSON → AST).
    #[error("protocol conversion error: {0}")]
    Protocol(String),

    /// One or more files had compilation errors.
    #[error("{0} compilation error(s)")]
    CompilationErrors(usize),
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
        CliError::ParseErrors(_) | CliError::CompilationErrors(_) => {
            // Errors are already printed inline during processing.
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

/// Resolves a path to its canonical (absolute) form, falling back to
/// the original path if canonicalization fails (e.g., the file was deleted).
fn canonical_or_original(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Formats a parse error message with a clickable file location.
///
/// Produces output like:
/// ```text
/// Expected ';', found identifier at /absolute/path/to/file.pure:7:3
/// ```
///
/// IDE terminals (VS Code, `IntelliJ`, iTerm2) auto-detect `path:line:col`
/// as a clickable hyperlink that opens the file at the error location.
pub fn format_error_with_path(
    path: &std::path::Path,
    error: &legend_pure_parser_parser::ParseError,
) -> String {
    let abs_path = canonical_or_original(path);

    if let Some(si) = error.source_info() {
        format!(
            "{} at {}:{}:{}",
            error.message(),
            abs_path.display(),
            si.start_line,
            si.start_column
        )
    } else {
        format!("{} at {}", error.message(), abs_path.display())
    }
}

/// Renders a source code snippet with line numbers and underline carets
/// pointing to the error location.
///
/// Produces output like:
/// ```text
///    --> /absolute/path/to/file.pure:7:3
///     |
///   6 |   name: String[1]
///   7 |   age: Integer[1]
///     |   ^^^ Expected ';' here
///   8 |   active: Boolean[1]
///     |
/// ```
///
/// The `-->` header uses the absolute file path so IDE terminals can
/// open the file at the error location with a click.
pub fn render_source_snippet(
    source: &str,
    path: &std::path::Path,
    error: &legend_pure_parser_parser::ParseError,
) {
    let Some(si) = error.source_info() else {
        return;
    };
    let msg = error.message();
    render_snippet_inner(
        source,
        path,
        si.start_line as usize,
        si.start_column as usize,
        si.end_column as usize,
        &msg,
    );
}

/// Renders a compilation error with a source code snippet.
///
/// Produces output similar to `render_source_snippet` but works with
/// `CompilationError` from the Pure semantic layer.
///
/// ```text
///    --> /absolute/path/to/file.pure:5:12
///     |
///   4 |   Class Person {
///   5 |     boss: NonExistent[1];
///     |           ^^^^^^^^^^^ Cannot resolve type 'NonExistent'
///   6 |   }
///     |
/// ```
pub fn render_compilation_snippet(
    source: &str,
    path: &std::path::Path,
    error: &legend_pure_parser_pure::error::CompilationError,
) {
    let si = &error.source_info;

    if si.start_line == 0 {
        // Can't render snippet — just print the message
        eprintln!(
            "      {} {}",
            "error:".red().bold(),
            error.message
        );
        return;
    }

    render_snippet_inner(
        source,
        path,
        si.start_line as usize,
        si.start_column as usize,
        si.end_column as usize,
        &error.message,
    );
}

/// Shared implementation for rendering a source snippet with carets.
fn render_snippet_inner(
    source: &str,
    path: &std::path::Path,
    error_line: usize,
    error_col_start: usize,
    error_col_end: usize,
    message: &str,
) {
    let lines: Vec<&str> = source.lines().collect();

    if error_line == 0 || error_line > lines.len() + 1 {
        return;
    }

    // Resolve to absolute path for IDE linking
    let abs_path = canonical_or_original(path);

    // Clickable location header
    eprintln!(
        "     {} {}:{}:{}",
        "-->".cyan().bold(),
        abs_path.display(),
        error_line,
        error_col_start
    );

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
                format!("{carets} {message}").red().bold()
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

/// Formats a compilation error as a clickable path string.
pub fn format_compilation_error_with_path(
    path: &std::path::Path,
    error: &legend_pure_parser_pure::error::CompilationError,
) -> String {
    let abs_path = canonical_or_original(path);

    format!(
        "{} at {}:{}:{}",
        error.message,
        abs_path.display(),
        error.source_info.start_line,
        error.source_info.start_column
    )
}
