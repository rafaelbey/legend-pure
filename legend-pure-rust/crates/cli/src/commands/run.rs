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

//! `legend run <fqn>` — evaluate a parameterless Pure function and
//! print its return value.
//!
//! This is the terminal-side companion to the IntelliJ plugin's
//! `▶ Run` gutter affordance. The plugin spawns this command as a
//! subprocess and pipes its stdout / stderr into the Run tool-window
//! (Phase 2); for now the CLI also drives standalone use from a
//! shell or CI step.
//!
//! Built on top of the same evaluator-bootstrap path as
//! `legend test` (`commands/test.rs`): load the platform model
//! (with optional classpath cascade), construct an [`Evaluator`]
//! with the standard `NativeRegistry`, call `evaluator.call(fqn,
//! &[])` and render the resulting [`Value`].

use std::path::PathBuf;

use clap::Args;
use owo_colors::OwoColorize;

use legend_pure_core_platform::platform::load_platform;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

use crate::diagnostics::CliError;

/// `legend run <fqn>` arguments.
#[derive(Args, Debug)]
pub struct RunArgs {
    /// Fully-qualified name of a parameterless function to invoke.
    /// Accepts either the canonical mangled form
    /// (e.g. `my::pkg::demo__Integer_1_`) or the unmangled
    /// short form (`my::pkg::demo`) — the evaluator's
    /// dispatch resolves both.
    pub function: String,

    /// Live platform mode — recompile from filesystem on every
    /// invocation rather than loading the embedded `.purem`. Useful
    /// when iterating on platform sources locally.
    #[arg(long)]
    pub live: bool,

    /// Path to the platform sources for `--live` mode.
    #[arg(long, value_name = "DIR")]
    pub platform_dir: Option<PathBuf>,
}

/// `legend run` entry point.
///
/// `classpath` is the global `--classpath` flag — threaded through
/// the same way `legend test` consumes it (currently a no-op
/// pass-through; full classpath integration tracked alongside the
/// test command).
#[allow(clippy::needless_pass_by_value)] // clap convention
pub fn run(args: RunArgs, classpath: Option<&std::path::Path>) -> Result<(), CliError> {
    let _ = classpath; // wired through; full integration deferred (parity with `legend test`)

    eprintln!(
        "{} {}",
        "Running".cyan().bold(),
        args.function.as_str().bold(),
    );

    let model = if args.live {
        let descriptor =
            crate::live::resolve_platform_descriptor(args.platform_dir.as_deref())?;
        let repos = crate::live::live_repos(&descriptor)?;
        let auto_imports: Vec<smol_str::SmolStr> =
            legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
                .iter()
                .map(|&s| smol_str::SmolStr::new(s))
                .collect();
        match legend_pure_core_platform::repo::load(&repos, &auto_imports) {
            Ok(m) => m,
            Err(partial) => {
                eprintln!(
                    "  {} platform compiled with {} error(s)",
                    "warning:".yellow().bold(),
                    partial.errors.len()
                );
                partial.model
            }
        }
    } else {
        match load_platform() {
            Ok(m) => m,
            Err(partial) => {
                eprintln!(
                    "  {} platform compiled with {} error(s)",
                    "warning:".yellow().bold(),
                    partial.errors.len()
                );
                partial.model
            }
        }
    };

    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let result = evaluator
        .call(&args.function, &[])
        .map_err(|e| CliError::Custom(format!("{e}")))?;

    println!("{}", render_value(&evaluator, &result));
    Ok(())
}

/// Render a [`Value`] for terminal output. Mirrors the lightweight
/// shape `legend test`'s result rendering uses for individual return
/// values — a one-line summary for primitives + a compact debug for
/// object handles. Full structural pretty-printing is a separate
/// feature (the planned `--format=tree`).
fn render_value(evaluator: &Evaluator<'_>, value: &Value) -> String {
    match value {
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Boolean(b) => b.to_string(),
        Value::Collection(items) => {
            let rendered: Vec<String> = items.iter().map(|v| render_value(evaluator, v)).collect();
            format!("[{}]", rendered.join(", "))
        }
        // Heap-backed values (Object / Function / Element / …) —
        // print the runtime's debug shape. Refining to a Pure-level
        // toString is deferred; the user can pipe to `legend test`
        // for richer rendering paths.
        other => format!("{other:?}"),
    }
}
