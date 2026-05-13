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

//! Typed result structs for the [runner](super) API.
//!
//! The shapes here are deliberately serde-friendly without being
//! locked to any particular wire format. Callers (LSP's
//! `workspace/executeCommand`, the MCP `tools/call` flow, future
//! REPL embedders) translate to their own envelope as needed.

use serde::Serialize;

/// Outcome of calling a single function via [`run_function`](super::run_function).
///
/// `ok == true` ↔ the call returned a value without raising a
/// `PureException`. On the happy path, `value` carries the rendered
/// result and `error` is `None`. On a runtime failure, `value` is
/// `None` and `error` is `Some` with the structured exception
/// details.
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    /// FQN we attempted to call. Echoed back so callers don't need to
    /// thread the input through their own response shape.
    pub fqn: String,
    /// `true` iff the call returned a value cleanly.
    pub ok: bool,
    /// Rendered return value when `ok`. `None` on error.
    pub value: Option<String>,
    /// Captured `print` / `println` output produced during the call.
    /// Empty when the function didn't write to stdout.
    pub stdout: String,
    /// Structured error details when `!ok`.
    pub error: Option<RunError>,
}

/// Structured view of a runtime [`PureException`](crate::error::PureException).
#[derive(Debug, Clone, Serialize)]
pub struct RunError {
    /// One-line user-facing message (the quoted body of the
    /// PureException, or the header when no body is present).
    pub message: String,
    /// Parsed `Full Stack:` frames, innermost first. Empty when the
    /// exception carried no stack.
    pub stack: Vec<StackFrame>,
    /// Original `PureException::Display` text. Kept verbatim so
    /// callers that need the raw form (e.g. for downstream diffing
    /// against the Java engine) don't have to re-format.
    pub raw: String,
}

/// One frame of a [`PureException`](crate::error::PureException)'s
/// call stack — the substring the runtime emits on each
/// `\n    <name>     <-     resource:X line:Y column:Z` line.
#[derive(Debug, Clone, Serialize)]
pub struct StackFrame {
    /// Function name as printed by `printPureStackTrace`.
    pub function: String,
    /// Canonical source path of the call site (`/platform/.../X.pure`).
    pub source: String,
    /// 1-based line, matching `SourceInformation`.
    pub line: u32,
    /// 1-based column, matching `SourceInformation`.
    pub column: u32,
}

/// Outcome of running a `<<test::Test>>`-tagged function (or a
/// `<<PCT.test>>`) through the platform surveyor.
///
/// `ok == fail_count == 0 && error_count == 0`. Failure / error
/// detail is in `failures`; the `rendered` field is a pre-formatted
/// human-readable summary suitable for an IDE notification balloon
/// (same shape the old `legend.runTest` extras consumed).
#[derive(Debug, Clone, Serialize)]
pub struct TestRunResult {
    /// FQN of the test entry point we surveyed.
    pub fqn: String,
    /// `true` iff every collected test passed (or skipped) with no
    /// failures or errors.
    pub ok: bool,
    /// Surveyor counters off the `TestReport` heap object.
    pub passed: i64,
    /// Failed counter.
    pub failed: i64,
    /// Errored counter (uncaught exception during run).
    pub errored: i64,
    /// Skipped counter (manifest exclusions / representational gaps).
    pub skipped: i64,
    /// Total elapsed time in milliseconds, off `TestReport.totalElapsed`.
    pub total_elapsed_ms: i64,
    /// Structured per-test failure list. Empty on green.
    pub failures: Vec<TestFailure>,
    /// Captured `print` / `println` output across the whole surveyor
    /// run.
    pub stdout: String,
    /// One-line summary suitable for notification UIs.
    pub rendered: String,
}

/// Structured view of one failing test, ready for the IDE to render
/// as a clickable link.
#[derive(Debug, Clone, Serialize)]
pub struct TestFailure {
    /// FQN of the failing test function.
    pub fqn: String,
    /// One-line human-readable message (the quoted body of the
    /// PureException, stripped of surrounding quotes when present).
    pub message: String,
    /// Parsed call-stack frames, innermost first. Empty when the
    /// PureException carried no stack (older / hand-rolled errors).
    pub stack: Vec<StackFrame>,
}

/// One PCT adapter discovered in the compiled workspace.
///
/// Returned by [`list_pct_adapters`](super::list_pct_adapters).
/// `name` falls back to `"<unnamed>"` when the adapter function
/// lacks an `adapterName` tagged value.
#[derive(Debug, Clone, Serialize)]
pub struct PctAdapterInfo {
    /// Fully-qualified name of the adapter function.
    pub fqn: String,
    /// Human-readable name, sourced from the `PCT.adapterName`
    /// tagged value on the adapter function.
    pub name: String,
}

/// Errors that the runner itself raises before — or instead of —
/// executing user code.
///
/// These are setup / pre-flight failures (missing arguments, profile
/// not loaded, adapter not found), not Pure-level runtime failures.
/// Pure-level failures travel inside [`RunResult::error`] /
/// [`TestRunResult::failures`].
#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail")]
pub enum RunnerError {
    /// `legend.runPCT` was invoked without an adapter FQN.
    #[error(
        "missing adapter FQN — expected arguments: [testFqn, adapterFqn]"
    )]
    MissingAdapterFqn,
    /// `legend.runPCT` was given an adapter FQN that doesn't resolve
    /// against the current `PureModel`.
    #[error("adapter not found in model: {0}")]
    AdapterNotFound(String),
    /// `pathToElement(testFqn)` failed — usually because the FQN is
    /// not a valid path or the element isn't loaded.
    #[error("pathToElement failed: {0}")]
    PathToElementFailed(String),
    /// The PCT profile (`meta::pure::test::pct::PCT`) isn't loaded
    /// in the model — the workspace's platform repo is missing.
    #[error(
        "PCT profile (meta::pure::test::pct::PCT) not resolvable in workspace model — is the platform loaded?"
    )]
    PctProfileMissing,
    /// The surveyor returned a value that isn't a `TestReport`
    /// object — indicates the surveyor itself broke its contract or
    /// the wrong overload was dispatched.
    #[error("surveyor returned non-Object: {0}")]
    NonObjectReport(String),
    /// `read_test_report_summary` couldn't decode the `TestReport`
    /// heap object — usually means the metamodel shape drifted.
    #[error("failed to read TestReport: {0}")]
    TestReportReadFailed(String),
}
