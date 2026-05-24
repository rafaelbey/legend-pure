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
//! Holds the current [`WorkspaceSnapshot`] through
//! `Mutex<Arc<WorkspaceSnapshot>>`. Tool reads clone the inner `Arc`
//! and drop the lock immediately so reads run lock-free. The
//! `reload_workspace` tool is the only writer — it recompiles and
//! swaps the slot under the same Mutex. All 11 MVP tools live on
//! this struct; new tools land here as additional `#[tool]` methods
//! until volume justifies splitting into sub-routers.

use std::sync::{Arc, Mutex};

use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_ide::ReferenceIndex;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::nodes::class::{Class, Property, QualifiedProperty};
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::state::WorkspaceSnapshot;

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

/// Input shape for [`LegendMcpServer::find_references`]. Separate from
/// [`FqnArgs`] so the tool's JSON schema carries its own description
/// (rmcp surfaces the struct-level docstring in the schema).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindReferencesArgs {
    /// Fully-qualified name of the element whose usages should be
    /// returned. Same format `read_element` accepts — for functions
    /// this is the mangled form (e.g.
    /// `meta::pure::functions::collection::map_T_m__Function_1__V_$0_n$_`).
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

/// Input shape for [`LegendMcpServer::search_properties`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchPropertiesArgs {
    /// Case-insensitive substring to match against property names
    /// across every Class and Association in the workspace. Example:
    /// `"zip"` finds `zipCode` on `Address`.
    pub name_substring: String,
    /// Maximum number of property hits to return. Defaults to 50;
    /// capped at 500 even if a larger value is requested.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Input shape for [`LegendMcpServer::eval_expression`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EvalExpressionArgs {
    /// Pure expression text to evaluate against the current workspace
    /// snapshot. The expression is wrapped in a synthetic function
    /// returning `Any[*]` and compiled into a transient repo; the
    /// persistent snapshot is not mutated. Example:
    /// `"Person.all()->filter(p | $p.age > 18)->size()"`.
    pub expression: String,
}

/// Input shape for [`LegendMcpServer::apply_edit`].
///
/// `edits` mirrors the LSP `TextDocumentEdit.edits` JSON shape
/// (`[{range: {start: {line, character}, end: {...}}, newText:
/// "..."}]`) so future code-action wiring is a straight pass-through.
///
/// Uses local wire structs ([`WireTextEdit`] / [`WireRange`] /
/// [`WirePosition`]) rather than re-exporting the core types because
/// the rmcp `#[tool]` macro requires [`schemars::JsonSchema`] (v1.x,
/// re-exported through `rmcp::schemars`), and adding schemars 1.x to
/// `core-platform-pure` would inflate that crate's dep graph for a
/// single MCP integration concern.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplyEditArgs {
    /// Canonical source path (`/myproj/foo.pure`) **or** absolute
    /// disk path of the file to edit. The file must already exist
    /// in a `Repo::Filesystem` repo in the configured classpath —
    /// `apply_edit` does not create new files.
    pub file: String,
    /// LSP-style ranged text edits. Each carries a half-open range
    /// (UTF-16 character positions, 0-indexed lines) and the
    /// replacement text. Edits must not overlap; they are applied
    /// in descending-start order so earlier-byte offsets remain
    /// valid.
    pub edits: Vec<WireTextEdit>,
}

/// JSON-schema-friendly wire shape for one ranged text edit. Maps
/// to [`legend_pure_core_platform::edit::TextEdit`] via
/// [`WireTextEdit::into_core`].
///
/// `Serialize` is derived alongside `Deserialize` so tests and
/// snapshot fixtures can round-trip the exact JSON the IDE sees
/// without reaching for the core type's serializer (which writes
/// `new_text` in snake-case rather than the LSP-spec `newText`).
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireTextEdit {
    /// Range of text in the source file to replace.
    pub range: WireRange,
    /// New text to insert. Empty string deletes the range.
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// JSON-schema-friendly wire shape for an LSP range.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WireRange {
    /// Inclusive start position.
    pub start: WirePosition,
    /// Exclusive end position.
    pub end: WirePosition,
}

/// JSON-schema-friendly wire shape for an LSP position. `character`
/// counts UTF-16 code units per LSP spec.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WirePosition {
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed UTF-16 code-unit offset within the line.
    pub character: u32,
}

impl WireTextEdit {
    /// Convert into the canonical core type used by
    /// [`legend_pure_core_platform::edit::apply_text_edits`].
    fn into_core(self) -> legend_pure_core_platform::edit::TextEdit {
        legend_pure_core_platform::edit::TextEdit {
            range: legend_pure_core_platform::edit::Range {
                start: legend_pure_core_platform::edit::Position {
                    line: self.range.start.line,
                    character: self.range.start.character,
                },
                end: legend_pure_core_platform::edit::Position {
                    line: self.range.end.line,
                    character: self.range.end.character,
                },
            },
            new_text: self.new_text,
        }
    }
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

/// One reference site in the [`LegendMcpServer::find_references`]
/// response — where the target element is used in the source.
#[derive(Debug, Serialize)]
pub struct ReferenceLocation {
    /// Canonical source path of the file containing the reference.
    pub source: String,
    /// 1-based line of the reference's start position.
    pub line: u32,
    /// 1-based column of the reference's start position.
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

/// One property descriptor in [`ClassDetail`] / [`PropertyHit`].
#[derive(Debug, Serialize)]
pub struct PropertyInfo {
    /// Property name as declared on the class or association.
    pub name: String,
    /// Rendered FQN of the property's type — e.g. `"String"`,
    /// `"pkg::Address"`, `"meta::pure::metamodel::type::Generic"`. For
    /// generic instantiations the type arguments are rendered inline
    /// (`"List<pkg::Address>"`). `"<unresolved>"` if the compiler
    /// could not resolve the type (only happens in error states).
    pub type_fqn: String,
    /// Pure-form multiplicity literal: `1`, `0..1`, `*`, `1..*`, or a
    /// numeric range like `0..3`. Matches the syntax users write in
    /// `.pure` source after the type name in `[ ... ]`.
    pub multiplicity: String,
    /// 1-based source line of the property declaration. `0` for
    /// synthetic properties (e.g. bootstrap-injected `Any.classifierGenericType`).
    pub source_line: u32,
}

/// One qualified-property descriptor in [`ClassDetail`].
#[derive(Debug, Serialize)]
pub struct QualifiedPropertyInfo {
    /// QP name as declared (without parameter list).
    pub name: String,
    /// Parameters in declaration order. Empty for parameter-less QPs.
    pub parameters: Vec<ParameterInfo>,
    /// Rendered FQN of the return type.
    pub return_type_fqn: String,
    /// Return multiplicity.
    pub return_multiplicity: String,
    /// 1-based source line of the QP declaration.
    pub source_line: u32,
}

/// One parameter descriptor in [`QualifiedPropertyInfo::parameters`].
#[derive(Debug, Serialize)]
pub struct ParameterInfo {
    /// Parameter name.
    pub name: String,
    /// Rendered FQN of the parameter type.
    pub type_fqn: String,
    /// Parameter multiplicity.
    pub multiplicity: String,
}

/// One association-edge row connecting a class to another class via a
/// declared property. Returned both inline by
/// [`LegendMcpServer::read_class_detail`] and standalone by
/// [`LegendMcpServer::find_associations_for_class`].
#[derive(Debug, Serialize)]
pub struct AssociationEdge {
    /// FQN of the Association element declaring this edge.
    pub association_fqn: String,
    /// Name of the property the source class navigates *to* (the
    /// endpoint pointing at `target_class_fqn`). When `Address` has an
    /// association to `Person` with properties `[address: Address[1],
    /// owner: Person[1]]`, the edge from `Address`'s side reports
    /// `role_name = "owner"`, `target_class_fqn = "...::Person"`.
    pub role_name: String,
    /// FQN of the class on the other end of the association.
    pub target_class_fqn: String,
    /// Multiplicity of the property the source class navigates *to*.
    pub multiplicity: String,
}

/// Detailed view of one Class returned by
/// [`LegendMcpServer::read_class_detail`].
///
/// Bundles everything an agent needs to compose a Pure expression
/// against the class: property surface (declared + qualified),
/// supertypes (so inheritance chains are visible), type parameters,
/// and association edges (so multi-hop navigation is discoverable
/// from one tool call).
#[derive(Debug, Serialize)]
pub struct ClassDetail {
    /// Fully-qualified name of the class.
    pub fqn: String,
    /// Canonical source path the class was declared in.
    pub source: String,
    /// 1-based line of the class name span.
    pub line: u32,
    /// 1-based column of the class name span.
    pub column: u32,
    /// Type-parameter names in declaration order. Empty for
    /// non-generic classes.
    pub type_parameters: Vec<String>,
    /// Rendered FQNs of direct supertypes. Order matches declaration
    /// order in `extends`. For most user classes this is one entry
    /// (or empty when the class implicitly extends `Any`).
    pub super_types: Vec<String>,
    /// Declared properties (not including association-injected ones —
    /// those are surfaced via [`Self::associations`] instead).
    pub properties: Vec<PropertyInfo>,
    /// Qualified (derived) properties.
    pub qualified_properties: Vec<QualifiedPropertyInfo>,
    /// Every association that lists this class as one of its
    /// endpoints. Use these to discover navigation paths to
    /// neighbouring classes (e.g. `Person → addresses → Address`).
    pub associations: Vec<AssociationEdge>,
}

/// One property hit in the [`LegendMcpServer::search_properties`] response.
#[derive(Debug, Serialize)]
pub struct PropertyHit {
    /// FQN of the Class (or Association) the property is declared on.
    pub owner_fqn: String,
    /// Element kind of the owner — `"Class"` or `"Association"`.
    pub owner_kind: String,
    /// Property name.
    pub property_name: String,
    /// Rendered FQN of the property's type.
    pub type_fqn: String,
    /// Property multiplicity.
    pub multiplicity: String,
    /// 1-based source line of the property declaration.
    pub source_line: u32,
}

/// Response shape for [`LegendMcpServer::eval_expression`].
///
/// `ok == true` means the wrapped expression compiled cleanly and the
/// runtime returned a value (which may be empty). On compile failure
/// `diagnostics` carries the scoped errors; on runtime failure `error`
/// carries the structured runner error. Both fields may be non-empty
/// simultaneously if the compile produced warnings but evaluation
/// still failed.
#[derive(Debug, Serialize)]
pub struct EvalExpressionResult {
    /// `true` when compile + execution both succeeded.
    pub ok: bool,
    /// Rendered runtime value. `None` on compile or runtime failure.
    pub value: Option<String>,
    /// Captured `print` / `println` output.
    pub stdout: String,
    /// Compile-time diagnostics for the synthetic scratch file. Empty
    /// when the expression compiled without errors.
    pub diagnostics: Vec<DiagnosticRow>,
    /// Structured runtime error. `None` when execution succeeded or
    /// when compilation failed before execution.
    pub error: Option<serde_json::Value>,
}

/// Response shape for [`LegendMcpServer::apply_edit`].
///
/// `status` carries the freshly-recompiled snapshot's metadata so
/// the caller can confirm a new `compiled_at`. `diagnostics`
/// surfaces post-edit errors restricted to the file that was just
/// modified — workspace-wide diagnostics are still available via
/// `get_diagnostics { file: None }`.
#[derive(Debug, Serialize)]
pub struct ApplyEditResult {
    /// Post-edit workspace status (new `compiled_at`, error counts).
    pub status: WorkspaceStatus,
    /// Diagnostics restricted to the edited file. Empty when the
    /// recompile produced no errors for that file.
    pub diagnostics: Vec<DiagnosticRow>,
}

/// Status of the current workspace snapshot. Returned by
/// [`LegendMcpServer::workspace_status`] and (after recompilation)
/// by [`LegendMcpServer::reload_workspace`].
#[derive(Debug, Serialize)]
pub struct WorkspaceStatus {
    /// Wall-clock time the current snapshot was compiled, formatted
    /// as RFC 3339 (e.g. `2026-05-12T01:23:45Z`). Agents can compare
    /// against external mtimes on `.pure` sources to decide whether
    /// they need to call `reload_workspace`.
    pub compiled_at: String,
    /// Number of compile errors in the current snapshot.
    pub error_count: usize,
    /// Number of compiled chunks (bootstrap chunk + one per loaded
    /// source file).
    pub chunk_count: usize,
    /// Number of repos in the configured classpath. Stable across
    /// reloads (reload uses the same input set as startup).
    pub repo_count: usize,
}

/// rmcp `ServerHandler` that exposes the Legend Pure workspace as
/// MCP tools.
///
/// Holds the current [`WorkspaceSnapshot`] through `Mutex<Arc<…>>`
/// so the `reload_workspace` tool can swap it without touching any
/// in-flight read. Read tools clone the inner `Arc` and drop the
/// lock immediately — no lock is ever held across an `await`.
///
/// `repos` lives behind `Arc<Mutex<Vec<Repo>>>` so the
/// [`LegendMcpServer::apply_edit`] tool can mutate the matching
/// `Repo::Filesystem` file entry before re-compiling. Everywhere
/// that reads the repos clones the inner `Vec<Repo>` out of the
/// lock and drops the guard before any `.await`, so the std mutex
/// never extends across a yield point.
///
/// `repos` + `auto_imports` are kept outside the snapshot so reload
/// can recompile against the same input set as startup.
#[derive(Clone)]
pub struct LegendMcpServer {
    current: Arc<Mutex<Arc<WorkspaceSnapshot>>>,
    repos: Arc<Mutex<Vec<Repo>>>,
    auto_imports: Arc<Vec<SmolStr>>,
    // Stored on the struct because rmcp's `#[tool_handler]` macro
    // expects a `tool_router` field on `Self`. Rust's dead-code
    // analysis doesn't see through proc-macro expansions; suppress
    // explicitly.
    #[allow(dead_code)]
    tool_router: ToolRouter<LegendMcpServer>,
}

impl LegendMcpServer {
    /// Construct a server with the given initial snapshot and the
    /// repos / auto-imports that produced it (needed for
    /// `reload_workspace`).
    #[must_use]
    pub fn new(
        initial: Arc<WorkspaceSnapshot>,
        repos: Arc<Mutex<Vec<Repo>>>,
        auto_imports: Arc<Vec<SmolStr>>,
    ) -> Self {
        Self {
            current: Arc::new(Mutex::new(initial)),
            repos,
            auto_imports,
            tool_router: Self::tool_router(),
        }
    }

    /// Clone out the current snapshot under the lock, then drop the
    /// lock. Subsequent reads run on the cloned `Arc` without
    /// contention.
    fn snapshot(&self) -> Arc<WorkspaceSnapshot> {
        let guard = self.current.lock().unwrap_or_else(|p| p.into_inner());
        guard.clone()
    }

    /// Snapshot the current repo count without holding the lock
    /// across an await. Returns 0 for a poisoned mutex.
    fn repo_count(&self) -> usize {
        let guard = self
            .repos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.len()
    }

    /// Clone the current `Vec<Repo>` out of the lock and drop the
    /// guard. Caller owns the clone, so subsequent `.await` calls
    /// are safe.
    fn repos_clone(&self) -> Vec<Repo> {
        let guard = self
            .repos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    }
}

#[tool_router]
impl LegendMcpServer {
    /// Search the compiled workspace for elements whose FQN matches
    /// a substring (case-insensitive).
    #[tool(
        description = "Find Legend Pure elements (classes, functions, profiles, enums, associations) whose FQN contains the given substring. Returns matches with source location."
    )]
    pub async fn search_symbols(
        &self,
        Parameters(args): Parameters<SearchSymbolsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let result =
            tokio::task::spawn_blocking(move || search_symbols_impl(&snapshot.model, &args))
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
        let snapshot = self.snapshot();
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

    /// Call a parameter-less Pure function and return its rendered
    /// value.
    #[tool(
        description = "Execute a parameter-less Pure function by FQN and return its rendered value plus any captured stdout. Failures return a structured error with a parsed stack trace."
    )]
    async fn run_function(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let result = tokio::task::spawn_blocking(move || {
            // Wiring mirrors `legend test` (crates/cli/src/commands/test.rs):
            // relational natives + Mapping/Relational DSL populators. The
            // duplication is intentional pending the distributed-slice
            // backlog item that will auto-register both.
            let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
            let registry =
                legend_pure_runtime::native::NativeRegistry::with_extensions(&[&relational_ext]);
            let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
            let database_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
            let class_mapping_pop =
                legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
            let populators: &[&dyn legend_pure_runtime::dsl::DSLPopulator] =
                &[&mapping_pop, &database_pop, &class_mapping_pop];
            legend_pure_runtime::runner::run_function(
                &snapshot.model,
                &registry,
                populators,
                &args.fqn,
            )
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
        let snapshot = self.snapshot();
        let result = tokio::task::spawn_blocking(move || {
            let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
            let registry =
                legend_pure_runtime::native::NativeRegistry::with_extensions(&[&relational_ext]);
            let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
            let database_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
            let class_mapping_pop =
                legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
            let populators: &[&dyn legend_pure_runtime::dsl::DSLPopulator] =
                &[&mapping_pop, &database_pop, &class_mapping_pop];
            legend_pure_runtime::runner::run_test(&snapshot.model, &registry, populators, &args.fqn)
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
        let snapshot = self.snapshot();
        let result = tokio::task::spawn_blocking(move || {
            let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
            let registry =
                legend_pure_runtime::native::NativeRegistry::with_extensions(&[&relational_ext]);
            let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
            let database_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
            let class_mapping_pop =
                legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
            let populators: &[&dyn legend_pure_runtime::dsl::DSLPopulator] =
                &[&mapping_pop, &database_pop, &class_mapping_pop];
            legend_pure_runtime::runner::run_pct(
                &snapshot.model,
                &registry,
                populators,
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
        let snapshot = self.snapshot();
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

    /// Resolve an FQN to its kind + source location.
    #[tool(
        description = "Look up a Legend Pure element by FQN and return its kind + source location. Use this after search_symbols to confirm an element exists and find where it's defined."
    )]
    async fn read_element(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let fqn = args.fqn.clone();
        let result =
            tokio::task::spawn_blocking(move || read_element_impl(&snapshot.model, &args.fqn))
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

    /// List every source location that references a given element.
    #[tool(
        description = "Find all source locations that reference a Legend Pure element by FQN. Returns one row per reference site (function calls, type uses, property accesses, stereotype refs, etc.) sorted by source + line. Use this to scope a refactor before renaming or changing a signature. Class properties address with a dot suffix on the class FQN: `pkg::Person.name`. V1 limitations: association-injected properties and let-bound local variables are not yet indexed — an empty array for those targets means \"not yet supported\", not \"definitely zero usages\"."
    )]
    pub async fn find_references(
        &self,
        Parameters(args): Parameters<FindReferencesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let fqn = args.fqn.clone();
        let result = tokio::task::spawn_blocking(move || {
            find_references_impl(&snapshot.model, &snapshot.references, &args.fqn)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Some(refs) => json_result(&refs),
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
        let snapshot = self.snapshot();
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
        let snapshot = self.snapshot();
        let result = tokio::task::spawn_blocking(move || {
            list_tests_impl(&snapshot.model, args.package_prefix.as_deref())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }

    /// Return the timestamp + counters of the current workspace
    /// snapshot. Agents call this to decide whether to invoke
    /// `reload_workspace`.
    #[tool(
        description = "Return the current workspace snapshot's compiled-at timestamp (RFC 3339), error count, chunk count, and repo count. Agents can compare `compiled_at` against external file mtimes to decide whether to call reload_workspace."
    )]
    pub async fn workspace_status(&self) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let repo_count = self.repo_count();
        json_result(&status_from(&snapshot, repo_count))
    }

    /// Recompile the workspace using the same repos and auto-imports
    /// the server was started with, then swap the in-memory snapshot.
    #[tool(
        description = "Recompile the workspace from disk and swap the in-memory snapshot. Uses the same classpath / auto-imports the server was started with. Returns the new workspace_status. Call this after editing .pure files so subsequent tool calls see the updated model."
    )]
    async fn reload_workspace(&self) -> Result<CallToolResult, McpError> {
        // Clone the repo vec OUT of the std Mutex BEFORE crossing the
        // .await on `spawn_blocking` — the std Mutex must never be
        // held across an await, and the compile takes `&[Repo]`
        // anyway. Captured into the blocking closure by ownership.
        let repos = self.repos_clone();
        let auto_imports = self.auto_imports.clone();
        let new_snapshot: Arc<WorkspaceSnapshot> = tokio::task::spawn_blocking(move || {
            Arc::new(WorkspaceSnapshot::compile(&repos, &auto_imports))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        // Swap under the mutex. The `lock`/`unwrap_or_else(into_inner)`
        // pattern matches `snapshot()` — a poisoned mutex still
        // returns the inner value rather than panicking.
        {
            let mut guard = self.current.lock().unwrap_or_else(|p| p.into_inner());
            *guard = new_snapshot.clone();
        }
        tracing::info!(
            error_count = new_snapshot.error_count,
            chunks = new_snapshot.model.chunks.len(),
            compiled_at = %new_snapshot.compiled_at,
            "workspace reloaded",
        );
        let repo_count = self.repo_count();
        json_result(&status_from(&new_snapshot, repo_count))
    }

    /// Apply LSP-style ranged text edits to one `.pure` file owned
    /// by a Filesystem repo in the workspace, persist to disk, then
    /// recompile and return file-scoped diagnostics for the edited
    /// file.
    #[tool(
        description = "Apply LSP-style ranged text edits to a single `.pure` file in a Filesystem repo, then recompile the workspace and return file-scoped diagnostics. The file must already exist in the workspace (no file creation). Edits must not overlap."
    )]
    pub async fn apply_edit(
        &self,
        Parameters(args): Parameters<ApplyEditArgs>,
    ) -> Result<CallToolResult, McpError> {
        use legend_pure_core_platform::edit;

        // Translate the JSON-schema wire shapes into the core
        // [`edit::TextEdit`] values that `apply_text_edits` consumes.
        let core_edits: Vec<edit::TextEdit> = args
            .edits
            .into_iter()
            .map(WireTextEdit::into_core)
            .collect();

        // 1) Mutate the in-memory repo list under the std mutex.
        //    Scoped tight so the guard drops before any other call
        //    — disk write and recompile both run with the lock free
        //    so concurrent `workspace_status` / `reload_workspace` /
        //    `apply_edit` reads don't serialize behind us.
        let applied = {
            let mut guard = self
                .repos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            edit::apply_text_edits(&mut guard, &args.file, &core_edits)
                .map_err(|e| McpError::invalid_params(format!("apply_edit: {e}"), None))?
        };

        // 2) Persist to disk outside the lock. Synchronous fs::write
        //    on a single .pure file is microseconds in practice; if
        //    we ever start editing multi-megabyte sources, wrap this
        //    in `spawn_blocking` to free the tokio executor thread.
        edit::write_to_disk(&applied)
            .map_err(|e| McpError::internal_error(format!("write_to_disk: {e}"), None))?;

        // 3) Snapshot the now-mutated repo list and recompile on
        //    the blocking pool. The compile takes `&[Repo]`, so we
        //    move the clone into the closure.
        let repos = self.repos_clone();
        let auto_imports = self.auto_imports.clone();
        let new_snapshot: Arc<WorkspaceSnapshot> = tokio::task::spawn_blocking(move || {
            Arc::new(WorkspaceSnapshot::compile(&repos, &auto_imports))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;

        // 4) Swap snapshot under the current-snapshot mutex.
        {
            let mut guard = self
                .current
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = new_snapshot.clone();
        }

        // 5) Build a file-scoped diagnostic view from the new
        //    snapshot.
        let canonical = applied.canonical_path.as_str();
        let diagnostics: Vec<DiagnosticRow> = new_snapshot
            .diagnostics
            .get(canonical)
            .map(|errs| errs.iter().map(diagnostic_row).collect())
            .unwrap_or_default();

        let repo_count = self.repo_count();
        tracing::info!(
            file = canonical,
            error_count = new_snapshot.error_count,
            file_error_count = diagnostics.len(),
            compiled_at = %new_snapshot.compiled_at,
            "apply_edit applied + recompiled",
        );

        json_result(&ApplyEditResult {
            status: status_from(&new_snapshot, repo_count),
            diagnostics,
        })
    }

    /// Return a Class's properties, qualified properties, supertypes,
    /// type parameters, and association edges in one call.
    #[tool(
        description = "Inspect a Class by FQN: returns declared properties (name, type FQN, multiplicity), qualified (derived) properties, supertypes, type parameters, and every Association that lists this class as an endpoint. Use after search_symbols to discover what fields you can navigate and which neighbouring classes are reachable. Errors if the FQN doesn't resolve or doesn't name a Class."
    )]
    pub async fn read_class_detail(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let result =
            tokio::task::spawn_blocking(move || read_class_detail_impl(&snapshot.model, &args.fqn))
                .await
                .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Ok(detail) => json_result(&detail),
            Err(msg) => Err(McpError::invalid_params(msg, None)),
        }
    }

    /// List every Association that names a given Class as one of its
    /// endpoints. The reverse-direction lookup for navigation.
    #[tool(
        description = "List every Association in the workspace whose properties reference the given Class FQN. Returns one AssociationEdge per endpoint that points away from the input class (association_fqn, role_name, target_class_fqn, multiplicity). Use when you've located a class and need to discover what other classes link to it. Errors if the FQN doesn't resolve or doesn't name a Class."
    )]
    pub async fn find_associations_for_class(
        &self,
        Parameters(args): Parameters<FqnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let result = tokio::task::spawn_blocking(move || {
            find_associations_for_class_impl(&snapshot.model, &args.fqn)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        match result {
            Ok(edges) => json_result(&edges),
            Err(msg) => Err(McpError::invalid_params(msg, None)),
        }
    }

    /// Find every property across every Class / Association whose
    /// declared name contains a substring.
    #[tool(
        description = "Search across every Class and Association in the workspace for properties whose declared name contains the given substring (case-insensitive). Returns (owner_fqn, owner_kind, property_name, type_fqn, multiplicity, source_line). Use when the user mentions a field name (e.g. \"zipcode\") and you don't yet know which class owns it. Default limit 50, cap 500."
    )]
    pub async fn search_properties(
        &self,
        Parameters(args): Parameters<SearchPropertiesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.snapshot();
        let result =
            tokio::task::spawn_blocking(move || search_properties_impl(&snapshot.model, &args))
                .await
                .map_err(|e| McpError::internal_error(format!("join error: {e}"), None))?;
        json_result(&result)
    }

    /// Compile + evaluate a Pure expression against the current
    /// workspace without persisting any source files.
    #[tool(
        description = "Evaluate a Pure expression against the current workspace snapshot. The expression is wrapped in `function __mcp_eval__::__eval__():Any[*] { <expr> }` and compiled into a transient in-memory repo alongside the snapshot; the persistent workspace is not mutated. Returns ok + value (or stdout) on success, or scoped compile diagnostics + structured runtime errors on failure. Use to validate a candidate expression before returning it to the user."
    )]
    pub async fn eval_expression(
        &self,
        Parameters(args): Parameters<EvalExpressionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let repos = self.repos_clone();
        let auto_imports = self.auto_imports.clone();
        let expression = args.expression;
        let result = tokio::task::spawn_blocking(move || {
            eval_expression_impl(&repos, &auto_imports, &expression)
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
             - read_class_detail / find_associations_for_class / search_properties — inspect a class's surface (properties, qualified properties, supertypes, association edges) and locate properties by name across classes\n\
             - find_references — list every source site that references a given element by FQN\n\
             - get_diagnostics — surface compile errors\n\
             - run_function / run_test / run_pct / list_pct_adapters — execute Pure code\n\
             - eval_expression — compile + run an ad-hoc Pure expression against the current snapshot without persisting any file (use to validate a generated expression)\n\
             - workspace_status / reload_workspace / apply_edit — inspect, refresh, or mutate the in-memory snapshot\n\
             \nThe workspace is compiled once at startup. Call reload_workspace after editing .pure files so subsequent tool calls see the updated model; use workspace_status to check the current snapshot's compiled_at timestamp."
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

fn status_from(snapshot: &WorkspaceSnapshot, repo_count: usize) -> WorkspaceStatus {
    WorkspaceStatus {
        compiled_at: snapshot.compiled_at.to_string(),
        error_count: snapshot.error_count,
        chunk_count: snapshot.model.chunks.len(),
        repo_count,
    }
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

fn find_references_impl(
    model: &PureModel,
    references: &ReferenceIndex,
    fqn: &str,
) -> Option<Vec<ReferenceLocation>> {
    // `None` here = element doesn't exist in the workspace; bubbles
    // up as a 400-style McpError so the agent learns the FQN was bad.
    // `Some(vec![])` = element exists but isn't referenced anywhere —
    // a valid empty result, surfaced as `[]`.
    //
    // A `.` in the FQN routes to property lookup: `pkg::Class.prop`.
    // Pure packages use `::`, so `.` unambiguously separates the
    // class FQN from the property name. Split on the **last** `.`
    // so dotted property names (`Profile.tagName`-style) still
    // round-trip correctly should the addressing convention ever
    // expand.
    let usages: Vec<legend_pure_ide::RefLocation> = if let Some(dot_idx) = fqn.rfind('.') {
        let (class_fqn, prop_name) = (&fqn[..dot_idx], &fqn[dot_idx + 1..]);
        if prop_name.is_empty() {
            return None;
        }
        let class_id = model.resolve_fqn_str(class_fqn)?;
        // Only Class / Association declare properties.
        match model.try_get_element(class_id) {
            Some(legend_pure_parser_pure::model::Element::Class(_))
            | Some(legend_pure_parser_pure::model::Element::Association(_)) => {}
            _ => return None,
        }
        // Verify the property actually exists on the class chain
        // before deciding "no usages" vs. "doesn't exist".
        if !legend_pure_parser_pure::resolve::class_has_property_in_hierarchy(
            model, class_id, prop_name,
        ) {
            return None;
        }
        references
            .usages_of_property(class_id, prop_name)
            .unwrap_or(&[])
            .to_vec()
    } else {
        let id = model.resolve_fqn_str(fqn)?;
        references.usages_of(id).unwrap_or(&[]).to_vec()
    };

    let mut out: Vec<ReferenceLocation> = usages
        .iter()
        .map(|loc| ReferenceLocation {
            source: loc.canonical_path.to_string(),
            line: loc.range.start_line,
            column: loc.range.start_column,
        })
        .collect();
    out.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });
    Some(out)
}

fn list_packages_impl(model: &PureModel, prefix: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (pkg_id, _pkg) in model.global_packages.iter() {
        let fqn = render_package_fqn(model, legend_pure_parser_pure::ids::PackageId(pkg_id));
        if fqn.is_empty() {
            continue; // root package — not a useful tool output
        }
        if let Some(p) = prefix
            && !fqn.starts_with(p)
        {
            continue;
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
                let Some(profile_name) = model
                    .try_get_element(s.profile)
                    .map(|_| render_fqn(model, s.profile))
                else {
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
            if let Some(p) = package_prefix
                && !fqn.starts_with(p)
            {
                continue;
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

fn render_package_fqn(
    model: &PureModel,
    pkg_id: legend_pure_parser_pure::ids::PackageId,
) -> String {
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

/// Render a `TypeExpr` to a Pure-flavoured FQN string. Differs from
/// the private helper in `pure::access::render_type_expr` in that the
/// `Named` element is rendered as its full FQN (`pkg::Class`) rather
/// than just its simple name, so the agent can resolve it back via
/// `read_element` / `read_class_detail` without guessing the package.
fn render_type_fqn(model: &PureModel, type_expr: &TypeExpr) -> String {
    let mut out = String::new();
    render_type_fqn_into(model, type_expr, &mut out);
    out
}

fn render_type_fqn_into(model: &PureModel, type_expr: &TypeExpr, out: &mut String) {
    match type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            out.push_str(&render_fqn(model, *element));
            if !type_arguments.is_empty() {
                out.push('<');
                for (i, arg) in type_arguments.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_type_fqn_into(model, arg, out);
                }
                out.push('>');
            }
        }
        TypeExpr::Generic(name) => out.push_str(name),
        TypeExpr::FunctionType {
            parameters,
            return_type,
            return_multiplicity,
        } => {
            out.push('{');
            for (i, (pty, pm)) in parameters.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_type_fqn_into(model, pty, out);
                out.push('[');
                out.push_str(&render_multiplicity(pm));
                out.push(']');
            }
            out.push_str(" -> ");
            render_type_fqn_into(model, return_type, out);
            out.push('[');
            out.push_str(&render_multiplicity(return_multiplicity));
            out.push(']');
            out.push('}');
        }
        TypeExpr::AlgebraUnion(a, b) => {
            render_type_fqn_into(model, a, out);
            out.push_str(" + ");
            render_type_fqn_into(model, b, out);
        }
        TypeExpr::Relation(_) => out.push_str("<Relation>"),
        TypeExpr::Unresolved => out.push_str("<unresolved>"),
    }
}

fn render_multiplicity(m: &Multiplicity) -> String {
    match m {
        Multiplicity::PureOne => "1".to_string(),
        Multiplicity::ZeroOrOne => "0..1".to_string(),
        Multiplicity::ZeroOrMany => "*".to_string(),
        Multiplicity::OneOrMany => "1..*".to_string(),
        Multiplicity::Range { lower, upper } => match upper {
            Some(u) if u == lower => format!("{lower}"),
            Some(u) => format!("{lower}..{u}"),
            None => format!("{lower}..*"),
        },
        Multiplicity::Variable(v) => v.to_string(),
    }
}

/// Convert a `Property` value into the agent-facing [`PropertyInfo`].
/// Shared by `read_class_detail` and `search_properties`.
fn property_info(model: &PureModel, prop: &Property) -> PropertyInfo {
    PropertyInfo {
        name: prop.name.to_string(),
        type_fqn: render_type_fqn(model, &prop.type_expr),
        multiplicity: render_multiplicity(&prop.multiplicity),
        source_line: prop.source_info.start_line,
    }
}

fn qualified_property_info(model: &PureModel, qp: &QualifiedProperty) -> QualifiedPropertyInfo {
    let parameters = qp
        .parameters
        .iter()
        .map(|p| ParameterInfo {
            name: p.name.to_string(),
            type_fqn: render_type_fqn(model, &p.type_expr),
            multiplicity: render_multiplicity(&p.multiplicity),
        })
        .collect();
    QualifiedPropertyInfo {
        name: qp.name.to_string(),
        parameters,
        return_type_fqn: render_type_fqn(model, &qp.return_type),
        return_multiplicity: render_multiplicity(&qp.return_multiplicity),
        source_line: qp.source_info.start_line,
    }
}

/// Walk every Association in the model and yield one
/// [`AssociationEdge`] for each endpoint that points *away from*
/// `class_id`. So if `Assoc` has properties `[address: Address[1],
/// owner: Person[1]]` and `class_id` refers to `Address`, the result
/// includes one edge with `role_name = "owner"`, `target_class_fqn =
/// "...::Person"`. Same association consulted from `Person`'s side
/// would emit the `"address"` edge. Self-associations yield both
/// edges (one per endpoint) when the property names differ.
fn scan_association_edges(model: &PureModel, class_id: ElementId) -> Vec<AssociationEdge> {
    let mut out: Vec<AssociationEdge> = Vec::new();
    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Association(assoc) = element else {
                continue;
            };
            // First pass: does any endpoint point at class_id? Skip
            // the whole association if not — avoids scanning every
            // property's type_expr twice for unrelated associations.
            let endpoints_referencing: Vec<usize> = assoc
                .properties
                .iter()
                .enumerate()
                .filter_map(|(idx, p)| match &p.type_expr {
                    TypeExpr::Named { element, .. } if *element == class_id => Some(idx),
                    _ => None,
                })
                .collect();
            if endpoints_referencing.is_empty() {
                continue;
            }
            let assoc_id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            let assoc_fqn = render_fqn(model, assoc_id);
            // Emit one edge per *other* endpoint — the property the
            // input class navigates to. For a 2-prop association where
            // both endpoints are class_id (self-association on the
            // same class with different role names), every endpoint
            // counts as an "other" endpoint.
            for (idx, prop) in assoc.properties.iter().enumerate() {
                let TypeExpr::Named { element: tgt, .. } = &prop.type_expr else {
                    continue;
                };
                // Skip the endpoint(s) that point back at us, unless
                // every endpoint does (self-association case).
                let is_self_assoc = endpoints_referencing.len() == assoc.properties.len();
                if !is_self_assoc && endpoints_referencing.contains(&idx) {
                    continue;
                }
                out.push(AssociationEdge {
                    association_fqn: assoc_fqn.clone(),
                    role_name: prop.name.to_string(),
                    target_class_fqn: render_fqn(model, *tgt),
                    multiplicity: render_multiplicity(&prop.multiplicity),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.association_fqn
            .cmp(&b.association_fqn)
            .then(a.role_name.cmp(&b.role_name))
    });
    out
}

/// Backing impl for [`LegendMcpServer::read_class_detail`].
///
/// `Err(msg)` carries the user-facing error text; the tool wrapper
/// turns it into a `McpError::invalid_params`.
fn read_class_detail_impl(model: &PureModel, fqn: &str) -> Result<ClassDetail, String> {
    let id = model
        .resolve_fqn_str(fqn)
        .ok_or_else(|| format!("element not found in workspace: {fqn}"))?;
    let element = model
        .try_get_element(id)
        .ok_or_else(|| format!("element not found in workspace: {fqn}"))?;
    let Element::Class(class) = element else {
        return Err(format!(
            "element '{fqn}' is not a Class (kind: {}); use read_element for non-Class elements",
            element_kind(element)
        ));
    };
    Ok(build_class_detail(model, id, class))
}

fn build_class_detail(model: &PureModel, id: ElementId, class: &Class) -> ClassDetail {
    let node = model.get_node(id);
    let type_parameters = class
        .type_parameters
        .iter()
        .map(|tp| tp.name.to_string())
        .collect();
    let super_types = class
        .super_types
        .iter()
        .map(|st| render_type_fqn(model, st))
        .collect();
    let properties = class
        .properties
        .iter()
        .map(|p| property_info(model, p))
        .collect();
    let qualified_properties = class
        .qualified_properties
        .iter()
        .map(|qp| qualified_property_info(model, qp))
        .collect();
    let associations = scan_association_edges(model, id);
    ClassDetail {
        fqn: render_fqn(model, id),
        source: node.source_info.source.to_string(),
        line: node.source_info.start_line,
        column: node.source_info.start_column,
        type_parameters,
        super_types,
        properties,
        qualified_properties,
        associations,
    }
}

/// Backing impl for [`LegendMcpServer::find_associations_for_class`].
fn find_associations_for_class_impl(
    model: &PureModel,
    fqn: &str,
) -> Result<Vec<AssociationEdge>, String> {
    let id = model
        .resolve_fqn_str(fqn)
        .ok_or_else(|| format!("element not found in workspace: {fqn}"))?;
    let element = model
        .try_get_element(id)
        .ok_or_else(|| format!("element not found in workspace: {fqn}"))?;
    if !matches!(element, Element::Class(_)) {
        return Err(format!(
            "element '{fqn}' is not a Class (kind: {})",
            element_kind(element)
        ));
    }
    Ok(scan_association_edges(model, id))
}

/// Backing impl for [`LegendMcpServer::search_properties`]. Mirrors
/// the bootstrap-skip and limit-cap conventions of
/// [`search_symbols_impl`].
fn search_properties_impl(model: &PureModel, args: &SearchPropertiesArgs) -> Vec<PropertyHit> {
    let needle = args.name_substring.to_lowercase();
    let limit = args.limit.unwrap_or(50).min(500) as usize;
    let mut out: Vec<PropertyHit> = Vec::new();
    'outer: for chunk in &model.chunks {
        if chunk.chunk_id == 0 {
            continue;
        }
        for (local_idx, element) in chunk.elements.iter() {
            let id = ElementId::InstanceId {
                chunk_id: chunk.chunk_id,
                local_idx,
            };
            match element {
                Element::Class(c) => {
                    if push_property_hits(
                        model,
                        id,
                        "Class",
                        &c.properties,
                        &needle,
                        &mut out,
                        limit,
                    ) {
                        break 'outer;
                    }
                }
                Element::Association(a) => {
                    if push_property_hits(
                        model,
                        id,
                        "Association",
                        &a.properties,
                        &needle,
                        &mut out,
                        limit,
                    ) {
                        break 'outer;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Append matching property hits onto `out`, returning `true` when
/// the cap was reached so the caller can break out of its outer
/// scan loop.
fn push_property_hits(
    model: &PureModel,
    owner_id: ElementId,
    owner_kind: &str,
    properties: &[Property],
    needle_lower: &str,
    out: &mut Vec<PropertyHit>,
    limit: usize,
) -> bool {
    for prop in properties {
        if !prop.name.to_lowercase().contains(needle_lower) {
            continue;
        }
        out.push(PropertyHit {
            owner_fqn: render_fqn(model, owner_id),
            owner_kind: owner_kind.to_string(),
            property_name: prop.name.to_string(),
            type_fqn: render_type_fqn(model, &prop.type_expr),
            multiplicity: render_multiplicity(&prop.multiplicity),
            source_line: prop.source_info.start_line,
        });
        if out.len() >= limit {
            return true;
        }
    }
    false
}

/// Canonical URL prefix the eval tool uses for its synthetic in-memory
/// repo. The leading `/` is required by the loader (matches the
/// `Repo::Filesystem.prefix` convention everywhere else).
const MCP_SCRATCH_PREFIX: &str = "/__mcp_scratch__";
/// Canonical URL of the synthetic source file inside [`MCP_SCRATCH_PREFIX`].
const MCP_SCRATCH_PATH: &str = "/__mcp_scratch__/eval.pure";
/// FQN of the synthetic function the eval tool runs. The single-segment
/// package name avoids any chance of colliding with real workspace
/// elements (no real repo lives under `__mcp_eval__::`).
const MCP_SCRATCH_FQN: &str = "__mcp_eval__::__eval__";

/// Backing impl for [`LegendMcpServer::eval_expression`].
///
/// Wraps `expression` in a synthetic parameter-less function returning
/// `Any[*]`, appends an in-memory `Repo::Filesystem` carrying just
/// that source file, recompiles the snapshot, and (if compilation
/// succeeds) runs the synthetic function. The persistent workspace
/// snapshot on the server is not mutated — only the scratch model
/// built here sees the synthetic chunk.
///
/// `repos` is the cloned snapshot of the server's repo list (taken
/// before this function ran so the std mutex is no longer held).
/// `auto_imports` is the same list the server started with.
fn eval_expression_impl(
    repos: &[Repo],
    auto_imports: &[SmolStr],
    expression: &str,
) -> EvalExpressionResult {
    let mut augmented: Vec<Repo> = repos.to_vec();
    // Emit explicit `import` directives for every configured
    // auto-import. The compiler applies auto-imports lazily through
    // the section's import scope; without an explicit `###Pure`
    // section header + `import` lines, ad-hoc arrow calls like
    // `->filter(...)` cannot resolve the platform function names
    // even though those packages are technically auto-imported for
    // the rest of the workspace. Writing the imports inline makes
    // the scratch file behave the same way regardless of the
    // server's auto-import configuration.
    let imports = if auto_imports.is_empty() {
        String::new()
    } else {
        let mut s = String::with_capacity(auto_imports.len() * 48);
        for pkg in auto_imports {
            s.push_str("import ");
            s.push_str(pkg);
            s.push_str("::*;\n");
        }
        s
    };
    let wrapped =
        format!("###Pure\n{imports}function {MCP_SCRATCH_FQN}():Any[*]\n{{\n  {expression}\n}}\n");
    // The synthetic repo needs a `RepoMeta` so `topo_sort_repos` can
    // place it after every other loaded repo. Its dependency list
    // must enumerate every other repo by name so the visibility map
    // (populated by `populate_repo_visibility`) lets the scratch
    // chunk reference user classes (e.g. `pkg::Person`) defined in
    // other repos. `RepoMeta` carries three `&'static str` slots
    // (the type predates this MCP integration), so the deps slice is
    // built with `Box::leak`. The leak is bounded — `O(repo_count)`
    // pointers per eval call — and the MCP server is long-lived
    // enough that the alternative (pinning the meta on the server
    // and reusing it across calls) would also need a guard for the
    // repo-list-changed case. Document and accept.
    let scratch_meta = make_scratch_meta(repos);
    augmented.push(Repo::Filesystem {
        prefix: MCP_SCRATCH_PREFIX.to_string(),
        files: vec![OwnedSourceFile {
            path: MCP_SCRATCH_PATH.to_string(),
            content: wrapped,
        }],
        meta: Some(scratch_meta),
        source_root: None,
    });
    let snapshot = WorkspaceSnapshot::compile(&augmented, auto_imports);
    // Diagnostics: surface (a) anything scoped to the scratch file and
    // (b) any errors whose message text mentions the scratch FQN.
    // Pre-existing workspace errors that didn't mention the scratch
    // file or function aren't this tool's problem to report.
    let diagnostics: Vec<DiagnosticRow> = snapshot
        .diagnostics
        .iter()
        .flat_map(|(path, errs)| {
            errs.iter().filter_map(move |e| {
                if path == MCP_SCRATCH_PATH
                    || e.source_info.source.contains(MCP_SCRATCH_PREFIX)
                    || e.message.contains(MCP_SCRATCH_FQN)
                {
                    Some(diagnostic_row(e))
                } else {
                    None
                }
            })
        })
        .collect();

    // If the scratch chunk failed to produce a runnable function,
    // surface diagnostics only — running a missing function produces
    // a noisy stack trace the user would otherwise have to ignore.
    // `resolve_function_by_path` does prefix-matching on the mangled
    // name; `resolve_fqn_str` would require the caller to pre-mangle.
    let scratch_segments: Vec<SmolStr> = MCP_SCRATCH_FQN.split("::").map(SmolStr::new).collect();
    if snapshot
        .model
        .resolve_function_by_path(&scratch_segments)
        .is_none()
    {
        return EvalExpressionResult {
            ok: false,
            value: None,
            stdout: String::new(),
            diagnostics,
            error: None,
        };
    }

    // Wiring mirrors `run_function`'s tool path. Pending the
    // distributed-slice unification, populators + relational natives
    // are listed inline at every call site.
    let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
    let registry = legend_pure_runtime::native::NativeRegistry::with_extensions(&[&relational_ext]);
    let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
    let database_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
    let class_mapping_pop = legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
    let populators: &[&dyn legend_pure_runtime::dsl::DSLPopulator] =
        &[&mapping_pop, &database_pop, &class_mapping_pop];
    let run_result = legend_pure_runtime::runner::run_function(
        &snapshot.model,
        &registry,
        populators,
        MCP_SCRATCH_FQN,
    );

    let error = if run_result.ok {
        None
    } else {
        // RunResult.error is a structured RunError; pass it through
        // as JSON so the agent sees the typed stack trace rather than
        // a flattened string.
        serde_json::to_value(&run_result.error).ok()
    };
    EvalExpressionResult {
        ok: run_result.ok,
        value: run_result.value,
        stdout: run_result.stdout,
        diagnostics,
        error,
    }
}

/// Build the synthetic [`RepoMeta`] for the scratch repo. Dependencies
/// list every other repo's name so the visibility map allows the
/// scratch chunk to reference any user class. Name + pattern are
/// fixed literals. See the call site in [`eval_expression_impl`] for
/// the per-call leak rationale.
fn make_scratch_meta(other_repos: &[Repo]) -> RepoMeta {
    // `RepoMeta.name` on every well-formed repo is already
    // `&'static str` (the type carries that lifetime, populated
    // either from a `&'static` constant or via a one-time leak in
    // the repo constructor). Borrow the slot directly — no per-call
    // leak needed for the names themselves. Only the *outer*
    // dependency slice needs a fresh leak so its length matches the
    // live repo count.
    let deps: Vec<&'static str> = other_repos
        .iter()
        .filter_map(|r| r.meta().map(|m| m.name))
        .collect();
    let dependencies: &'static [&'static str] = Box::leak(deps.into_boxed_slice());
    RepoMeta {
        name: "__mcp_scratch__",
        pattern: "(.*)",
        dependencies,
    }
}
