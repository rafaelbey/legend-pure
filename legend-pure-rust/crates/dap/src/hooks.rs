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

//! `EvalHooks` implementation for the DAP server.
//!
//! The hook lives on the eval thread (`spawn_blocking`) and:
//!   1. Maintains a per-frame snapshot (`Vec<Frame>`) via
//!      `enter_function` / `leave_function`.
//!   2. On every `before_eval`, checks the breakpoint set against
//!      `source.source + source.start_line`. If hit (or step-over
//!      target depth caught up), takes a [`PauseSnapshot`] of the
//!      paused state, emits a `Stopped` event, and blocks on the
//!      command channel for the next step decision.
//!   3. Forwards captured `print` / `println` chunks as DAP
//!      `Output` events so the IDE's debug console mirrors what
//!      the user's code wrote.

use crate::protocol::{Event, OutputEventBody, ServerMessage, StoppedEventBody};
use crate::session::{DapCommand, FrameInfo, HookWiring, MAIN_THREAD_ID, PauseSnapshot};
use legend_pure_parser_ast::SourceInfo;
use legend_pure_runtime::context::VariableContext;
use legend_pure_runtime::debug::StepMode;
use legend_pure_runtime::hooks::EvalHooks;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// EvalHooks impl that intercepts every expression entry for
/// breakpoint matching + step-over arming.
pub struct DapHooks {
    wiring: HookWiring,
    /// Live call stack built from `enter_function` / `leave_function`.
    /// Innermost-last; the front frame is the current outermost
    /// function.
    frames: Vec<FrameInfo>,
    /// Current scope depth, mirroring
    /// [`VariableContext::depth`](legend_pure_runtime::context::VariableContext::depth).
    /// Tracked manually because the hook doesn't have a reference
    /// to the context — we approximate by counting
    /// `enter_function` / `leave_function`.
    depth: usize,
    /// Step semantic flag. `None` → run until breakpoint. `Some(d)`
    /// → pause again when the eval loop reaches an expression at
    /// `depth <= d`.
    step_target: Option<usize>,
    /// `true` once `Terminate` arrived — every subsequent
    /// `before_eval` returns immediately and the eval loop runs to
    /// the end of the current expression chain before the launch
    /// task exits.
    terminating: bool,
    /// `(source, line)` of the most recent pause site. While the
    /// next `before_eval` matches this same location, skip the
    /// pause check — one Pure source line lowers to many `eval`
    /// entries (function call + each arg + literals etc.), so a
    /// naive breakpoint match would re-fire on every sub-expression
    /// and force the user to click Continue many times to advance
    /// one line. Cleared the moment execution reaches a different
    /// `(source, line)` pair.
    last_paused_at: Option<(String, u32)>,
}

impl DapHooks {
    /// Construct a fresh hook with the given session wiring.
    pub fn new(wiring: HookWiring) -> Self {
        Self {
            wiring,
            frames: Vec::new(),
            depth: 0,
            step_target: None,
            terminating: false,
            last_paused_at: None,
        }
    }

    /// Have we matched a breakpoint or stepped into our target depth?
    ///
    /// Breakpoint paths come from two sources that disagree on
    /// shape: IntelliJ's `XSourcePositionDto` is an absolute
    /// filesystem path (`/Users/…/proj/abc.pure`); the runtime's
    /// `SourceInfo::source` is the canonical workspace-relative
    /// path the compiler uses (`/myproj/abc.pure` or just
    /// `/abc.pure`). Suffix-match either way so a breakpoint set
    /// on the IDE side fires regardless of how the runtime ends
    /// up addressing the same physical file.
    fn should_pause(&self, source: &SourceInfo) -> bool {
        if self.terminating {
            return false;
        }
        // Same-line re-fire suppression: many sub-expressions
        // share the parent statement's start_line, so a single
        // breakpoint would otherwise trigger N pauses per click.
        // Once we've paused on `(src, line)`, ignore further
        // matches until execution reaches a different location.
        if self
            .last_paused_at
            .as_ref()
            .is_some_and(|(s, l)| *l == source.start_line && path_match(s, source.source.as_str()))
        {
            return false;
        }
        if let Some(target) = self.step_target
            && self.depth <= target
        {
            return true;
        }
        let runtime_src = source.source.as_str();
        let state = match self.wiring.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for (bp_path, bp_line) in &state.breakpoints {
            if *bp_line != source.start_line {
                continue;
            }
            if path_match(bp_path, runtime_src) {
                return true;
            }
        }
        false
    }

    /// Build the pause snapshot, store it on shared state, emit a
    /// `Stopped` event, and block waiting for the next step
    /// command from the DAP server.
    fn pause(&mut self, source: &SourceInfo, locals: Vec<(SmolStr, String)>) {
        self.last_paused_at = Some((source.source.to_string(), source.start_line));
        let snapshot = PauseSnapshot {
            frames: self.frames.clone(),
            locals,
            source: source.clone(),
        };
        let seq;
        {
            let mut state = match self.wiring.state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            state.paused = Some(snapshot);
            seq = state.next_seq();
        }
        // Send `Stopped`. Step-over self-fire vs. breakpoint:
        // surface the reason so the IDE shows the right icon.
        let reason = if self.step_target.is_some_and(|t| self.depth <= t) {
            "step".to_string()
        } else {
            "breakpoint".to_string()
        };
        self.write_event(
            seq,
            "stopped",
            StoppedEventBody {
                reason,
                thread_id: Some(MAIN_THREAD_ID),
                all_threads_stopped: true,
                text: None,
            },
        );
        // Block until told to resume.
        self.step_target = None;
        let command = self
            .wiring
            .commands_rx
            .recv()
            .unwrap_or(DapCommand::Terminate);
        if matches!(command, DapCommand::Terminate) {
            self.terminating = true;
        }
        match command.to_step_mode(self.depth) {
            StepMode::Continue => self.step_target = None,
            StepMode::StepOver { target_depth } => {
                self.step_target = Some(target_depth);
            }
        }
        // Clear the snapshot — the eval thread is about to run
        // again and a stale `paused` would confuse the request
        // handlers if they fire before the next pause.
        let mut state = match self.wiring.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        state.paused = None;
    }

    fn write_event<B: serde::Serialize>(&self, seq: i64, event: &str, body: B) {
        let msg = ServerMessage::Event(Event {
            seq,
            event: event.to_string(),
            body: Some(serde_json::to_value(&body).unwrap_or(serde_json::Value::Null)),
        });
        let payload = match serde_json::to_vec(&msg) {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut out = match self.wiring.outbound.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let _ = crate::transport::write_frame(&mut *out, &payload);
    }
}

impl EvalHooks for DapHooks {
    fn before_eval(&mut self, source: &SourceInfo, context: &VariableContext) {
        // Clear the same-line suppression as soon as execution
        // reaches a different `(source, line)` pair — after that
        // any future breakpoint match on the original line should
        // fire again (e.g. a loop body re-entering the line).
        if let Some((prev_src, prev_line)) = self.last_paused_at.as_ref()
            && (*prev_line != source.start_line || !path_match(prev_src, source.source.as_str()))
        {
            self.last_paused_at = None;
        }
        if !self.should_pause(source) {
            return;
        }
        // Snapshot live bindings now, on the eval thread, before
        // the pause. `Value` is non-`Send`, so we render to
        // display strings here and store the owned text on
        // `PauseSnapshot.locals`. The variables handler reads
        // that snapshot when the IDE fetches the Variables panel
        // contents.
        let mut locals: Vec<(SmolStr, String)> = context
            .iter_bindings()
            .map(|(name, value)| (name.clone(), render_value(value)))
            .collect();
        // Stable ordering — the underlying storage is a
        // `HashMap`, but the IDE displays the list in the order
        // we send it. Alphabetical name order matches what most
        // debug UIs default to.
        locals.sort_by(|a, b| a.0.cmp(&b.0));
        self.pause(source, locals);
    }

    fn after_eval(&mut self, _source: &SourceInfo, _result: &Value) {}

    fn enter_function(&mut self, name: &str, source: &SourceInfo) {
        self.depth += 1;
        self.frames.push(FrameInfo {
            name: name.to_string(),
            source: source.clone(),
            depth: self.depth,
        });
    }

    fn leave_function(&mut self, _name: &str) {
        self.frames.pop();
        if self.depth > 0 {
            self.depth -= 1;
        }
    }

    fn console_output(&mut self, msg: &str) {
        // Forward to the DAP client as an `output` event with the
        // `stdout` category — IDE debug consoles render this
        // exactly like a real subprocess's stdout.
        let seq = {
            let mut state = match self.wiring.state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            state.next_seq()
        };
        self.write_event(
            seq,
            "output",
            OutputEventBody {
                category: "stdout".to_string(),
                output: msg.to_string(),
            },
        );
    }
}

/// Loose path matcher for breakpoint vs. runtime SourceInfo.
///
/// Accept the match if either path is a suffix of the other (or
/// both — equal paths trivially match). Handles the workspace-
/// relative vs. absolute filesystem-path mismatch between the
/// IDE-supplied breakpoint location and the runtime's compiler-
/// internal canonical path.
fn path_match(a: &str, b: &str) -> bool {
    let na = normalize(a);
    let nb = normalize(b);
    na.ends_with(&nb) || nb.ends_with(&na)
}

fn normalize(p: &str) -> String {
    let s = p.trim_start_matches('/');
    s.to_string()
}

/// Render a runtime [`Value`] for display in the IDE Variables
/// panel. Mirrors `server::render_value` byte-for-byte; kept inline
/// in the hooks module so the snapshot path doesn't need a
/// cross-module call on a hot path.
fn render_value(v: &Value) -> String {
    match v {
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Collection(items) => {
            let rendered: Vec<String> = items.iter().map(render_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        other => format!("{other:?}"),
    }
}
