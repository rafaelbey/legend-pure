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

//! `legend test` — Run Pure tests via the platform surveyor.
//!
//! Discovers `<<test.Test>>`-stereotyped functions under a package, executes
//! each in a sandbox, and renders the resulting `TestReport` heap object
//! with per-test status, summary counters, and (optionally) detailed
//! pass/skip lines.
//!
//! # Usage
//!
//! ```bash
//! legend test                              # Run everything from Root
//! legend test --package meta::pure::functions::collection::tests
//! legend test --filter testCollect         # Substring filter on test FQN
//! legend test --show-detail                # Also print PASS/SKIP lines
//! ```

use std::path::PathBuf;

use owo_colors::OwoColorize;

use legend_pure_core_platform::platform::load_platform;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::heap::{ObjectId, RuntimeHeap};
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

use crate::diagnostics::CliError;

/// Arguments for the `legend test` command.
#[derive(clap::Args)]
pub struct TestArgs {
    /// Input `.pure` file(s) or directory containing tests.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Scope test execution to a specific package.
    #[arg(long, default_value = "Root")]
    pub package: String,

    /// Filter tests by source path prefix to scope execution to a particular repo.
    #[arg(long, short)]
    pub filter: Option<String>,

    /// Print every PASS / SKIP line, not just FAIL / ERROR.
    #[arg(long = "show-detail")]
    pub show_detail: bool,
}

/// Execute the `legend test` command.
#[allow(clippy::needless_pass_by_value)] // clap convention
pub fn run(args: TestArgs) -> Result<(), CliError> {
    eprintln!(
        "{} {}{}",
        "Running tests in".cyan().bold(),
        args.package,
        args.filter
            .as_deref()
            .map(|f| format!(" (filter: {f})"))
            .unwrap_or_default(),
    );

    // 1. Load platform model — accept partial models so a partially-broken
    //    platform doesn't block test discovery.
    let model = match load_platform() {
        Ok(m) => m,
        Err(partial) => {
            eprintln!(
                "  {} platform compiled with {} error(s)",
                "warning:".yellow().bold(),
                partial.errors.len()
            );
            partial.model
        }
    };

    // 2. Drive the Pure surveyor.
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let result = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String(args.package.clone().into()),
                Value::String(args.filter.clone().unwrap_or_default().into()),
            ],
        )
        .map_err(|e| CliError::Custom(format!("Test execution failed: {e}")))?;

    // 3. Render the TestReport.
    let Value::Object(report_id) = result else {
        return Err(CliError::Custom(format!(
            "Test surveyor returned non-Object: {result:?}"
        )));
    };
    let report = TestReport::read(evaluator.heap(), report_id)?;
    report.render(args.show_detail);

    if report.fail_count + report.error_count > 0 {
        Err(CliError::Custom(format!(
            "{} test(s) failed, {} test(s) errored",
            report.fail_count, report.error_count
        )))
    } else {
        Ok(())
    }
}

/// Decoded view of the platform `TestReport` heap object —
/// `meta::pure::test::surveyor::TestReport` from `surveyor.pure:35`.
struct TestReport {
    pass_count: i64,
    fail_count: i64,
    error_count: i64,
    skip_count: i64,
    total_elapsed_ms: i64,
    results: Vec<TestResult>,
}

/// Decoded view of a single platform `TestResult` —
/// `meta::pure::test::surveyor::TestResult` from `surveyor.pure:19`.
struct TestResult {
    fqn: String,
    status: TestStatus,
    elapsed_ms: i64,
    message: Option<String>,
}

/// Mirrors `meta::pure::test::surveyor::TestStatus`.
enum TestStatus {
    Pass,
    Fail,
    Error,
    Skip,
    Other(String),
}

impl TestReport {
    fn read(heap: &RuntimeHeap, id: ObjectId) -> Result<Self, CliError> {
        let pass_count = read_int_slot(heap, id, "passCount")?;
        let fail_count = read_int_slot(heap, id, "failCount")?;
        let error_count = read_int_slot(heap, id, "errorCount")?;
        let skip_count = read_int_slot(heap, id, "skipCount")?;
        let total_elapsed_ms = read_int_slot(heap, id, "totalElapsed").unwrap_or(0);

        let raw_results = heap
            .get_property_values(id, "results")
            .map_err(|e| CliError::Custom(format!("TestReport.results read failed: {e}")))?;
        let mut results = Vec::with_capacity(raw_results.len());
        for v in raw_results.iter() {
            if let Value::Object(rid) = v {
                results.push(TestResult::read(heap, *rid)?);
            }
        }
        Ok(Self {
            pass_count,
            fail_count,
            error_count,
            skip_count,
            total_elapsed_ms,
            results,
        })
    }

    fn render(&self, show_detail: bool) {
        // Per-test lines — always show FAIL / ERROR; show PASS / SKIP only
        // when `--show-detail` is set.
        for r in &self.results {
            match (&r.status, show_detail) {
                (TestStatus::Pass, true) | (TestStatus::Skip, true) => r.render_line(),
                (TestStatus::Fail | TestStatus::Error | TestStatus::Other(_), _) => r.render_line(),
                _ => {}
            }
        }

        eprintln!();
        eprintln!("{}", "Test Suite completed".bold());
        eprintln!("  Tests   : {}", self.results.len());
        eprintln!("  Passed  : {}", self.pass_count.to_string().green());
        let fail_str = self.fail_count.to_string();
        eprintln!(
            "  Failed  : {}",
            if self.fail_count > 0 {
                fail_str.red().to_string()
            } else {
                fail_str
            }
        );
        let err_str = self.error_count.to_string();
        eprintln!(
            "  Errors  : {}",
            if self.error_count > 0 {
                err_str.red().to_string()
            } else {
                err_str
            }
        );
        eprintln!("  Skipped : {}", self.skip_count.to_string().yellow());
        eprintln!("  Elapsed : {}ms", self.total_elapsed_ms);
    }
}

impl TestResult {
    fn read(heap: &RuntimeHeap, id: ObjectId) -> Result<Self, CliError> {
        let fqn = read_string_slot(heap, id, "fqn").unwrap_or_else(|_| "<unknown>".into());
        let status = match heap.get_property_values(id, "status") {
            Ok(values) => match values.iter().next() {
                Some(Value::EnumValue { member, .. }) => match member.as_str() {
                    "PASS" => TestStatus::Pass,
                    "FAIL" => TestStatus::Fail,
                    "ERROR" => TestStatus::Error,
                    "SKIP" => TestStatus::Skip,
                    other => TestStatus::Other(other.to_string()),
                },
                _ => TestStatus::Other("?".into()),
            },
            Err(_) => TestStatus::Other("?".into()),
        };
        let elapsed_ms = read_int_slot(heap, id, "elapsed").unwrap_or(0);
        let message = read_string_slot(heap, id, "message").ok();
        Ok(Self {
            fqn,
            status,
            elapsed_ms,
            message,
        })
    }

    fn render_line(&self) {
        let mark = match self.status {
            TestStatus::Pass => "✓".green().to_string(),
            TestStatus::Fail => "✗".red().to_string(),
            TestStatus::Error => "!".red().to_string(),
            TestStatus::Skip => "○".yellow().to_string(),
            TestStatus::Other(_) => "?".dimmed().to_string(),
        };
        eprintln!(
            "  {mark} {fqn}  {timing}",
            fqn = self.fqn,
            timing = format!("({}ms)", self.elapsed_ms).dimmed(),
        );
        if matches!(self.status, TestStatus::Fail | TestStatus::Error)
            && let Some(msg) = &self.message
        {
            // First non-empty line of the message — full stack traces go to
            // a future `--full-output` flag.
            if let Some(first) = msg.lines().map(str::trim_start).find(|l| !l.is_empty()) {
                eprintln!("    {} {}", "└─".dimmed(), first.dimmed());
            }
        }
    }
}

fn read_int_slot(heap: &RuntimeHeap, id: ObjectId, slot: &str) -> Result<i64, CliError> {
    let values = heap
        .get_property_values(id, slot)
        .map_err(|e| CliError::Custom(format!("read {slot}: {e}")))?;
    match values.iter().next() {
        Some(Value::Integer(n)) => Ok(*n),
        Some(other) => Err(CliError::Custom(format!(
            "{slot}: expected Integer, got {other:?}"
        ))),
        None => Err(CliError::Custom(format!("{slot}: empty"))),
    }
}

fn read_string_slot(heap: &RuntimeHeap, id: ObjectId, slot: &str) -> Result<String, CliError> {
    let values = heap
        .get_property_values(id, slot)
        .map_err(|e| CliError::Custom(format!("read {slot}: {e}")))?;
    match values.iter().next() {
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(other) => Err(CliError::Custom(format!(
            "{slot}: expected String, got {other:?}"
        ))),
        None => Err(CliError::Custom(format!("{slot}: empty"))),
    }
}
