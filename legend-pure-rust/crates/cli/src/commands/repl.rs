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

//! `legend repl` — Interactive Pure expression evaluator.
//!
//! Provides a Read-Eval-Print Loop for Pure expressions. The platform
//! sources are parsed once at startup; each user input is wrapped in a
//! temporary function body, compiled alongside the platform sources,
//! and evaluated.
//!
//! `let` bindings persist across inputs — they are accumulated and
//! replayed in each subsequent function body.
//!
//! # Usage
//!
//! ```bash
//! legend repl
//!
//! pure> 1 + 2
//! => 3
//!
//! pure> let x = 5;
//! => 5
//!
//! pure> $x * 3
//! => 15
//!
//! pure> :quit
//! ```

use std::path::PathBuf;
use std::time::Instant;

use owo_colors::OwoColorize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use smol_str::SmolStr;

use legend_pure_core_platform::platform::{parse_and_compile, PLATFORM_AUTO_IMPORTS};
use legend_pure_core_platform::sources;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

use crate::diagnostics::CliError;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

/// Arguments for the `legend repl` command.
#[derive(clap::Args)]
pub struct ReplArgs {
    /// Additional `.pure` source files or directories to load.
    #[arg(short, long)]
    pub source: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// REPL loop
// ---------------------------------------------------------------------------

/// Execute the `legend repl` command.
#[allow(clippy::needless_pass_by_value)]
pub fn run(_args: ReplArgs) -> Result<(), CliError> {
    // Build the auto-import list once.
    let auto_imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();

    // Collect the platform source pairs for re-use in every compile cycle.
    let platform = sources::platform_sources();
    let platform_pairs: Vec<(&str, &str)> = platform
        .iter()
        .map(|s| (s.content, s.path))
        .collect();

    print_banner();

    // State: accumulated let bindings (source lines) and a sequence counter.
    let mut let_bindings: Vec<String> = Vec::new();
    let mut seq: u64 = 0;

    let mut rl = DefaultEditor::new()
        .map_err(|e| CliError::Custom(format!("Failed to initialise readline: {e}")))?;

    // Try to load history (ignore errors — file may not exist yet).
    let history_path = dirs_history_path();
    if let Some(ref p) = history_path {
        let _ = rl.load_history(p);
    }

    loop {
        let prompt = format!("{} ", "pure>".cyan().bold());
        let line = match rl.readline(&prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => return Err(CliError::Custom(format!("Readline error: {e}"))),
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let _ = rl.add_history_entry(trimmed);

        // Special commands.
        match trimmed {
            ":quit" | ":q" => break,
            ":reset" => {
                let_bindings.clear();
                seq = 0;
                eprintln!("{}", "  State reset.".dimmed());
                continue;
            }
            ":help" | ":h" => {
                print_help();
                continue;
            }
            ":lets" => {
                if let_bindings.is_empty() {
                    eprintln!("{}", "  No bindings.".dimmed());
                } else {
                    for b in &let_bindings {
                        eprintln!("  {}", b.dimmed());
                    }
                }
                continue;
            }
            _ => {}
        }

        // Build the function body: accumulated lets + current expression.
        let is_let = trimmed.starts_with("let ");

        let body_source = build_body(&let_bindings, trimmed);
        let fn_name = format!("__repl_{seq}__");
        let wrapper = format!("function {fn_name}():Any[*] {{ {body_source} }}");
        seq += 1;

        // Compile: platform sources + REPL wrapper.
        let t0 = Instant::now();
        let mut all_sources: Vec<(&str, &str)> = platform_pairs.clone();
        all_sources.push((wrapper.as_str(), "<repl>"));

        let model = match parse_and_compile(all_sources.into_iter(), &auto_imports) {
            Ok(m) => m,
            Err(partial) => {
                // Show only REPL-related errors (filter out known platform warnings).
                let repl_errors: Vec<_> = partial
                    .errors
                    .iter()
                    .filter(|e| e.source_info.source == "<repl>")
                    .collect();
                if repl_errors.is_empty() {
                    // Only platform errors — use the partial model.
                    partial.model
                } else {
                    for e in &repl_errors {
                        eprintln!(
                            "  {} {}",
                            "error:".red().bold(),
                            e.message,
                        );
                    }
                    continue;
                }
            }
        };
        let compile_ms = t0.elapsed().as_millis();

        // Evaluate.
        let registry = NativeRegistry::standard();
        let mut evaluator = Evaluator::new(&model, &registry);

        match evaluator.call(&fn_name, &[]) {
            Ok(value) => {
                print_value(&value);
                // If the input was a `let`, persist it.
                if is_let {
                    let_bindings.push(trimmed.to_string());
                }
                eprintln!(
                    "{}",
                    format!("  ({compile_ms}ms)").dimmed(),
                );
            }
            Err(e) => {
                eprintln!(
                    "  {} {}",
                    "error:".red().bold(),
                    e,
                );
            }
        }
    }

    // Save history.
    if let Some(ref p) = history_path {
        if let Some(parent) = std::path::Path::new(p).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = rl.save_history(p);
    }

    eprintln!("{}", "Goodbye!".dimmed());
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the function body from accumulated lets + current expression.
fn build_body(let_bindings: &[String], current: &str) -> String {
    let mut body = String::new();
    for binding in let_bindings {
        body.push_str(binding);
        if !binding.ends_with(';') {
            body.push(';');
        }
        body.push(' ');
    }
    body.push_str(current);
    if !current.ends_with(';') {
        body.push(';');
    }
    body
}

/// Pretty-print a Pure value with colors.
fn print_value(value: &Value) {
    match value {
        Value::Unit => {
            eprintln!("  {}", "=> ()".dimmed());
        }
        Value::String(s) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                format!("'{s}'").green(),
            );
        }
        Value::Integer(n) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                n.to_string().yellow(),
            );
        }
        Value::Float(n) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                n.to_string().yellow(),
            );
        }
        Value::Decimal(d) => {
            eprintln!(
                "  {} {}{}",
                "=>".green().bold(),
                d.to_string().yellow(),
                "D".dimmed(),
            );
        }
        Value::Boolean(b) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                b.to_string().magenta(),
            );
        }
        Value::Date(d) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                format!("%{d}").cyan(),
            );
        }
        Value::StrictTime(t) => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                format!("%{t}").cyan(),
            );
        }
        Value::Collection(items) => {
            if items.is_empty() {
                eprintln!("  {} {}", "=>".green().bold(), "[]".dimmed());
            } else {
                let parts: Vec<String> = items
                    .iter()
                    .take(20)
                    .map(|v| format!("{v}"))
                    .collect();
                let suffix = if items.len() > 20 {
                    format!(", ... ({} more)", items.len() - 20)
                } else {
                    String::new()
                };
                eprintln!(
                    "  {} [{}{}]",
                    "=>".green().bold(),
                    parts.join(", "),
                    suffix,
                );
            }
        }
        Value::EnumValue { member, .. } => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                member.cyan(),
            );
        }
        // Fallback: use Display for Objects, Maps, Functions, Elements, etc.
        other => {
            eprintln!(
                "  {} {}",
                "=>".green().bold(),
                other,
            );
        }
    }
}

/// Banner printed at REPL startup.
fn print_banner() {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!(
        "\n  {} {} {}",
        "Legend Pure REPL".bold(),
        version.green(),
        "(type :help for commands)".dimmed(),
    );
    eprintln!();
}

/// Help text for special REPL commands.
fn print_help() {
    eprintln!();
    eprintln!("  {}", "Commands:".bold());
    eprintln!("    {}    Exit the REPL", ":quit, :q".cyan());
    eprintln!("    {}     Reset all let bindings", ":reset".cyan());
    eprintln!("    {}      Show accumulated let bindings", ":lets".cyan());
    eprintln!("    {}  Show this help", ":help, :h".cyan());
    eprintln!();
    eprintln!("  {}", "Examples:".bold());
    eprintln!("    {}  1 + 2", "pure>".dimmed());
    eprintln!("    {}  let x = 'hello';", "pure>".dimmed());
    eprintln!("    {}  $x + ' world'", "pure>".dimmed());
    eprintln!("    {}  [1, 2, 3]->filter(x | $x > 1)", "pure>".dimmed());
    eprintln!();
}

/// Returns the path for REPL history, if determinable.
fn dirs_history_path() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .map(|home| format!("{home}/.legend/repl_history"))
}
