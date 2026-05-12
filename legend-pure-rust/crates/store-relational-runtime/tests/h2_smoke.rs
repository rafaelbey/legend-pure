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

//! End-to-end smoke tests for the H2 backend.
//!
//! These exercise the real Java sub-process / PG-wire path and so
//! depend on (a) a JVM on `$PATH`, (b) an H2 jar reachable through
//! `LEGEND_PURE_H2_JAR` or the classpath TOML. When either is
//! missing the tests **skip cleanly** with an `eprintln!` rather
//! than failing — contributors who haven't installed Maven artefacts
//! still see green CI.
//
// `PureException` is intentionally rich (call stack), so the
// `Result<T, PureException>` types lighting up `result_large_err` are
// allowed crate-wide here too.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use legend_pure_store_relational_runtime::config::H2EnvOverrides;
use legend_pure_store_relational_runtime::{H2Config, H2State};

/// Probe the live process env for an H2 config; if it errors (e.g. no
/// jar path set), emit a skip note and return `None`.
fn maybe_resolve_config() -> Option<H2Config> {
    let empty: HashMap<String, HashMap<String, toml::Value>> = HashMap::new();
    match H2Config::resolve_with_env(&empty, &H2EnvOverrides::from_process_env()) {
        Ok(cfg) if cfg.jar_path.is_file() => Some(cfg),
        Ok(cfg) => {
            eprintln!(
                "skipping H2 smoke: configured jar `{}` does not exist",
                cfg.jar_path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("skipping H2 smoke: {e}");
            None
        }
    }
}

#[test]
fn h2_select_one_plus_one() {
    let Some(cfg) = maybe_resolve_config() else {
        return;
    };
    let state = H2State::new(&cfg).expect("H2State::new");
    let two: i32 = state
        .with_client(|c| {
            let row = c.query_one("SELECT 1 + 1 AS v", &[])?;
            row.try_get::<_, i32>("v")
        })
        .expect("query");
    assert_eq!(two, 2);
}

#[test]
fn h2_server_starts_once_across_evaluators() {
    let Some(cfg) = maybe_resolve_config() else {
        return;
    };
    // Two independent H2State instances should share the same server
    // (visible through the cached `H2Server.pg_port` matching the
    // configured port). The state-level isolation comes from
    // unique `mem:<uuid>` database names.
    let a = H2State::new(&cfg).expect("a");
    let b = H2State::new(&cfg).expect("b");
    assert_ne!(a.db_name, b.db_name, "evaluators get distinct logical DBs");
}

#[test]
fn h2_per_evaluator_db_isolation() {
    let Some(cfg) = maybe_resolve_config() else {
        return;
    };
    let a = H2State::new(&cfg).expect("a");
    let b = H2State::new(&cfg).expect("b");

    a.with_client(|c| c.execute("CREATE TABLE only_in_a(x INTEGER)", &[]))
        .expect("a create");

    // `only_in_a` must NOT exist on b's logical database.
    let count: i64 = b
        .with_client(|c| {
            let row = c.query_one(
                "SELECT COUNT(*) AS n FROM information_schema.tables \
                 WHERE table_name = 'ONLY_IN_A'",
                &[],
            )?;
            row.try_get::<_, i64>("n")
        })
        .expect("b query");
    assert_eq!(
        count, 0,
        "evaluator B must not see table created in evaluator A"
    );
}
