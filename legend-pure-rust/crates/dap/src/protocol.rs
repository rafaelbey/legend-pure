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

#![allow(missing_docs)]
// protocol DTOs — DAP spec is the contract
// DTO fields/variants are populated by serde `Deserialize` and exist for
// protocol completeness; rustc's dead_code lint can't see serde usage.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Generic protocol envelope dispatched by `type`. Requests are
/// the only shape we act on — DAP defines reverse-direction
/// messages (`runInTerminal`, `startDebugging`) where the client
/// can also send responses or events; we accept those into the
/// enum and the server just drops them to keep the read loop
/// resilient against an over-talkative client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    Request(Request),
    /// Client-emitted response (reverse-request reply). Currently
    /// discarded — we don't issue reverse requests yet.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
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

/// `request_seq` is the one outlier in DAP — the spec defines it
/// as snake_case (historical from JSON-RPC) while every other
/// multi-word field uses camelCase. Marked explicitly so the
/// global `rename_all = "camelCase"` on neighbouring structs
/// doesn't accidentally rebrand it as `requestSeq`.
#[derive(Debug, Serialize)]
pub struct Response {
    pub seq: i64,
    #[serde(rename = "request_seq")]
    pub request_seq: i64,
    pub success: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct Event {
    pub seq: i64,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// DAP request/response/event bodies use camelCase on the wire
/// for every multi-word field except `Response.request_seq` (see
/// the explicit override on `Response`). The `rename_all` attribute
/// flips serde's default Rust-snake-case → JSON-camelCase mapping
/// without forcing per-field `#[serde(rename)]` annotations
/// everywhere. `clientID` / `adapterID` on `InitializeArguments`
/// are the only fields where the DAP spec uses caps-style
/// (`clientID` rather than `clientId`); some clients accept both
/// — we add explicit `serde(alias)` for tolerance.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeArguments {
    #[serde(alias = "clientID")]
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    #[serde(alias = "adapterID")]
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
    ///
    /// `supports_configuration_done_request` is intentionally
    /// `false`: in the modern DAP handshake the client sends
    /// `setBreakpoints` after `launch` and waits for the server's
    /// breakpoint-verification responses before sending
    /// `configurationDone` to release execution. IntelliJ's lsp4j-
    /// debug client tracks the verification state internally and
    /// can stall here in ways that aren't visible from the
    /// outside. Saying we don't support the request flips the
    /// client into the legacy flow: it sends `setBreakpoints` in
    /// parallel with `launch`, the server starts running
    /// immediately, and the breakpoint set is consulted on each
    /// expression visit. The race window — eval running before
    /// the first `setBreakpoints` arrives — is a few ms on a fresh
    /// `legend dap` process; the function setup phase itself
    /// takes longer than the message ping, so the breakpoint set
    /// is populated well before user code executes.
    ///
    /// Step-in / step-out / conditional / function breakpoints
    /// stay off so clients don't surface those buttons as enabled
    /// until the runtime supports them.
    #[must_use]
    pub fn mvp() -> Self {
        Self {
            supports_configuration_done_request: false,
            supports_step_in: false,
            supports_step_out: false,
            supports_terminate_request: true,
            supports_function_breakpoints: false,
            supports_conditional_breakpoints: false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SetBreakpointsResponse {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Serialize)]
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
pub struct ThreadsResponse {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Serialize)]
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
    /// Pure type + multiplicity label (`Integer[1]`,
    /// `abc::Class1[1]`). Populates the IDE's Type column. Omitted
    /// from the wire when the renderer could not recover a stable
    /// label (rare).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// `0` if the variable is a leaf with no expandable children;
    /// otherwise the DAP child reference for the `variables(ref)`
    /// follow-up request.
    pub variables_reference: i64,
    /// Named child count for objects / maps. Surfaced to the IDE so
    /// it can show a hover count before the user expands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    /// Indexed child count for collections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueResponse {
    /// DAP semantic: did `continue` affect every thread? Single-
    /// threaded so trivially `true`.
    pub all_threads_continued: bool,
}

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
pub struct OutputEventBody {
    pub category: String,
    pub output: String,
}

#[derive(Debug, Serialize)]
pub struct TerminatedEventBody {
    /// `true` if the client should restart the launch automatically.
    /// We never set this; the user re-clicks Debug to retry.
    #[serde(default)]
    pub restart: bool,
}
