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

//! Decode a `meta::pure::test::surveyor::TestReport` heap object
//! into the runner's typed [`TestRunResult`](super::types::TestRunResult)
//! shape.
//!
//! The reader keeps the same field set the original LSP path had:
//! integer counters off the report plus a per-row walk of `results`
//! to surface `FAIL` / `ERROR` rows. `PASS` and `SKIP` rows are
//! intentionally dropped — the surfaced UI only cares about
//! non-green outcomes, and ballooning them with the full pass list
//! makes the IDE notification unreadable on large surveys.

use super::exception::parse_failure_components;
use super::types::TestFailure;
use crate::heap::{ObjectHandle, RuntimeHeap};
use crate::value::Value;

/// Decoded view of a `TestReport` heap object — internal companion
/// to [`TestRunResult`](super::types::TestRunResult).
pub(super) struct TestReportSummary {
    pub(super) pass_count: i64,
    pub(super) fail_count: i64,
    pub(super) error_count: i64,
    pub(super) skip_count: i64,
    pub(super) total_elapsed_ms: i64,
    pub(super) failures: Vec<TestFailure>,
}

impl TestReportSummary {
    /// One-line summary suitable for a notification title — same
    /// format the IDE's Pure Run tool window expects.
    pub(super) fn render(&self) -> String {
        let header = format!(
            "passed={} failed={} errored={} skipped={} ({}ms)",
            self.pass_count,
            self.fail_count,
            self.error_count,
            self.skip_count,
            self.total_elapsed_ms,
        );
        if self.failures.is_empty() {
            return header;
        }
        // Cap at the first few failures so the notification stays
        // readable; the rest are still in the full failures vector
        // and the IDE renders them all via the structured `failures`
        // payload.
        let mut out = String::with_capacity(256);
        out.push_str(&header);
        for f in self.failures.iter().take(5) {
            out.push_str("\n  ✗ ");
            out.push_str(&f.fqn);
            if let Some(frame) = f.stack.first() {
                out.push_str(&format!(" @ {}:{}", frame.source, frame.line));
            }
            if !f.message.is_empty() {
                out.push_str("\n      ");
                out.push_str(&f.message);
            }
            for frame in f.stack.iter().take(8) {
                out.push_str(&format!(
                    "\n      at {} ({}:{}:{})",
                    frame.function, frame.source, frame.line, frame.column,
                ));
            }
        }
        if self.failures.len() > 5 {
            out.push_str(&format!("\n  … and {} more", self.failures.len() - 5));
        }
        out
    }
}

/// Read a `TestReport` heap object, decode it into a typed summary.
pub(super) fn read_test_report_summary(
    heap: &RuntimeHeap,
    report: &ObjectHandle,
) -> Result<TestReportSummary, String> {
    let pass_count = read_int_property(heap, report, "passCount")?;
    let fail_count = read_int_property(heap, report, "failCount")?;
    let error_count = read_int_property(heap, report, "errorCount")?;
    let skip_count = read_int_property(heap, report, "skipCount")?;
    let total_elapsed_ms = read_int_property(heap, report, "totalElapsed").unwrap_or(0);
    let mut failures = Vec::new();
    let results = heap
        .get_property_values(report, "results")
        .map_err(|e| format!("results: {e}"))?;
    for v in results.iter() {
        let Value::Object(rid) = v else {
            continue;
        };
        let status = read_status_property(heap, rid).unwrap_or_default();
        let is_failure = matches!(status.as_str(), "FAIL" | "ERROR");
        if !is_failure {
            continue;
        }
        let fqn = read_string_property(heap, rid, "fqn").unwrap_or_else(|_| "<unknown>".into());
        let raw = read_string_property(heap, rid, "message").ok();
        // Debug aid: log what we got off the heap. Two common failure
        // modes:
        //   - raw=None → `TestResult.message` slot was never
        //     populated. Surveyor / executePCTTest didn't classify
        //     the result as FAIL/ERROR, or the populate path
        //     dropped the message somewhere.
        //   - raw=Some(s) but parse yields empty body → the
        //     PureException Display ended up as just the kind header
        //     (no quoted message body). Caller likely raised a
        //     runtime error without setting the message.
        tracing::info!(
            test_fqn = %fqn,
            status = %status,
            raw_message_len = raw.as_deref().map(str::len).unwrap_or(0),
            raw_head = %raw.as_deref().map(|s| s.chars().take(200).collect::<String>()).unwrap_or_default(),
            "TestReport row read",
        );
        let (message, stack) = match raw.as_deref() {
            Some(s) => parse_failure_components(s),
            None => (String::new(), Vec::new()),
        };
        tracing::info!(
            test_fqn = %fqn,
            parsed_msg_len = message.len(),
            frames = stack.len(),
            "parsed failure components",
        );
        failures.push(TestFailure {
            fqn,
            message,
            stack,
        });
    }
    Ok(TestReportSummary {
        pass_count,
        fail_count,
        error_count,
        skip_count,
        total_elapsed_ms,
        failures,
    })
}

fn read_int_property(heap: &RuntimeHeap, obj: &ObjectHandle, name: &str) -> Result<i64, String> {
    let vs = heap
        .get_property_values(obj, name)
        .map_err(|e| format!("{name}: {e}"))?;
    match vs.iter().next() {
        Some(Value::Integer(n)) => Ok(*n),
        Some(other) => Err(format!("{name}: expected Integer, got {other:?}")),
        None => Err(format!("{name}: empty")),
    }
}

fn read_string_property(
    heap: &RuntimeHeap,
    obj: &ObjectHandle,
    name: &str,
) -> Result<String, String> {
    let vs = heap
        .get_property_values(obj, name)
        .map_err(|e| format!("{name}: {e}"))?;
    match vs.iter().next() {
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(other) => Err(format!("{name}: expected String, got {other:?}")),
        None => Err(format!("{name}: empty")),
    }
}

/// The `status` slot on a `TestResult` is an Enum value — we just
/// want its name (`PASS` / `FAIL` / `ERROR` / `SKIP`) so we can pull
/// out the failing tests for the summary.
fn read_status_property(heap: &RuntimeHeap, obj: &ObjectHandle) -> Result<String, String> {
    let vs = heap
        .get_property_values(obj, "status")
        .map_err(|e| format!("status: {e}"))?;
    match vs.iter().next() {
        Some(Value::EnumValue { member, .. }) => Ok(member.to_string()),
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(other) => Err(format!("status: unexpected shape {other:?}")),
        None => Err("status: empty".into()),
    }
}
