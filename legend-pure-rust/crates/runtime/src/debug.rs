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

//! Debugger-side data types shared by every embedder that drives the
//! [`Evaluator`](crate::eval::Evaluator) with pause/step semantics.
//!
//! The runtime itself does not own a debug session. The DAP server in
//! `crates/dap/` consumes these types via a custom
//! [`EvalHooks`](crate::hooks::EvalHooks) implementation, and any
//! future embedder (REPL with breakpoints, Web playground, etc.) can
//! reuse them rather than reinventing the small surface.
//!
//! Keeping these types in `runtime` instead of `crates/dap/` does
//! three things:
//!   * stops the runtime crate from accreting parking primitives — the
//!     `DebugBackbone` trait is purely policy, the parking lives in
//!     the consumer.
//!   * keeps the surface consumed by the eval loop tiny and stable.
//!     `Evaluator` itself never imports from this module today; the
//!     types are intended for the embedder's `EvalHooks` impl.
//!   * gives non-DAP consumers (a future tracing REPL, batch
//!     coverage tool, etc.) something to plug into.

use crate::value::Value;
use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

/// What the debugger wants the eval loop to do after a pause point.
///
/// Returned by [`DebugBackbone::on_pause`]. The DAP-side hooks impl
/// translates DAP client commands (`continue`, `next`, …) into one
/// of these and hands it back to the evaluator's hook entry — there
/// is no other handshake required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    /// Resume normal execution until the next breakpoint hit.
    Continue,
    /// Stop again the next time the evaluator reaches an expression
    /// at a call depth `<=` the captured `target_depth`. This is the
    /// "step over" semantic: child function calls run to completion
    /// invisibly, then we pause one expression later in the same or
    /// shallower frame.
    StepOver {
        /// The depth captured at pause time (typically
        /// [`crate::context::VariableContext::depth`]). The next
        /// `should_pause` check compares current depth against this.
        target_depth: usize,
    },
}

/// One frame of the live call stack as seen from a pause point.
///
/// `name` is the function FQN as the runtime knows it
/// (`enter_function` is where the hook impl captures it). `source`
/// is the call-site source info. `depth` is the scope depth
/// (matching [`crate::context::VariableContext::depth`]) at function
/// entry — used by [`StepMode::StepOver`] to compare against the
/// "should I pause again here?" question.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Function FQN, e.g. `meta::pure::functions::math::pow`.
    pub name: SmolStr,
    /// Source position of the function entry the runtime reported
    /// via [`crate::hooks::EvalHooks::enter_function`].
    pub source: SourceInfo,
    /// Variable-context depth at function entry.
    pub depth: usize,
}

/// Pluggable policy for "should we pause here?" + "what does the
/// debugger want next?".
///
/// Consumed by the DAP server's [`EvalHooks`](crate::hooks::EvalHooks)
/// impl, not by the evaluator directly. The split lets the runtime
/// stay free of parking primitives (Condvars, channels) — those live
/// in the consumer alongside the DAP transport. The two methods are
/// called from inside the hook's `before_eval` and the consumer
/// implementation parks the eval thread between them as required.
///
/// `Send` because both DAP and the REPL host their session state on
/// a different thread from the eval loop; the trait object travels
/// across the `spawn_blocking` boundary.
pub trait DebugBackbone: Send {
    /// Cheap pre-check: is the current expression on a line the user
    /// has set a breakpoint on, or has step-over caught up to the
    /// target depth?
    ///
    /// Returning `true` is a commitment to pause — the eval loop's
    /// hook impl will call [`on_pause`](Self::on_pause) next.
    /// Returning `false` skips both work and the snapshot construction.
    fn should_pause(&self, source: &SourceInfo, depth: usize) -> bool;

    /// Notify the backbone that the eval loop is paused, hand it the
    /// snapshot of the paused state, and receive the next instruction.
    ///
    /// `frames` is innermost-last; `locals` is a flat snapshot of
    /// [`crate::context::VariableContext::iter_bindings`] taken at
    /// the same instant. The backbone is responsible for blocking
    /// internally until the DAP client commands a step or continue
    /// — by the time this method returns the evaluator resumes.
    fn on_pause(&mut self, frames: &[Frame], locals: &[(SmolStr, Value)]) -> StepMode;
}
