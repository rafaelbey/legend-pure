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

//! High-level entry points for running a single Pure function, a
//! `<<test::Test>>`-tagged test, or a PCT test against the current
//! [`PureModel`].
//!
//! This module exists so the LSP (`workspace/executeCommand`), the
//! MCP server (`tools/call run_function` / `run_test` / `run_pct` /
//! `list_pct_adapters`), and any future headless embedder share the
//! same evaluation path. The LSP previously hosted these flows
//! inline in `handlers.rs::execute_legend_command`; that surface
//! is now a thin wrapper around this module.
//!
//! # Threading model
//!
//! Every entry point is **synchronous** — callers wrap in
//! `tokio::task::spawn_blocking` (the LSP does), in
//! `tokio::task::spawn_blocking` again for the MCP server, or call
//! directly from a sync thread. The runner makes no assumptions
//! about the surrounding executor.
//!
//! # Hook injection
//!
//! Every entry point installs a [`CapturingHooks`] internally so
//! that Pure-level `print` / `println` is captured into the returned
//! `stdout` field rather than written to the process's actual
//! stdout. Embedders that want a different hook strategy (live
//! streaming, debugger, …) can build their own
//! [`Evaluator`] and call
//! [`Evaluator::call`](crate::eval::Evaluator::call) directly.

mod exception;
mod report;
mod types;

pub use types::{
    PctAdapterInfo, RunError, RunResult, RunnerError, StackFrame, TestFailure, TestRunResult,
};

use std::sync::{Arc, Mutex};

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};

use crate::dsl::{DSLPopulator, run_populators};
use crate::eval::Evaluator;
use crate::hooks::EvalHooks;
use crate::native::NativeRegistry;
use crate::value::Value;

use exception::parse_failure_components;
use report::{TestReportSummary, read_test_report_summary};

// ---------------------------------------------------------------------------
// Hooks — captures `print` / `println` output into an in-memory
// buffer so callers can return it alongside the result.
// ---------------------------------------------------------------------------

/// Custom [`EvalHooks`] that captures Pure-level `print` / `println`
/// output into a thread-safe buffer.
///
/// Used internally by every runner entry point; exposed `pub` so
/// embedders that build their own [`Evaluator`] can opt in to the
/// same capture pattern.
///
/// `Arc<Mutex<…>>` (not `Rc<RefCell<…>>`) so the value is `Send` —
/// callers commonly wrap the evaluator in
/// `tokio::task::spawn_blocking` whose closure must be `Send`.
#[derive(Clone, Debug)]
pub struct CapturingHooks {
    buffer: Arc<Mutex<String>>,
}

impl CapturingHooks {
    /// Build a hook with an empty buffer plus the shared handle the
    /// caller will drain after the evaluator returns.
    #[must_use]
    pub fn new() -> (Self, Arc<Mutex<String>>) {
        let buffer = Arc::new(Mutex::new(String::new()));
        (
            Self {
                buffer: buffer.clone(),
            },
            buffer,
        )
    }
}

impl EvalHooks for CapturingHooks {
    fn before_eval(
        &mut self,
        _source: &legend_pure_parser_ast::SourceInfo,
        _context: &crate::context::VariableContext,
    ) -> bool {
        // CapturingHooks never requests a pause — it only intercepts
        // `console_output` to redirect Pure-level `print` / `println`
        // into a captured buffer. The new EvalHooks trait signature
        // returns `bool` so DAP-aware hooks can signal "build a
        // snapshot tree before running this expression"; for plain
        // capture we always say "no, just keep going".
        false
    }
    fn after_eval(&mut self, _source: &legend_pure_parser_ast::SourceInfo, _result: &Value) {}
    fn enter_function(&mut self, _name: &str, _source: &legend_pure_parser_ast::SourceInfo) {}
    fn leave_function(&mut self, _name: &str) {}
    fn console_output(&mut self, msg: &str) {
        if let Ok(mut guard) = self.buffer.lock() {
            guard.push_str(msg);
        }
    }
}

/// Call a function by FQN with no arguments and return its rendered
/// value.
///
/// Backs the IDE's ▶ Run gutter, `legend run <fqn>`, the MCP
/// `run_function` tool, and the LSP's `legend.run` executeCommand.
///
/// `populators` are DSL populators (e.g.
/// `RelationalDatabaseDSLPopulator`) that hydrate `Element::DSLInstance`
/// heap rows for sources whose semantics aren't reachable from the
/// metamodel alone (the Database/Schema/Table chain a `###Relational`
/// block produces). Pass `&[]` if the call doesn't touch DSL
/// instances; without the matching populator, reflective navigation
/// (`mydb.schemas->at(0).tables->at(0)`) returns empty.
pub fn run_function(
    model: &PureModel,
    registry: &NativeRegistry,
    populators: &[&dyn DSLPopulator],
    fqn: &str,
) -> RunResult {
    let (hooks, capture) = CapturingHooks::new();
    let mut evaluator = Evaluator::with_hooks(model, registry, hooks);
    run_populators(model, evaluator.heap_mut(), populators);
    let outcome = run_function_inner(&mut evaluator, fqn);
    drop(evaluator);
    let stdout = drain_capture(&capture);
    let mut result = outcome;
    result.stdout = stdout;
    result
}

/// Run a `<<test::Test>>`-tagged function via the platform's
/// surveyor (`meta::pure::test::surveyor::runTestsFromPath`).
///
/// The surveyor walks the package tree, collects every test under
/// the path, and brackets each with the right
/// `<<test::BeforePackage>>` / `<<test::AfterPackage>>` hooks. We
/// then read the populated `TestReport` heap object into a typed
/// [`TestRunResult`].
pub fn run_test(
    model: &PureModel,
    registry: &NativeRegistry,
    populators: &[&dyn DSLPopulator],
    fqn: &str,
) -> Result<TestRunResult, RunnerError> {
    let (hooks, capture) = CapturingHooks::new();
    let mut evaluator = Evaluator::with_hooks(model, registry, hooks);
    run_populators(model, evaluator.heap_mut(), populators);
    let result = evaluator.call(
        "meta::pure::test::surveyor::runTestsFromPath",
        &[
            Value::String(fqn.to_string().into()),
            Value::String(String::new().into()),
        ],
    );
    let outcome = interpret_test_report(result, fqn, evaluator.heap());
    drop(evaluator);
    let stdout = drain_capture(&capture);
    outcome.map(|mut r| {
        r.stdout = stdout;
        r
    })
}

/// Run a `<<PCT.test>>` against the chosen adapter via
/// `meta::pure::test::surveyor::runPCTTests`.
///
/// `test_fqn` is the FQN of a PCT test; `adapter_fqn` is the FQN of
/// the adapter function (discoverable via [`list_pct_adapters`]).
/// Exclusions default to the runtime's
/// [`pct::rust_native_exclusions`](crate::pct::rust_native_exclusions)
/// — the same 9-test list the CLI uses by default.
pub fn run_pct(
    model: &PureModel,
    registry: &NativeRegistry,
    populators: &[&dyn DSLPopulator],
    test_fqn: &str,
    adapter_fqn: &str,
) -> Result<TestRunResult, RunnerError> {
    if adapter_fqn.is_empty() {
        return Err(RunnerError::MissingAdapterFqn);
    }
    let Some(adapter_id) = model.resolve_fqn_str(adapter_fqn) else {
        return Err(RunnerError::AdapterNotFound(adapter_fqn.to_string()));
    };
    let (hooks, capture) = CapturingHooks::new();
    let mut evaluator = Evaluator::with_hooks(model, registry, hooks);
    run_populators(model, evaluator.heap_mut(), populators);
    // Resolve the test FQN to a PackageableElement via the
    // platform's `pathToElement`. The surveyor's
    // `getPCTTestFunctions` has a `ConcreteFunctionDefinition`
    // match-arm that returns the singleton when the input is a
    // function and the stereotype checks out.
    let pkg = match evaluator.call(
        "meta::pure::functions::meta::pathToElement",
        &[
            Value::String(test_fqn.to_string().into()),
            Value::String("::".into()),
        ],
    ) {
        Ok(v) => v,
        Err(e) => return Err(RunnerError::PathToElementFailed(e.to_string())),
    };
    let result = evaluator.call(
        "meta::pure::test::surveyor::runPCTTests",
        &[
            pkg,
            Value::String(String::new().into()),
            Value::Element(adapter_id),
            crate::pct::rust_native_exclusions(),
        ],
    );
    let outcome = interpret_test_report(result, test_fqn, evaluator.heap());
    drop(evaluator);
    let stdout = drain_capture(&capture);
    outcome.map(|mut r| {
        r.stdout = stdout;
        r
    })
}

/// Discover every PCT adapter (`<<PCT.adapter>>`-tagged function) in
/// the model and return their FQNs plus their `PCT.adapterName`
/// tagged value.
///
/// Backs the IDE's PCT-adapter chooser popup and the MCP
/// `list_pct_adapters` tool.
pub fn list_pct_adapters(model: &PureModel) -> Result<Vec<PctAdapterInfo>, RunnerError> {
    let Some(pct_profile) = crate::m3_paths::resolve(model, "meta::pure::test::pct::PCT") else {
        return Err(RunnerError::PctProfileMissing);
    };
    let mut adapters: Vec<PctAdapterInfo> = Vec::new();
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            let has_adapter_stereotype = func
                .stereotypes
                .iter()
                .any(|s| s.profile == pct_profile && s.value == "adapter");
            if !has_adapter_stereotype {
                continue;
            }
            let adapter_name = func
                .tagged_values
                .iter()
                .find(|t| t.profile == pct_profile && t.tag == "adapterName")
                .map(|t| t.value.to_string());
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            adapters.push(PctAdapterInfo {
                fqn: render_fqn(model, id),
                name: adapter_name.unwrap_or_else(|| "<unnamed>".to_string()),
            });
        }
    }
    Ok(adapters)
}

fn run_function_inner<H>(evaluator: &mut Evaluator<'_, H>, fqn: &str) -> RunResult
where
    H: EvalHooks,
{
    match evaluator.call(fqn, &[]) {
        Ok(value) => RunResult {
            fqn: fqn.to_string(),
            ok: true,
            value: Some(render_runtime_value(&value)),
            stdout: String::new(),
            error: None,
        },
        Err(e) => {
            let raw = e.to_string();
            let (message, stack) = parse_failure_components(&raw);
            RunResult {
                fqn: fqn.to_string(),
                ok: false,
                value: None,
                stdout: String::new(),
                error: Some(RunError {
                    message,
                    stack,
                    raw,
                }),
            }
        }
    }
}

fn interpret_test_report(
    result: Result<Value, crate::error::PureException>,
    fqn: &str,
    heap: &crate::heap::RuntimeHeap,
) -> Result<TestRunResult, RunnerError> {
    match result {
        Ok(Value::Object(ref report_id)) => match read_test_report_summary(heap, report_id) {
            Ok(summary) => Ok(make_test_run_result(summary, fqn)),
            Err(e) => Err(RunnerError::TestReportReadFailed(e)),
        },
        Ok(other) => Err(RunnerError::NonObjectReport(format!("{other:?}"))),
        // Pure-level errors during the surveyor itself (not in
        // individual tests) bubble up as a "no tests ran, errored
        // out" outcome rather than a setup failure — the surveyor
        // ran, it just blew up. Surface it as a TestRunResult so the
        // caller sees the same structured shape as a per-test
        // failure.
        Err(e) => Ok(test_run_result_for_surveyor_error(fqn, &e)),
    }
}

fn make_test_run_result(summary: TestReportSummary, fqn: &str) -> TestRunResult {
    let ok = summary.fail_count == 0 && summary.error_count == 0;
    let rendered = summary.render();
    tracing::info!(
        fqn = %fqn,
        failures = summary.failures.len(),
        rendered_len = rendered.len(),
        rendered = %rendered,
        "test report dispatch",
    );
    TestRunResult {
        fqn: fqn.to_string(),
        ok,
        passed: summary.pass_count,
        failed: summary.fail_count,
        errored: summary.error_count,
        skipped: summary.skip_count,
        total_elapsed_ms: summary.total_elapsed_ms,
        failures: summary.failures,
        stdout: String::new(),
        rendered,
    }
}

fn test_run_result_for_surveyor_error(fqn: &str, e: &crate::error::PureException) -> TestRunResult {
    let raw = e.to_string();
    let (message, stack) = parse_failure_components(&raw);
    let rendered = format!("surveyor failed: {message}");
    TestRunResult {
        fqn: fqn.to_string(),
        ok: false,
        passed: 0,
        failed: 0,
        errored: 1,
        skipped: 0,
        total_elapsed_ms: 0,
        failures: vec![TestFailure {
            fqn: fqn.to_string(),
            message,
            stack,
        }],
        stdout: String::new(),
        rendered,
    }
}

fn drain_capture(capture: &Arc<Mutex<String>>) -> String {
    let mut guard = capture.lock().unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut *guard)
}

/// Best-effort rendering of a runtime [`Value`] for one-line display
/// (notification balloons, MCP JSON output). Refining to a Pure-
/// level `toString` (i.e. dispatching back through the runtime to
/// format objects via the `toOne` / `toString` natives) is deferred.
fn render_runtime_value(value: &Value) -> String {
    match value {
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Boolean(b) => b.to_string(),
        Value::Collection(items) => {
            let rendered: Vec<String> = items.iter().map(render_runtime_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        other => format!("{other:?}"),
    }
}

fn render_fqn(model: &PureModel, id: ElementId) -> String {
    // Best-effort FQN rendering — climb packages by ID.
    if model.try_get_element(id).is_none() {
        return model.element_name(id).to_string();
    }
    // `get_node` panics for `ElementId::Package`; the caller (list
    // adapters) only ever feeds in InstanceIds today, but partial-
    // compile fixtures have tripped this path before. Fall back to
    // the element-name lookup which is total over both ID kinds.
    if matches!(id, ElementId::Package(_)) {
        return model.element_name(id).to_string();
    }
    let node = model.get_node(id);
    let mut parts: Vec<String> = vec![node.name.to_string()];
    let mut pkg_id = node.parent_package;
    loop {
        let pkg = model.get_package(pkg_id);
        if pkg.parent.is_none() {
            break;
        }
        parts.push(pkg.name.to_string());
        let Some(parent) = pkg.parent else {
            break;
        };
        pkg_id = parent;
    }
    parts.reverse();
    parts.join("::")
}
