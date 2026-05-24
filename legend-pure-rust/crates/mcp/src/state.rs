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
//! The MCP server compiles **once at startup** and holds the result
//! behind a `Mutex<Arc<WorkspaceSnapshot>>` on the server struct.
//! Tool calls clone the inner `Arc` and drop the lock immediately so
//! reads run without contention. The `reload_workspace` tool is the
//! only writer — it compiles fresh, builds a new `Arc<Snapshot>`,
//! and swaps the slot.
//!
//! The snapshot itself is **immutable** once constructed. That keeps
//! the read path simple: clone the Arc, drop the lock, walk the
//! model without any further locking.

use std::collections::HashMap;
use std::sync::Arc;

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_store::compiler::RelationStoreExtension;
use legend_pure_ide::{ReferenceIndex, build_reference_index};
use legend_pure_parser_parser::SectionParser;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

/// Immutable snapshot of one compile pass.
///
/// Every tool clones the outer `Arc<WorkspaceSnapshot>` (cheap atomic
/// refcount bump) and reads from it without locking.
#[derive(Clone)]
pub struct WorkspaceSnapshot {
    /// The compiled (possibly partially-compiled) model.
    pub model: Arc<PureModel>,
    /// Diagnostics from the compile, grouped by canonical source
    /// path so `get_diagnostics{file: "..."}` can look them up in
    /// O(1).
    pub diagnostics: HashMap<String, Vec<CompilationError>>,
    /// Total error count across all sources (cached so callers
    /// don't iterate the map).
    pub error_count: usize,
    /// Wall-clock time the snapshot was produced. Surfaced by
    /// [`workspace_status`](crate::LegendMcpServer::workspace_status)
    /// so agents can decide whether to call
    /// [`reload_workspace`](crate::LegendMcpServer::reload_workspace).
    pub compiled_at: jiff::Timestamp,
    /// Reverse-references index over the model. Drives the
    /// `find_references` tool: given an [`ElementId`], list every
    /// source site that references it (function calls, type uses,
    /// stereotype refs, etc.). Built once per compile via
    /// [`build_reference_index`]; the same index the LSP uses for
    /// `textDocument/references`.
    ///
    /// [`ElementId`]: legend_pure_parser_pure::ids::ElementId
    pub references: Arc<ReferenceIndex>,
}

impl WorkspaceSnapshot {
    /// Compile the given repos through the same pipeline the LSP
    /// uses: `load_with_extensions` with the three DSL extensions
    /// and fresh section parsers per repo.
    ///
    /// Takes repos by reference — callers (the server) hold the
    /// canonical `Arc<Vec<Repo>>` so `reload_workspace` can recompile
    /// against the same input set.
    pub fn compile(repos: &[Repo], auto_imports: &[SmolStr]) -> Self {
        let mapping = MappingExtension::new();
        let relational = RelationalExtension::new();
        let store = RelationStoreExtension::new();
        let extensions: [&dyn CompilerExtension; 3] = [&mapping, &relational, &store];
        let result = repo::load_with_extensions(
            repos,
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
        let model = Arc::new(model);
        let references = Arc::new(build_reference_index(&model));
        Self {
            model,
            diagnostics,
            error_count,
            compiled_at: jiff::Timestamp::now(),
            references,
        }
    }
}
