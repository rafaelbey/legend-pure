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
    Source, StackFrame, StackTraceArguments, StackTraceResponse, Thread,
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
    let path_resolver = build_path_resolver(&config);
    let state = Arc::new(Mutex::new(SessionState {
        breakpoints: HashSet::new(),
        paused: None,
        seq: 0,
        path_resolver,
    }));
    // Compile once at startup. Re-running `launch` after a hot edit
    // is deferred — the IDE workflow restarts the DAP server when
    // the source changes.
    let model = compile_workspace(&config);

    // Channel: server → eval thread. Created lazily on
    // `configurationDone` (the DAP sequence is: launch ack →
    // setBreakpoints → configurationDone → only THEN start
    // running). `pending_launch` holds the `launch`-time
    // arguments until configurationDone fires the thread.
    let mut commands_tx: Option<Sender<DapCommand>> = None;
    let mut pending_launch: Option<LaunchArguments> = None;

    loop {
        let body = match read_frame(&mut reader) {
            Ok(b) => b,
            Err(DapError::Disconnected) => return Ok(()),
            Err(e) => return Err(e),
        };
        let message: ClientMessage = serde_json::from_slice(&body)?;
        let req = match message {
            ClientMessage::Request(r) => r,
            ClientMessage::Other => {
                tracing::debug!(
                    "ignored non-request client message: {}",
                    String::from_utf8_lossy(&body).chars().take(120).collect::<String>(),
                );
                continue;
            }
        };
        dispatch(
            &req,
            &model,
            &config,
            &state,
            &outbound,
            &mut commands_tx,
            &mut pending_launch,
        );
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
    _config: &DapConfig,
    state: &Arc<Mutex<SessionState>>,
    outbound: &Outbound,
    commands_tx: &mut Option<Sender<DapCommand>>,
    pending_launch: &mut Option<LaunchArguments>,
) {
    tracing::info!(command = %req.command, seq = req.seq, "DAP request");
    let result: Result<Option<serde_json::Value>, String> = match req.command.as_str() {
        "initialize" => handle_initialize(req).map(Some),
        // Legacy DAP flow: start the eval thread right after
        // `launch` ACK. We don't advertise
        // `supportsConfigurationDoneRequest` (see
        // `Capabilities::mvp` rationale), so the client sends
        // `setBreakpoints` in parallel with `launch` and we don't
        // wait. `configurationDone` is still accepted as a no-op
        // for clients that send it anyway.
        "launch" => handle_launch(
            req, model, state, outbound, commands_tx, pending_launch,
        )
        .map(|_| None),
        "configurationDone" => Ok(None),
        "setBreakpoints" => handle_set_breakpoints(req, state).map(Some),
        // Setting exception breakpoints is a no-op for now — the
        // runtime doesn't emit Exception events, so the
        // `<<test::Test>>` failure pathway never triggers a
        // breakpoint stop. We accept the request silently so the
        // configuration-sequence handshake completes.
        "setExceptionBreakpoints" => Ok(Some(serde_json::json!({ "breakpoints": [] }))),
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
    // Per the DAP spec, the server emits an `initialized` event
    // after the `initialize` response — *not* as part of the
    // response itself. The event signals to the client that the
    // server is ready to accept configuration requests
    // (`setBreakpoints`, `setExceptionBreakpoints`,
    // `configurationDone`). IntelliJ's `DapDebugSessionImpl` waits
    // for it before considering the session initialized; without
    // it, the 10s `dap.timeout.initialize` registry key fires and
    // the session aborts.
    if req.command == "initialize" {
        send_initialized_event(state, outbound);
    }
}

fn send_initialized_event(state: &Arc<Mutex<SessionState>>, outbound: &Outbound) {
    let seq = {
        let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
        s.next_seq()
    };
    let event = ServerMessage::Event(Event {
        seq,
        event: "initialized".to_string(),
        body: None,
    });
    if let Ok(payload) = serde_json::to_vec(&event) {
        let mut out = outbound.lock().unwrap_or_else(|p| p.into_inner());
        let _ = write_frame(&mut *out, &payload);
    }
    tracing::info!("DAP `initialized` event sent");
}

fn handle_initialize(req: &Request) -> Result<serde_json::Value, String> {
    let _args: InitializeArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("initialize args: {e}"))?;
    let caps = Capabilities::mvp();
    serde_json::to_value(&caps).map_err(|e| e.to_string())
}

fn handle_set_breakpoints(
    req: &Request,
    state: &Arc<Mutex<SessionState>>,
) -> Result<serde_json::Value, String> {
    let args: SetBreakpointsArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("setBreakpoints args: {e}"))?;
    let path = args.source.path.clone().unwrap_or_default();
    let lines: Vec<i64> = args.breakpoints.iter().map(|bp| bp.line).collect();
    tracing::info!(path = %path, lines = ?lines, "setBreakpoints stored");
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
    let Some(snap) = s.paused.as_ref() else {
        return serde_json::to_value(&StackTraceResponse {
            stack_frames: Vec::new(),
            total_frames: 0,
        })
        .map_err(|e| e.to_string());
    };
    let frames = &snap.frames;
    // DAP wants frame 0 = innermost. The innermost (paused)
    // frame's source/line is the *expression* we just paused at,
    // not the function-entry location stored in `FrameInfo`. The
    // IDE uses frame 0's location to scroll/highlight the editor
    // — without this override it focuses on the function
    // declaration row instead of where execution actually
    // stopped.
    let stack_frames: Vec<StackFrame> = frames
        .iter()
        .rev()
        .enumerate()
        .map(|(idx, f)| {
            let (canonical_src, line, col) = if idx == 0 {
                (
                    snap.source.source.to_string(),
                    i64::from(snap.source.start_line),
                    i64::from(snap.source.start_column),
                )
            } else {
                (
                    f.source.source.to_string(),
                    i64::from(f.source.start_line),
                    i64::from(f.source.start_column),
                )
            };
            let absolute_src = s.resolve_source_path(&canonical_src);
            StackFrame {
                id: i64::try_from(idx).unwrap_or(0),
                name: f.name.to_string(),
                source: Some(Source {
                    path: Some(absolute_src),
                    name: None,
                }),
                line,
                column: col,
            }
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

/// Start the eval thread.
///
/// Legacy DAP flow: ack `launch` and spawn the eval immediately.
/// `setBreakpoints` arrives in parallel and races for the
/// breakpoint set; in practice it lands well before user code
/// executes since `Evaluator::call` does FQN lookup + arg
/// preparation before entering the first expression.
///
/// `pending_launch` is kept on the function signature for parity
/// with the configurationDone-aware variant — we just consume the
/// args inline.
fn handle_launch(
    req: &Request,
    model: &Arc<PureModel>,
    state: &Arc<Mutex<SessionState>>,
    outbound: &Outbound,
    commands_tx: &mut Option<Sender<DapCommand>>,
    pending_launch: &mut Option<LaunchArguments>,
) -> Result<(), String> {
    let args: LaunchArguments = serde_json::from_value(req.arguments.clone())
        .map_err(|e| format!("launch args: {e}"))?;
    tracing::info!(fqn = %args.program, "launch — starting eval thread");
    *pending_launch = None;
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
    if let Err(e) = std::thread::Builder::new()
        .name("legend-dap-eval".to_string())
        .spawn(move || {
            // Small startup delay so `setBreakpoints` arriving in
            // parallel with `launch` can populate the breakpoint
            // set before user-code expressions start running. The
            // IDE typically sends `setBreakpoints` within ~1ms of
            // `launch`; 200ms is a comfortable safety margin
            // (and unnoticeable to a human clicking Debug). Once
            // the runtime gains a configurationDone-aware
            // pause-on-entry handshake, this sleep goes away.
            std::thread::sleep(std::time::Duration::from_millis(200));

            let registry = NativeRegistry::standard();
            let hooks = DapHooks::new(wiring);
            let mut evaluator = Evaluator::with_hooks(
                model_arc.as_ref(),
                &registry,
                hooks,
            );
            let outcome = evaluator.call(&args.program, &[]);
            match &outcome {
                Ok(_) => {
                    tracing::info!(fqn = %args.program, "DAP launch eval complete (ok)");
                }
                Err(e) => {
                    // Log the full exception text — without this
                    // an immediate eval failure (function not
                    // found, type error during dispatch, etc.)
                    // looks identical to a slow run that exits
                    // cleanly. Surface enough context to debug
                    // from idea.log alone.
                    tracing::warn!(
                        fqn = %args.program,
                        error = %e,
                        "DAP launch eval failed",
                    );
                }
            }
            if let Err(ref e) = outcome {
                // Surface eval errors as a stderr-category Output
                // event so the user sees them in the debug
                // console. Without this the IDE silently closes
                // the session with no indication of why.
                let seq = {
                    let mut s = state_term.lock().unwrap_or_else(|p| p.into_inner());
                    s.next_seq()
                };
                let body = crate::protocol::OutputEventBody {
                    category: "stderr".to_string(),
                    output: format!("{e}\n"),
                };
                let msg = ServerMessage::Event(Event {
                    seq,
                    event: "output".to_string(),
                    body: Some(
                        serde_json::to_value(&body).unwrap_or(serde_json::Value::Null),
                    ),
                });
                if let Ok(payload) = serde_json::to_vec(&msg) {
                    let mut out = outbound_term.lock().unwrap_or_else(|p| p.into_inner());
                    let _ = write_frame(&mut *out, &payload);
                }
            }
            // Emit `exited` first — DAP's "the debuggee has
            // ended" event — followed by `terminated`, which is
            // "the whole debug session is over". IntelliJ
            // releases the session and sends `disconnect` back
            // when it processes `terminated`. We don't process-
            // exit here; the main loop's `disconnect` arm exits
            // cleanly when the request arrives.
            let exit_seq = {
                let mut s = state_term.lock().unwrap_or_else(|p| p.into_inner());
                s.next_seq()
            };
            let exited = ServerMessage::Event(Event {
                seq: exit_seq,
                event: "exited".to_string(),
                body: Some(serde_json::json!({ "exitCode": 0 })),
            });
            if let Ok(payload) = serde_json::to_vec(&exited) {
                let mut out = outbound_term.lock().unwrap_or_else(|p| p.into_inner());
                let _ = write_frame(&mut *out, &payload);
            }
            let term_seq = {
                let mut s = state_term.lock().unwrap_or_else(|p| p.into_inner());
                s.next_seq()
            };
            let terminated = ServerMessage::Event(Event {
                seq: term_seq,
                event: "terminated".to_string(),
                body: None,
            });
            if let Ok(payload) = serde_json::to_vec(&terminated) {
                let mut out = outbound_term.lock().unwrap_or_else(|p| p.into_inner());
                let _ = write_frame(&mut *out, &payload);
            }
            // Safety net: if `disconnect` doesn't arrive within a
            // grace window, exit anyway so the process doesn't
            // linger. IntelliJ usually disconnects within a few
            // hundred ms of receiving `terminated`; 3s is well
            // beyond that. Skipping the timer entirely would let
            // a misbehaving client orphan the server.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(3));
                tracing::info!("DAP grace period elapsed, exiting");
                std::process::exit(0);
            });
        })
    {
        tracing::error!("spawn eval thread failed: {e}");
        return Err(format!("spawn eval thread: {e}"));
    }
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

/// Build the `(canonical prefix, absolute root)` table the
/// stack-trace handler uses to map runtime source paths back to
/// filesystem locations the IDE can navigate to. Only filesystem
/// repos contribute — embedded / Purem repos don't have an
/// addressable on-disk path, and the IDE wouldn't be able to
/// scroll into them anyway.
fn build_path_resolver(config: &DapConfig) -> Vec<(String, std::path::PathBuf)> {
    use legend_pure_core_platform::repo::Repo;
    let mut out = Vec::new();
    for repo in &config.repos {
        if let Repo::Filesystem {
            prefix,
            source_root: Some(root),
            ..
        } = repo
        {
            out.push((prefix.clone(), root.clone()));
        }
    }
    out
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
    // Log workspace shape so the DAP failure mode "function not
    // found in model" can be distinguished from "model didn't
    // include user code" at the source. Mirrors what the LSP
    // emits via `window/logMessage` on initialize.
    let repo_count = config.repos.len();
    tracing::info!(
        repo_count = repo_count,
        chunk_count = model.chunks.len(),
        "DAP workspace compiled",
    );
    Arc::new(model)
}

