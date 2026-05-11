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

//! DAP message types — hand-rolled subset.
//!
//! The Debug Adapter Protocol defines ~50 request types and ~20 event
//! types in full. We model only the dozen-ish messages the MVP needs
//! (`initialize`, `launch`, `setBreakpoints`, `configurationDone`,
//! `threads`, `stackTrace`, `scopes`, `variables`, `continue`,
//! `next`, `disconnect`, `terminate`, plus the `initialized` /
//! `stopped` / `output` / `terminated` events). Each message uses
//! `#[serde(rename_all = "camelCase")]` to match the wire format.
//!
//! Wire envelope (DAP's `ProtocolMessage` base):
//! ```json
//! { "seq": 1, "type": "request", "command": "initialize",
//!   "arguments": { ... } }
//! ```
//!
//! Hand-rolling keeps the dependency footprint tight — the
//! `debug-types` crate would pull in the full protocol surface and
//! introduce another versioning axis. The MVP surface is small
//! enough to maintain inline; we'll re-evaluate when stretch features
//! land.

#![allow(missing_docs)] // protocol DTOs — DAP spec is the contract

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Generic protocol envelope dispatched by `type`. We only ever
/// receive `request` from the client; `response` and `event` are
/// emitted from the server side.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    Request(Request),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub seq: i64,
    pub command: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Outgoing message. `type` is serialized as a string discriminator.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    Response(Response),
    Event(Event),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub seq: i64,
    pub request_seq: i64,
    pub success: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub seq: i64,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeArguments {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub adapter_id: Option<String>,
    pub locale: Option<String>,
    pub lines_start_at_1: Option<bool>,
    pub columns_start_at_1: Option<bool>,
    pub path_format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchArguments {
    /// FQN of the parameterless Pure function to run.
    pub program: String,
    /// `true` = stop on entry (first expression). Defaults to false.
    #[serde(default)]
    pub stop_on_entry: bool,
    /// Optional `noDebug` flag — when true, runs to completion
    /// without honouring breakpoints. We implement it as a
    /// no-breakpoint launch for now (still routes through the same
    /// hooks, just with an empty breakpoint set).
    #[serde(default)]
    pub no_debug: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    pub source: Source,
    #[serde(default)]
    pub breakpoints: Vec<SourceBreakpoint>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub path: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    pub column: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceArguments {
    pub thread_id: i64,
    #[serde(default)]
    pub start_frame: i64,
    pub levels: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesArguments {
    pub frame_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesArguments {
    pub variables_reference: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueArguments {
    pub thread_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextArguments {
    pub thread_id: i64,
}

// ---------------------------------------------------------------------------
// Response bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_step_in: bool,
    pub supports_step_out: bool,
    pub supports_terminate_request: bool,
    pub supports_function_breakpoints: bool,
    pub supports_conditional_breakpoints: bool,
}

impl Capabilities {
    /// MVP capability set — see `crates/dap/src/lib.rs` rustdoc.
    /// Step-in / step-out / conditional / function breakpoints stay
    /// off so clients don't surface those buttons as enabled until
    /// the runtime supports them.
    #[must_use]
    pub fn mvp() -> Self {
        Self {
            supports_configuration_done_request: true,
            supports_step_in: false,
            supports_step_out: false,
            supports_terminate_request: true,
            supports_function_breakpoints: false,
            supports_conditional_breakpoints: false,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsResponse {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    /// `true` once the server has resolved the file+line to an
    /// actual expression node. We currently mark every requested
    /// breakpoint as verified — the DAP spec lets us defer
    /// verification, but for the MVP we accept all and let
    /// `should_pause` filter at runtime.
    pub verified: bool,
    pub line: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsResponse {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceResponse {
    pub stack_frames: Vec<StackFrame>,
    pub total_frames: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub source: Option<Source>,
    pub line: i64,
    pub column: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesResponse {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesResponse {
    pub variables: Vec<Variable>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    /// `0` if the variable doesn't have nested structure visible to
    /// the debugger. Nested expansion of `Value::Object` is a
    /// Phase 4 item.
    pub variables_reference: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueResponse {
    /// DAP semantic: did `continue` affect every thread? Single-
    /// threaded so trivially `true`.
    pub all_threads_continued: bool,
}

// ---------------------------------------------------------------------------
// Event bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEventBody {
    pub reason: String,
    pub thread_id: Option<i64>,
    pub all_threads_stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventBody {
    pub category: String,
    pub output: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminatedEventBody {
    /// `true` if the client should restart the launch automatically.
    /// We never set this; the user re-clicks Debug to retry.
    #[serde(default)]
    pub restart: bool,
}
