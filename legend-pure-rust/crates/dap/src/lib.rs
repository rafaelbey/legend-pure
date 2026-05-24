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

//! Debug Adapter Protocol server for Legend Pure.
//!
//! Mirrors [`legend-pure-lsp`](../legend_pure_lsp/index.html) one
//! protocol layer over: the same `Repo` cascade compiles a fresh
//! `PureModel` at launch, and the runtime `Evaluator` runs the
//! target function with a `DapHooks`
//! instance instead of the LSP's `CapturingHooks`.
//!
//! Transport: stdio with `Content-Length:` framing, same as LSP.
//!
//! # Supported requests (MVP)
//!
//! `initialize`, `launch`, `setBreakpoints`, `configurationDone`,
//! `threads`, `stackTrace`, `scopes`, `variables`, `continue`,
//! `next`, `disconnect`, `terminate`.
//!
//! Step-in / step-out, evaluate-while-paused, conditional /
//! log breakpoints, and watches are intentionally deferred — they
//! require either a re-entrancy guard in the hook or extra
//! `StepMode` variants beyond
//! [`legend_pure_runtime::debug::StepMode`]'s
//! `Continue` + `StepOver`. See the plan at
//! `docs/deferred/` for the full backlog.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod config;
mod error;
mod hooks;
mod protocol;
mod server;
mod session;
mod transport;

pub use config::DapConfig;
pub use error::DapError;
pub use server::run;
