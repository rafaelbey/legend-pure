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

//! `LegendMcpServer` — the rmcp `ServerHandler` that backs `legend mcp`.
//!
//! Holds a [`WorkspaceSnapshot`] in an `Arc` (cheap clone). All 9
//! MVP tools live on this struct; new tools land here as additional
//! `#[tool]` methods until volume justifies splitting into
//! sub-routers.

use std::sync::Arc;

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::state::WorkspaceSnapshot;

// ---------------------------------------------------------------------------
// Tool argument schemas (one per tool with non-empty inputs)
// ---------------------------------------------------------------------------

/// Input shape for [`LegendMcpServer::search_symbols`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchSymbolsArgs {
    /// Case-insensitive substring to match against element FQNs.
    /// Example: `"Person"` matches `my::pkg::Person`,
    /// `meta::pure::test::Person`, etc.
    pub query: String,
    /// Maximum number of results to return. Defaults to 50; capped
    /// at 500 even if a larger value is requested (prevents
    /// pathological queries from drowning the response).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Input shape for [`LegendMcpServer::get_diagnostics`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetDiagnosticsArgs {
    /// Canonical source path (e.g. `/myproj/foo.pure`). If omitted,
    /// every file's diagnostics are returned.
    #[serde(default)]
    pub file: Option<String>,
}

/// Input shape for tools that take a single FQN.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FqnArgs {
    /// Fully-qualified name of the element (e.g.
    /// `my::pkg::Person`, `meta::pure::tests::testPlus`).
    pub fqn: String,
}

/// Input shape for [`LegendMcpServer::run_pct`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunPctArgs {
    /// FQN of the `<<PCT.test>>`-tagged function.
    pub test_fqn: String,
    /// FQN of the adapter to run the test against. Discoverable via
    /// [`LegendMcpServer::list_pct_adapters`].
    pub adapter_fqn: String,
}

/// Input shape for [`LegendMcpServer::list_packages`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPackagesArgs {
    /// Optional FQN prefix filter (e.g. `meta::pure`). If omitted,
    /// every package is returned.
    #[serde(default)]
    pub prefix: Option<String>,
}

/// Input shape for [`LegendMcpServer::list_tests`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTestsArgs {
    /// Optional package-FQN prefix to narrow the search (e.g.
    /// `meta::pure::functions::math`). If omitted, every test in
    /// the workspace is returned.
    #[serde(default)]
    pub package_prefix: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool result types — produced by the read/introspect tools.
// Execute tools serialize the runner's own typed results directly.
// ---------------------------------------------------------------------------

/// One row of the [`LegendMcpServer::search_symbols`] response.
#[derive(Debug, Serialize)]
pub struct SymbolHit {
    /// Fully-qualified name.
    pub fqn: String,
    /// Element kind — `Class`, `Function`, `Profile`, `Enumeration`,
    /// `Association`, `Measure`, `Package`, or `(unknown)` for
    /// element kinds we don't yet classify.
    pub kind: String,
    /// Canonical source path the element was loaded from.
    pub source: String,
    /// 1-based line of the element's name span.
    pub line: u32,
    /// 1-based column of the element's name span.
    pub column: u32,
}

/// One row of the [`LegendMcpServer::get_diagnostics`] response.
#[derive(Debug, Serialize)]
pub struct DiagnosticRow {
    /// Canonical source path the diagnostic refers to.
    pub source: String,
    /// 1-based line of the diagnostic span.
    pub line: u32,
    /// 1-based column of the diagnostic span.
    pub column: u32,
    /// Severity (`error` / `warning` — currently always `error`
    /// because the compiler doesn't yet emit warnings).
    pub severity: String,
    /// The diagnostic message.
    pub message: String,
}

/// Detailed view of one element returned by
/// [`LegendMcpServer::read_element`].
#[derive(Debug, Serialize)]
pub struct ElementInfo {
    /// Fully-qualified name.
    pub fqn: String,
    /// Element kind (see [`SymbolHit::kind`] for the value set).
    pub kind: String,
    /// Canonical source path.
    pub source: String,
    /// 1-based line of the name span.
    pub line: u32,
    /// 1-based column of the name span.
    pub column: u32,
}

/// One test-function entry in the [`LegendMcpServer::list_tests`]
/// response.
#[derive(Debug, Serialize)]
pub struct TestEntry {
    /// FQN of the test function.
    pub fqn: String,
    /// Canonical source path.
    pub source: String,
    /// 1-based line of the test function's name span.
    pub line: u32,
    /// Test stereotypes attached to the function, e.g.
    /// `["test::Test"]` or `["PCT.test"]`.
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// LegendMcpServer
// ---------------------------------------------------------------------------

/// rmcp `ServerHandler` that exposes the Legend Pure workspace as
/// MCP tools.
#[derive(Clone)]
pub struct LegendMcpServer {
    snapshot: Arc<WorkspaceSnapshot>,
    // Stored on the struct because rmcp's `#[tool_handler]` macro
    // expects a `tool_router` field on `Self`. Rust's dead-code
    // analysis doesn't see through proc-macro expansions; suppress
    // explicitly.
    #[allow(dead_code)]
    tool_router: ToolRouter<LegendMcpServer>,
}

impl LegendMcpServer {
    /// Construct a server backed by the given compiled-workspace
    /// snapshot.
    #[must_use]
    pub fn new(snapshot: Arc<WorkspaceSnapshot>) -> Self {
        Self {
            snapshot,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl LegendMcpServer {
    // ----- Read tools -----------------------------------------------------

    /// Search the compiled workspace for elements whose FQN matches
    /// a substring (case-insensitive).
    #[tool(
        description = "Find Legend Pure elements (classes, functions, profiles, enums, associations) whose FQN contains the given substring. Returns matches with source location."
    )]
    async fn search_symbols(
        &self,
        Parameters(args): Parameters<SearchSymbolsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            search_symbols_impl(&snapshot.model, &args)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }

    /// Return compilation diagnostics for the workspace.
    #[tool(
        description = "Return compilation diagnostics (errors). With `file` set, returns diagnostics for that canonical source path only; otherwise returns every diagnostic across the workspace."
    )]
    async fn get_diagnostics(
        &self,
        Parameters(args): Parameters<GetDiagnosticsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let rows: Vec<DiagnosticRow> = match args.file {
            Some(file) => snapshot
                .diagnostics
                .get(&file)
                .map(|errs| errs.iter().map(diagnostic_row).collect())
                .unwrap_or_default(),
            None => snapshot
                .diagnostics
                .values()
                .flat_map(|errs| errs.iter().map(diagnostic_row))
                .collect(),
        };
        json_result(&rows)
    }

    // ----- Execute tools --------------------------------------------------

    /// Call a parameter-less Pure function and return its rendered
    /// value.
    #[tool(
        description = "Execute a parameter-less Pure function by FQN and return its rendered value plus any captured stdout. Failures return a structured error with a parsed stack trace."
    )]
    async fn run_function(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            let registry = legend_pure_runtime::native::NativeRegistry::standard();
            legend_pure_runtime::runner::run_function(&snapshot.model, &registry, &args.fqn)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }

    /// Run a `<<test::Test>>`-tagged function via the platform
    /// surveyor (with `BeforePackage` / `AfterPackage` hooks
    /// applied).
    #[tool(
        description = "Run a <<test::Test>>-tagged Pure function through the platform surveyor (with BeforePackage / AfterPackage lifecycle hooks). Returns pass/fail counts and structured failure list."
    )]
    async fn run_test(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            let registry = legend_pure_runtime::native::NativeRegistry::standard();
            legend_pure_runtime::runner::run_test(&snapshot.model, &registry, &args.fqn)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Ok(r) => json_result(&r),
            Err(e) => Err(McpError::invalid_params(e.to_string(), None)),
        }
    }

    /// Run a `<<PCT.test>>` against the given adapter.
    #[tool(
        description = "Run a Pure <<PCT.test>>-tagged function against a chosen PCT adapter. The adapter FQN must resolve in the workspace (use list_pct_adapters to discover them)."
    )]
    async fn run_pct(
        &self,
        Parameters(args): Parameters<RunPctArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            let registry = legend_pure_runtime::native::NativeRegistry::standard();
            legend_pure_runtime::runner::run_pct(
                &snapshot.model,
                &registry,
                &args.test_fqn,
                &args.adapter_fqn,
            )
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Ok(r) => json_result(&r),
            Err(e) => Err(McpError::invalid_params(e.to_string(), None)),
        }
    }

    /// Discover every PCT adapter in the workspace.
    #[tool(
        description = "List every <<PCT.adapter>>-tagged function in the workspace. Returns each adapter's FQN plus its human-readable name (from the PCT.adapterName tagged value)."
    )]
    async fn list_pct_adapters(&self) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            legend_pure_runtime::runner::list_pct_adapters(&snapshot.model)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Ok(adapters) => json_result(&adapters),
            Err(e) => Err(McpError::invalid_params(e.to_string(), None)),
        }
    }

    // ----- Introspection tools -------------------------------------------

    /// Resolve an FQN to its kind + source location.
    #[tool(
        description = "Look up a Legend Pure element by FQN and return its kind + source location. Use this after search_symbols to confirm an element exists and find where it's defined."
    )]
    async fn read_element(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let fqn = args.fqn.clone();
        let result = tokio::task::spawn_blocking(move || {
            read_element_impl(&snapshot.model, &args.fqn)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Some(info) => json_result(&info),
            None => Err(McpError::invalid_params(
                format!("element not found in workspace: {fqn}"),
                None,
            )),
        }
    }

    /// List every package in the workspace, optionally filtered by FQN prefix.
    #[tool(
        description = "List every package FQN known to the workspace. Optional `prefix` filter narrows results to packages whose FQN starts with the prefix (e.g. \"meta::pure\")."
    )]
    async fn list_packages(
        &self,
        Parameters(args): Parameters<ListPackagesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            list_packages_impl(&snapshot.model, args.prefix.as_deref())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }

    /// List every test function in the workspace, optionally narrowed by package.
    #[tool(
        description = "List every function tagged with test::Test, test::AlloyOnly, or PCT.test. Optional `package_prefix` narrows the search."
    )]
    async fn list_tests(
        &self,
        Parameters(args): Parameters<ListTestsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot.clone();
        let result = tokio::task::spawn_blocking(move || {
            list_tests_impl(&snapshot.model, args.package_prefix.as_deref())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }
}

#[tool_handler]
impl ServerHandler for LegendMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.instructions = Some(
            "Legend Pure MCP server. Tools:\n\
             - search_symbols / read_element / list_packages / list_tests — discover elements in the compiled workspace\n\
             - get_diagnostics — surface compile errors\n\
             - run_function / run_test / run_pct / list_pct_adapters — execute Pure code\n\
             \nThe workspace is compiled once at startup. Restart the server to pick up source changes."
                .to_string(),
        );
        info
    }
}

// ---------------------------------------------------------------------------
// Helpers — kept module-private so `#[tool]` methods stay declarative.
// ---------------------------------------------------------------------------

/// Pretty-print a serializable value into an MCP `text` content block.
fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serde error: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn diagnostic_row(err: &legend_pure_parser_pure::error::CompilationError) -> DiagnosticRow {
    DiagnosticRow {
        source: err.source_info.source.to_string(),
        line: err.source_info.start_line,
        column: err.source_info.start_column,
        severity: "error".to_string(),
        message: err.message.clone(),
    }
}

fn search_symbols_impl(model: &PureModel, args: &SearchSymbolsArgs) -> Vec<SymbolHit> {
    let query = args.query.to_lowercase();
    let limit = args.limit.unwrap_or(50).min(500) as usize;
    let mut out: Vec<SymbolHit> = Vec::new();
    'outer: for chunk in &model.chunks {
        // Skip the bootstrap chunk — Any / Nil / primitives are
        // noise in 99% of searches and the user-facing equivalent
        // is to grep `meta::pure::metamodel::*`.
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, node) in chunk.nodes.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let fqn = render_fqn(model, id);
            if !fqn.to_lowercase().contains(&query) {
                continue;
            }
            let element = chunk.elements.get(local_idx);
            out.push(SymbolHit {
                fqn,
                kind: element_kind(element).to_string(),
                source: node.source_info.source.to_string(),
                line: node.source_info.start_line,
                column: node.source_info.start_column,
            });
            if out.len() >= limit {
                break 'outer;
            }
        }
    }
    out
}

fn read_element_impl(model: &PureModel, fqn: &str) -> Option<ElementInfo> {
    let id = model.resolve_fqn_str(fqn)?;
    let node = model.try_get_element(id).map(|_| model.get_node(id))?;
    let element = model.try_get_element(id)?;
    Some(ElementInfo {
        fqn: render_fqn(model, id),
        kind: element_kind(element).to_string(),
        source: node.source_info.source.to_string(),
        line: node.source_info.start_line,
        column: node.source_info.start_column,
    })
}

fn list_packages_impl(model: &PureModel, prefix: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (pkg_id, _pkg) in model.global_packages.iter() {
        let fqn = render_package_fqn(model, legend_pure_parser_pure::ids::PackageId(pkg_id));
        if fqn.is_empty() {
            continue; // root package — not a useful tool output
        }
        if let Some(p) = prefix {
            if !fqn.starts_with(p) {
                continue;
            }
        }
        out.push(fqn);
    }
    out.sort();
    out
}

fn list_tests_impl(model: &PureModel, package_prefix: Option<&str>) -> Vec<TestEntry> {
    let mut out: Vec<TestEntry> = Vec::new();
    for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            let mut tags = Vec::new();
            for s in &func.stereotypes {
                let Some(profile_name) = model.try_get_element(s.profile).map(|_| {
                    render_fqn(model, s.profile)
                }) else {
                    continue;
                };
                let stereotype_fqn = format!("{profile_name}.{}", s.value);
                if matches!(
                    stereotype_fqn.as_str(),
                    "meta::pure::profiles::test.Test"
                        | "meta::pure::profiles::test.AlloyOnly"
                        | "meta::pure::test::pct::PCT.test"
                ) {
                    // Strip the leading meta::pure::profiles:: so the
                    // tag set stays human-readable; PCT keeps its
                    // full prefix because there's no profiles
                    // namespace collision.
                    let short = stereotype_fqn
                        .strip_prefix("meta::pure::profiles::")
                        .unwrap_or(&stereotype_fqn);
                    tags.push(short.to_string());
                }
            }
            if tags.is_empty() {
                continue;
            }
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let fqn = render_fqn(model, id);
            if let Some(p) = package_prefix {
                if !fqn.starts_with(p) {
                    continue;
                }
            }
            let node = model.get_node(id);
            out.push(TestEntry {
                fqn,
                source: node.source_info.source.to_string(),
                line: node.source_info.start_line,
                tags,
            });
        }
    }
    out.sort_by(|a, b| a.fqn.cmp(&b.fqn));
    out
}

fn element_kind(element: &Element) -> &'static str {
    match element {
        Element::Class(_) => "Class",
        Element::Function(_) => "Function",
        Element::Profile(_) => "Profile",
        Element::Enumeration(_) => "Enumeration",
        Element::Association(_) => "Association",
        Element::Measure(_) => "Measure",
        Element::Package(_) => "Package",
        _ => "(other)",
    }
}

fn render_fqn(model: &PureModel, id: ElementId) -> String {
    if model.try_get_element(id).is_none() {
        return model.element_name(id).to_string();
    }
    if matches!(id, ElementId::Package(_)) {
        return model.element_name(id).to_string();
    }
    let node = model.get_node(id);
    let mut parts: Vec<String> = vec![node.name.to_string()];
    let mut pkg_id = node.parent_package;
    loop {
        let pkg = model.get_package(pkg_id);
        if pkg.parent.is_none() {
            break;
        }
        parts.push(pkg.name.to_string());
        let Some(parent) = pkg.parent else {
            break;
        };
        pkg_id = parent;
    }
    parts.reverse();
    parts.join("::")
}

fn render_package_fqn(model: &PureModel, pkg_id: legend_pure_parser_pure::ids::PackageId) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = pkg_id;
    loop {
        let pkg = model.get_package(current);
        if pkg.parent.is_none() {
            break;
        }
        parts.push(pkg.name.to_string());
        let Some(parent) = pkg.parent else {
            break;
        };
        current = parent;
    }
    parts.reverse();
    parts.join("::")
}
