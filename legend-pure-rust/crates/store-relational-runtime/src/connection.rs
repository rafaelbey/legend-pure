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

//! Per-Evaluator DuckDB connection state.
//!
//! [`DuckDBState`] is the value installed in
//! [`legend_pure_runtime::extensions::ExtensionStateStore`] under the
//! relational store's `TypeId`. Each [`legend_pure_runtime::eval::Evaluator`]
//! gets one — concurrent evaluations are therefore independent (different
//! threads, separate in-memory DBs) without any process-wide singletons.
//!
//! The connection is wrapped in [`RefCell`] because `duckdb::Connection`
//! is `Send` but not `Sync`; multiple natives within one evaluator run
//! sequentially under the evaluator's `&mut self`, so interior mutability
//! is sound and lock-free.
//!
//! When the evaluator drops, the `RefCell<Connection>` drops with it,
//! which closes the DuckDB handle and tears down every temp table inside
//! — no explicit cleanup contract on the native layer.

use std::cell::RefCell;

use legend_pure_runtime::error::{PureException, PureRuntimeError};

/// Lazily-initialised per-Evaluator DuckDB state.
///
/// Constructed by [`DuckDBState::new`] on the first relational native
/// call inside an evaluator and reused for all subsequent calls in that
/// evaluator. Drops with the evaluator.
pub struct DuckDBState {
    conn: RefCell<duckdb::Connection>,
}

impl DuckDBState {
    /// Open a fresh in-memory DuckDB and set UTC as the session timezone.
    ///
    /// UTC matches Java's `TestDatabaseConnectDuckDB.java` so the Date /
    /// DateTime values surfaced through `executeInDb` agree across
    /// stacks. Setting the timezone is best-effort — older DuckDB
    /// versions silently ignore unknown configuration variables, which
    /// is fine; the default is already UTC.
    ///
    /// # Errors
    /// Returns a [`PureException`] wrapping the DuckDB error if opening
    /// the in-memory database fails (unlikely for `:memory:`, but
    /// surfaced so the caller can attach a stack frame).
    pub fn new() -> Result<Self, PureException> {
        let conn = duckdb::Connection::open_in_memory().map_err(map_duckdb_err)?;
        // Best-effort; older DuckDB builds ignore this — fine since
        // their default is already UTC.
        let _ = conn.execute_batch("SET TimeZone='UTC';");
        Ok(Self {
            conn: RefCell::new(conn),
        })
    }

    /// Run a closure against the inner [`duckdb::Connection`].
    ///
    /// Borrows mutably under the hood — DuckDB's API takes `&Connection`
    /// for read paths and `&Connection` (with internal locking) for
    /// `execute*`, so a `Ref` is sufficient. Caller-side
    /// borrow conflicts across the same Evaluator can only happen
    /// through a misuse of this method; the borrow checker will complain
    /// at compile time inside `with_conn` callers.
    ///
    /// # Errors
    /// Propagates whatever the closure returns, mapped from
    /// [`duckdb::Error`] to [`PureException`] via [`map_duckdb_err`].
    pub fn with_conn<R, F>(&self, f: F) -> Result<R, PureException>
    where
        F: FnOnce(&duckdb::Connection) -> duckdb::Result<R>,
    {
        f(&self.conn.borrow()).map_err(map_duckdb_err)
    }
}

/// Lift a [`duckdb::Error`] into a [`PureException`] with a stable prefix
/// so message-pattern test assertions can match a single shape regardless
/// of the underlying SQL state.
pub fn map_duckdb_err(e: duckdb::Error) -> PureException {
    PureRuntimeError::EvaluationError(format!("DuckDB error: {e}")).into()
}

// ---------------------------------------------------------------------------
// H2 — PostgreSQL-wire over a Java sub-process
// ---------------------------------------------------------------------------

/// Lazily-initialised per-Evaluator H2 state.
///
/// Mirrors [`DuckDBState`] in shape — a `RefCell<…>` around the
/// engine-specific client because the underlying handle isn't `Sync`.
/// Unlike DuckDB, the heavy lifting (spawning the JVM that hosts H2)
/// happens process-wide in [`crate::h2_server::ensure_started`]; what
/// this per-Evaluator state owns is just a `postgres::Client` against
/// that shared server with a unique `mem:<uuid>` database name.
///
/// Two evaluators on the same process get logically independent
/// databases via different UUIDs; closing the client drops the H2
/// in-memory DB once H2 garbage-collects unreferenced mem DBs.
pub struct H2State {
    client: RefCell<postgres::Client>,
    /// Logical H2 database name (`"mem:eval_<uuid>"`). Public for
    /// diagnostics — drop it into error messages so cross-evaluator
    /// confusion is impossible.
    pub db_name: String,
}

impl H2State {
    /// Open a fresh PG-wire client against the shared H2 server,
    /// targeting a unique `mem:eval_<uuid>` logical database.
    ///
    /// Spawns the H2 sub-process if it's not already running (see
    /// [`crate::h2_server::ensure_started`]).
    ///
    /// # Errors
    /// Returns a [`PureException`] when the H2 server fails to start
    /// or the PG client cannot connect.
    pub fn new(cfg: &crate::config::H2Config) -> Result<Self, PureException> {
        let server = crate::h2_server::ensure_started(cfg)?;
        let db_name = format!("mem:eval_{}", uuid::Uuid::new_v4().simple());
        // URL form, not libpq key=value: H2's PG-wire server treats
        // colons in `dbname=...` poorly through the libpq path, but
        // accepts them fine in a postgresql:// URL. Empty password
        // between `:` and `@` matches H2's SA user (no password).
        let conn_str = format!("postgresql://sa:@127.0.0.1:{}/{}", server.pg_port, db_name);
        let client =
            postgres::Client::connect(&conn_str, postgres::NoTls).map_err(map_postgres_err)?;
        Ok(Self {
            client: RefCell::new(client),
            db_name,
        })
    }

    /// Construct an `H2State` using the supplied
    /// `[extension.relational]` sub-table (from per-evaluator
    /// configuration) overlaid with env-var overrides.
    ///
    /// Callers typically extract the sub-table via
    /// [`legend_pure_runtime::native::EvalContextTrait::config_for`]
    /// just before calling this — see the dispatch.rs / resultset.rs
    /// use sites for the canonical shape.
    ///
    /// # Errors
    /// Returns the underlying [`crate::H2ConfigError`] surfaced as a
    /// [`PureException`] when no jar is configured anywhere, or
    /// propagates spawn / connect failures.
    pub fn from_relational_table(
        relational: std::collections::HashMap<String, toml::Value>,
    ) -> Result<Self, PureException> {
        // H2Config::resolve expects the outer-shape
        // HashMap<extension_name, HashMap<field, Value>>; wrap the
        // single sub-table we have. Keeps H2Config's API stable while
        // unblocking per-evaluator config flow.
        let mut wrapper: std::collections::HashMap<
            String,
            std::collections::HashMap<String, toml::Value>,
        > = std::collections::HashMap::new();
        if !relational.is_empty() {
            wrapper.insert("relational".to_string(), relational);
        }
        let cfg = crate::H2Config::resolve(&wrapper)
            .map_err(|e| PureRuntimeError::EvaluationError(e.to_string()))?;
        Self::new(&cfg)
    }

    /// Run a closure against the inner [`postgres::Client`].
    ///
    /// Borrows mutably under the hood (the `postgres` API needs
    /// `&mut Client` for `query`/`execute`). Concurrent calls on the
    /// same `H2State` aren't possible — evaluator dispatch is
    /// single-threaded.
    ///
    /// # Errors
    /// Propagates whatever the closure returns, mapped to
    /// [`PureException`] via [`map_postgres_err`].
    pub fn with_client<R, F>(&self, f: F) -> Result<R, PureException>
    where
        F: FnOnce(&mut postgres::Client) -> Result<R, postgres::Error>,
    {
        f(&mut self.client.borrow_mut()).map_err(map_postgres_err)
    }
}

/// Lift a [`postgres::Error`] into a [`PureException`] with a stable
/// prefix, mirroring [`map_duckdb_err`].
///
/// `postgres::Error`'s `Display` impl is unhelpfully terse ("db
/// error" / "communication error") — the server-side message lives
/// on the wrapped `DbError`. Walk the error chain so the surfaced
/// `PureException` carries the SQL state + message instead of a
/// generic placeholder.
pub fn map_postgres_err(e: postgres::Error) -> PureException {
    use std::error::Error;
    let mut detail = e.to_string();
    if let Some(db) = e.as_db_error() {
        detail = format!("{}: {}", db.severity(), db.message());
    } else if let Some(src) = e.source() {
        detail = format!("{e}: {src}");
    }
    PureRuntimeError::EvaluationError(format!("H2 (PG-wire) error: {detail}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_and_execute() {
        let s = DuckDBState::new().expect("open");
        s.with_conn(|c| c.execute_batch("CREATE TABLE t(x INTEGER)"))
            .expect("create");
        s.with_conn(|c| c.execute_batch("INSERT INTO t VALUES (1), (2)"))
            .expect("insert");
        let count: i64 = s
            .with_conn(|c| {
                let mut stmt = c.prepare("SELECT count(*) FROM t")?;
                let mut rows = stmt.query([])?;
                let row = rows.next()?.expect("at least one row");
                row.get::<_, i64>(0)
            })
            .expect("count");
        assert_eq!(count, 2);
    }

    #[test]
    fn independent_states_have_independent_databases() {
        let a = DuckDBState::new().expect("a");
        let b = DuckDBState::new().expect("b");
        a.with_conn(|c| c.execute_batch("CREATE TABLE only_in_a(x INTEGER)"))
            .expect("a create");
        // `only_in_a` must not exist on b's connection.
        let b_sees_a = b.with_conn(|c| {
            let mut stmt =
                c.prepare("SELECT count(*) FROM duckdb_tables() WHERE table_name = 'only_in_a'")?;
            let mut rows = stmt.query([])?;
            rows.next()?.expect("row").get::<_, i64>(0)
        });
        assert_eq!(b_sees_a.expect("b query"), 0);
    }
}
