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

//! Server configuration. Mirrors
//! [`legend_pure_lsp::config`](../../legend_pure_lsp/config/index.html)
//! one-for-one — same `Vec<Repo>` cascade, same auto-import defaults
//! — so the same `legend-pure-classpath.toml` works for both
//! `legend lsp` and `legend mcp`.

use legend_pure_core_platform::repo::Repo;
use smol_str::SmolStr;

/// The default auto-imported packages, mirroring `legend compile`'s
/// `AUTO_IMPORT_PACKAGES`. Kept in lock-step with the LSP's list
/// (see `crates/lsp/src/config.rs`) so a workspace compiled by either
/// server sees the same import surface.
const DEFAULT_AUTO_IMPORTS: &[&str] = &[
    "meta::pure::metamodel",
    "meta::pure::metamodel::type",
    "meta::pure::metamodel::type::generics",
    "meta::pure::metamodel::relationship",
    "meta::pure::metamodel::valuespecification",
    "meta::pure::metamodel::multiplicity",
    "meta::pure::metamodel::function",
    "meta::pure::metamodel::function::property",
    "meta::pure::metamodel::extension",
    "meta::pure::metamodel::import",
    "meta::pure::functions::date",
    "meta::pure::functions::string",
    "meta::pure::functions::collection",
    "meta::pure::functions::meta",
    "meta::pure::functions::constraints",
    "meta::pure::functions::lang",
    "meta::pure::functions::boolean",
    "meta::pure::functions::tools",
    "meta::pure::functions::io",
    "meta::pure::functions::math",
    "meta::pure::functions::asserts",
    "meta::pure::functions::test",
    "meta::pure::functions::multiplicity",
    "meta::pure::router",
    "meta::pure::service",
    "meta::pure::tds",
    "meta::pure::tools",
    "meta::pure::profiles",
];

/// MCP server configuration.
#[derive(Clone)]
pub struct McpConfig {
    /// Repos available to the workspace, in declaration order. Each
    /// is consumed via
    /// [`legend_pure_core_platform::repo::load_with_extensions`].
    pub repos: Vec<Repo>,
    /// Packages auto-imported into every section. Defaults to the
    /// same list `legend compile` uses.
    pub auto_imports: Vec<SmolStr>,
    /// Log level hint (`trace`/`debug`/`info`/`warn`/`error`/`off`).
    /// The CLI forwards this to `tracing-subscriber`; the server
    /// itself only reads it for the initial banner.
    pub log_level: String,
}

impl McpConfig {
    /// Build a config with the default auto-import set.
    #[must_use]
    pub fn from_repos(repos: Vec<Repo>) -> Self {
        Self {
            repos,
            auto_imports: DEFAULT_AUTO_IMPORTS
                .iter()
                .copied()
                .map(SmolStr::new)
                .collect(),
            log_level: "info".to_string(),
        }
    }

    /// Convenience: load only the embedded platform `.purem`. Useful
    /// for smoke tests against a stripped-down environment.
    #[must_use]
    pub fn embedded_platform_only() -> Self {
        Self::from_repos(Repo::default_embedded())
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self::embedded_platform_only()
    }
}

impl std::fmt::Debug for McpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConfig")
            .field("repos", &self.repos.len())
            .field("auto_imports", &self.auto_imports.len())
            .field("log_level", &self.log_level)
            .finish()
    }
}
