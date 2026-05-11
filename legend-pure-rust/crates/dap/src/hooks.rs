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
use crate::session::{DapCommand, FrameInfo, HookWiring, PauseSnapshot, MAIN_THREAD_ID};
use legend_pure_parser_ast::SourceInfo;
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
        }
    }

    /// Have we matched a breakpoint or stepped into our target depth?
    fn should_pause(&self, source: &SourceInfo) -> bool {
        if self.terminating {
            return false;
        }
        if let Some(target) = self.step_target {
            if self.depth <= target {
                return true;
            }
        }
        let state = match self.wiring.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        state
            .breakpoints
            .contains(&(source.source.to_string(), source.start_line))
    }

    /// Build the pause snapshot, store it on shared state, emit a
    /// `Stopped` event, and block waiting for the next step
    /// command from the DAP server.
    fn pause(&mut self, source: &SourceInfo, locals: Vec<(SmolStr, String)>) {
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
        self.write_event(seq, "stopped", StoppedEventBody {
            reason,
            thread_id: Some(MAIN_THREAD_ID),
            all_threads_stopped: true,
            text: None,
        });
        // Block until told to resume.
        self.step_target = None;
        let command = self.wiring.commands_rx.recv().unwrap_or(DapCommand::Terminate);
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
    fn before_eval(&mut self, source: &SourceInfo) {
        if !self.should_pause(source) {
            return;
        }
        // Locals snapshot: the hook can't reach into
        // `VariableContext` directly (we don't have a reference
        // here), so for MVP we leave the locals list empty. The
        // server's `variables` response will return an empty
        // collection — sufficient for the gutter-breakpoint UX,
        // and the "expose locals" follow-up is small and isolated
        // (extending `EvalHooks::before_eval` to take a
        // `&VariableContext` parameter, which monomorphizes away
        // for `NoOpHooks`).
        let locals: Vec<(SmolStr, String)> = Vec::new();
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
        self.write_event(seq, "output", OutputEventBody {
            category: "stdout".to_string(),
            output: msg.to_string(),
        });
    }
}
