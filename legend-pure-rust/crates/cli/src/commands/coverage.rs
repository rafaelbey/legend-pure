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

//! Pure code coverage instrumentation.
//!
//! Provides [`CoverageHooks`], an [`EvalHooks`] implementation that records
//! line-level, branch-level, and function-level coverage data during Pure
//! expression evaluation. The collected data is stored in a [`CoverageMap`]
//! which can be serialized to LCOV format for consumption by standard tools
//! like `genhtml`.
//!
//! # Usage
//!
//! ```ignore
//! use legend_pure_runtime::coverage::CoverageHooks;
//! use legend_pure_runtime::eval::Evaluator;
//!
//! let mut hooks = CoverageHooks::new("");
//! hooks.map_mut().populate_coverable(&model);
//! let mut evaluator = Evaluator::with_hooks(&model, &registry, hooks);
//! // ... run tests ...
//! let map = evaluator.into_hooks().into_map();
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{ExprKind, ValueSpec};
use smol_str::SmolStr;

use legend_pure_runtime::hooks::EvalHooks;
use legend_pure_runtime::value::Value;

// ---------------------------------------------------------------------------
// Line Coverage
// ---------------------------------------------------------------------------

/// Per-file line coverage data.
#[derive(Debug, Default)]
pub struct FileCoverage {
    /// `line_number → execution_count`. Only lines that were actually
    /// executed appear here (lazy insertion on first hit).
    pub line_hits: BTreeMap<u32, u64>,
    /// All lines that contain evaluable expression nodes. Populated by
    /// [`CoverageMap::populate_coverable`] before execution.
    pub coverable_lines: BTreeSet<u32>,
}

impl FileCoverage {
    /// Number of coverable lines in this file.
    #[must_use]
    pub fn lines_found(&self) -> u32 {
        u32::try_from(self.coverable_lines.len()).unwrap_or(u32::MAX)
    }

    /// Number of coverable lines that were executed at least once.
    #[must_use]
    pub fn lines_hit(&self) -> u32 {
        self.coverable_lines
            .iter()
            .filter(|line| self.line_hits.get(line).is_some_and(|&c| c > 0))
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Line coverage percentage (0.0–100.0). Returns 100.0 if no coverable lines.
    #[must_use]
    pub fn line_percentage(&self) -> f64 {
        let found = self.lines_found();
        if found == 0 {
            return 100.0;
        }
        (f64::from(self.lines_hit()) / f64::from(found)) * 100.0
    }
}

// ---------------------------------------------------------------------------
// Branch Coverage
// ---------------------------------------------------------------------------

/// The kind of branching construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    /// `if(cond, {true}, {false})` — always 2 arms.
    If,
    /// `$x->match([...lambdas...])` — N arms.
    Match,
}

/// One arm of a branch point (one lambda body in `if`/`match`).
#[derive(Debug)]
#[allow(dead_code)]
pub struct BranchArm {
    /// Source location of the lambda argument.
    pub source: SourceInfo,
    /// Number of times this arm was entered during execution.
    pub hit_count: u64,
}

/// A single branch point — one `if` or `match` call site.
#[derive(Debug)]
#[allow(dead_code)]
pub struct BranchPoint {
    /// Source location of the branching expression (`if`/`match` call).
    pub call_source: SourceInfo,
    /// Whether this is an `if` or `match`.
    pub kind: BranchKind,
    /// Per-arm tracking. Index = branch number.
    pub arms: Vec<BranchArm>,
}

/// Tracks all branch points across the program.
#[derive(Debug, Default)]
pub struct BranchTracker {
    /// All registered branch points, indexed for O(1) lookup.
    pub(crate) points: Vec<BranchPoint>,
    /// Reverse map: lambda `SourceInfo` → `(branch_point_index, arm_index)`.
    /// Used by the hot path in `before_eval` to mark arms as taken.
    pub(crate) lambda_to_branch: HashMap<SourceInfo, (usize, usize)>,
}

impl BranchTracker {
    /// Total number of branch arms found across all branch points.
    #[must_use]
    pub fn branches_found(&self) -> u32 {
        self.points
            .iter()
            .map(|p| u32::try_from(p.arms.len()).unwrap_or(u32::MAX))
            .sum()
    }

    /// Number of branch arms that were taken at least once.
    #[must_use]
    pub fn branches_hit(&self) -> u32 {
        self.points
            .iter()
            .flat_map(|p| &p.arms)
            .filter(|arm| arm.hit_count > 0)
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Branch points belonging to a specific source file.
    pub fn points_in_file<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a BranchPoint> {
        self.points
            .iter()
            .filter(move |p| p.call_source.source.as_str() == source)
    }
}

// ---------------------------------------------------------------------------
// Function Coverage
// ---------------------------------------------------------------------------

/// A tracked function definition.
#[derive(Debug)]
pub struct FunctionEntry {
    /// Source location of the function definition.
    pub source: SourceInfo,
    /// Number of times this function was called.
    pub hit_count: u64,
}

/// Tracks which functions were defined vs. called.
#[derive(Debug, Default)]
pub struct FunctionTracker {
    /// FQN → entry. Pre-populated with all non-native functions.
    pub(crate) functions: BTreeMap<SmolStr, FunctionEntry>,
}

impl FunctionTracker {
    /// Register a function definition (pre-scan).
    pub fn register(&mut self, fqn: SmolStr, source: SourceInfo) {
        self.functions.entry(fqn).or_insert(FunctionEntry {
            source,
            hit_count: 0,
        });
    }

    /// Record a function call (runtime hot path).
    pub fn record_call(&mut self, name: &str) {
        if let Some(entry) = self.functions.get_mut(name) {
            entry.hit_count += 1;
        }
    }

    /// Total number of tracked functions.
    #[must_use]
    pub fn functions_found(&self) -> u32 {
        u32::try_from(self.functions.len()).unwrap_or(u32::MAX)
    }

    /// Number of functions that were called at least once.
    #[must_use]
    pub fn functions_hit(&self) -> u32 {
        self.functions
            .values()
            .filter(|e| e.hit_count > 0)
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Functions belonging to a specific source file.
    pub fn functions_in_file<'a>(
        &'a self,
        source: &'a str,
    ) -> impl Iterator<Item = (&'a SmolStr, &'a FunctionEntry)> {
        self.functions
            .iter()
            .filter(move |(_, entry)| entry.source.source.as_str() == source)
    }
}

// ---------------------------------------------------------------------------
// CoverageMap — aggregated coverage data
// ---------------------------------------------------------------------------

/// Aggregated coverage data across all Pure source files.
///
/// Created by [`CoverageHooks`] during execution. After evaluation,
/// serialize to LCOV format using the CLI's coverage report module.
#[derive(Debug, Default)]
pub struct CoverageMap {
    /// Per-source-file line coverage. Key = source path from `SourceInfo.source`.
    files: BTreeMap<SmolStr, FileCoverage>,
    /// Branch tracking data.
    pub branches: BranchTracker,
    /// Function tracking data.
    pub functions: FunctionTracker,
    /// Known test function FQNs (functions with `<<test.Test>>` or `<<PCT.test>>`).
    /// Populated by [`populate_coverable`] during model pre-scan.
    pub test_fqns: HashSet<SmolStr>,
    /// Per-test coverage attribution: test FQN → set of `(source_file, line)`.
    /// Populated at runtime when `current_test` is active in [`CoverageHooks`].
    pub test_attribution: BTreeMap<SmolStr, BTreeSet<(SmolStr, u32)>>,
}

/// Summary statistics for all coverage dimensions.
#[derive(Debug)]
#[allow(dead_code)]
pub struct CoverageSummary {
    /// Total coverable lines across all files.
    pub lines_found: u32,
    /// Total lines hit.
    pub lines_hit: u32,
    /// Line coverage percentage (0.0–100.0).
    pub line_percentage: f64,
    /// Total branch arms found.
    pub branches_found: u32,
    /// Total branch arms hit.
    pub branches_hit: u32,
    /// Branch coverage percentage (0.0–100.0).
    pub branch_percentage: f64,
    /// Total functions found.
    pub functions_found: u32,
    /// Total functions hit.
    pub functions_hit: u32,
    /// Function coverage percentage (0.0–100.0).
    pub function_percentage: f64,
}

impl CoverageMap {
    /// Create an empty coverage map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a line hit (hot path). Inserts the file entry if not yet present.
    pub fn record_hit(&mut self, source: &SmolStr, line: u32) {
        *self
            .files
            .entry(source.clone())
            .or_default()
            .line_hits
            .entry(line)
            .or_insert(0) += 1;
    }

    /// Register a line as coverable (pre-scan).
    pub fn mark_coverable(&mut self, source: &SmolStr, line: u32) {
        self.files
            .entry(source.clone())
            .or_default()
            .coverable_lines
            .insert(line);
    }

    /// Iterate over all tracked files and their coverage data.
    pub fn files(&self) -> impl Iterator<Item = (&SmolStr, &FileCoverage)> {
        self.files.iter()
    }

    /// Get coverage data for a specific file.
    #[must_use]
    #[allow(dead_code)]
    pub fn file_coverage(&self, source: &str) -> Option<&FileCoverage> {
        self.files.get(source)
    }

    /// Compute summary statistics.
    #[must_use]
    pub fn summary(&self) -> CoverageSummary {
        let lines_found: u32 = self.files.values().map(FileCoverage::lines_found).sum();
        let lines_hit: u32 = self.files.values().map(FileCoverage::lines_hit).sum();
        let line_percentage = if lines_found == 0 {
            100.0
        } else {
            (f64::from(lines_hit) / f64::from(lines_found)) * 100.0
        };

        let branches_found = self.branches.branches_found();
        let branches_hit = self.branches.branches_hit();
        let branch_percentage = if branches_found == 0 {
            100.0
        } else {
            (f64::from(branches_hit) / f64::from(branches_found)) * 100.0
        };

        let functions_found = self.functions.functions_found();
        let functions_hit = self.functions.functions_hit();
        let function_percentage = if functions_found == 0 {
            100.0
        } else {
            (f64::from(functions_hit) / f64::from(functions_found)) * 100.0
        };

        CoverageSummary {
            lines_found,
            lines_hit,
            line_percentage,
            branches_found,
            branches_hit,
            branch_percentage,
            functions_found,
            functions_hit,
            function_percentage,
        }
    }

    // -- Model pre-scan ---------------------------------------------------

    /// Walk the compiled model to discover all coverable lines, branch
    /// points (`if`/`match`), and function definitions.
    ///
    /// Call this after compilation but before execution.
    pub fn populate_coverable(&mut self, model: &PureModel) {
        for chunk in &model.chunks {
            for (local_idx, element) in chunk.elements.iter() {
                let node = chunk.nodes.get(local_idx);

                match element {
                    Element::Function(func) => {
                        // Skip native functions — they have no Pure body.
                        if func.is_native {
                            continue;
                        }

                        // Register the function for function-level coverage.
                        self.functions
                            .register(node.name.clone(), node.source_info.clone());

                        // Walk function body expressions.
                        for expr in func.body.iter() {
                            self.walk_expr_coverable(expr);
                        }
                    }
                    Element::Class(class) => {
                        // Walk constraint bodies.
                        for constraint in &class.constraints {
                            self.walk_expr_coverable(&constraint.function);
                            if let Some(ref msg) = constraint.message {
                                self.walk_expr_coverable(msg);
                            }
                        }
                        // Walk qualified property bodies.
                        for qp in &class.qualified_properties {
                            // Register QP as a function for function-level coverage.
                            let qp_fqn = SmolStr::new(format!("{}.{}", node.name, qp.name));
                            self.functions.register(qp_fqn, qp.source_info.clone());

                            for expr in qp.body.iter() {
                                self.walk_expr_coverable(expr);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Recursively walk an expression tree, marking all lines as coverable
    /// and registering branch points for `if`/`match` calls.
    fn walk_expr_coverable(&mut self, expr: &ValueSpec) {
        // Mark every line spanned by this expression as coverable.
        for line in expr.source_info.start_line..=expr.source_info.end_line {
            self.mark_coverable(&expr.source_info.source, line);
        }

        // Check for branch-producing function calls and recurse.
        match &*expr.kind {
            ExprKind::FunctionCall(data) => {
                // Register branch points for `if` and `match`.
                if data.function_name == "if" || data.function_name == "match" {
                    self.register_branches(
                        &expr.source_info,
                        data.function_name.as_str(),
                        &data.arguments,
                    );
                }
                for arg in &data.arguments {
                    self.walk_expr_coverable(arg);
                }
            }
            ExprKind::Lambda { body, .. } => {
                for e in body {
                    self.walk_expr_coverable(e);
                }
            }
            ExprKind::Collection { elements } => {
                for e in elements {
                    self.walk_expr_coverable(e);
                }
            }
            ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
                for arg in &data.arguments {
                    self.walk_expr_coverable(arg);
                }
            }
            // Leaf nodes — literals, variables, enum values, type refs, etc.
            _ => {}
        }
    }

    /// Register a branch point for an `if` or `match` call site.
    ///
    /// For `if`: branches are `args[1]` (true arm) and `args[2]` (false arm).
    /// For `match`: branches are `args[1..]` (each match lambda).
    fn register_branches(
        &mut self,
        call_source: &SourceInfo,
        function_name: &str,
        args: &[ValueSpec],
    ) {
        let kind = match function_name {
            "if" => BranchKind::If,
            "match" => BranchKind::Match,
            _ => return,
        };

        // Branch arms are all arguments after the first (condition for `if`,
        // matched value for `match`).
        let branch_args = if args.len() > 1 { &args[1..] } else { return };

        let point_idx = self.branches.points.len();
        let mut arms = Vec::with_capacity(branch_args.len());

        for (arm_idx, arg) in branch_args.iter().enumerate() {
            // Register reverse mapping for O(1) hot-path lookup.
            self.branches
                .lambda_to_branch
                .insert(arg.source_info.clone(), (point_idx, arm_idx));

            arms.push(BranchArm {
                source: arg.source_info.clone(),
                hit_count: 0,
            });
        }

        self.branches.points.push(BranchPoint {
            call_source: call_source.clone(),
            kind,
            arms,
        });
    }
}

// ---------------------------------------------------------------------------
// CoverageHooks — EvalHooks implementation
// ---------------------------------------------------------------------------

/// [`EvalHooks`] implementation that records line, branch, and function
/// coverage during Pure expression evaluation.
///
/// # Performance
///
/// Every `before_eval` call performs:
/// - A `starts_with` prefix check (fast short-circuit for filtered files)
/// - A `BTreeMap` entry increment per spanned line (hot path)
/// - A single `HashMap::get` for branch arm detection
///
/// This adds ~5-15% overhead compared to [`NoOpHooks`](super::hooks::NoOpHooks).
/// Coverage mode is explicitly opt-in (`--coverage` flag).
pub struct CoverageHooks {
    map: CoverageMap,
    /// Only record hits for sources whose path starts with this prefix.
    /// Empty string means track all files.
    source_filter: String,
    /// The FQN of the currently executing test function (if any).
    /// Set on `enter_function` when the name is in `map.test_fqns`,
    /// cleared on the corresponding `leave_function`.
    current_test: Option<SmolStr>,
}

impl CoverageHooks {
    /// Create new coverage hooks.
    ///
    /// `source_filter` is a prefix match on `SourceInfo.source` —
    /// only files whose path starts with this string are tracked.
    /// Pass `""` to track all files.
    pub fn new(source_filter: impl Into<String>) -> Self {
        Self {
            map: CoverageMap::new(),
            source_filter: source_filter.into(),
            current_test: None,
        }
    }

    /// Access the accumulated coverage map (immutable).
    #[must_use]
    #[allow(dead_code)]
    pub fn map(&self) -> &CoverageMap {
        &self.map
    }

    /// Access the accumulated coverage map (mutable).
    ///
    /// Used to call [`CoverageMap::populate_coverable`] before execution.
    pub fn map_mut(&mut self) -> &mut CoverageMap {
        &mut self.map
    }

    /// Consume the hooks and return the coverage map.
    #[must_use]
    pub fn into_map(self) -> CoverageMap {
        self.map
    }

    /// Register a set of test function FQNs for per-test attribution.
    ///
    /// Call this after model compilation — the test runner / surveyor
    /// discovers which functions carry `<<test.Test>>` or `<<PCT.test>>`
    /// and passes them here before execution.
    #[allow(dead_code)]
    pub fn register_test_fqns(&mut self, fqns: impl IntoIterator<Item = SmolStr>) {
        self.map.test_fqns.extend(fqns);
    }

    /// Check whether a source path passes the prefix filter.
    fn passes_filter(&self, source: &SmolStr) -> bool {
        self.source_filter.is_empty() || source.starts_with(self.source_filter.as_str())
    }
}

impl EvalHooks for CoverageHooks {
    fn before_eval(
        &mut self,
        source: &SourceInfo,
        _context: &legend_pure_runtime::context::VariableContext,
    ) {
        // Fast prefix check — skip non-matching files.
        if !self.passes_filter(&source.source) {
            return;
        }

        // 1. Record line hits for every line this expression spans.
        for line in source.start_line..=source.end_line {
            self.map.record_hit(&source.source, line);

            // 2. Test attribution — record which test covers this line.
            if let Some(ref test_name) = self.current_test {
                self.map
                    .test_attribution
                    .entry(test_name.clone())
                    .or_default()
                    .insert((source.source.clone(), line));
            }
        }

        // 3. Branch arm detection — O(1) HashMap lookup.
        if let Some(&(point_idx, arm_idx)) = self.map.branches.lambda_to_branch.get(source) {
            self.map.branches.points[point_idx].arms[arm_idx].hit_count += 1;
        }
    }

    fn after_eval(&mut self, _source: &SourceInfo, _result: &Value) {}

    fn enter_function(&mut self, name: &str, source: &SourceInfo) {
        if !self.passes_filter(&source.source) {
            return;
        }
        self.map.functions.record_call(name);

        // Track current test — only set if no test is already active
        // (avoid overwriting with nested calls).
        if self.current_test.is_none() && self.map.test_fqns.contains(name) {
            self.current_test = Some(SmolStr::new(name));
        }
    }

    fn leave_function(&mut self, name: &str) {
        // Clear current test when the test function returns.
        if self.current_test.as_deref().is_some_and(|t| t == name) {
            self.current_test = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source(file: &str, start: u32, end: u32) -> SourceInfo {
        SourceInfo::new(file, start, 1, end, 10)
    }

    #[test]
    fn coverage_map_record_and_query() {
        let mut map = CoverageMap::new();
        let source = SmolStr::new("test.pure");

        map.mark_coverable(&source, 1);
        map.mark_coverable(&source, 2);
        map.mark_coverable(&source, 3);

        map.record_hit(&source, 1);
        map.record_hit(&source, 1);
        map.record_hit(&source, 2);

        let fc = map.file_coverage("test.pure").unwrap();
        assert_eq!(fc.lines_found(), 3);
        assert_eq!(fc.lines_hit(), 2);
        assert!((fc.line_percentage() - 66.666).abs() < 1.0);
    }

    #[test]
    fn coverage_map_empty_returns_100_percent() {
        let map = CoverageMap::new();
        let summary = map.summary();
        assert!((summary.line_percentage - 100.0).abs() < f64::EPSILON);
        assert!((summary.branch_percentage - 100.0).abs() < f64::EPSILON);
        assert!((summary.function_percentage - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn function_tracker_register_and_call() {
        let mut tracker = FunctionTracker::default();
        let src = test_source("test.pure", 1, 5);

        tracker.register(SmolStr::new("my::func"), src);
        assert_eq!(tracker.functions_found(), 1);
        assert_eq!(tracker.functions_hit(), 0);

        tracker.record_call("my::func");
        assert_eq!(tracker.functions_hit(), 1);

        // Calling a non-registered function is a no-op.
        tracker.record_call("unknown::func");
        assert_eq!(tracker.functions_found(), 1);
    }

    #[test]
    fn branch_tracker_counts() {
        let mut tracker = BranchTracker::default();
        let src_true = test_source("test.pure", 2, 2);
        let src_false = test_source("test.pure", 3, 3);

        tracker.points.push(BranchPoint {
            call_source: test_source("test.pure", 1, 3),
            kind: BranchKind::If,
            arms: vec![
                BranchArm {
                    source: src_true.clone(),
                    hit_count: 0,
                },
                BranchArm {
                    source: src_false.clone(),
                    hit_count: 0,
                },
            ],
        });
        tracker.lambda_to_branch.insert(src_true, (0, 0));
        tracker.lambda_to_branch.insert(src_false, (0, 1));

        assert_eq!(tracker.branches_found(), 2);
        assert_eq!(tracker.branches_hit(), 0);

        // Simulate hitting the true branch.
        tracker.points[0].arms[0].hit_count = 1;
        assert_eq!(tracker.branches_hit(), 1);
    }

    #[test]
    fn coverage_hooks_prefix_filter() {
        let mut hooks = CoverageHooks::new("my/model/");

        // Should be tracked.
        let src_match = SourceInfo::new("my/model/Person.pure", 1, 1, 1, 10);
        hooks.before_eval(&src_match);
        assert!(hooks.map.files.contains_key("my/model/Person.pure"));

        // Should be filtered out.
        let src_skip = SourceInfo::new("platform/collection.pure", 1, 1, 1, 10);
        hooks.before_eval(&src_skip);
        assert!(!hooks.map.files.contains_key("platform/collection.pure"));
    }

    #[test]
    fn coverage_hooks_multiline_span() {
        let mut hooks = CoverageHooks::new("");
        let src = SourceInfo::new("test.pure", 5, 1, 8, 10);
        hooks.before_eval(&src);

        let fc = hooks.map.file_coverage("test.pure").unwrap();
        // Should have recorded hits for lines 5, 6, 7, 8.
        assert_eq!(fc.line_hits.len(), 4);
        for line in 5..=8 {
            assert_eq!(*fc.line_hits.get(&line).unwrap(), 1);
        }
    }

    #[test]
    fn coverage_hooks_empty_filter_tracks_all() {
        let mut hooks = CoverageHooks::new("");
        let src = SourceInfo::new("anything.pure", 1, 1, 1, 10);
        hooks.before_eval(&src);
        assert!(hooks.map.files.contains_key("anything.pure"));
    }
}
