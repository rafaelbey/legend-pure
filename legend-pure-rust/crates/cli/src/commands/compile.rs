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

use smol_str::SmolStr;

use crate::diagnostics::{self, CliError};
use crate::discovery;

/// The default auto-imported packages, matching the Java M3 definition.
///
/// These packages are implicitly imported in every section, so users can
/// reference their elements (e.g., `String`, `Integer`) without an explicit
/// `import` statement.
const AUTO_IMPORT_PACKAGES: &[&str] = &[
    "meta::pure::metamodel",
    "meta::pure::metamodel::type",
    "meta::pure::metamodel::type::generics",
    "meta::pure::metamodel::relationship",
    "meta::pure::metamodel::valuespecification",
    "meta::pure::metamodel::multiplicity",
    "meta::pure::metamodel::function",
    "meta::pure::metamodel::function::property",
    "meta::pure::metamodel::extension",
    "meta::pure::metamodel::import",
    "meta::pure::functions::date",
    "meta::pure::functions::string",
    "meta::pure::functions::collection",
    "meta::pure::functions::meta",
    "meta::pure::functions::constraints",
    "meta::pure::functions::lang",
    "meta::pure::functions::boolean",
    "meta::pure::functions::tools",
    "meta::pure::functions::io",
    "meta::pure::functions::math",
    "meta::pure::functions::asserts",
    "meta::pure::functions::test",
    "meta::pure::functions::multiplicity",
    "meta::pure::router",
    "meta::pure::service",
    "meta::pure::tds",
    "meta::pure::tools",
    "meta::pure::profiles",
];

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
#[allow(clippy::too_many_lines)]
pub fn run(args: CompileArgs) -> Result<(), CliError> {
    let files = discovery::resolve_paths(&args.paths)?;

    if files.is_empty() {
        return Err(CliError::NoFilesFound);
    }

    let total = files.len();
    eprintln!("{} {} .pure file(s)...", "Compiling".cyan().bold(), total);

    let start = Instant::now();

    // -- Phase 1: Load + parse all files in parallel --
    let inputs: Vec<_> = files
        .iter()
        .map(legend_pure_parser_parser::source::SourceInput::file_system)
        .collect();

    let outputs = legend_pure_parser_parser::parse_many(&inputs);

    // -- Phase 1b: Sequential reporting + collection --
    let mut source_files = Vec::new();
    let mut sources: HashMap<String, (PathBuf, String)> = HashMap::new();
    let mut parse_error_count = 0;

    for (path, output) in files.iter().zip(outputs) {
        match output.outcome {
            legend_pure_parser_parser::ParseOutcome::Success(sf) => {
                let count = sf.element_count();
                eprintln!(
                    "  {} {} ({} element{})",
                    "✓".green(),
                    path.display().dimmed(),
                    count,
                    if count == 1 { "" } else { "s" }
                );
                if let Some(text) = output.source_text {
                    sources.insert(output.name, (path.clone(), text));
                }
                source_files.push(sf);
            }
            legend_pure_parser_parser::ParseOutcome::Partial(partial) => {
                let count = partial.source_file.element_count();
                eprintln!(
                    "  {} {} ({} element{} recovered, {} error{})",
                    "⚠".yellow(),
                    path.display().dimmed(),
                    count,
                    if count == 1 { "" } else { "s" },
                    partial.errors.len(),
                    if partial.errors.len() == 1 { "" } else { "s" }
                );
                for e in &partial.errors {
                    eprintln!(
                        "      {} {}",
                        "✗".red(),
                        diagnostics::format_error_with_path(path, e).red()
                    );
                    if args.show_source
                        && let Some(ref text) = output.source_text
                    {
                        diagnostics::render_source_snippet(text, path, e);
                    }
                }
                if let Some(text) = output.source_text {
                    sources.insert(output.name, (path.clone(), text));
                }
                parse_error_count += partial.errors.len();
                // Include valid elements from the partial parse
                source_files.push(partial.source_file);
            }
            legend_pure_parser_parser::ParseOutcome::IoError(e) => {
                return Err(CliError::Io {
                    path: path.clone(),
                    source: e,
                });
            }
        }
    }

    if parse_error_count > 0 {
        eprintln!(
            "\n  {} {parse_error_count} parse error(s) — proceeding with recovered elements",
            "⚠".yellow()
        );
    }

    // -- Phase 2: Compile --
    eprintln!();
    eprintln!("{} semantic analysis...", "Running".cyan().bold());

    let auto_imports: Vec<SmolStr> = AUTO_IMPORT_PACKAGES
        .iter()
        .copied()
        .map(SmolStr::new)
        .collect();

    match legend_pure_parser_pure::pipeline::compile(&source_files, &auto_imports) {
        Ok(model) => {
            let elapsed = start.elapsed();
            print_success_stats(&model, total, elapsed);
            Ok(())
        }
        Err(partial) => {
            let error_count = partial.errors.len();

            eprintln!();
            for error in &partial.errors {
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
                    eprintln!("  {} {}", "✗".red(), error.message.red());
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
    let mut unit_count = 0usize;

    for chunk in model.chunks.iter().skip(1) {
        for (_, element) in chunk.elements.iter() {
            match element {
                Element::Class(_) => class_count += 1,
                Element::Enumeration(_) => enum_count += 1,
                Element::Function(_) => func_count += 1,
                Element::Association(_) => assoc_count += 1,
                Element::Profile(_) => profile_count += 1,
                Element::Measure(_) => measure_count += 1,
                Element::Unit(_) => unit_count += 1,
                Element::PrimitiveType(_)
                | Element::PackageableMultiplicity(_)
                | Element::Package(_) => {}
            }
        }
    }

    let total_elements = class_count
        + enum_count
        + func_count
        + assoc_count
        + profile_count
        + measure_count
        + unit_count;

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
    if class_count > 0 {
        breakdown.push(format!("{class_count} class(es)"));
    }
    if enum_count > 0 {
        breakdown.push(format!("{enum_count} enum(s)"));
    }
    if func_count > 0 {
        breakdown.push(format!("{func_count} function(s)"));
    }
    if assoc_count > 0 {
        breakdown.push(format!("{assoc_count} association(s)"));
    }
    if profile_count > 0 {
        breakdown.push(format!("{profile_count} profile(s)"));
    }
    if measure_count > 0 {
        breakdown.push(format!("{measure_count} measure(s)"));
    }
    if unit_count > 0 {
        breakdown.push(format!("{unit_count} unit(s)"));
    }

    if !breakdown.is_empty() {
        eprintln!("         {}", breakdown.join(", ").dimmed());
    }
}
