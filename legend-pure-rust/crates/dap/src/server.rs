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

//! DAP message loop.
//!
//! Single thread reads `Content-Length`-framed JSON from stdin,
//! dispatches each request to a handler, writes the response (and
//! any synthesised events) back to stdout. On `launch` it spawns a
//! second thread that owns the [`Evaluator`] driven by
//! [`DapHooks`](crate::hooks::DapHooks).

use crate::config::DapConfig;
use crate::error::DapError;
use crate::hooks::DapHooks;
use crate::protocol::{
    Breakpoint, Capabilities, ClientMessage, ContinueResponse, Event,
    InitializeArguments, LaunchArguments, Request, Response, ScopesArguments,
    Scope, ScopesResponse, ServerMessage, SetBreakpointsArguments, SetBreakpointsResponse,
    Source, StackFrame, StackTraceArguments, StackTraceResponse, TerminatedEventBody, Thread,
    ThreadsResponse, Variable, VariablesArguments, VariablesResponse,
};
use crate::session::{
    DapCommand, HookWiring, Outbound, SessionState, MAIN_THREAD_ID,
};
use crate::transport::{read_frame, write_frame};
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use std::collections::HashSet;
use std::io::BufReader;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Run the DAP server over stdio until the client disconnects or
/// sends `disconnect`/`terminate`.
///
/// The function compiles the workspace eagerly so the first
/// `launch` doesn't pay the compile cost twice — same trade-off as
/// the LSP's startup behaviour.
///
/// # Errors
/// Returns [`DapError::Io`] for stdio failures and
/// [`DapError::Parse`] for malformed client frames. A clean
/// disconnect returns `Ok(())`.
pub fn run(config: DapConfig) -> Result<(), DapError> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let outbound: Outbound = Arc::new(Mutex::new(Box::new(std::io::stdout())));
    let state = Arc::new(Mutex::new(SessionState {
        breakpoints: HashSet::new(),
        paused: None,
        seq: 0,
    }));
    // Compile once at startup. Re-running `launch` after a hot edit
    // is deferred — the IDE workflow restarts the DAP server when
    // the source changes.
    let model = compile_workspace(&config);

    // Channel: server → eval thread. Created lazily on `launch`.
    let mut commands_tx: Option<Sender<DapCommand>> = None;

    loop {
        let body = match read_frame(&mut reader) {
            Ok(b) => b,
            Err(DapError::Disconnected) => return Ok(()),
            Err(e) => return Err(e),
        };
        let message: ClientMessage = serde_json::from_slice(&body)?;
        let ClientMessage::Request(req) = message;
        dispatch(&req, &model, &config, &state, &outbound, &mut commands_tx);
        if req.command == "disconnect" || req.command == "terminate" {
            // Signal the eval thread (if any) to wind down. The
            // hook's `Terminate` arm flips its abort flag, so the
            // outer loop ends naturally once the function returns.
            if let Some(tx) = commands_tx.as_ref() {
                let _ = tx.send(DapCommand::Terminate);
            }
            return Ok(());
        }
    }
}

fn dispatch(
    req: &Request,
    model: &Arc<PureModel>,
    config: &DapConfig,
    state: &Arc<Mutex<SessionState>>,
    outbound: &Outbound,
    commands_tx: &mut Option<Sender<DapCommand>>,
) {
    tracing::info!(command = %req.command, seq = req.seq, "DAP request");
    let result: Result<Option<serde_json::Value>, String> = match req.command.as_str() {
        "initialize" => handle_initialize(req).map(Some),
        "launch" => handle_launch(req, model, config, state, outbound, commands_tx).map(|_| None),
        "configurationDone" => Ok(None),
        "setBreakpoints" => handle_set_breakpoints(req, state).map(Some),
        "threads" => Ok(Some(threads_response())),
        "stackTrace" => handle_stack_trace(req, state).map(Some),
        "scopes" => handle_scopes(req).map(Some),
        "variables" => handle_variables(req, state).map(Some),
        "continue" => handle_continue(req, commands_tx),
        "next" => handle_next(req, commands_tx),
        "disconnect" | "terminate" => Ok(None),
        _ => Err(format!("unsupported command: {}", req.command)),
    };
    send_response(req, state, outbound, result);
}

fn handle_initialize(req: &Request) -> Result<serde_json::Value, String> {
    let _args: InitializeArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("initialize args: {e}"))?;
    let caps = Capabilities::mvp();
    Ok(serde_json::to_value(&caps).map_err(|e| e.to_string())?)
}

fn handle_set_breakpoints(
    req: &Request,
    state: &Arc<Mutex<SessionState>>,
) -> Result<serde_json::Value, String> {
    let args: SetBreakpointsArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("setBreakpoints args: {e}"))?;
    let path = args.source.path.clone().unwrap_or_default();
    let mut returned = Vec::with_capacity(args.breakpoints.len());
    {
        let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
        // Clear existing breakpoints for this source — DAP's
        // `setBreakpoints` is total-replace per file.
        s.breakpoints.retain(|(p, _)| p != &path);
        for bp in &args.breakpoints {
            let line = u32::try_from(bp.line).unwrap_or(0);
            if line > 0 {
                s.breakpoints.insert((path.clone(), line));
            }
            returned.push(Breakpoint {
                verified: true,
                line: bp.line,
                source: Some(args.source.clone()),
            });
        }
    }
    let body = SetBreakpointsResponse { breakpoints: returned };
    serde_json::to_value(&body).map_err(|e| e.to_string())
}

fn threads_response() -> serde_json::Value {
    serde_json::to_value(&ThreadsResponse {
        threads: vec![Thread {
            id: MAIN_THREAD_ID,
            name: "main".to_string(),
        }],
    })
    .unwrap_or(serde_json::Value::Null)
}

fn handle_stack_trace(
    req: &Request,
    state: &Arc<Mutex<SessionState>>,
) -> Result<serde_json::Value, String> {
    let _args: StackTraceArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("stackTrace args: {e}"))?;
    let s = state.lock().unwrap_or_else(|p| p.into_inner());
    let frames = match s.paused.as_ref() {
        Some(p) => p.frames.clone(),
        None => Vec::new(),
    };
    let stack_frames: Vec<StackFrame> = frames
        .iter()
        .rev() // DAP wants frame 0 = innermost
        .enumerate()
        .map(|(idx, f)| StackFrame {
            id: i64::try_from(idx).unwrap_or(0),
            name: f.name.to_string(),
            source: Some(Source {
                path: Some(f.source.source.to_string()),
                name: None,
            }),
            line: i64::from(f.source.start_line),
            column: i64::from(f.source.start_column),
        })
        .collect();
    let total = stack_frames.len() as i64;
    serde_json::to_value(&StackTraceResponse {
        stack_frames,
        total_frames: total,
    })
    .map_err(|e| e.to_string())
}

fn handle_scopes(req: &Request) -> Result<serde_json::Value, String> {
    let args: ScopesArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("scopes args: {e}"))?;
    // Single Locals scope per frame. The `variablesReference` is
    // synthesised from the frame_id; the variables handler decodes
    // it the same way.
    let scopes = vec![Scope {
        name: "Locals".to_string(),
        variables_reference: args.frame_id + 1,
        expensive: false,
    }];
    serde_json::to_value(&ScopesResponse { scopes }).map_err(|e| e.to_string())
}

fn handle_variables(
    req: &Request,
    state: &Arc<Mutex<SessionState>>,
) -> Result<serde_json::Value, String> {
    let _args: VariablesArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("variables args: {e}"))?;
    let s = state.lock().unwrap_or_else(|p| p.into_inner());
    let vars: Vec<Variable> = s
        .paused
        .as_ref()
        .map(|p| {
            p.locals
                .iter()
                .map(|(name, rendered)| Variable {
                    name: name.to_string(),
                    value: rendered.clone(),
                    variables_reference: 0,
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::to_value(&VariablesResponse { variables: vars }).map_err(|e| e.to_string())
}

fn handle_continue(
    _req: &Request,
    commands_tx: &mut Option<Sender<DapCommand>>,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(tx) = commands_tx.as_ref() {
        let _ = tx.send(DapCommand::Continue);
    }
    Ok(Some(
        serde_json::to_value(&ContinueResponse {
            all_threads_continued: true,
        })
        .map_err(|e| e.to_string())?,
    ))
}

fn handle_next(
    _req: &Request,
    commands_tx: &mut Option<Sender<DapCommand>>,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(tx) = commands_tx.as_ref() {
        let _ = tx.send(DapCommand::Next);
    }
    Ok(None)
}

fn handle_launch(
    req: &Request,
    model: &Arc<PureModel>,
    _config: &DapConfig,
    state: &Arc<Mutex<SessionState>>,
    outbound: &Outbound,
    commands_tx: &mut Option<Sender<DapCommand>>,
) -> Result<(), String> {
    let args: LaunchArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("launch args: {e}"))?;
    let (tx, rx) = channel::<DapCommand>();
    *commands_tx = Some(tx);
    let wiring = HookWiring {
        state: state.clone(),
        commands_rx: rx,
        outbound: outbound.clone(),
    };
    let model_arc = model.clone();
    let outbound_term = outbound.clone();
    let state_term = state.clone();
    std::thread::Builder::new()
        .name("legend-dap-eval".to_string())
        .spawn(move || {
            let registry = NativeRegistry::standard();
            let hooks = DapHooks::new(wiring);
            let mut evaluator = Evaluator::with_hooks(
                model_arc.as_ref(),
                &registry,
                hooks,
            );
            let outcome = evaluator.call(&args.program, &[]);
            tracing::info!(
                fqn = %args.program,
                ok = outcome.is_ok(),
                "DAP launch eval complete",
            );
            // Emit Terminated so the IDE clears the debug session.
            let seq = {
                let mut s = state_term.lock().unwrap_or_else(|p| p.into_inner());
                s.next_seq()
            };
            let msg = ServerMessage::Event(Event {
                seq,
                event: "terminated".to_string(),
                body: Some(
                    serde_json::to_value(&TerminatedEventBody { restart: false })
                        .unwrap_or(serde_json::Value::Null),
                ),
            });
            if let Ok(payload) = serde_json::to_vec(&msg) {
                let mut out = outbound_term.lock().unwrap_or_else(|p| p.into_inner());
                let _ = write_frame(&mut *out, &payload);
            }
        })
        .map_err(|e| format!("spawn eval thread: {e}"))?;
    Ok(())
}

/// Build the response envelope and emit it through the outbound
/// writer. `result` distinguishes three cases:
///   * `Ok(Some(body))` — successful response with a body.
///   * `Ok(None)`         — successful response, no body (e.g. `next`).
///   * `Err(message)`     — failed response with an error message.
fn send_response(
    req: &Request,
    state: &Arc<Mutex<SessionState>>,
    outbound: &Outbound,
    result: Result<Option<serde_json::Value>, String>,
) {
    let seq = {
        let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
        s.next_seq()
    };
    let (success, body, message) = match result {
        Ok(body) => (true, body, None),
        Err(e) => (false, None, Some(e)),
    };
    let resp = ServerMessage::Response(Response {
        seq,
        request_seq: req.seq,
        success,
        command: req.command.clone(),
        message,
        body,
    });
    if let Ok(payload) = serde_json::to_vec(&resp) {
        let mut out = outbound.lock().unwrap_or_else(|p| p.into_inner());
        let _ = write_frame(&mut *out, &payload);
    }
}

/// Render a [`Value`] for a `Variable.value` field. Reserved for
/// future use when `before_eval` is extended to take a
/// `&VariableContext` parameter — at that point the hook can call
/// this on the eval thread (where `Value`'s non-`Send`-ness is
/// fine) before storing rendered strings in the snapshot. Kept here
/// rather than inline in `hooks.rs` so the rendering policy stays
/// adjacent to the consumer that reads it back.
#[allow(dead_code)]
pub(crate) fn render_value(v: &Value) -> String {
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

/// Compile the configured workspace into a `PureModel`. Mirrors the
/// LSP's `Workspace::compile` but without buffer-overlay logic —
/// the DAP server doesn't track open buffers; the IDE restarts the
/// debug session when files change.
fn compile_workspace(config: &DapConfig) -> Arc<PureModel> {
    use legend_pure_dsl_mapping::compiler::MappingExtension;
    use legend_pure_dsl_mapping::parser::MappingSectionParser;
    use legend_pure_dsl_relational::compiler::RelationalExtension;
    use legend_pure_dsl_relational::parser::RelationalSectionParser;
    use legend_pure_dsl_store::compiler::RelationStoreExtension;
    use legend_pure_parser_parser::SectionParser;

    let mapping = MappingExtension::new();
    let relational = RelationalExtension::new();
    let store = RelationStoreExtension::new();
    let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];
    let result = legend_pure_core_platform::repo::load_with_extensions(
        &config.repos,
        &config.auto_imports,
        &extensions,
        &mut || -> Vec<Box<dyn SectionParser>> {
            vec![
                Box::new(MappingSectionParser::new()),
                Box::new(RelationalSectionParser),
            ]
        },
    );
    let (model, errs) = match result {
        Ok(m) => (m, Vec::<CompilationError>::new()),
        Err(partial) => (partial.model, partial.errors),
    };
    if !errs.is_empty() {
        tracing::warn!(error_count = errs.len(), "DAP server compiled with errors");
    }
    Arc::new(model)
}

