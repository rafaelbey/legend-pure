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

//! Engine-specific configuration for the relational store extension.
//!
//! Today only H2 has runtime knobs (driver-jar path, PG-compat port,
//! JVM binary). DuckDB ships fully self-contained via the bundled
//! `duckdb` crate and needs no config. Future engines can grow their
//! own structs alongside [`H2Config`].
//!
//! # Resolution order for `H2Config`
//!
//! 1. Environment variables (highest precedence):
//!    - `LEGEND_PURE_H2_JAR` — absolute path to `h2-X.Y.Z.jar`
//!    - `LEGEND_PURE_H2_VERSION`
//!    - `LEGEND_PURE_H2_PG_PORT`
//!    - `LEGEND_PURE_H2_JAVA` — path to the `java` binary
//! 2. The `[extension.relational.h2]` table in the active
//!    `legend-pure-classpath.toml`, harvested by
//!    `legend-cli::classpath::resolve_classpath` and threaded as a
//!    `&HashMap<String, HashMap<String, toml::Value>>` into
//!    [`H2Config::resolve`].
//! 3. Hard-stop error. There is no in-code default path — the
//!    `~/.m2/...` fallback is a config-time *default*, not a code
//!    *default*, so binaries don't accidentally pick up the wrong jar.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Runtime configuration for the H2 backend.
#[derive(Debug, Clone, Deserialize)]
pub struct H2Config {
    /// Absolute path to the H2 driver jar. `~` is expanded against
    /// `$HOME` when read from TOML; env-var values are used verbatim
    /// (callers can pre-expand or pass an absolute path).
    pub jar_path: PathBuf,
    /// H2 version string, e.g. `"2.1.214"`. Carried for diagnostics
    /// and future per-version branching; not consumed by the sub-process
    /// spawn directly.
    #[serde(default = "default_version")]
    pub version: String,
    /// TCP port the spawned H2 server listens on for PG-wire clients.
    #[serde(default = "default_pg_port")]
    pub pg_port: u16,
    /// Path to the `java` binary. Defaults to `"java"`, relying on
    /// `$PATH`.
    #[serde(default = "default_java")]
    pub java: PathBuf,
}

fn default_version() -> String {
    "2.1.214".into()
}
fn default_pg_port() -> u16 {
    5435
}
fn default_java() -> PathBuf {
    PathBuf::from("java")
}

/// Env-var overrides for [`H2Config::resolve`].
///
/// Split from process env reading so tests can exercise the precedence
/// logic without mutating real process state — the crate is
/// `#![forbid(unsafe_code)]` and `std::env::set_var` requires unsafe
/// in modern Rust.
#[derive(Debug, Clone, Default)]
pub struct H2EnvOverrides {
    /// Override for `jar_path`. Used verbatim (no `~` expansion).
    pub jar_path: Option<PathBuf>,
    /// Override for `version`.
    pub version: Option<String>,
    /// Override for `pg_port`. Raw string so [`H2Config::resolve`] can
    /// surface a parse error with the offending value attached.
    pub pg_port: Option<String>,
    /// Override for `java`.
    pub java: Option<PathBuf>,
    /// `$HOME` value used to expand a leading `~` in the TOML
    /// `jar_path`. None disables tilde expansion (no fallback to
    /// `dirs::home_dir()` — keeps the function deterministic).
    pub home: Option<PathBuf>,
}

impl H2EnvOverrides {
    /// Snapshot the process env into an `H2EnvOverrides`. This is the
    /// only function in the module that touches real env state.
    #[must_use]
    pub fn from_process_env() -> Self {
        Self {
            jar_path: std::env::var_os("LEGEND_PURE_H2_JAR").map(PathBuf::from),
            version: std::env::var("LEGEND_PURE_H2_VERSION").ok(),
            pg_port: std::env::var("LEGEND_PURE_H2_PG_PORT").ok(),
            java: std::env::var_os("LEGEND_PURE_H2_JAVA").map(PathBuf::from),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

/// Failure modes for [`H2Config::resolve`].
#[derive(Debug, Error)]
pub enum H2ConfigError {
    /// Neither env vars nor classpath TOML provided a `jar_path`.
    #[error(
        "H2 backend requested but no jar configured. Set the \
         `LEGEND_PURE_H2_JAR` env var or add `jar_path` to the \
         `[extension.relational.h2]` section of legend-pure-classpath.toml."
    )]
    NoJar,
    /// The classpath TOML's `[extension.relational.h2]` table failed
    /// to deserialize into [`H2Config`].
    #[error("invalid [extension.relational.h2] section: {0}")]
    InvalidToml(String),
    /// An env-var value could not be parsed (e.g. `LEGEND_PURE_H2_PG_PORT`
    /// is not a u16).
    #[error("env var {name}={value:?}: {reason}")]
    InvalidEnv {
        /// Env-var name.
        name: &'static str,
        /// Raw env-var value, for diagnostics.
        value: String,
        /// Human-readable explanation.
        reason: String,
    },
}

impl H2Config {
    /// Resolve a final config from the process environment overlaid on
    /// the `[extension.relational.h2]` table of an already-parsed
    /// classpath.
    ///
    /// Convenience wrapper around [`H2Config::resolve_with_env`] that
    /// snapshots the live process env. Production call site for the
    /// CLI.
    ///
    /// # Errors
    /// See [`H2ConfigError`].
    pub fn resolve(
        extension_configs: &HashMap<String, HashMap<String, toml::Value>>,
    ) -> Result<Self, H2ConfigError> {
        Self::resolve_with_env(extension_configs, &H2EnvOverrides::from_process_env())
    }

    /// Resolve a final config from explicit env overrides overlaid on
    /// the `[extension.relational.h2]` table.
    ///
    /// Pure function; useful for tests that need to exercise the
    /// precedence logic without mutating real env state.
    ///
    /// # Errors
    /// See [`H2ConfigError`].
    pub fn resolve_with_env(
        extension_configs: &HashMap<String, HashMap<String, toml::Value>>,
        env: &H2EnvOverrides,
    ) -> Result<Self, H2ConfigError> {
        // Seed from the TOML slice if present; otherwise start with
        // every field empty and rely on env-var overrides + defaults.
        let mut cfg = match extension_configs
            .get("relational")
            .and_then(|d| d.get("h2"))
        {
            Some(value) => value
                .clone()
                .try_into::<H2Config>()
                .map_err(|e| H2ConfigError::InvalidToml(e.to_string()))?,
            None => Self {
                jar_path: PathBuf::new(),
                version: default_version(),
                pg_port: default_pg_port(),
                java: default_java(),
            },
        };

        // Expand `~` in the TOML-supplied jar_path against the
        // provided `home` (typically `$HOME`). Env-var paths bypass
        // expansion — callers can pre-expand if they want.
        if !cfg.jar_path.as_os_str().is_empty()
            && let Some(home) = env.home.as_deref()
        {
            cfg.jar_path = expand_tilde(cfg.jar_path, home);
        }

        // Apply env overrides.
        if let Some(jar) = &env.jar_path {
            cfg.jar_path.clone_from(jar);
        }
        if let Some(v) = &env.version {
            cfg.version.clone_from(v);
        }
        if let Some(port_str) = &env.pg_port {
            cfg.pg_port = port_str.parse().map_err(|e: std::num::ParseIntError| {
                H2ConfigError::InvalidEnv {
                    name: "LEGEND_PURE_H2_PG_PORT",
                    value: port_str.clone(),
                    reason: e.to_string(),
                }
            })?;
        }
        if let Some(java) = &env.java {
            cfg.java.clone_from(java);
        }

        if cfg.jar_path.as_os_str().is_empty() {
            return Err(H2ConfigError::NoJar);
        }
        Ok(cfg)
    }
}

/// Expand a leading `~` against the supplied `home`. Leaves the path
/// unchanged when the prefix is something else.
fn expand_tilde(p: PathBuf, home: &std::path::Path) -> PathBuf {
    let Some(s) = p.to_str() else { return p };
    let Some(rest) = s.strip_prefix('~') else {
        return p;
    };
    let trimmed = rest.strip_prefix('/').unwrap_or(rest);
    if trimmed.is_empty() {
        home.to_path_buf()
    } else {
        home.join(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_toml(toml_text: &str) -> HashMap<String, HashMap<String, toml::Value>> {
        let value: toml::Value = toml::from_str(toml_text).expect("parse");
        let mut out: HashMap<String, HashMap<String, toml::Value>> = HashMap::new();
        if let Some(ext) = value.get("extension").and_then(|v| v.as_table()) {
            for (domain, engines) in ext {
                if let Some(et) = engines.as_table() {
                    let mut inner = HashMap::new();
                    for (engine, cfg) in et {
                        inner.insert(engine.clone(), cfg.clone());
                    }
                    out.insert(domain.clone(), inner);
                }
            }
        }
        out
    }

    #[test]
    fn resolve_from_toml_defaults() {
        let cfgs = make_toml(
            r#"
[extension.relational.h2]
jar_path = "/opt/h2/h2.jar"
"#,
        );
        let cfg = H2Config::resolve_with_env(&cfgs, &H2EnvOverrides::default()).expect("resolve");
        assert_eq!(cfg.jar_path, PathBuf::from("/opt/h2/h2.jar"));
        assert_eq!(cfg.version, "2.1.214");
        assert_eq!(cfg.pg_port, 5435);
        assert_eq!(cfg.java, PathBuf::from("java"));
    }

    #[test]
    fn env_overrides_toml() {
        let cfgs = make_toml(
            r#"
[extension.relational.h2]
jar_path = "/opt/h2/h2.jar"
pg_port  = 5435
"#,
        );
        let env = H2EnvOverrides {
            jar_path: Some(PathBuf::from("/env/h2.jar")),
            pg_port: Some("6543".to_string()),
            ..Default::default()
        };
        let cfg = H2Config::resolve_with_env(&cfgs, &env).expect("resolve");
        assert_eq!(cfg.jar_path, PathBuf::from("/env/h2.jar"));
        assert_eq!(cfg.pg_port, 6543);
    }

    #[test]
    fn tilde_expansion_in_toml_jar_path() {
        let cfgs = make_toml(
            r#"
[extension.relational.h2]
jar_path = "~/.m2/repository/com/h2database/h2/2.1.214/h2-2.1.214.jar"
"#,
        );
        let env = H2EnvOverrides {
            home: Some(PathBuf::from("/home/test-user")),
            ..Default::default()
        };
        let cfg = H2Config::resolve_with_env(&cfgs, &env).expect("resolve");
        assert_eq!(
            cfg.jar_path,
            PathBuf::from(
                "/home/test-user/.m2/repository/com/h2database/h2/2.1.214/h2-2.1.214.jar"
            )
        );
    }

    #[test]
    fn env_jar_path_skips_tilde_expansion() {
        // env-var values are used verbatim — even if they contain a
        // leading `~`. Callers pre-expand or pass absolutes.
        let cfgs = HashMap::new();
        let env = H2EnvOverrides {
            jar_path: Some(PathBuf::from("~/raw.jar")),
            home: Some(PathBuf::from("/home/test-user")),
            ..Default::default()
        };
        let cfg = H2Config::resolve_with_env(&cfgs, &env).expect("resolve");
        assert_eq!(cfg.jar_path, PathBuf::from("~/raw.jar"));
    }

    #[test]
    fn missing_jar_path_errors() {
        let cfgs: HashMap<String, HashMap<String, toml::Value>> = HashMap::new();
        match H2Config::resolve_with_env(&cfgs, &H2EnvOverrides::default()) {
            Err(H2ConfigError::NoJar) => {}
            other => panic!("expected NoJar, got {other:?}"),
        }
    }

    #[test]
    fn malformed_port_env_errors() {
        let cfgs: HashMap<String, HashMap<String, toml::Value>> = HashMap::new();
        let env = H2EnvOverrides {
            jar_path: Some(PathBuf::from("/x.jar")),
            pg_port: Some("not-a-number".to_string()),
            ..Default::default()
        };
        match H2Config::resolve_with_env(&cfgs, &env) {
            Err(H2ConfigError::InvalidEnv { name, .. }) => {
                assert_eq!(name, "LEGEND_PURE_H2_PG_PORT");
            }
            other => panic!("expected InvalidEnv, got {other:?}"),
        }
    }
}
