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
//! With `--pct`, switches to the PCT (Pure Compatibility Tests) surveyor
//! which discovers `<<PCT.test>>`-stereotyped functions and runs them
//! against the in-memory adapter. Exclusions for the Rust port are loaded
//! by default from the bundled
//! [`pct_grammar_rust_native.json`](legend_pure_runtime::pct) manifest —
//! tests pinned by representational limits (i16 year, i64 Integer
//! arithmetic) report PASS, and stale entries flip back to FAIL with
//! `"PCT exclusion needs rebase"`. Pass `--no-default-exclusions` to
//! disable, or `--manifest <path>` to load a different manifest.
//!
//! # Usage
//!
//! ```bash
//! legend test                              # Run everything from Root
//! legend test --package meta::pure::functions::collection::tests
//! legend test --filter testCollect         # Substring filter on test FQN
//! legend test --show-detail                # Also print PASS/SKIP lines
//! legend test --pct --package meta::pure::functions::boolean
//! legend test --pct --manifest pct_grammar_native.json
//! legend test --pct --no-default-exclusions   # Run PCT raw, no exclusions
//! ```

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use owo_colors::OwoColorize;

use super::coverage::{CoverageHooks, CoverageMap};
use legend_pure_core_platform::platform::load_platform;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::heap::{ObjectId, RuntimeHeap};
use legend_pure_runtime::hooks::EvalHooks;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::native::testing::find_pct_adapter;
use legend_pure_runtime::value::{MapState, Value};

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

    /// Run PCT tests (`<<PCT.test>>`-stereotyped) instead of the default
    /// `<<test.Test>>` surveyor. The adapter is selected via `--adapter`
    /// (default `"In-Memory"`); a `--manifest <path>` overrides that and
    /// loads adapter + exclusions from a JSON manifest.
    #[arg(long)]
    pub pct: bool,

    /// PCT adapter name — matched against the `PCT.adapterName` tag of
    /// `<<PCT.adapter>>`-stereotyped functions in the model. Default is
    /// `"In-Memory"` (the shipped `testAdapterForInMemoryExecution`).
    /// Ignored when `--manifest` is set.
    #[arg(long, default_value = "In-Memory")]
    pub adapter: String,

    /// PCT manifest path (matched against embedded platform manifests by
    /// suffix; e.g. `pct_essential_native.json`). When set, overrides
    /// `--adapter` and loads adapter + exclusions from JSON. Only
    /// meaningful with `--pct`.
    #[arg(long)]
    pub manifest: Option<String>,

    /// Skip the bundled `pct_grammar_rust_native.json` exclusions when
    /// running PCT. By default the CLI loads them so tests known to
    /// fail for representational reasons (i16 year, i64-overflowing
    /// Integer literals) report PASS. Disable when validating those
    /// underlying limits or chasing regressions in the exclusion
    /// mechanism itself. Ignored unless `--pct` is set; ignored when
    /// `--manifest` is set (the manifest path supplies its own
    /// exclusions).
    #[arg(long = "no-default-exclusions")]
    pub no_default_exclusions: bool,

    /// Collect Pure code coverage during test execution.
    #[arg(long)]
    pub coverage: bool,

    /// Only track coverage for Pure source files whose path starts with
    /// this prefix. Requires `--coverage`.
    #[arg(long, default_value = "")]
    pub coverage_filter: String,

    /// Write LCOV tracefile to this path (default: `coverage.lcov`).
    /// Requires `--coverage`.
    #[arg(long, default_value = "coverage.lcov")]
    pub coverage_output: PathBuf,

    /// Generate an HTML coverage report in this directory via `genhtml`.
    /// Requires `--coverage`.
    #[arg(long)]
    pub coverage_html: Option<PathBuf>,

    /// Root directories for Pure source files. Virtual source paths in the
    /// compiled model (e.g. `/platform/pure/essential/...`) are resolved
    /// relative to these directories, allowing `genhtml` to display actual
    /// source code in the HTML report. Multiple roots can be specified;
    /// each is tried in order until the file is found.
    #[arg(long)]
    pub coverage_source_root: Vec<PathBuf>,
}

/// Execute the `legend test` command.
#[allow(clippy::needless_pass_by_value)] // clap convention
pub fn run(args: TestArgs) -> Result<(), CliError> {
    let mode_label = if args.pct { "PCT tests" } else { "tests" };
    let pct_via = if args.pct {
        if let Some(m) = &args.manifest { format!(" (manifest: {m})") } else {
            let suffix = if args.no_default_exclusions {
                ", no default exclusions"
            } else {
                ", default exclusions: pct_grammar_rust_native.json"
            };
            format!(" (adapter: {}{})", args.adapter, suffix)
        }
    } else {
        String::new()
    };
    eprintln!(
        "{} {} in {}{}{}",
        "Running".cyan().bold(),
        mode_label,
        args.package,
        args.filter
            .as_deref()
            .map(|f| format!(" (filter: {f})"))
            .unwrap_or_default(),
        pct_via,
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

    let registry = NativeRegistry::standard();

    if args.coverage {
        // Coverage path — use CoverageHooks.
        let mut hooks = CoverageHooks::new(args.coverage_filter.clone());
        hooks.map_mut().populate_coverable(&model);
        let mut evaluator = Evaluator::with_hooks(&model, &registry, hooks);

        let (report, fail) = run_tests(&model, &mut evaluator, &args)?;
        report.render(args.show_detail);

        // Extract coverage data and generate reports.
        let map = evaluator.into_hooks().into_map();
        print_coverage_summary(&map);

        super::coverage_report::write_lcov(&map, &args.coverage_output, &args.coverage_source_root)
            .map_err(|e| CliError::Custom(format!("Failed to write LCOV: {e}")))?;
        eprintln!(
            "  {} LCOV tracefile written to {}",
            "✓".green().bold(),
            args.coverage_output.display(),
        );

        if let Some(ref html_dir) = args.coverage_html {
            match super::coverage_report::generate_html(&args.coverage_output, html_dir) {
                Ok(()) => eprintln!(
                    "  {} HTML report generated in {}",
                    "✓".green().bold(),
                    html_dir.display(),
                ),
                Err(e) => eprintln!("  {} HTML report: {}", "warning:".yellow().bold(), e,),
            }
        }

        if fail {
            Err(CliError::Custom(format!(
                "{} test(s) failed, {} test(s) errored",
                report.fail_count, report.error_count
            )))
        } else {
            Ok(())
        }
    } else {
        // Production path — zero-overhead NoOpHooks.
        let mut evaluator = Evaluator::new(&model, &registry);
        let (report, fail) = run_tests(&model, &mut evaluator, &args)?;
        report.render(args.show_detail);

        if fail {
            Err(CliError::Custom(format!(
                "{} test(s) failed, {} test(s) errored",
                report.fail_count, report.error_count
            )))
        } else {
            Ok(())
        }
    }
}

/// Common test execution logic — generic over hooks so both production
/// and coverage paths share the same code.
fn run_tests<'m, H: EvalHooks>(
    model: &'m PureModel,
    evaluator: &mut Evaluator<'m, H>,
    args: &TestArgs,
) -> Result<(TestReport, bool), CliError> {
    let result = if args.pct {
        run_pct(model, evaluator, args)
    } else {
        evaluator.call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String(args.package.clone().into()),
                Value::String(args.filter.clone().unwrap_or_default().into()),
            ],
        )
    }
    .map_err(|e| CliError::Custom(format!("Test execution failed: {e}")))?;

    let Value::Object(report_id) = result else {
        return Err(CliError::Custom(format!(
            "Test surveyor returned non-Object: {result:?}"
        )));
    };
    let report = TestReport::read(evaluator.heap(), report_id)?;
    let fail = report.fail_count + report.error_count > 0;
    Ok((report, fail))
}

/// Drive the PCT surveyor — either via adapter discovery (default) or via
/// a JSON manifest path (`--manifest`). Both paths return a `TestReport`
/// heap object identical in shape to `runTestsFromPath`.
fn run_pct<H: EvalHooks>(
    model: &PureModel,
    evaluator: &mut Evaluator<'_, H>,
    args: &TestArgs,
) -> Result<Value, PureException> {
    if let Some(manifest_path) = &args.manifest {
        return evaluator.call(
            "meta::pure::test::surveyor::runPCTTestsFromPath",
            &[
                Value::String(args.package.clone().into()),
                Value::String(args.filter.clone().unwrap_or_default().into()),
                Value::String(manifest_path.clone().into()),
            ],
        );
    }

    // Adapter-name path: discover the function via PCT.adapterName, build
    // an empty exclusions Map, and call `runPCTTests` directly. This sidesteps
    // the JSON manifest entirely — adapters self-register in Pure code via
    // the `<<PCT.adapter>>` stereotype + `PCT.adapterName='<name>'` tag.
    let adapter_id = find_pct_adapter(model, &args.adapter).ok_or_else(|| {
        legend_pure_runtime::error::PureRuntimeError::EvaluationError(format!(
            "no PCT adapter found with PCT.adapterName='{}' — \
             check that a function carrying <<PCT.adapter>> + that tag exists in the model",
            args.adapter
        ))
    })?;
    let adapter_value = Value::Element(adapter_id);
    let exclusions = if args.no_default_exclusions {
        Value::Map(Rc::new(RefCell::new(MapState::default())))
    } else {
        legend_pure_runtime::pct::rust_native_exclusions()
    };

    // `runPCTTests` takes an already-resolved package, so call `pathToElement`
    // first. Both calls share the same evaluator scope so the package value
    // doesn't escape its model lifetime.
    let pkg = evaluator.call(
        "meta::pure::functions::meta::pathToElement",
        &[
            Value::String(args.package.clone().into()),
            Value::String("::".into()),
        ],
    )?;
    evaluator.call(
        "meta::pure::test::surveyor::runPCTTests",
        &[
            pkg,
            Value::String(args.filter.clone().unwrap_or_default().into()),
            adapter_value,
            exclusions,
        ],
    )
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
        for v in &raw_results {
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
                (TestStatus::Pass | TestStatus::Skip, true) => r.render_line(),
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

// ---------------------------------------------------------------------------
// Coverage Summary (CLI output)
// ---------------------------------------------------------------------------

/// Print a colored coverage summary table to stderr.
fn print_coverage_summary(map: &CoverageMap) {
    let summary = map.summary();

    eprintln!();
    eprintln!(
        "┌──────────────────────────────────────────────────────────────────────────┐"
    );
    eprintln!(
        "│ {}                                                    │",
        "Pure Coverage Summary".bold()
    );
    eprintln!("├──────────────────────────────────┬────────┬───────┬───────┬───────┬──────┤");
    eprintln!(
        "│ {:<32} │ {:>6} │ {:>5} │ {:>5} │ {:>5} │ {:>4} │",
        "File".bold(),
        "Lines".bold(),
        "Hit".bold(),
        "L%".bold(),
        "Br".bold(),
        "F%".bold(),
    );
    eprintln!("├──────────────────────────────────┼────────┼───────┼───────┼───────┼──────┤");

    for (source, file_cov) in map.files() {
        let lf = file_cov.lines_found();
        let lh = file_cov.lines_hit();
        let lp = file_cov.line_percentage();

        let mut file_br_found: u32 = 0;
        let mut file_br_hit: u32 = 0;
        for point in map.branches.points_in_file(source.as_str()) {
            for arm in &point.arms {
                file_br_found += 1;
                if arm.hit_count > 0 {
                    file_br_hit += 1;
                }
            }
        }
        let br_str = format!("{file_br_hit}/{file_br_found}");

        let mut file_fn_found: u32 = 0;
        let mut file_fn_hit: u32 = 0;
        for (_, entry) in map.functions.functions_in_file(source.as_str()) {
            file_fn_found += 1;
            if entry.hit_count > 0 {
                file_fn_hit += 1;
            }
        }
        let fp = if file_fn_found == 0 {
            100.0
        } else {
            (f64::from(file_fn_hit) / f64::from(file_fn_found)) * 100.0
        };

        let display_source = if source.len() > 32 {
            format!("…{}", &source[source.len() - 31..])
        } else {
            source.to_string()
        };

        let lp_str = format!("{lp:.1}%");
        let fp_str = format!("{fp:.0}%");

        eprintln!(
            "│ {display_source:<32} │ {lf:>6} │ {lh:>5} │ {lp_str:>5} │ {br_str:>5} │ {fp_str:>4} │",
        );
    }

    eprintln!("├──────────────────────────────────┼────────┼───────┼───────┼───────┼──────┤");

    let lp_str = format!("{:.1}%", summary.line_percentage);
    let br_str = format!("{}/{}", summary.branches_hit, summary.branches_found);
    let fp_str = format!("{:.0}%", summary.function_percentage);

    eprintln!(
        "│ {:<32} │ {:>6} │ {:>5} │ {:>5} │ {:>5} │ {:>4} │",
        "TOTAL".bold(),
        summary.lines_found,
        summary.lines_hit,
        lp_str,
        br_str,
        fp_str,
    );
    eprintln!("└──────────────────────────────────┴────────┴───────┴───────┴───────┴──────┘");
    eprintln!();
    eprintln!(
        " {} = line coverage   {} = branches hit/total   {} = function coverage",
        "L%".dimmed(),
        "Br".dimmed(),
        "F%".dimmed(),
    );
}
