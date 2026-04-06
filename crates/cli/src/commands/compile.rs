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
//! Parses `.pure` files into an AST, then runs the Pure semantic layer to build
//! a fully resolved `PureModel`. Reports both parse errors and compilation errors
//! (unresolved types, duplicate elements, cyclic inheritance, etc.).
//!
//! # Examples
//!
//! ```bash
//! legend compile src/main/pure         # Compile a directory
//! legend compile model/Person.pure     # Compile specific files
//! legend compile --show-source .       # Show source snippets on errors
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use owo_colors::OwoColorize;

use crate::diagnostics::{self, CliError};
use crate::discovery;

/// Arguments for the `legend compile` command.
#[derive(clap::Args)]
pub struct CompileArgs {
    /// Input `.pure` file(s) or directory to compile.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Show source code snippets for errors with line numbers and carets.
    #[arg(long)]
    pub show_source: bool,

    /// Fail on warnings (strict mode).
    #[arg(long)]
    pub strict: bool,
}

/// Execute the `legend compile` command.
///
/// 1. Discover and parse all `.pure` files
/// 2. Compile ALL parsed source files together into a `PureModel`
/// 3. Report any parse errors or compilation errors with source snippets
#[allow(clippy::needless_pass_by_value)] // clap convention: Args are passed by value
pub fn run(args: CompileArgs) -> Result<(), CliError> {
    let files = discovery::resolve_paths(&args.paths)?;

    if files.is_empty() {
        return Err(CliError::NoFilesFound);
    }

    let total = files.len();
    eprintln!(
        "{} {} .pure file(s)...",
        "Compiling".cyan().bold(),
        total
    );

    let start = Instant::now();

    // -- Phase 1: Parse all files --
    let mut source_files = Vec::new();
    let mut sources: HashMap<String, (PathBuf, String)> = HashMap::new();
    let mut parse_error_count = 0;

    for path in &files {
        let source = std::fs::read_to_string(path).map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?;

        let file_name = discovery::file_name(path);

        match legend_pure_parser_parser::parse(&source, &file_name) {
            Ok(sf) => {
                let count = sf.element_count();
                eprintln!(
                    "  {} {} ({} element{})",
                    "✓".green(),
                    path.display().dimmed(),
                    count,
                    if count == 1 { "" } else { "s" }
                );
                sources.insert(file_name, (path.clone(), source));
                source_files.push(sf);
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
                parse_error_count += 1;
            }
        }
    }

    if parse_error_count > 0 {
        return Err(CliError::ParseErrors(parse_error_count));
    }

    // -- Phase 2: Compile --
    eprintln!();
    eprintln!(
        "{} semantic analysis...",
        "Running".cyan().bold()
    );

    match legend_pure_parser_pure::pipeline::compile(&source_files) {
        Ok(model) => {
            let elapsed = start.elapsed();
            print_success_stats(&model, total, elapsed);
            Ok(())
        }
        Err(errors) => {
            let error_count = errors.len();

            eprintln!();
            for error in &errors {
                let source_name = &error.source_info.source;

                // Find the original source text and path for this error
                if let Some((path, source)) = sources.get(source_name.as_str()) {
                    eprintln!(
                        "  {} {}",
                        "✗".red(),
                        diagnostics::format_compilation_error_with_path(path, error).red()
                    );
                    if args.show_source {
                        diagnostics::render_compilation_snippet(source, path, error);
                    }
                } else {
                    // Fallback: error from a source we don't have (e.g., bootstrap)
                    eprintln!(
                        "  {} {}",
                        "✗".red(),
                        error.message.red()
                    );
                }
            }

            Err(CliError::CompilationErrors(error_count))
        }
    }
}

/// Prints success stats after a successful compilation.
fn print_success_stats(
    model: &legend_pure_parser_pure::model::PureModel,
    file_count: usize,
    elapsed: std::time::Duration,
) {
    use legend_pure_parser_pure::model::Element;

    // Count elements by type (skip chunk 0 = bootstrap)
    let mut class_count = 0usize;
    let mut enum_count = 0usize;
    let mut func_count = 0usize;
    let mut assoc_count = 0usize;
    let mut profile_count = 0usize;
    let mut measure_count = 0usize;

    for chunk in model.chunks.iter().skip(1) {
        for (_, element) in chunk.elements.iter() {
            match element {
                Element::Class(_) => class_count += 1,
                Element::Enumeration(_) => enum_count += 1,
                Element::Function(_) => func_count += 1,
                Element::Association(_) => assoc_count += 1,
                Element::Profile(_) => profile_count += 1,
                Element::Measure(_) => measure_count += 1,
                _ => {}
            }
        }
    }

    let total_elements = class_count + enum_count + func_count
        + assoc_count + profile_count + measure_count;

    eprintln!();
    eprintln!(
        "{} compiled {} element(s) from {} file(s) in {:.2}s",
        "Success:".green().bold(),
        total_elements,
        file_count,
        elapsed.as_secs_f64()
    );

    // Breakdown by type
    let mut breakdown = Vec::new();
    if class_count > 0 { breakdown.push(format!("{class_count} class(es)")); }
    if enum_count > 0 { breakdown.push(format!("{enum_count} enum(s)")); }
    if func_count > 0 { breakdown.push(format!("{func_count} function(s)")); }
    if assoc_count > 0 { breakdown.push(format!("{assoc_count} association(s)")); }
    if profile_count > 0 { breakdown.push(format!("{profile_count} profile(s)")); }
    if measure_count > 0 { breakdown.push(format!("{measure_count} measure(s)")); }

    if !breakdown.is_empty() {
        eprintln!(
            "         {}",
            breakdown.join(", ").dimmed()
        );
    }
}
