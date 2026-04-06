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

mod commands;
mod diagnostics;
mod discovery;

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

    /// Run Pure tests on testable elements
    Test(commands::test::TestArgs),

    /// Generate an execution plan for a function or query
    Plan(commands::plan::PlanArgs),

    /// Parse, compile, and generate distributable artifacts
    Package(commands::package::PackageArgs),

    /// Package and publish artifacts to the depot repository
    Publish(commands::publish::PublishArgs),

    /// Initialize a new Legend project
    Init(commands::init::InitArgs),

    /// Generate shell completion scripts
    Completions(commands::completions::CompletionsArgs),

    /// Print version information
    Version,
}

fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let result = match cli.command {
        Commands::Parse(args) => commands::parse::run(args),
        Commands::Emit(args) => commands::emit::run(args),
        Commands::Compile(args) => commands::compile::run(args),
        Commands::Check(args) => commands::check::run(args),
        Commands::Test(args) => commands::test::run(args),
        Commands::Plan(args) => commands::plan::run(args),
        Commands::Package(args) => commands::package::run(args),
        Commands::Publish(args) => commands::publish::run(args),
        Commands::Init(args) => commands::init::run(args),
        Commands::Completions(args) => commands::completions::run(args),
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
        format!("(rust/{} {})", env!("CARGO_PKG_NAME"), std::env::consts::ARCH).dimmed()
    );
    println!();
    println!("  {} Pure grammar parser", "✓".green());
    println!("  {} Protocol JSON ↔ Grammar conversion", "✓".green());
    println!("  {} Compile Pure models  {}", "◌".yellow(), "(coming soon)".dimmed());
    println!("  {} Package artifacts    {}", "◌".yellow(), "(coming soon)".dimmed());
    println!("  {} Publish to depot     {}", "◌".yellow(), "(coming soon)".dimmed());
    println!("  {} Run Pure tests       {}", "◌".yellow(), "(coming soon)".dimmed());
    println!("  {} SDLC integration     {}", "◌".yellow(), "(coming soon)".dimmed());
}

/// Initializes the tracing subscriber based on verbosity level.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;

    let default_level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
