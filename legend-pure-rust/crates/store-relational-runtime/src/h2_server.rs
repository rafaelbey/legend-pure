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

//! Process-wide H2 sub-process management.
//!
//! H2 is a Java DB; the Rust port talks to it through H2's
//! PostgreSQL-wire compatibility mode. This module owns the spawn /
//! liveness handshake / port-handshake of a single child JVM and
//! exposes it to natives through [`ensure_started`].
//!
//! Why process-wide (not per-Evaluator): a JVM cold-start runs ~500ms
//! plus H2 init; per-Evaluator spawning would multiply test runtime by
//! evaluator count. Per-Evaluator isolation is still preserved
//! downstream — each [`crate::connection::H2State`] opens a unique
//! `mem:<uuid>` logical database against the shared server.
//!
//! Lifecycle: the Java child outlives the `H2Server` struct (stored
//! in a static `OnceLock`) and is reaped by the OS when the test
//! binary exits normally — no explicit shutdown call is made.
//!
//! **Parent-SIGKILL edge case.** When `cargo test` (or any other
//! parent) is SIGKILL'd rather than exiting normally, the H2 child
//! becomes a zombie. The standard Linux fix is
//! `Command::pre_exec(|| prctl(PR_SET_PDEATHSIG, SIGTERM))`, but
//! `pre_exec` requires `unsafe` and this crate has
//! `#![forbid(unsafe_code)]`. Rather than lift the crate-wide forbid
//! (a meaningful policy change) or pull in a transitive `nix` / `prctl`
//! crate dep for one syscall, the limitation is accepted: H2 zombies
//! after parent-SIGKILL are recovered manually with:
//!
//! ```bash
//! pkill -f h2.tools.Server   # one-shot cleanup
//! lsof -i :5435              # verify port released
//! ```
//!
//! In practice this only matters during local-dev iteration where the
//! user explicitly SIGKILLs `cargo test`; in CI the runner cleanup
//! handles the orphan and the next run gets a fresh port.

use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use legend_pure_runtime::error::{PureException, PureRuntimeError};
use thiserror::Error;

use crate::config::H2Config;

/// Live H2 sub-process plus the PG-wire port it's listening on.
///
/// Stored in a `OnceLock` so `ensure_started` is idempotent across
/// every relational native in the process. Drops are best-effort —
/// see the module docstring.
#[derive(Debug)]
pub struct H2Server {
    /// The Java child process. Held to keep ownership; the OS reaps
    /// it on test-binary exit.
    #[allow(dead_code)]
    child: Child,
    /// The PG-compat port `H2State::new` connects to.
    pub pg_port: u16,
}

/// Spawn / handshake failure modes.
#[derive(Debug, Error)]
pub enum H2SpawnError {
    /// Configured jar path doesn't exist.
    #[error("H2 jar not found at {0}")]
    JarMissing(PathBuf),
    /// `java` binary not on PATH or path mis-configured.
    #[error("failed to spawn `{cmd}`: {source}")]
    SpawnIo {
        /// Command tried.
        cmd: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The child died before the PG-wire port came up.
    #[error("H2 server exited during startup (status {status:?}): {first_line}")]
    DiedDuringStartup {
        /// Child exit status.
        status: Option<i32>,
        /// First stdout / stderr line, for diagnostics.
        first_line: String,
    },
    /// 5s elapsed without a TCP listener appearing on the PG port.
    #[error("H2 server didn't accept TCP on port {port} within {millis}ms")]
    PortNotReady {
        /// The PG port that never opened.
        port: u16,
        /// Elapsed budget.
        millis: u64,
    },
}

impl From<H2SpawnError> for PureException {
    fn from(value: H2SpawnError) -> Self {
        PureRuntimeError::EvaluationError(value.to_string()).into()
    }
}

/// Process-wide singleton. The first H2-routed native call spawns;
/// subsequent calls reuse the cached result (success or failure).
static H2_SERVER: OnceLock<Result<H2Server, H2SpawnError>> = OnceLock::new();

/// Ensure the H2 server is running, spawning it on first call.
///
/// `cfg` is consulted only on the first call — subsequent calls
/// return the cached server regardless of any later config changes.
/// Matches `OnceLock` semantics; document the constraint.
///
/// # Errors
/// Returns the cached spawn error if the first call failed, OR a
/// fresh spawn error from the first call.
pub fn ensure_started(cfg: &H2Config) -> Result<&'static H2Server, PureException> {
    let cached = H2_SERVER.get_or_init(|| spawn(cfg));
    match cached {
        Ok(server) => Ok(server),
        Err(e) => Err(PureRuntimeError::EvaluationError(e.to_string()).into()),
    }
}

fn spawn(cfg: &H2Config) -> Result<H2Server, H2SpawnError> {
    if !cfg.jar_path.is_file() {
        return Err(H2SpawnError::JarMissing(cfg.jar_path.clone()));
    }
    let cmd_display = format!(
        "{} -cp {} org.h2.tools.Server -pg -pgPort {}",
        cfg.java.display(),
        cfg.jar_path.display(),
        cfg.pg_port
    );
    // Flags:
    //   * `-pg` / `-pgPort`     — enable the PostgreSQL-wire server.
    //   * `-ifNotExists`        — let a `mem:eval_<uuid>` dbname be
    //                              created on first connect. Default
    //                              H2 rejects remote DB creation
    //                              ("Database not found, either
    //                              pre-create it or allow remote
    //                              database creation").
    //   * `-baseDir <tmp>`      — confine any accidental on-disk DBs
    //                              to a per-process temp dir so a
    //                              bare `dbname=foo` (no `mem:`)
    //                              doesn't write into the cwd.
    //
    // `-pgAllowOthers` is a presence-only flag in H2 v2 — passing
    // `false` errors `JdbcSQLFeatureNotSupportedException`. Omitting
    // it locks the server to local connections, which is what we
    // want.
    let base_dir = std::env::temp_dir().join(format!("legend-h2-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&base_dir);
    let mut child = Command::new(&cfg.java)
        .arg("-cp")
        .arg(&cfg.jar_path)
        .arg("org.h2.tools.Server")
        .arg("-pg")
        .arg("-pgPort")
        .arg(cfg.pg_port.to_string())
        .arg("-ifNotExists")
        .arg("-baseDir")
        .arg(&base_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| H2SpawnError::SpawnIo {
            cmd: cmd_display,
            source,
        })?;

    // Drain stdout in a background thread; H2 writes "PG server running
    // at pg://…" once it's ready. We don't strictly need that line —
    // the TCP-poll handshake is authoritative — but keeping the pipe
    // drained avoids the child blocking on a full stdout buffer.
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for _ in reader.lines() {
                // Intentionally discard; H2 is verbose.
            }
        });
    }

    wait_for_pg_ready(&mut child, cfg.pg_port)?;
    Ok(H2Server {
        child,
        pg_port: cfg.pg_port,
    })
}

/// Poll `127.0.0.1:port` for ~5s until a TCP connect succeeds, OR the
/// child dies, OR the budget elapses.
fn wait_for_pg_ready(child: &mut Child, port: u16) -> Result<(), H2SpawnError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sleep = Duration::from_millis(50);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            // Child died before the port came up. Drain stderr so the
            // error message carries some context.
            let first_line = child
                .stderr
                .as_mut()
                .and_then(|s| {
                    let mut buf = String::new();
                    BufReader::new(s).read_line(&mut buf).ok().map(|_| buf)
                })
                .unwrap_or_default();
            return Err(H2SpawnError::DiedDuringStartup {
                status: status.code(),
                first_line: first_line.trim().to_string(),
            });
        }
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(H2SpawnError::PortNotReady { port, millis: 5000 });
        }
        thread::sleep(sleep);
        sleep = (sleep * 2).min(Duration::from_millis(400));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn jar_missing_surfaces_clean_error() {
        let cfg = H2Config {
            jar_path: PathBuf::from("/nonexistent/path/to/h2.jar"),
            pg_port: 1,
            java: PathBuf::from("java"),
        };
        match spawn(&cfg) {
            Err(H2SpawnError::JarMissing(p)) => {
                assert_eq!(p, PathBuf::from("/nonexistent/path/to/h2.jar"));
            }
            other => panic!("expected JarMissing, got {other:?}"),
        }
    }

    #[test]
    fn missing_java_binary_surfaces_spawn_error() {
        // Use this crate's own Cargo.toml as a stand-in for a jar so
        // the existence check passes; then point at a bogus java
        // binary. `CARGO_MANIFEST_DIR` is set at compile time and is
        // always absolute.
        let jar = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(jar.is_file(), "test setup: {jar:?} should exist");
        let cfg = H2Config {
            jar_path: jar,
            pg_port: 1,
            java: PathBuf::from("/definitely/not/a/real/java"),
        };
        match spawn(&cfg) {
            Err(H2SpawnError::SpawnIo { .. }) => {}
            other => panic!("expected SpawnIo, got {other:?}"),
        }
    }
}
