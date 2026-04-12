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

//! `legend test` — Run Pure tests on testable elements.
//!
//! **Status: Not yet implemented.**
//!
//! This command will discover testable elements (functions with `tests {}` blocks),
//! compile them, execute the test cases, and report results.
//!
//! # Future Usage
//!
//! ```bash
//! legend test                          # Run all tests
//! legend test --filter "myFunction"    # Run matching tests
//! legend test src/test/pure/           # Run tests in a directory
//! ```

use std::path::PathBuf;

use legend_pure_core_platform::platform::load_platform;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

use crate::diagnostics::CliError;

/// Arguments for the `legend test` command.
#[derive(clap::Args)]
pub struct TestArgs {
    /// Input `.pure` file(s) or directory containing tests.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Scope test execution to a specific package
    #[arg(long)]
    pub package: Option<String>,

    /// Filter tests by source path prefix (e.g. a repo root) to scope execution to a particular repo.
    #[arg(long, short)]
    pub filter: Option<String>,

    /// Show verbose test output.
    #[arg(long)]
    pub verbose: bool,
}

pub fn run(args: TestArgs) -> Result<(), CliError> {
    // 1. Load platform model
    let platform = load_platform();
    // For now, ignore pure compilation errors if any, but in production we'd report them.
    if !platform.compilation_errors.is_empty() {
        println!(
            "Warning: Loaded platform with {} compilation errors",
            platform.compilation_errors.len()
        );
    }

    // 2. Call the Pure test surveyor:
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&platform.model, &registry);

    // Evaluate via surveyor
    let result = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String(args.package.unwrap_or("Root".into()).into()),
                Value::String(args.filter.unwrap_or_default().into()),
            ],
        )
        .map_err(|e| CliError::Custom(format!("Test execution failed: {e:?}")))?;

    // 3. Pretty-print the TestReport
    render_test_report(&result, args.verbose);

    Ok(())
}

fn render_test_report(report_val: &Value, _verbose: bool) {
    // Report rendering placeholder
    println!("Test Report: {report_val:?}");
}
