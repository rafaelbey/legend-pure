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

//! Per-launch session state shared between the DAP server thread and
//! the spawned evaluator thread.
//!
//! Architecture: a single `Arc<Mutex<SessionState>>` carries every
//! piece of state both sides need to touch. The eval thread parks
//! itself by blocking on a `std::sync::mpsc::Receiver<DapCommand>`
//! inside the [`DapHooks`](crate::hooks::DapHooks) implementation; the
//! server thread pushes the next step decision onto that channel when
//! the DAP client requests `next` / `continue`.
//!
//! Outbound events (`stopped`, `output`, `terminated`) travel from
//! the eval thread to the client via a shared serializer — see
//! [`Outbound`] — so the hook can emit a frame without crossing
//! through the server's message loop.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_runtime::debug::StepMode;
use smol_str::SmolStr;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Synthetic single-thread id used in `threads` / `stackTrace`
/// responses. DAP requires a thread id but Pure runs single-
/// threaded today.
pub const MAIN_THREAD_ID: i64 = 1;

/// State shared between the server's message loop and the
/// eval-thread hooks.
pub struct SessionState {
    /// Active breakpoints keyed by `(canonical source path, 1-based line)`.
    pub breakpoints: HashSet<(String, u32)>,
    /// Populated by the hook on pause; read by `stackTrace` /
    /// `scopes` / `variables`. `None` while the evaluator is running
    /// or before any pause has occurred.
    pub paused: Option<PauseSnapshot>,
    /// Outbound message sequence number — both responses and events
    /// share the same numbering per the DAP spec.
    pub seq: i64,
}

impl SessionState {
    /// Allocate a fresh outbound sequence number.
    pub fn next_seq(&mut self) -> i64 {
        self.seq += 1;
        self.seq
    }
}

/// Snapshot taken at a pause point. The DAP server's request
/// handlers read from this; they never reach into the live evaluator.
///
/// `Value` is intentionally not stored here — the runtime's
/// `Value` carries `Rc<RefCell<…>>` heap handles and is therefore
/// not `Send`, which would prevent us from sharing the snapshot
/// across the eval thread / server thread boundary. The hook
/// renders values to display strings on the eval thread at pause
/// time and stores the rendered text only.
#[derive(Debug, Clone)]
pub struct PauseSnapshot {
    /// Captured call stack as `(function_name, source_path, line,
    /// column)` rows. Innermost-last; the DAP `stackTrace` handler
    /// reverses on render.
    pub frames: Vec<FrameInfo>,
    /// Flat snapshot of locals rendered to display strings at pause
    /// time. The same list serves every `variables`-on-Locals
    /// request until the eval thread resumes.
    pub locals: Vec<(SmolStr, String)>,
    /// Source position the eval loop was about to evaluate when it
    /// hit the breakpoint.
    pub source: SourceInfo,
}

/// Plain-data frame for the `stackTrace` response — same fields as
/// [`legend_pure_runtime::debug::Frame`] but with the function name
/// captured as `String` so the snapshot type is fully owned.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub name: String,
    pub source: SourceInfo,
    pub depth: usize,
}

/// Stable handle the server hands to the eval-thread machinery so
/// it can serialize outbound `Stopped` / `Output` / `Terminated`
/// frames without contending with the message loop for stdout
/// ownership.
///
/// Behind the scenes this is a `Mutex<Box<dyn Write>>` — the
/// concrete writer is `std::io::stdout()` in production and a
/// `Vec<u8>` in tests.
pub type Outbound = Arc<Mutex<Box<dyn std::io::Write + Send>>>;

/// Wire-up bundle threaded into [`DapHooks`]: the shared state and
/// the parking channel receive end.
pub struct HookWiring {
    pub state: Arc<Mutex<SessionState>>,
    pub commands_rx: std::sync::mpsc::Receiver<DapCommand>,
    pub outbound: Outbound,
}

/// Command pushed by the DAP server to the eval thread when the
/// client wants to step or continue.
#[derive(Debug, Clone, Copy)]
pub enum DapCommand {
    Continue,
    Next,
    /// Soft termination — the eval thread should return from the
    /// hook as if continuing, and the eval loop will check the
    /// abort flag on its next entry.
    Terminate,
}

impl DapCommand {
    /// Translate the command into a runtime [`StepMode`] given the
    /// depth captured at pause time. `Next` becomes `StepOver` so the
    /// hook fires again at the same depth or shallower.
    pub fn to_step_mode(self, paused_depth: usize) -> StepMode {
        match self {
            DapCommand::Continue | DapCommand::Terminate => StepMode::Continue,
            DapCommand::Next => StepMode::StepOver {
                target_depth: paused_depth,
            },
        }
    }
}
