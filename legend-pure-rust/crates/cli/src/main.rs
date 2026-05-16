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

//! # Legend CLI
//!
//! The primary developer tool for the Legend ecosystem.
//! Parse, compile, test, and publish Pure models from the command line.
//!
//! ```bash
//! legend parse model.pure           # Grammar → Protocol JSON
//! legend check src/                 # Validate .pure files
//! legend init my-project            # Scaffold a new project
//! ```

#![forbid(unsafe_code)]

mod classpath;
mod commands;
mod diagnostics;
mod discovery;
mod live;

// Force-link the runtime-extension crates so their
// `#[distributed_slice]` registrations end up in the final binary.
// Without these, the linker is free to drop the entire crate object
// file (no reachable code → no contribution to RUNTIME_EXTENSIONS /
// DSL_POPULATORS / etc.). The `discovery_smoke` integration test in
// `crates/runtime/tests/` demonstrates the same pattern.
//
// Downstream consumers building their own CLI binaries must follow
// the same recipe — see `docs/extensions/downstream-recipe.md`.
#[allow(unused_imports)]
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_store_relational_runtime::RelationalStoreExtension as _;

use clap::{Parser, Subcommand};

/// The Legend CLI — parse, compile, test, and publish Pure models.
///
/// Legend is to the Pure language what Cargo is to Rust: a unified
/// command-line tool for the entire development workflow.
#[derive(Parser)]
#[command(
    name = "legend",
    version,
    about = "The Legend CLI — parse, compile, test, and publish Pure models",
    long_about = "Legend is the developer CLI for the Legend ecosystem.\n\n\
                  It provides tools to parse Pure grammar, convert between\n\
                  grammar and Protocol JSON, compile models, run tests, and\n\
                  manage dependencies.\n\n\
                  Get started:\n  \
                  legend init my-project    Create a new Legend project\n  \
                  legend parse model.pure   Parse a Pure file to JSON\n  \
                  legend check src/         Validate Pure files",
    after_help = "See 'legend <command> --help' for more information on a specific command."
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Increase logging verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Path to a `legend-pure-classpath.toml` describing the repos to
    /// load. Overrides `LEGEND_PURE_CLASSPATH`, the cwd-ancestor walk,
    /// and next-to-binary auto-discovery.
    #[arg(long, global = true, value_name = "PATH")]
    classpath: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse Pure grammar into Protocol JSON
    Parse(commands::parse::ParseArgs),

    /// Convert Protocol JSON back to Pure grammar
    Emit(commands::emit::EmitArgs),

    /// Compile Pure files (parse + semantic analysis)
    Compile(commands::compile::CompileArgs),

    /// Validate Pure files (parse-only, no semantic analysis)
    Check(commands::check::CheckArgs),

    /// Run the Pure Language Server Protocol server over stdio
    Lsp(commands::lsp::LspArgs),

    /// Run the Pure Debug Adapter Protocol server over stdio
    Dap(commands::dap::DapArgs),

    /// Run the Pure Model Context Protocol server over stdio (for AI agents)
    Mcp(commands::mcp::McpArgs),

    /// Run Pure tests on testable elements
    Test(commands::test::TestArgs),

    /// Run a parameterless Pure function and print its return value
    Run(commands::run::RunArgs),

    /// Merge and report on Pure code coverage
    Coverage(commands::coverage_cmd::CoverageArgs),

    /// Initialize a new Legend project
    Init(commands::init::InitArgs),

    /// Generate Java wrappers around the JNI evaluator
    JavaBindings(commands::java_bindings::JavaBindingsArgs),

    /// Generate shell completion scripts
    Completions(commands::completions::CompletionsArgs),

    /// Interactive Pure expression evaluator
    Repl(commands::repl::ReplArgs),

    /// Compile the embedded platform and write a `.purem` snapshot
    Snapshot(commands::snapshot::SnapshotArgs),

    /// Print version information
    Version,
}

fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let classpath = cli.classpath.as_deref();

    let result = match cli.command {
        Commands::Parse(args) => commands::parse::run(args),
        Commands::Emit(args) => commands::emit::run(args),
        Commands::Compile(args) => commands::compile::run(args),
        Commands::Check(args) => commands::check::run(args),
        Commands::Lsp(args) => commands::lsp::run(args, classpath),
        Commands::Dap(args) => commands::dap::run(args, classpath),
        Commands::Mcp(args) => commands::mcp::run(args, classpath),
        Commands::Test(args) => commands::test::run(args, classpath),
        Commands::Run(args) => commands::run::run(args, classpath),
        Commands::Coverage(args) => commands::coverage_cmd::run(args),
        Commands::Init(args) => commands::init::run(args),
        Commands::JavaBindings(args) => commands::java_bindings::run(args),
        Commands::Completions(args) => commands::completions::run(args),
        Commands::Repl(args) => commands::repl::run(args, classpath),
        Commands::Snapshot(args) => commands::snapshot::run(args, classpath),
        Commands::Version => {
            print_version();
            Ok(())
        }
    };

    if let Err(e) = result {
        diagnostics::print_error(&e);
        std::process::exit(1);
    }
}

/// Prints detailed version information.
fn print_version() {
    use owo_colors::OwoColorize;

    let version = env!("CARGO_PKG_VERSION");
    println!(
        "{} {} {}",
        "legend".bold(),
        version.green(),
        format!(
            "(rust/{} {})",
            env!("CARGO_PKG_NAME"),
            std::env::consts::ARCH
        )
        .dimmed()
    );
    println!();
    println!("  {} Pure grammar parser", "✓".green());
    println!("  {} Protocol JSON ↔ Grammar conversion", "✓".green());
    println!("  {} Compile Pure models", "✓".green());
    println!("  {} Run Pure tests", "✓".green());
    println!(
        "  {} Package artifacts    {}",
        "◌".yellow(),
        "(coming soon)".dimmed()
    );
    println!(
        "  {} Publish to depot     {}",
        "◌".yellow(),
        "(coming soon)".dimmed()
    );
    println!(
        "  {} SDLC integration     {}",
        "◌".yellow(),
        "(coming soon)".dimmed()
    );
}

/// Initializes the tracing subscriber based on verbosity level.
///
/// Default (`verbosity == 0`) is `error` so routine compile-time
/// warnings (e.g. the platform's pre-existing `AmbiguousImport`
/// diagnostics in `fold.pure` / `plus.pure`) don't leak into normal
/// CLI output. Pass `-v` for warnings, `-vv` for info, etc.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;

    let default_level = match verbosity {
        0 => "error",
        1 => "warn",
        2 => "info",
        3 => "debug",
        _ => "trace",
    };

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
