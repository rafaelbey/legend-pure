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

//! `legend coverage` — Merge and report on Pure code coverage.
//!
//! Designed for a map/reduce workflow:
//!
//! 1. **Map**: Run N test jobs in parallel, each producing its own `.lcov`:
//!    ```bash
//!    legend test --coverage --coverage-output shard-1.lcov --package ...
//!    legend test --coverage --coverage-output shard-2.lcov --package ...
//!    ```
//!
//! 2. **Reduce**: Merge all shards and generate the combined report:
//!    ```bash
//!    legend coverage merge shard-*.lcov -o merged.lcov
//!    legend coverage report merged.lcov --html coverage-html/
//!    ```

use std::path::PathBuf;

use owo_colors::OwoColorize;

use crate::diagnostics::CliError;

/// Arguments for the `legend coverage` command.
#[derive(clap::Args)]
pub struct CoverageArgs {
    #[command(subcommand)]
    pub action: CoverageAction,
}

/// Coverage subcommands.
#[derive(clap::Subcommand)]
pub enum CoverageAction {
    /// Merge multiple LCOV tracefiles into one.
    ///
    /// Reads all input tracefiles and concatenates them. Tools like
    /// `genhtml` automatically sum hit counts for duplicate entries,
    /// producing correct merged coverage data.
    Merge(MergeArgs),

    /// Generate an HTML coverage report from an LCOV tracefile.
    Report(ReportArgs),
}

/// Arguments for `legend coverage merge`.
#[derive(clap::Args)]
pub struct MergeArgs {
    /// Input LCOV tracefiles to merge.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Output path for the merged tracefile.
    #[arg(short, long, default_value = "coverage.lcov")]
    pub output: PathBuf,
}

/// Arguments for `legend coverage report`.
#[derive(clap::Args)]
pub struct ReportArgs {
    /// Input LCOV tracefile.
    pub input: PathBuf,

    /// Output directory for the HTML report.
    #[arg(long, default_value = "coverage-html")]
    pub html: PathBuf,
}

/// Execute the `legend coverage` command.
#[allow(clippy::needless_pass_by_value)]
pub fn run(args: CoverageArgs) -> Result<(), CliError> {
    match args.action {
        CoverageAction::Merge(merge_args) => run_merge(merge_args),
        CoverageAction::Report(report_args) => run_report(report_args),
    }
}

/// Merge N LCOV tracefiles into one.
fn run_merge(args: MergeArgs) -> Result<(), CliError> {
    eprintln!(
        "{} {} tracefile(s) into {}",
        "Merging".cyan().bold(),
        args.inputs.len(),
        args.output.display(),
    );

    super::coverage_report::merge_lcov_files(&args.inputs, &args.output)
        .map_err(|e| CliError::Custom(format!("Failed to merge LCOV: {e}")))?;

    eprintln!(
        "  {} merged tracefile written to {}",
        "✓".green().bold(),
        args.output.display(),
    );
    Ok(())
}

/// Generate an HTML report from an LCOV tracefile.
fn run_report(args: ReportArgs) -> Result<(), CliError> {
    eprintln!(
        "{} HTML report from {}",
        "Generating".cyan().bold(),
        args.input.display(),
    );

    super::coverage_report::generate_html(&args.input, &args.html)
        .map_err(|e| CliError::Custom(e))?;

    eprintln!(
        "  {} HTML report generated in {}",
        "✓".green().bold(),
        args.html.display(),
    );
    Ok(())
}
