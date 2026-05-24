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
use legend_pure_core_platform::platform::{PLATFORM_AUTO_IMPORTS, load_platform};
use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::heap::{ObjectHandle, RuntimeHeap};
use legend_pure_runtime::hooks::EvalHooks;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::native::testing::find_pct_adapter;
use legend_pure_runtime::value::{MapState, Value};

use crate::diagnostics::CliError;

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub enum TestMode {
    Normal,
    Pct,
}

/// Output format for `legend test`.
#[derive(Default, Clone, Copy, clap::ValueEnum)]
pub enum TestFormat {
    /// Human-readable terminal output (default).
    #[default]
    Pretty,
    /// One JSON object per line (NDJSON), suitable for IDE / CI
    /// integrations. Each line is `{"type":"result", ...}` per test
    /// outcome plus a final `{"type":"summary", ...}` line.
    Json,
}

/// Arguments for the `legend test` command.
#[derive(clap::Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct TestArgs {
    /// Input `.pure` file(s) or directory containing tests.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,

    /// Output format. `pretty` writes coloured human output to stderr;
    /// `json` writes one diagnostic JSON object per test result to
    /// stdout (NDJSON) plus a final summary line.
    #[arg(long, value_enum, default_value_t = TestFormat::Pretty)]
    pub format: TestFormat,

    /// Scope test execution to a specific package.
    #[arg(long, default_value = "Root")]
    pub package: String,

    /// Filter tests by source path prefix to scope execution to a particular repo.
    #[arg(long, short)]
    pub filter: Option<String>,

    /// Print every PASS / SKIP line, not just FAIL / ERROR.
    #[arg(long = "show-detail")]
    pub show_detail: bool,

    /// The test mode to run (normal, pct). Can be specified multiple times
    /// (e.g., `--mode normal,pct` or `--mode normal --mode pct`).
    #[arg(long, value_enum, default_value = "normal", value_delimiter = ',')]
    pub mode: Vec<TestMode>,

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

    /// Read platform .pure files from disk instead of embedded copies.
    #[arg(long)]
    pub live: bool,

    /// Override the platform source directory. Only with --live.
    #[arg(long)]
    pub platform_dir: Option<PathBuf>,

    /// Re-run tests whenever .pure files change (implies --live).
    #[arg(long)]
    pub watch: bool,
}

/// Execute the `legend test` command.
///
/// `classpath` is the resolved `--classpath` flag. When set explicitly,
/// the command compiles the classpath's resolved repo set (same path
/// `legend snapshot` / `legend run` / `legend repl` take); `--live` /
/// `--watch` / `--platform-dir` are then ignored with a stderr warning,
/// and the watch loop short-circuits to a single run. When unset, the
/// legacy `--live` / embedded cascade applies.
#[allow(clippy::needless_pass_by_value)] // clap convention
pub fn run(args: TestArgs, classpath: Option<&std::path::Path>) -> Result<(), CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Custom(format!("cwd: {e}")))?;
    let resolved_classpath = crate::classpath::resolve_classpath(classpath, &cwd)
        .map_err(|e| CliError::Custom(format!("classpath: {e}")))?;
    let extension_configs = resolved_classpath.extension_configs;
    let classpath_explicit = classpath.is_some();
    if classpath_explicit && (args.live || args.watch) {
        eprintln!(
            "  {} `--live` / `--watch` / `--platform-dir` ignored when `--classpath` is set",
            "warning:".yellow().bold(),
        );
    }
    let classpath_repos: Option<Vec<Repo>> = classpath_explicit.then(|| resolved_classpath.repos);
    let classpath_extra_auto_imports: Vec<smol_str::SmolStr> = if classpath_explicit {
        resolved_classpath.extra_auto_imports
    } else {
        Vec::new()
    };
    // --watch only makes sense for the live-source-root watcher; with
    // an explicit classpath there's no single root to watch.
    let watch_effective = args.watch && !classpath_explicit;
    let mode_label = args
        .mode
        .iter()
        .map(|m| match m {
            TestMode::Normal => "normal tests",
            TestMode::Pct => "PCT tests",
        })
        .collect::<Vec<_>>()
        .join(" + ");

    let pct_via = if args.mode.contains(&TestMode::Pct) {
        if let Some(m) = &args.manifest {
            format!(" (manifest: {m})")
        } else {
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
    let descriptor = if !classpath_explicit && (args.live || args.watch) {
        Some(crate::live::resolve_platform_descriptor(
            args.platform_dir.as_deref(),
        )?)
    } else {
        None
    };

    let mut watcher_setup = false;
    let mut rx: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;
    let mut _debouncer = None;

    loop {
        let res = run_once(
            &args,
            &mode_label,
            &pct_via,
            descriptor.as_deref(),
            classpath_repos.as_deref(),
            &classpath_extra_auto_imports,
            &extension_configs,
        );

        if !watch_effective {
            return res;
        }

        if let Err(e) = res {
            crate::diagnostics::print_error(&e);
        }

        if !watcher_setup && let Some(ref d) = descriptor {
            let watch_root = crate::live::platform_source_root(d)?;
            match crate::live::watch_dir(&watch_root) {
                Ok((debouncer, flag)) => {
                    _debouncer = Some(debouncer);
                    rx = Some(flag);
                    watcher_setup = true;
                }
                Err(e) => {
                    return Err(CliError::Custom(format!("Failed to start watcher: {e}")));
                }
            }
        }

        eprintln!(
            "\n  {}",
            "Watching for changes... (Ctrl+C to stop)".dimmed()
        );

        if let Some(flag) = &rx {
            loop {
                if flag.load(std::sync::atomic::Ordering::SeqCst) {
                    flag.store(false, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        eprintln!("  {}\n", "File changed — re-running...".dimmed());
    }
}

fn run_once(
    args: &TestArgs,
    mode_label: &str,
    pct_via: &str,
    live_descriptor: Option<&std::path::Path>,
    classpath_repos: Option<&[Repo]>,
    classpath_extra_auto_imports: &[smol_str::SmolStr],
    extension_configs: &std::collections::HashMap<
        String,
        std::collections::HashMap<String, toml::Value>,
    >,
) -> Result<(), CliError> {
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

    let model = if let Some(repos) = classpath_repos {
        let mut auto_imports: Vec<smol_str::SmolStr> = PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| smol_str::SmolStr::new(s))
            .collect();
        auto_imports.extend(classpath_extra_auto_imports.iter().cloned());
        match repo::load(repos, &auto_imports) {
            Ok(m) => m,
            Err(partial) => {
                eprintln!(
                    "  {} classpath compiled with {} error(s)",
                    "warning:".yellow().bold(),
                    partial.errors.len()
                );
                partial.model
            }
        }
    } else if let Some(descriptor) = live_descriptor {
        let repos = crate::live::live_repos(descriptor)?;
        let auto_imports: Vec<smol_str::SmolStr> = PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| smol_str::SmolStr::new(s))
            .collect();
        match repo::load(&repos, &auto_imports) {
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

    // Discover every `RuntimeExtension` and `DSLPopulator` linked into
    // this binary via the `#[distributed_slice]` registrations in
    // their owning crates (see `crates/cli/src/main.rs` for the
    // force-link `use` statements that ensure the linker doesn't drop
    // the registrations). The platform standard set + the relational
    // store DuckDB-backed natives + the Mapping/Database/ClassMapping
    // populators all land in the same `discovered()` call — no
    // per-extension wiring in this command body.
    let registry = NativeRegistry::discovered();
    let populators = legend_pure_runtime::dsl::discovered_populators();

    if args.coverage {
        // Coverage path — use CoverageHooks.
        let mut hooks = CoverageHooks::new(args.coverage_filter.clone());
        hooks.map_mut().populate_coverable(&model);
        let mut evaluator = Evaluator::with_hooks(&model, &registry, hooks);
        evaluator.set_extension_configs(extension_configs.clone());
        legend_pure_runtime::dsl::run_populators(&model, evaluator.heap_mut(), &populators);

        let (report, fail) = run_tests(&model, &mut evaluator, args)?;
        match args.format {
            TestFormat::Pretty => report.render(&model, args.show_detail),
            TestFormat::Json => report.render_json(&model),
        }

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
        evaluator.set_extension_configs(extension_configs.clone());
        legend_pure_runtime::dsl::run_populators(&model, evaluator.heap_mut(), &populators);
        let (report, fail) = run_tests(&model, &mut evaluator, args)?;
        match args.format {
            TestFormat::Pretty => report.render(&model, args.show_detail),
            TestFormat::Json => report.render_json(&model),
        }

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

/// Result counters from a `runTestsFromPath` invocation. Exposed for
/// `legend build` so the orchestrator can decide pass/fail per repo
/// without duplicating the surveyor wiring.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BuildTestSummary {
    pub pass: i64,
    pub fail: i64,
    pub error: i64,
    pub skip: i64,
    pub elapsed_ms: i64,
}

impl BuildTestSummary {
    pub fn has_failures(self) -> bool {
        self.fail + self.error > 0
    }
}

/// Run normal tests against a pre-built model. Wraps the same surveyor
/// call `legend test` uses but takes a `PureModel` directly instead of
/// loading one from classpath; `legend build` calls this after its
/// per-repo compile loop has materialized the final model.
///
/// Renders results in the requested format (pretty → stderr, json →
/// stdout NDJSON), and returns the summary counters so the caller can
/// decide success / failure and stamp per-repo `last_test_result`.
///
/// # Errors
///
/// Returns [`CliError`] if the surveyor call fails or its report
/// object can't be decoded.
pub(crate) fn run_build_tests(
    model: &PureModel,
    extension_configs: &std::collections::HashMap<
        String,
        std::collections::HashMap<String, toml::Value>,
    >,
    package: &str,
    filter: &str,
    format: TestFormat,
    show_detail: bool,
) -> Result<BuildTestSummary, CliError> {
    let registry = NativeRegistry::discovered();
    let populators = legend_pure_runtime::dsl::discovered_populators();
    let mut evaluator = Evaluator::new(model, &registry);
    evaluator.set_extension_configs(extension_configs.clone());
    legend_pure_runtime::dsl::run_populators(model, evaluator.heap_mut(), &populators);

    let result = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[Value::String(package.into()), Value::String(filter.into())],
        )
        .map_err(|e| CliError::Custom(format!("Test execution failed: {e}")))?;

    let Value::Object(ref report_id) = result else {
        return Err(CliError::Custom(format!(
            "Test surveyor returned non-Object: {result:?}"
        )));
    };
    let report = TestReport::read(evaluator.heap(), report_id)?;

    match format {
        TestFormat::Pretty => report.render(model, show_detail),
        TestFormat::Json => report.render_json(model),
    }

    Ok(BuildTestSummary {
        pass: report.pass_count,
        fail: report.fail_count,
        error: report.error_count,
        skip: report.skip_count,
        elapsed_ms: report.total_elapsed_ms,
    })
}

/// Common test execution logic — generic over hooks so both production
/// and coverage paths share the same code.
fn run_tests<'m, H: EvalHooks>(
    model: &'m PureModel,
    evaluator: &mut Evaluator<'m, H>,
    args: &TestArgs,
) -> Result<(TestReport, bool), CliError> {
    let mut combined_report = TestReport::empty();

    for mode in &args.mode {
        let result = match mode {
            TestMode::Pct => run_pct(model, evaluator, args),
            TestMode::Normal => evaluator.call(
                "meta::pure::test::surveyor::runTestsFromPath",
                &[
                    Value::String(args.package.clone().into()),
                    Value::String(args.filter.clone().unwrap_or_default().into()),
                ],
            ),
        }
        .map_err(|e| CliError::Custom(format!("Test execution failed: {e}")))?;

        let Value::Object(ref report_id) = result else {
            return Err(CliError::Custom(format!(
                "Test surveyor returned non-Object: {result:?}"
            )));
        };
        let report = TestReport::read(evaluator.heap(), report_id)?;
        combined_report.merge(report);
    }

    let fail = combined_report.fail_count + combined_report.error_count > 0;
    Ok((combined_report, fail))
}

/// Drive the PCT surveyor — either via adapter discovery (default) or via
/// a JSON manifest path (`--manifest`). Both paths return a `TestReport`
/// heap object identical in shape to `runTestsFromPath`.
#[allow(clippy::result_large_err)]
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
    Other(#[allow(dead_code)] String),
}

impl TestReport {
    fn empty() -> Self {
        Self {
            pass_count: 0,
            fail_count: 0,
            error_count: 0,
            skip_count: 0,
            total_elapsed_ms: 0,
            results: Vec::new(),
        }
    }

    fn merge(&mut self, mut other: Self) {
        self.pass_count += other.pass_count;
        self.fail_count += other.fail_count;
        self.error_count += other.error_count;
        self.skip_count += other.skip_count;
        self.total_elapsed_ms += other.total_elapsed_ms;
        self.results.append(&mut other.results);
    }

    fn read(heap: &RuntimeHeap, id: &ObjectHandle) -> Result<Self, CliError> {
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
                results.push(TestResult::read(heap, rid));
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

    fn render(&self, model: &PureModel, show_detail: bool) {
        // Per-test lines — always show FAIL / ERROR; show PASS / SKIP only
        // when `--show-detail` is set.
        for r in &self.results {
            match (&r.status, show_detail) {
                (TestStatus::Pass | TestStatus::Skip, true)
                | (TestStatus::Fail | TestStatus::Error | TestStatus::Other(_), _) => {
                    r.render_line(model);
                }
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

    /// Emit one NDJSON object per test result on stdout, followed by a
    /// final `{"type":"summary",…}` line. Stable wire format consumed
    /// by the IntelliJ test-runner integration and any CI tooling.
    fn render_json(&self, model: &PureModel) {
        for r in &self.results {
            let status = match &r.status {
                TestStatus::Pass => "pass",
                TestStatus::Fail => "fail",
                TestStatus::Error => "error",
                TestStatus::Skip => "skip",
                TestStatus::Other(_) => "other",
            };
            // Resolve source location best-effort so the IDE runner can
            // jump to the failing test. Same lookup used by the pretty
            // printer.
            let path: Vec<smol_str::SmolStr> =
                r.fqn.split("::").map(smol_str::SmolStr::new).collect();
            let location = model.resolve_function_by_path(&path).map(|eid| {
                let node = model.get_node(eid);
                let si = &node.source_info;
                serde_json::json!({
                    "source": si.source.as_str(),
                    "line":   si.start_line,
                    "column": si.start_column,
                })
            });
            let payload = serde_json::json!({
                "type":      "result",
                "fqn":       r.fqn,
                "status":    status,
                "elapsedMs": r.elapsed_ms,
                "message":   r.message,
                "location":  location,
            });
            println!("{payload}");
        }
        let summary = serde_json::json!({
            "type":        "summary",
            "tests":       self.results.len(),
            "pass":        self.pass_count,
            "fail":        self.fail_count,
            "errors":      self.error_count,
            "skip":        self.skip_count,
            "elapsedMs":   self.total_elapsed_ms,
        });
        println!("{summary}");
    }
}

impl TestResult {
    fn read(heap: &RuntimeHeap, id: &ObjectHandle) -> Self {
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
        Self {
            fqn,
            status,
            elapsed_ms,
            message,
        }
    }

    fn render_line(&self, model: &PureModel) {
        let mark = match self.status {
            TestStatus::Pass => "✓".green().to_string(),
            TestStatus::Fail => "✗".red().to_string(),
            TestStatus::Error => "!".red().to_string(),
            TestStatus::Skip => "○".yellow().to_string(),
            TestStatus::Other(_) => "?".dimmed().to_string(),
        };

        let mut fqn_display = self.fqn.clone();
        let path: Vec<smol_str::SmolStr> =
            self.fqn.split("::").map(smol_str::SmolStr::new).collect();
        if let Some(element_id) = model.resolve_function_by_path(&path) {
            let node = model.get_node(element_id);
            let si = &node.source_info;

            let source_str = si.source.as_str();
            let mut real_path = std::path::PathBuf::from(source_str);
            if let Some(rel_path) = source_str.strip_prefix("/platform/")
                && let Ok(descriptor) = crate::live::resolve_platform_descriptor(None)
                && let Ok(source_root) = crate::live::platform_source_root(&descriptor)
            {
                real_path = source_root.join(rel_path);
            }

            let abs_path = crate::diagnostics::canonical_or_original(&real_path);
            fqn_display = format!(
                "{} ({}:{}:{})",
                self.fqn.bold(),
                abs_path.display(),
                si.start_line,
                si.start_column
            );
        }

        eprintln!(
            "  {mark} {fqn}  {timing}",
            fqn = fqn_display,
            timing = format!("({}ms)", self.elapsed_ms).dimmed(),
        );
        if matches!(self.status, TestStatus::Fail | TestStatus::Error)
            && let Some(msg) = &self.message
        {
            let mut first = true;
            for line in msg.lines().map(str::trim_end) {
                if line.is_empty() {
                    continue;
                }
                if first {
                    eprintln!("    {} {}", "└─".dimmed(), line.dimmed());
                    first = false;
                } else {
                    eprintln!("       {}", line.dimmed());
                }
            }
        }
    }
}

fn read_int_slot(heap: &RuntimeHeap, id: &ObjectHandle, slot: &str) -> Result<i64, CliError> {
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

fn read_string_slot(heap: &RuntimeHeap, id: &ObjectHandle, slot: &str) -> Result<String, CliError> {
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

/// Print a colored coverage summary table to stderr.
fn print_coverage_summary(map: &CoverageMap) {
    let summary = map.summary();

    eprintln!();
    eprintln!("┌──────────────────────────────────────────────────────────────────────────┐");
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
