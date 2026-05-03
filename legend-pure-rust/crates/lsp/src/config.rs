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

//! Server configuration.
//!
//! [`LspConfig`] carries everything the server needs to compile a
//! workspace: the pre-resolved repos, the auto-import packages, and a
//! log-level hint. The CLI builds it from `--classpath` flag + TOML
//! parsing; programmatic harnesses can construct it directly.

use legend_pure_core_platform::repo::Repo;
use smol_str::SmolStr;

/// The default auto-imported packages, mirroring `legend compile`'s
/// `AUTO_IMPORT_PACKAGES`.
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

/// Server configuration.
#[derive(Clone)]
pub struct LspConfig {
    /// Repos available to the workspace, in declaration order. Each
    /// `Repo` is consumed via
    /// [`legend_pure_core_platform::repo::load`].
    pub repos: Vec<Repo>,
    /// Packages auto-imported into every section (e.g.
    /// `meta::pure::metamodel`). Defaults to the same list `legend
    /// compile` uses.
    pub auto_imports: Vec<SmolStr>,
    /// Log level hint (`trace`/`debug`/`info`/`warn`/`error`/`off`).
    /// The CLI subcommand forwards this to `tracing-subscriber`; the
    /// LSP itself only reads it for the initial banner log line.
    pub log_level: String,
}

impl LspConfig {
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
    /// for scenarios where no classpath descriptor is available — the
    /// LSP still gives diagnostics, hover, and goto for any code that
    /// only references platform symbols.
    #[must_use]
    pub fn embedded_platform_only() -> Self {
        Self::from_repos(Repo::default_embedded())
    }
}

impl Default for LspConfig {
    fn default() -> Self {
        Self::embedded_platform_only()
    }
}

impl std::fmt::Debug for LspConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspConfig")
            .field("repos", &self.repos.len())
            .field("auto_imports", &self.auto_imports.len())
            .field("log_level", &self.log_level)
            .finish()
    }
}
