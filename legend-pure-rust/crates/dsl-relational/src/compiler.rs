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

//! Stage-4 [`CompilerExtension`] for the Relational DSL.
//!
//! Phases:
//!
//! - `declare()` — walks every source file's `###Relational` sections,
//!   downcasts each `Element::DSLElement` to [`DatabaseDef`], and
//!   registers it in extension-private state keyed by FQN. Duplicate
//!   FQNs produce a `DuplicateElement` diagnostic at declare time.
//! - `validate()` — runs the structural Stage-1+2+3 validators that
//!   don't require any cross-DSL lambda lowering:
//!
//!   1. **Include DAG acyclic** — `Database A include B; Database B
//!      include A` (or longer cycles) raises `CyclicInheritance` on the
//!      first database in the detected cycle.
//!   2. **Include FQNs resolve** — every `include other::DB` references
//!      another registered Database; otherwise `UnresolvedElement`.
//!   3. **Body uniqueness** — within one Database, schema names are
//!      unique, top-level Table / View names are unique, Join names are
//!      unique, Filter names are unique, MultiGrainFilter names are
//!      unique. (Different element kinds may still share a name —
//!      Java's processor does the same since Joins live in a different
//!      namespace from Tables.)
//!   4. **Op-expression alias columns resolve** — for every
//!      `tableAliasColumnWithScopeInfo` reference inside a Filter /
//!      Join / MultiGrainFilter body, the leading alias must be the
//!      simple name of a Table or View *visible* to the database (in
//!      the database itself, in any schema, or transitively via
//!      `include`). The first scope segment must be a column on that
//!      table.
//!   5. **Milestoning column refs exist** — every `KEY = column`
//!      identifier value in a [`MilestoneSpec`] references a column on
//!      the same table.
//!
//! Validators that need lambda lowering (e.g. checking a Filter
//! predicate has Boolean[1] return type) are deliberately deferred to
//! Stage 5+ when the relational mapping body lands and the
//! `lower_and_infer_expression` plumbing on `op_operation` matures.
//!
//! See [`crates/dsl-mapping/src/compiler.rs`] for the trait-shape
//! template these validators follow.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx, ValidateCtx};
use smol_str::SmolStr;

use crate::ast::{
    DatabaseDef, DatabaseElement, EmbeddedMapping, EmbeddedMappingTrailer, Filter,
    FilterMappingJoinSequence, Join, JoinColWithDbOrConstant, JoinSequence, MilestoneSpec,
    MilestoneValue, MultiGrainFilter, NonePlusMappingValue, OneJoin, OpColumn, OpExpr,
    RelationalClassMappingBody, SingleMappingLine, Table, View,
};
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef};

/// Compiler extension for the `###Relational` DSL.
///
/// Construct one per
/// [`compile_with_extensions`](legend_pure_parser_pure::pipeline::compile_with_extensions)
/// invocation; do not share across compilations because the
/// extension's internal map is reset per call.
#[derive(Default)]
pub struct RelationalExtension {
    /// Databases collected during `declare`, keyed by FQN
    /// (`"pkg::sub::Name"`). Per-extension state — not stored in
    /// `PureModel`. Use [`Self::databases`] to inspect after compile.
    databases: RefCell<HashMap<SmolStr, RegisteredDatabase>>,
    /// `Class : Relational { … }` mapping bodies collected during
    /// `declare`, indexed by enclosing-mapping FQN. Used by Stage-8
    /// validators to check class-mapping shape (embedded uniqueness,
    /// inline target lookup, association arity) without requiring
    /// the model to carry mapping AST.
    relational_class_mappings: RefCell<Vec<RegisteredRelationalClassMapping>>,
    /// Per-database resolved snapshot built during Pass 2b
    /// (`define_bodies`). Keyed by database FQN. See
    /// [`crate::processor::ResolvedDatabase`] for the shape and
    /// [`Self::resolved_databases`] for the post-compile accessor.
    resolved_databases: RefCell<HashMap<SmolStr, crate::processor::ResolvedDatabase>>,
}

/// One registered database, plus the source file it came from
/// (carried for diagnostics that point back at the original site).
#[derive(Debug, Clone)]
struct RegisteredDatabase {
    /// The database AST node (cloned from the parser output).
    def: DatabaseDef,
}

/// One `Class : Relational { … }` body, captured with the enclosing
/// `Mapping`'s FQN and the class-mapping id (for cross-reference
/// lookups in Stage-8 validators).
#[derive(Debug, Clone)]
struct RegisteredRelationalClassMapping {
    /// FQN of the enclosing `Mapping`.
    mapping_fqn: SmolStr,
    /// Class-mapping id — the explicit `[id]` if set, else the class FQN.
    class_mapping_id: SmolStr,
    /// `extends [superId]` — captured so Stage-8 can validate
    /// extends-on-association forbidden (G3).
    extends: Option<SmolStr>,
    /// Span of the entire class-mapping declaration (for diagnostics
    /// that span the whole `Class : Relational { ... }` shape).
    class_mapping_source_info: SourceInfo,
    /// The relational body (cloned from the AST).
    body: RelationalClassMappingBody,
}

impl RelationalExtension {
    /// Construct a fresh extension. Each `compile_with_extensions`
    /// call should get its own.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the databases registered during the most recent
    /// `declare()` pass — keyed by FQN. Cloned because the internal
    /// state is `RefCell`-locked while validation runs.
    #[must_use]
    pub fn databases(&self) -> HashMap<SmolStr, DatabaseDef> {
        self.databases
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.def.clone()))
            .collect()
    }

    /// Snapshot of the per-database resolved state built during Pass 2b.
    /// Keyed by database FQN. Empty until [`CompilerExtension::define_bodies`]
    /// runs — call this only post-compile.
    #[must_use]
    pub fn resolved_databases(&self) -> HashMap<SmolStr, crate::processor::ResolvedDatabase> {
        self.resolved_databases.borrow().clone()
    }
}

impl CompilerExtension for RelationalExtension {
    fn name(&self) -> &'static str {
        "RelationalExtension"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let mut by_fqn = self.databases.borrow_mut();
        by_fqn.clear();
        let mut relational_class_mappings = self.relational_class_mappings.borrow_mut();
        relational_class_mappings.clear();
        for source in ctx.source_files {
            for section in &source.sections {
                match section.kind.as_str() {
                    "Relational" => {
                        for element in &section.elements {
                            let AstElement::DSLElement(boxed) = element else {
                                continue;
                            };
                            let Some(db) = boxed.as_any().downcast_ref::<DatabaseDef>() else {
                                continue;
                            };
                            let fqn = database_fqn(db);
                            if by_fqn.contains_key(&fqn) {
                                ctx.errors.push(CompilationError {
                                    message: format!("Duplicate Database '{fqn}'"),
                                    source_info: db.source_info.clone(),
                                    kind: CompilationErrorKind::DuplicateElement { name: fqn },
                                });
                                continue;
                            }
                            by_fqn.insert(fqn, RegisteredDatabase { def: (*db).clone() });
                        }
                    }
                    "Mapping" => {
                        for element in &section.elements {
                            let AstElement::DSLElement(boxed) = element else {
                                continue;
                            };
                            let Some(mapping) = boxed.as_any().downcast_ref::<MappingDef>() else {
                                continue;
                            };
                            let mapping_fqn = mapping_fqn(mapping);
                            for cm in &mapping.class_mappings {
                                let ClassMappingBody::Foreign(boxed) = &cm.body else {
                                    continue;
                                };
                                let Some(body) =
                                    boxed.as_any().downcast_ref::<RelationalClassMappingBody>()
                                else {
                                    continue;
                                };
                                let class_mapping_id = cm.id.clone().unwrap_or_else(|| {
                                    let mut s = String::new();
                                    if let Some(pkg) = cm.class.package.as_ref() {
                                        for seg in pkg.segments() {
                                            s.push_str(seg.as_str());
                                            s.push_str("::");
                                        }
                                    }
                                    s.push_str(cm.class.name.as_str());
                                    SmolStr::new(&s)
                                });
                                relational_class_mappings.push(RegisteredRelationalClassMapping {
                                    mapping_fqn: mapping_fqn.clone(),
                                    class_mapping_id,
                                    extends: cm.extends.clone(),
                                    class_mapping_source_info: cm.source_info.clone(),
                                    body: body.clone(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn define_bodies(&self, _ctx: &mut legend_pure_parser_pure::extension::DefineCtx<'_>) {
        // Phase B1: build the per-database resolved snapshot.
        let dbs = self.databases.borrow();
        let mut resolved = self.resolved_databases.borrow_mut();
        resolved.clear();
        for (fqn, reg) in dbs.iter() {
            let snapshot = crate::processor::process_database(&reg.def);
            resolved.insert(fqn.clone(), snapshot);
        }

        // Phase B2: resolve op-body column refs against the snapshot
        // map. Cross-db resolution requires the full snapshot
        // population from B1, so this is a second pass.
        let defs_by_fqn: HashMap<SmolStr, DatabaseDef> = dbs
            .iter()
            .map(|(k, v)| (k.clone(), v.def.clone()))
            .collect();
        crate::processor::resolve_op_bodies(&mut resolved, &defs_by_fqn);
    }

    fn validate(&self, ctx: &mut ValidateCtx<'_>) {
        let dbs = self.databases.borrow();
        // V1 + V2: include graph (acyclic + each FQN resolves to a
        // registered database).
        validate_include_graph(&dbs, ctx.errors);
        // V3 + V4 + V5: per-database body validation.
        for reg in dbs.values() {
            validate_database(&reg.def, &dbs, ctx.errors);
        }
        // Stage 8 + 9: per-class-mapping validation.
        let class_mappings = self.relational_class_mappings.borrow();
        validate_relational_class_mappings(&class_mappings, &dbs, ctx.errors);
        // Phase D: repo-boundary visibility for `include` and `[db]`
        // qualifiers. No-op when `model.repo_visibility` is empty (so
        // existing tests that build a model without descriptors stay
        // green).
        validate_repo_visibility(&dbs, ctx.model, ctx.errors);
    }
}

// ---------------------------------------------------------------------------
// FQN helpers
// ---------------------------------------------------------------------------

fn database_fqn(db: &DatabaseDef) -> SmolStr {
    let mut s = String::new();
    if let Some(pkg) = db.package.as_ref() {
        for seg in pkg.segments() {
            s.push_str(seg.as_str());
            s.push_str("::");
        }
    }
    s.push_str(db.name.value.as_str());
    SmolStr::new(&s)
}

fn include_fqn(included: &legend_pure_parser_ast::annotation::PackageableElementPtr) -> SmolStr {
    let mut s = String::new();
    if let Some(pkg) = included.package.as_ref() {
        for seg in pkg.segments() {
            s.push_str(seg.as_str());
            s.push_str("::");
        }
    }
    s.push_str(included.name.as_str());
    SmolStr::new(&s)
}

// ---------------------------------------------------------------------------
// V1 + V2: include graph
// ---------------------------------------------------------------------------

/// Walk the include DAG starting from each database. Pushes
/// `UnresolvedElement` for any include FQN that doesn't match a
/// registered database, and `CyclicInheritance` for any cycle.
fn validate_include_graph(
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    // First pass: every include FQN must resolve to a registered DB.
    for reg in dbs.values() {
        for inc in &reg.def.includes {
            let fqn = include_fqn(&inc.included);
            if !dbs.contains_key(&fqn) {
                errors.push(CompilationError {
                    message: format!(
                        "Database '{}' includes unresolved Database '{fqn}'",
                        database_fqn(&reg.def),
                    ),
                    source_info: inc.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement { path: fqn },
                });
            }
        }
    }

    // Second pass: cycle detection over resolved edges only.
    let mut color: HashMap<SmolStr, u8> = HashMap::new(); // 0=unseen, 1=on stack, 2=done
    for fqn in dbs.keys() {
        if color.get(fqn).copied().unwrap_or(0) != 0 {
            continue;
        }
        let mut stack: Vec<(SmolStr, usize)> = vec![(fqn.clone(), 0)];
        color.insert(fqn.clone(), 1);
        while let Some((current, idx)) = stack.last().cloned() {
            let Some(reg) = dbs.get(&current) else {
                stack.pop();
                color.insert(current, 2);
                continue;
            };
            if idx >= reg.def.includes.len() {
                stack.pop();
                color.insert(current, 2);
                continue;
            }
            // Advance the parent's index BEFORE recursing.
            let last = stack
                .last_mut()
                .expect("stack non-empty in the body of `while let Some(_) = stack.last()`");
            last.1 += 1;
            let target = include_fqn(&reg.def.includes[idx].included);
            if !dbs.contains_key(&target) {
                continue; // already reported as unresolved
            }
            match color.get(&target).copied().unwrap_or(0) {
                1 => {
                    // Back-edge → cycle. Emit on the database that closes the cycle.
                    errors.push(CompilationError {
                        message: format!(
                            "Cyclic Database include: '{current}' eventually includes itself via '{target}'"
                        ),
                        source_info: reg.def.includes[idx].source_info.clone(),
                        kind: CompilationErrorKind::CyclicInheritance {
                            element_name: current.clone(),
                        },
                    });
                }
                0 => {
                    color.insert(target.clone(), 1);
                    stack.push((target, 0));
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-database body validation
// ---------------------------------------------------------------------------

fn validate_database(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    // V3: body name uniqueness — schemas, tables/views, joins, filters,
    // multi-grain filters, each in their own namespace.
    let mut seen_schema: HashSet<SmolStr> = HashSet::new();
    let mut seen_top_table_or_view: HashSet<SmolStr> = HashSet::new();
    let mut seen_join: HashSet<SmolStr> = HashSet::new();
    let mut seen_filter: HashSet<SmolStr> = HashSet::new();
    let mut seen_mgf: HashSet<SmolStr> = HashSet::new();

    for elem in &db.elements {
        match elem {
            DatabaseElement::Schema(s) => {
                check_unique(
                    &mut seen_schema,
                    &s.name.value,
                    &s.source_info,
                    "Schema",
                    errors,
                );
                let mut seen_table = HashSet::new();
                let mut seen_view = HashSet::new();
                for t in &s.tables {
                    check_unique(
                        &mut seen_table,
                        &t.name.value,
                        &t.source_info,
                        "Table (within schema)",
                        errors,
                    );
                    check_table_column_uniqueness(t, errors);
                }
                for v in &s.views {
                    check_unique(
                        &mut seen_view,
                        &v.name.value,
                        &v.source_info,
                        "View (within schema)",
                        errors,
                    );
                }
            }
            DatabaseElement::Table(t) => {
                check_unique(
                    &mut seen_top_table_or_view,
                    &t.name.value,
                    &t.source_info,
                    "Table",
                    errors,
                );
                check_table_column_uniqueness(t, errors);
            }
            DatabaseElement::View(v) => check_unique(
                &mut seen_top_table_or_view,
                &v.name.value,
                &v.source_info,
                "View",
                errors,
            ),
            DatabaseElement::Join(j) => check_unique(
                &mut seen_join,
                &j.name.value,
                &j.source_info,
                "Join",
                errors,
            ),
            DatabaseElement::Filter(f) => check_unique(
                &mut seen_filter,
                &f.name.value,
                &f.source_info,
                "Filter",
                errors,
            ),
            DatabaseElement::MultiGrainFilter(m) => check_unique(
                &mut seen_mgf,
                &m.name.value,
                &m.source_info,
                "MultiGrainFilter",
                errors,
            ),
        }
    }

    // V4: collect a flat (alias → table) view of every Table reachable
    // from this database (own + transitively included). Used to
    // resolve op-expression alias columns.
    let visible = collect_visible_tables(db, all_dbs);

    // V4: walk Filter / Join / MultiGrainFilter bodies and check
    // every aliased column reference resolves.
    for elem in &db.elements {
        match elem {
            DatabaseElement::Filter(Filter { name, body, .. }) => {
                validate_op_columns(body, &visible, name.value.as_str(), "Filter", errors);
            }
            DatabaseElement::Join(Join { name, body, .. }) => {
                validate_op_columns(body, &visible, name.value.as_str(), "Join", errors);
            }
            DatabaseElement::MultiGrainFilter(MultiGrainFilter { name, body, .. }) => {
                validate_op_columns(
                    body,
                    &visible,
                    name.value.as_str(),
                    "MultiGrainFilter",
                    errors,
                );
            }
            _ => {}
        }
    }

    // V5: milestoning column refs.
    for elem in &db.elements {
        match elem {
            DatabaseElement::Table(t) => {
                validate_milestoning(t, errors);
                validate_table_milestoning_column_types(t, errors);
            }
            DatabaseElement::Schema(s) => {
                for t in &s.tables {
                    validate_milestoning(t, errors);
                    validate_table_milestoning_column_types(t, errors);
                }
            }
            _ => {}
        }
    }

    // E (Phase A2): @joinName references inside view bodies must
    // resolve to a Join visible to the view's owning database.
    // Class-mapping body checks happen later inside
    // `validate_relational_class_mappings` per-class-mapping.
    for elem in &db.elements {
        match elem {
            DatabaseElement::View(v) => validate_view_join_refs(db, v, all_dbs, errors),
            DatabaseElement::Schema(s) => {
                for v in &s.views {
                    validate_view_join_refs(db, v, all_dbs, errors);
                }
            }
            _ => {}
        }
    }
}

fn check_unique(
    seen: &mut HashSet<SmolStr>,
    name: &SmolStr,
    source_info: &SourceInfo,
    kind: &str,
    errors: &mut Vec<CompilationError>,
) {
    if !seen.insert(name.clone()) {
        errors.push(CompilationError {
            message: format!("Duplicate {kind} '{name}'"),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::DuplicateElement { name: name.clone() },
        });
    }
}

// ---------------------------------------------------------------------------
// V4: alias column resolution
// ---------------------------------------------------------------------------

/// Map from a table/view's *simple name* to its column-name set, for
/// every Table or View reachable from `db` (own elements + the
/// transitive closure of `include`). Cycles in the include graph are
/// reported elsewhere (V1); this walk uses a `visited` set so it
/// terminates even when the graph has a cycle that passed through
/// already.
fn collect_visible_tables(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
) -> HashMap<SmolStr, HashSet<SmolStr>> {
    let mut out: HashMap<SmolStr, HashSet<SmolStr>> = HashMap::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_visible(db, all_dbs, &mut out, &mut visited);
    out
}

fn walk_visible(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
    out: &mut HashMap<SmolStr, HashSet<SmolStr>>,
    visited: &mut HashSet<SmolStr>,
) {
    if !visited.insert(database_fqn(db)) {
        return;
    }
    for elem in &db.elements {
        match elem {
            DatabaseElement::Schema(s) => {
                for t in &s.tables {
                    record_table(out, t);
                }
                for v in &s.views {
                    record_view(out, v);
                }
            }
            DatabaseElement::Table(t) => record_table(out, t),
            DatabaseElement::View(v) => record_view(out, v),
            _ => {}
        }
    }
    for inc in &db.includes {
        let target_fqn = include_fqn(&inc.included);
        if let Some(reg) = all_dbs.get(&target_fqn) {
            walk_visible(&reg.def, all_dbs, out, visited);
        }
    }
}

fn record_table(out: &mut HashMap<SmolStr, HashSet<SmolStr>>, t: &Table) {
    let cols: HashSet<SmolStr> = t.columns.iter().map(|c| c.name.value.clone()).collect();
    out.entry(t.name.value.clone()).or_default().extend(cols);
}

fn record_view(out: &mut HashMap<SmolStr, HashSet<SmolStr>>, v: &View) {
    // Index a view's declared column names so alias-column refs like
    // `myView.col` resolve. A view's PK is computed by Java's
    // post-processor from the underlying `tableAliasColumn`'s
    // `PRIMARYKEY` flag inside the joinColWithDbOrConstant value — we
    // index every declared column name regardless of PK status here,
    // since the post-processor pass is Phase B.
    let cols: HashSet<SmolStr> = v
        .columns
        .iter()
        .map(|c| c.column_name.value.clone())
        .collect();
    out.entry(v.name.value.clone()).or_default().extend(cols);
}

/// Visible Filter / MultiGrainFilter names from `db`'s body + the
/// transitive closure of `include`. Used by Stage-9 (D) to validate
/// `~filter [db]name` references in class-mapping bodies.
fn collect_visible_filters(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
) -> HashSet<SmolStr> {
    let mut out: HashSet<SmolStr> = HashSet::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_visible_filters(db, all_dbs, &mut out, &mut visited);
    out
}

fn walk_visible_filters(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
    out: &mut HashSet<SmolStr>,
    visited: &mut HashSet<SmolStr>,
) {
    if !visited.insert(database_fqn(db)) {
        return;
    }
    for elem in &db.elements {
        match elem {
            DatabaseElement::Filter(f) => {
                out.insert(f.name.value.clone());
            }
            DatabaseElement::MultiGrainFilter(m) => {
                out.insert(m.name.value.clone());
            }
            _ => {}
        }
    }
    for inc in &db.includes {
        let target_fqn = include_fqn(&inc.included);
        if let Some(reg) = all_dbs.get(&target_fqn) {
            walk_visible_filters(&reg.def, all_dbs, out, visited);
        }
    }
}

/// Visible Join names from `db`'s body + the transitive closure of
/// `include`. Used by Phase-A2 (E) to validate `@joinName` references
/// in class-mapping bodies and view bodies.
fn collect_visible_joins(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
) -> HashSet<SmolStr> {
    let mut out: HashSet<SmolStr> = HashSet::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_visible_joins(db, all_dbs, &mut out, &mut visited);
    out
}

fn walk_visible_joins(
    db: &DatabaseDef,
    all_dbs: &HashMap<SmolStr, RegisteredDatabase>,
    out: &mut HashSet<SmolStr>,
    visited: &mut HashSet<SmolStr>,
) {
    if !visited.insert(database_fqn(db)) {
        return;
    }
    for elem in &db.elements {
        if let DatabaseElement::Join(j) = elem {
            out.insert(j.name.value.clone());
        }
    }
    for inc in &db.includes {
        let target_fqn = include_fqn(&inc.included);
        if let Some(reg) = all_dbs.get(&target_fqn) {
            walk_visible_joins(&reg.def, all_dbs, out, visited);
        }
    }
}

/// Render a `PackageableElementPtr` to its FQN string (matches
/// `database_fqn` / `include_fqn`).
fn packageable_fqn(p: &legend_pure_parser_ast::annotation::PackageableElementPtr) -> SmolStr {
    let mut s = String::new();
    if let Some(pkg) = p.package.as_ref() {
        for seg in pkg.segments() {
            s.push_str(seg.as_str());
            s.push_str("::");
        }
    }
    s.push_str(p.name.as_str());
    SmolStr::new(&s)
}

fn validate_op_columns(
    expr: &OpExpr,
    visible: &HashMap<SmolStr, HashSet<SmolStr>>,
    owner_name: &str,
    owner_kind: &str,
    errors: &mut Vec<CompilationError>,
) {
    match expr {
        OpExpr::Bool { lhs, rhs, .. } | OpExpr::Compare { lhs, rhs, .. } => {
            validate_op_columns(lhs, visible, owner_name, owner_kind, errors);
            validate_op_columns(rhs, visible, owner_name, owner_kind, errors);
        }
        OpExpr::IsNull { expr, .. } | OpExpr::Group { inner: expr, .. } => {
            validate_op_columns(expr, visible, owner_name, owner_kind, errors);
        }
        OpExpr::Function { args, .. } => {
            for a in args {
                validate_op_columns(a, visible, owner_name, owner_kind, errors);
            }
        }
        OpExpr::Column(OpColumn::Aliased {
            alias,
            scope,
            source_info,
            ..
        }) => {
            let table_cols = visible.get(&alias.value);
            let Some(table_cols) = table_cols else {
                errors.push(CompilationError {
                    message: format!(
                        "{owner_kind} '{owner_name}': alias '{}' does not name any visible Table or View",
                        alias.value
                    ),
                    source_info: alias.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: alias.value.clone(),
                    },
                });
                return;
            };
            // Empty `scope` (alias alone, no `.col`) — nothing more to
            // validate at this stage.
            let Some(col) = scope.first() else {
                return;
            };
            // Skip the column-existence check when the column set is
            // empty (View bodies, see `record_view`).
            if !table_cols.is_empty() && !table_cols.contains(&col.value) {
                errors.push(CompilationError {
                    message: format!(
                        "{owner_kind} '{owner_name}': column '{}' does not exist on '{}'",
                        col.value, alias.value
                    ),
                    source_info: source_info.clone(),
                    kind: CompilationErrorKind::UnknownProperty {
                        type_name: alias.value.clone(),
                        property_name: col.value.clone(),
                    },
                });
            }
        }
        // `{target}.col` (`OpColumn::Target`) is unconstrained at
        // compile time — `target` is the implicit join / filter
        // subject, resolved at use-site. Stage 5+ will tighten this
        // when class-mapping bodies plumb the subject's table
        // through.
        OpExpr::Column(OpColumn::Target { .. }) | OpExpr::Literal(_) => {}
        OpExpr::Array { elements, .. } => {
            for elem in elements {
                validate_op_columns(elem, visible, owner_name, owner_kind, errors);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// V5: milestoning column refs
// ---------------------------------------------------------------------------

fn validate_milestoning(t: &Table, errors: &mut Vec<CompilationError>) {
    let Some(spec) = &t.milestoning else { return };
    let cols: HashSet<&str> = t.columns.iter().map(|c| c.name.value.as_str()).collect();
    validate_milestone_spec(spec, &cols, t.name.value.as_str(), errors);
}

fn validate_milestone_spec(
    spec: &MilestoneSpec,
    cols: &HashSet<&str>,
    table_name: &str,
    errors: &mut Vec<CompilationError>,
) {
    for def in &spec.definitions {
        for field in &def.fields {
            let MilestoneValue::Identifier(col_ref) = &field.value else {
                continue;
            };
            if !cols.contains(col_ref.value.as_str()) {
                errors.push(CompilationError {
                    message: format!(
                        "Table '{table_name}': milestoning {} '{}' references unknown column '{}'",
                        def.kind.value, field.key.value, col_ref.value
                    ),
                    source_info: col_ref.source_info.clone(),
                    kind: CompilationErrorKind::UnknownProperty {
                        type_name: SmolStr::new(table_name),
                        property_name: col_ref.value.clone(),
                    },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stage-9 (B): per-Table column-name uniqueness.
// ---------------------------------------------------------------------------

fn check_table_column_uniqueness(t: &Table, errors: &mut Vec<CompilationError>) {
    let mut seen: HashSet<SmolStr> = HashSet::new();
    for c in &t.columns {
        if !seen.insert(c.name.value.clone()) {
            errors.push(CompilationError {
                message: format!(
                    "Table '{}': duplicate column '{}'",
                    t.name.value, c.name.value
                ),
                source_info: c.name.source_info.clone(),
                kind: CompilationErrorKind::DuplicateProperty {
                    class_name: t.name.value.clone(),
                    property_name: c.name.value.clone(),
                },
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Stage-9 (C): milestoning column-TYPE check. The Java processor
// requires `BUS_FROM` / `BUS_THRU` / `PROCESSING_IN` / `PROCESSING_OUT`
// / `*_SNAPSHOT_DATE` to point at Date / Timestamp columns;
// `*_IS_INCLUSIVE` to point at Boolean / Bit columns;
// `INFINITY_DATE` to be either a Date literal or a Date column.
//
// `validate_milestone_spec` already runs the existence check (V5);
// this pass extends that with the type check, walking
// `MilestoneField` keys against a fixed lookup table.
// ---------------------------------------------------------------------------

fn validate_table_milestoning_column_types(t: &Table, errors: &mut Vec<CompilationError>) {
    let Some(spec) = &t.milestoning else { return };
    let by_name: HashMap<&str, &str> = t
        .columns
        .iter()
        .map(|c| (c.name.value.as_str(), c.type_name.value.as_str()))
        .collect();
    for def in &spec.definitions {
        for field in &def.fields {
            let MilestoneValue::Identifier(col_ref) = &field.value else {
                continue;
            };
            let Some(col_type) = by_name.get(col_ref.value.as_str()) else {
                continue; // V5 already flagged the missing column
            };
            let Some(expected) = milestoning_field_expected_type(field.key.value.as_str()) else {
                continue;
            };
            if !type_matches(col_type, expected) {
                errors.push(CompilationError {
                    message: format!(
                        "Table '{}': milestoning {} '{}' expects a {} column; \
                         '{}' is declared as '{}'",
                        t.name.value,
                        def.kind.value,
                        field.key.value,
                        expected.label(),
                        col_ref.value,
                        col_type
                    ),
                    source_info: col_ref.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAnnotation {
                        element_name: t.name.value.clone(),
                        reason: SmolStr::new(format!(
                            "milestoning {} expects {}; got {}",
                            field.key.value,
                            expected.label(),
                            col_type
                        )),
                    },
                });
            }
        }
    }
}

#[derive(Copy, Clone)]
enum MilestoneTypeCategory {
    Date,
    Boolean,
}

impl MilestoneTypeCategory {
    fn label(self) -> &'static str {
        match self {
            MilestoneTypeCategory::Date => "Date / Timestamp",
            MilestoneTypeCategory::Boolean => "Boolean / Bit",
        }
    }
}

fn milestoning_field_expected_type(key: &str) -> Option<MilestoneTypeCategory> {
    match key {
        "BUS_FROM"
        | "BUS_THRU"
        | "BUS_SNAPSHOT_DATE"
        | "PROCESSING_IN"
        | "PROCESSING_OUT"
        | "PROCESSING_SNAPSHOT_DATE"
        | "INFINITY_DATE" => Some(MilestoneTypeCategory::Date),
        "THRU_IS_INCLUSIVE" | "OUT_IS_INCLUSIVE" => Some(MilestoneTypeCategory::Boolean),
        _ => None,
    }
}

fn type_matches(col_type: &str, expected: MilestoneTypeCategory) -> bool {
    let upper = col_type.to_ascii_uppercase();
    match expected {
        MilestoneTypeCategory::Date => {
            matches!(upper.as_str(), "DATE" | "TIMESTAMP" | "DATETIME")
        }
        MilestoneTypeCategory::Boolean => matches!(upper.as_str(), "BOOLEAN" | "BIT"),
    }
}

// ---------------------------------------------------------------------------
// Stage 8: relational class-mapping validators
// ---------------------------------------------------------------------------

fn mapping_fqn(m: &MappingDef) -> SmolStr {
    use legend_pure_parser_ast::element::PackageableElement;
    use legend_pure_parser_ast::source_info::Spanned;
    let _ = m.source_info(); // tie-break against unused trait import
    let mut s = String::new();
    if let Some(pkg) = m.package() {
        for seg in pkg.segments() {
            s.push_str(seg.as_str());
            s.push_str("::");
        }
    }
    s.push_str(m.name().as_str());
    SmolStr::new(&s)
}

/// Stage-8 validators across all registered relational class
/// mappings. Each mapping is validated independently:
///
/// E1. Embedded uniqueness — within one class mapping, no two
///     embedded mappings target the same property name. Catches
///     `(details (taxLocation : col), details (taxLocation : col))`
///     where `details` is mapped twice as an embedded property
///     mapping.
/// E2. AssociationMapping arity — Java requires exactly two
///     property-mapping lines (one per association end). Empty or
///     single-line bodies are flagged.
/// E3. Inline trailer must reference a class-mapping id within the
///     same enclosing `Mapping`. Cross-mapping inline references
///     would require the mapping graph and aren't supported in
///     Stage 8 (Java's processor walks the include graph to resolve
///     them — out of scope here).
fn validate_relational_class_mappings(
    class_mappings: &[RegisteredRelationalClassMapping],
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    use std::collections::HashSet;

    // Build a lookup of all class-mapping ids per enclosing Mapping
    // for E3.
    let mut ids_by_mapping: HashMap<SmolStr, HashSet<SmolStr>> = HashMap::new();
    for reg in class_mappings {
        ids_by_mapping
            .entry(reg.mapping_fqn.clone())
            .or_default()
            .insert(reg.class_mapping_id.clone());
    }

    for reg in class_mappings {
        // G3: extends on AssociationMapping bodies is forbidden
        // (Java parity:
        // `TestMappingInheritanceValidOnlyForClassMappings::testMappingInheritanceInValidForAssociationMapping`).
        if reg.body.association_mapping.is_some() && reg.extends.is_some() {
            errors.push(CompilationError {
                message: format!(
                    "AssociationMapping '{}' cannot use `extends` (extends is only valid on class mappings)",
                    reg.class_mapping_id
                ),
                source_info: reg.class_mapping_source_info.clone(),
                kind: CompilationErrorKind::InvalidAssociation {
                    name: reg.class_mapping_id.clone(),
                    reason: SmolStr::new("extends is only valid on class mappings"),
                },
            });
        }

        // D: ~filter must reference a Filter visible to the database.
        if let Some(filter_block) = &reg.body.filter {
            let db_fqn = packageable_fqn(&filter_block.db);
            match dbs.get(&db_fqn) {
                Some(db) => {
                    let visible_filters = collect_visible_filters(&db.def, dbs);
                    if !visible_filters.contains(&filter_block.filter_name.value) {
                        errors.push(CompilationError {
                            message: format!(
                                "Class mapping '{}': ~filter '{}' is not declared on Database '{}'",
                                reg.class_mapping_id, filter_block.filter_name.value, db_fqn,
                            ),
                            source_info: filter_block.filter_name.source_info.clone(),
                            kind: CompilationErrorKind::UnresolvedElement {
                                path: filter_block.filter_name.value.clone(),
                            },
                        });
                    }
                }
                None => {
                    errors.push(CompilationError {
                        message: format!(
                            "Class mapping '{}': ~filter references unknown Database '{}'",
                            reg.class_mapping_id, db_fqn,
                        ),
                        source_info: filter_block.db.source_info.clone(),
                        kind: CompilationErrorKind::UnresolvedElement {
                            path: db_fqn.clone(),
                        },
                    });
                }
            }
        }

        // E: every `@joinName` in the class-mapping body must resolve
        // to a Join registered on its contextual database. The
        // contextual database is whichever explicit `[db]` qualifier
        // sits closest in source — currently we only validate
        // references where the qualifier is explicit (FilterMappingBlock
        // joins, [db]@... values, OtherwiseJoin with explicit db,
        // OneJoinRight with explicit db). References without an
        // explicit qualifier inherit from `~mainTable` / `scope(...)`
        // / per-property db chains, which require post-processor
        // context propagation deferred to Phase B.
        validate_class_mapping_join_refs(reg, dbs, errors);

        // E2: AssociationMapping arity.
        if let Some(lines) = &reg.body.association_mapping {
            if lines.len() != 2 {
                errors.push(CompilationError {
                    message: format!(
                        "AssociationMapping '{}' must declare exactly 2 property-mapping lines (one per end); got {}",
                        reg.class_mapping_id,
                        lines.len()
                    ),
                    source_info: reg.body.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: reg.class_mapping_id.clone(),
                        reason: SmolStr::new(format!(
                            "expected 2 lines, got {}",
                            lines.len()
                        )),
                    },
                });
            }
            // Skip the rest of the per-class-mapping checks for the
            // association shape (no embedded / inline within
            // AssociationMapping bodies in Java's grammar).
            continue;
        }

        // Walk the class-mapping body and gather the embedded /
        // inline targets for E1 + E3.
        let mut seen_embedded: HashSet<SmolStr> = HashSet::new();
        let mut walk_lines = |lines: &[SingleMappingLine], errors: &mut Vec<CompilationError>| {
            for line in lines {
                let SingleMappingLine::NonePlus(np) = line else {
                    continue;
                };
                let NonePlusMappingValue::Embedded(em) = &np.value else {
                    continue;
                };
                if !seen_embedded.insert(np.property.value.clone()) {
                    errors.push(CompilationError {
                        message: format!(
                            "Embedded property mapping '{}' declared twice in class mapping '{}'",
                            np.property.value, reg.class_mapping_id
                        ),
                        source_info: np.property.source_info.clone(),
                        kind: CompilationErrorKind::DuplicateProperty {
                            class_name: reg.class_mapping_id.clone(),
                            property_name: np.property.value.clone(),
                        },
                    });
                }
                // E3: Inline trailer must reference a mapping id.
                if let Some(EmbeddedMappingTrailer::Inline(inline)) = &em.trailer {
                    let known_ids = ids_by_mapping
                        .get(&reg.mapping_fqn)
                        .cloned()
                        .unwrap_or_default();
                    if !known_ids.contains(&inline.id.value) {
                        errors.push(CompilationError {
                            message: format!(
                                "Inline target '{}' on property '{}' does not reference a class \
                                 mapping in '{}'",
                                inline.id.value, np.property.value, reg.mapping_fqn
                            ),
                            source_info: inline.source_info.clone(),
                            kind: CompilationErrorKind::UnresolvedElement {
                                path: inline.id.value.clone(),
                            },
                        });
                    }
                }
                // F: Otherwise property mappings must declare each
                // property at most once within one Otherwise block.
                if let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &em.trailer {
                    let mut seen: HashSet<SmolStr> = HashSet::new();
                    for m in maps {
                        if !seen.insert(m.property.value.clone()) {
                            errors.push(CompilationError {
                                message: format!(
                                    "Otherwise property mapping '{}' declared twice in class mapping '{}'",
                                    m.property.value, reg.class_mapping_id,
                                ),
                                source_info: m.property.source_info.clone(),
                                kind: CompilationErrorKind::DuplicateProperty {
                                    class_name: reg.class_mapping_id.clone(),
                                    property_name: m.property.value.clone(),
                                },
                            });
                        }
                    }
                }
            }
        };
        // Top-level mapping elements + scope-wrapped lines.
        let mut top_level_lines: Vec<SingleMappingLine> = Vec::new();
        for elem in &reg.body.mapping_elements {
            match elem {
                crate::ast::MappingElement::Single(line) => top_level_lines.push(line.clone()),
                crate::ast::MappingElement::Scope(s) => {
                    top_level_lines.extend(s.mapping_lines.iter().cloned());
                }
            }
        }
        walk_lines(&top_level_lines, errors);
    }

    // Suppress unused-import warnings on cfg paths that don't see
    // the `EmbeddedMapping` type directly (validators only inspect
    // the trailer); reference the type to keep the import clean.
    let _ = std::marker::PhantomData::<EmbeddedMapping>;
}

/// Validator (E) helper: walk every explicit-db `@joinName` reference
/// in a class-mapping body and verify it resolves to a Join visible
/// to that database.
fn validate_class_mapping_join_refs(
    reg: &RegisteredRelationalClassMapping,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    // 1. FilterMappingBlock — when a join sequence is present, joins
    //    live in the OUTER `[db]` (i.e. `FilterMappingBlock.db`, the
    //    db the filter sequence walks FROM).
    if let Some(filter_block) = &reg.body.filter {
        if let Some(seq) = &filter_block.join_sequence {
            check_filter_mapping_join_sequence(
                seq,
                &filter_block.db,
                dbs,
                &reg.class_mapping_id,
                errors,
            );
        }
    }

    // 2. Walk every JoinColWithDbOrConstant + OtherwiseJoin in the
    //    body, validating only the explicit-db cases.
    let body = &reg.body;
    for elem in &body.mapping_elements {
        walk_mapping_element_for_joins(elem, dbs, &reg.class_mapping_id, errors);
    }
    if let Some(lines) = &body.association_mapping {
        for line in lines {
            walk_single_mapping_line_for_joins(line, dbs, &reg.class_mapping_id, errors);
        }
    }

    // 3. ~groupBy / ~primaryKey at the body level.
    if let Some(jcs) = &body.group_by {
        for jc in jcs {
            walk_join_col_for_joins(jc, dbs, &reg.class_mapping_id, errors);
        }
    }
    if let Some(jcs) = &body.primary_key {
        for jc in jcs {
            walk_join_col_for_joins(jc, dbs, &reg.class_mapping_id, errors);
        }
    }
}

fn walk_mapping_element_for_joins(
    e: &crate::ast::MappingElement,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match e {
        crate::ast::MappingElement::Single(line) => {
            walk_single_mapping_line_for_joins(line, dbs, owner, errors);
        }
        crate::ast::MappingElement::Scope(s) => {
            for line in &s.mapping_lines {
                walk_single_mapping_line_for_joins(line, dbs, owner, errors);
            }
        }
    }
}

fn walk_single_mapping_line_for_joins(
    line: &SingleMappingLine,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match line {
        SingleMappingLine::Plus(p) => {
            walk_join_col_for_joins(&p.mapping.value, dbs, owner, errors);
        }
        SingleMappingLine::NonePlus(np) => match &np.value {
            NonePlusMappingValue::Relational(rm) => {
                walk_join_col_for_joins(&rm.value, dbs, owner, errors);
            }
            NonePlusMappingValue::Embedded(em) => {
                if let Some(jcs) = &em.primary_key {
                    for jc in jcs {
                        walk_join_col_for_joins(jc, dbs, owner, errors);
                    }
                }
                for inner in &em.mapping_lines {
                    walk_single_mapping_line_for_joins(inner, dbs, owner, errors);
                }
                if let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &em.trailer {
                    for m in maps {
                        if let Some(db) = &m.otherwise_join.db {
                            check_join_sequence_against_db(
                                &m.otherwise_join.join_sequence,
                                db,
                                dbs,
                                owner,
                                errors,
                            );
                        }
                    }
                }
            }
        },
    }
}

fn walk_join_col_for_joins(
    jc: &JoinColWithDbOrConstant,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let Some(seq) = &jc.join else {
        return;
    };
    if let Some(db) = &jc.db {
        check_join_sequence_against_db(seq, db, dbs, owner, errors);
    } else {
        // Per-segment fallback: validate any OneJoinRight with its
        // own explicit `[db]` qualifier even if the head is
        // contextual.
        for r in &seq.right {
            if let Some(db) = &r.db {
                check_one_join_against_db(&r.join, db, dbs, owner, errors);
            }
        }
    }
}

fn check_filter_mapping_join_sequence(
    seq: &FilterMappingJoinSequence,
    db: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let visible = match resolve_visible_joins(db, dbs) {
        Ok(v) => v,
        Err(e) => {
            errors.push(e(owner.clone()));
            return;
        }
    };
    check_one_join_visible(&seq.head, &visible, db, owner, errors);
    for r in &seq.right {
        if let Some(rdb) = &r.db {
            check_one_join_against_db(&r.join, rdb, dbs, owner, errors);
        } else {
            check_one_join_visible(&r.join, &visible, db, owner, errors);
        }
    }
}

fn check_join_sequence_against_db(
    seq: &JoinSequence,
    db: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let visible = match resolve_visible_joins(db, dbs) {
        Ok(v) => v,
        Err(e) => {
            errors.push(e(owner.clone()));
            return;
        }
    };
    check_one_join_visible(&seq.head, &visible, db, owner, errors);
    for r in &seq.right {
        if let Some(rdb) = &r.db {
            check_one_join_against_db(&r.join, rdb, dbs, owner, errors);
        } else {
            check_one_join_visible(&r.join, &visible, db, owner, errors);
        }
    }
}

fn check_one_join_against_db(
    j: &OneJoin,
    db: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match resolve_visible_joins(db, dbs) {
        Ok(visible) => check_one_join_visible(j, &visible, db, owner, errors),
        Err(e) => errors.push(e(owner.clone())),
    }
}

fn check_one_join_visible(
    j: &OneJoin,
    visible: &HashSet<SmolStr>,
    db: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    if !visible.contains(&j.name.value) {
        errors.push(CompilationError {
            message: format!(
                "'{owner}': @{} is not declared on Database '{}'",
                j.name.value,
                packageable_fqn(db),
            ),
            source_info: j.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: j.name.value.clone(),
            },
        });
    }
}

/// Resolve `[db]` to its visible-join set, or return a closure that
/// builds an "unknown database" error tagged with the owner name.
fn resolve_visible_joins(
    db: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
) -> Result<HashSet<SmolStr>, Box<dyn FnOnce(SmolStr) -> CompilationError>> {
    let db_fqn = packageable_fqn(db);
    match dbs.get(&db_fqn) {
        Some(reg) => Ok(collect_visible_joins(&reg.def, dbs)),
        None => {
            let si = db.source_info.clone();
            Err(Box::new(move |owner: SmolStr| CompilationError {
                message: format!("'{owner}': join reference targets unknown Database '{db_fqn}'"),
                source_info: si,
                kind: CompilationErrorKind::UnresolvedElement {
                    path: db_fqn.clone(),
                },
            }))
        }
    }
}

/// Validator (E) helper: walk every `@joinName` reference inside a
/// View body and verify it resolves to a Join on the view's owning
/// database (or transitively via include).
fn validate_view_join_refs(
    db: &DatabaseDef,
    v: &View,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    let owner = SmolStr::new(format!("View '{}'", v.name.value));
    let local_visible = collect_visible_joins(db, dbs);

    // FilterViewBlock — explicit `[db1]@joinSeq | [db2]` chain;
    // joins live in `db1`.
    if let Some(filter) = &v.filter {
        if let Some(chain) = &filter.db_chain {
            check_join_sequence_against_db(
                &chain.join_sequence,
                &chain.first_db,
                dbs,
                &owner,
                errors,
            );
        }
    }

    // Each ViewColumnMappingLine value joins; explicit `[db]`
    // overrides the view's owning db.
    for col in &v.columns {
        if let Some(seq) = &col.value.join {
            if let Some(jdb) = &col.value.db {
                check_join_sequence_against_db(seq, jdb, dbs, &owner, errors);
            } else {
                check_one_join_visible_in(seq, &local_visible, db, &owner, errors);
            }
        }
    }
}

fn check_one_join_visible_in(
    seq: &JoinSequence,
    visible: &HashSet<SmolStr>,
    db: &DatabaseDef,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let local_db_ptr = legend_pure_parser_ast::annotation::PackageableElementPtr {
        package: db.package.clone(),
        name: db.name.value.clone(),
        source_info: db.name.source_info.clone(),
    };
    check_one_join_visible(&seq.head, visible, &local_db_ptr, owner, errors);
    for r in &seq.right {
        if let Some(rdb) = &r.db {
            // An explicit per-segment db overrides the contextual db.
            // We don't resolve includes against an unrelated db here
            // — `walk_join_col_for_joins`/`check_join_sequence_against_db`
            // covers that path.
            let _ = rdb;
            continue;
        }
        check_one_join_visible(&r.join, visible, &local_db_ptr, owner, errors);
    }
}

// ---------------------------------------------------------------------------
// Phase D: repo-boundary visibility
// ---------------------------------------------------------------------------

/// Walk every `include` and every `[db]` qualifier in every registered
/// database, and emit `NotVisible` for each cross-repo reference whose
/// target is not in the use-site repo's declared dependencies. No-op
/// when `model.repo_visibility` is empty.
fn validate_repo_visibility(
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    model: &legend_pure_parser_pure::model::PureModel,
    errors: &mut Vec<CompilationError>,
) {
    use legend_pure_parser_pure::visibility::source_repo_name;

    if model.repo_visibility.is_empty() {
        return;
    }

    for reg in databases.values() {
        let use_site = &reg.def.source_info.source;
        let Some(use_repo) = source_repo_name(use_site) else {
            continue;
        };
        let Some(visible) = model.repo_visibility.get(&use_repo) else {
            continue;
        };

        // 1. `include other::Db` — check target's home repo.
        for inc in &reg.def.includes {
            check_db_ref_visibility(&inc.included, use_site, visible, databases, errors);
        }

        // 2. Walk every body for `[db]` qualifiers.
        for elem in &reg.def.elements {
            walk_element_for_db_refs(elem, use_site, visible, databases, errors);
        }
    }
}

fn check_db_ref_visibility(
    target: &legend_pure_parser_ast::annotation::PackageableElementPtr,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    use legend_pure_parser_pure::visibility::source_repo_name;

    let target_fqn = packageable_fqn(target);
    let Some(target_db) = databases.get(&target_fqn) else {
        return; // Unknown DB — V2 already raised UnresolvedElement.
    };
    let target_source = &target_db.def.source_info.source;
    let Some(target_repo) = source_repo_name(target_source) else {
        return;
    };
    if visible.contains(&target_repo) {
        return;
    }
    errors.push(CompilationError {
        message: format!("{target_fqn} is not visible in the file {use_site}"),
        source_info: target.source_info.clone(),
        kind: CompilationErrorKind::NotVisible {
            target_fqn,
            source_id: use_site.clone(),
        },
    });
}

fn walk_element_for_db_refs(
    e: &DatabaseElement,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    match e {
        DatabaseElement::Schema(s) => {
            for v in &s.views {
                walk_view_for_db_refs(v, use_site, visible, databases, errors);
            }
            // Tables don't carry [db] refs in their body.
            // Schemas don't either.
            let _ = s;
        }
        DatabaseElement::View(v) => {
            walk_view_for_db_refs(v, use_site, visible, databases, errors);
        }
        DatabaseElement::Join(j) => {
            walk_op_expr_for_db_refs(&j.body, use_site, visible, databases, errors);
        }
        DatabaseElement::Filter(f) => {
            walk_op_expr_for_db_refs(&f.body, use_site, visible, databases, errors);
        }
        DatabaseElement::MultiGrainFilter(m) => {
            walk_op_expr_for_db_refs(&m.body, use_site, visible, databases, errors);
        }
        DatabaseElement::Table(_) => {}
    }
}

fn walk_view_for_db_refs(
    v: &View,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    if let Some(filter) = &v.filter {
        if let Some(chain) = &filter.db_chain {
            check_db_ref_visibility(&chain.first_db, use_site, visible, databases, errors);
            check_db_ref_visibility(&chain.second_db, use_site, visible, databases, errors);
            walk_join_sequence_for_db_refs(
                &chain.join_sequence,
                use_site,
                visible,
                databases,
                errors,
            );
        }
    }
    if let Some(jcs) = &v.group_by {
        for jc in jcs {
            walk_join_col_for_db_refs(jc, use_site, visible, databases, errors);
        }
    }
    for col in &v.columns {
        walk_join_col_for_db_refs(&col.value, use_site, visible, databases, errors);
    }
}

fn walk_op_expr_for_db_refs(
    expr: &OpExpr,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    match expr {
        OpExpr::Bool { lhs, rhs, .. } | OpExpr::Compare { lhs, rhs, .. } => {
            walk_op_expr_for_db_refs(lhs, use_site, visible, databases, errors);
            walk_op_expr_for_db_refs(rhs, use_site, visible, databases, errors);
        }
        OpExpr::IsNull { expr, .. } | OpExpr::Group { inner: expr, .. } => {
            walk_op_expr_for_db_refs(expr, use_site, visible, databases, errors);
        }
        OpExpr::Function { db, args, .. } => {
            if let Some(db) = db {
                check_db_ref_visibility(db, use_site, visible, databases, errors);
            }
            for a in args {
                walk_op_expr_for_db_refs(a, use_site, visible, databases, errors);
            }
        }
        OpExpr::Column(OpColumn::Aliased { db, .. }) => {
            if let Some(db) = db {
                check_db_ref_visibility(db, use_site, visible, databases, errors);
            }
        }
        OpExpr::Column(OpColumn::Target { .. }) | OpExpr::Literal(_) => {}
        OpExpr::Array { elements, .. } => {
            for e in elements {
                walk_op_expr_for_db_refs(e, use_site, visible, databases, errors);
            }
        }
    }
}

fn walk_join_col_for_db_refs(
    jc: &JoinColWithDbOrConstant,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    if let Some(db) = &jc.db {
        check_db_ref_visibility(db, use_site, visible, databases, errors);
    }
    if let Some(seq) = &jc.join {
        walk_join_sequence_for_db_refs(seq, use_site, visible, databases, errors);
    }
    if let Some(col) = &jc.column {
        if let OpColumn::Aliased { db: Some(db), .. } = col {
            check_db_ref_visibility(db, use_site, visible, databases, errors);
        }
    }
}

fn walk_join_sequence_for_db_refs(
    seq: &JoinSequence,
    use_site: &SmolStr,
    visible: &std::collections::BTreeSet<SmolStr>,
    databases: &HashMap<SmolStr, RegisteredDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    for r in &seq.right {
        if let Some(db) = &r.db {
            check_db_ref_visibility(db, use_site, visible, databases, errors);
        }
    }
}
