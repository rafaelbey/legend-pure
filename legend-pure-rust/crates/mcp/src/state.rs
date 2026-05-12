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

//! Compiled-workspace snapshot shared across every MCP tool invocation.
//!
//! Unlike the LSP — which recompiles on every keystroke via
//! `didChange` — the MCP server compiles **once** at startup and
//! holds the result for the lifetime of the process. Agents don't
//! type-edit the workspace through the MCP, so live recompilation
//! buys nothing today. (A `reload_workspace` tool can be added later
//! if agents start mutating sources through this surface.)
//!
//! All state is wrapped in `Arc` so the tool router (which is
//! `Clone`) can share it without locking on every call. The
//! [`PureModel`] itself is immutable, so cloning the outer `Arc` is
//! a cheap atomic refcount bump.

use std::collections::HashMap;
use std::sync::Arc;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_store::compiler::RelationStoreExtension;
use legend_pure_parser_parser::SectionParser;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

/// Snapshot of the workspace as compiled at server startup. Every
/// tool reads from this; nothing mutates it.
#[derive(Clone)]
pub struct WorkspaceSnapshot {
    /// The compiled (possibly partially-compiled) model. Always
    /// present — even on a failed compile we keep whatever was
    /// produced so `get_diagnostics` and `search_symbols` can surface
    /// what's there.
    pub model: Arc<PureModel>,
    /// Diagnostics from the compile, grouped by canonical source
    /// path so `get_diagnostics{file: "..."}` can look them up in
    /// O(1).
    pub diagnostics: HashMap<String, Vec<CompilationError>>,
    /// Total error count across all sources (cached so the server
    /// banner can log it without iterating the map).
    pub error_count: usize,
    /// Base repos used at compile time. Kept so future tools (e.g.
    /// `read_element`) can map canonical paths back to on-disk
    /// locations via the same logic the LSP uses.
    pub repos: Arc<Vec<Repo>>,
}

impl WorkspaceSnapshot {
    /// Compile the configured repos through the same pipeline the
    /// LSP uses: `load_with_extensions` with the three DSL
    /// extensions, fresh section parsers per repo.
    pub fn compile(repos: Vec<Repo>, auto_imports: &[SmolStr]) -> Self {
        let mapping = MappingExtension::new();
        let relational = RelationalExtension::new();
        let store = RelationStoreExtension::new();
        let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];
        let result = repo::load_with_extensions(
            &repos,
            auto_imports,
            &extensions,
            &mut || -> Vec<Box<dyn SectionParser>> {
                vec![
                    Box::new(MappingSectionParser::new()),
                    Box::new(RelationalSectionParser),
                ]
            },
        );
        let (model, errors) = match result {
            Ok(m) => (m, Vec::<CompilationError>::new()),
            Err(partial) => (partial.model, partial.errors),
        };
        let error_count = errors.len();
        let mut diagnostics: HashMap<String, Vec<CompilationError>> = HashMap::new();
        for err in &errors {
            diagnostics
                .entry(err.source_info.source.to_string())
                .or_default()
                .push(err.clone());
        }
        Self {
            model: Arc::new(model),
            diagnostics,
            error_count,
            repos: Arc::new(repos),
        }
    }
}
