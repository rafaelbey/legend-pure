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
    DatabaseDef, DatabaseElement, Filter, Join, MilestoneSpec, MilestoneValue, MultiGrainFilter,
    OpColumn, OpExpr, Table, View,
};

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
}

/// One registered database, plus the source file it came from
/// (carried for diagnostics that point back at the original site).
#[derive(Debug, Clone)]
struct RegisteredDatabase {
    /// The database AST node (cloned from the parser output).
    def: DatabaseDef,
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
}

impl CompilerExtension for RelationalExtension {
    fn name(&self) -> &'static str {
        "RelationalExtension"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let mut by_fqn = self.databases.borrow_mut();
        by_fqn.clear();
        for source in ctx.source_files {
            for section in &source.sections {
                if section.kind.as_str() != "Relational" {
                    continue;
                }
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
        }
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
            DatabaseElement::Table(t) => check_unique(
                &mut seen_top_table_or_view,
                &t.name.value,
                &t.source_info,
                "Table",
                errors,
            ),
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
            DatabaseElement::Table(t) => validate_milestoning(t, errors),
            DatabaseElement::Schema(s) => {
                for t in &s.tables {
                    validate_milestoning(t, errors);
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

fn record_view(out: &mut HashMap<SmolStr, HashSet<SmolStr>>, _v: &View) {
    // Views' column lists currently live inside their opaque
    // `TokenSlice` body — not yet structured. Stage 5 introduces a
    // structural view-body AST; until then we record an empty
    // column set so alias references to the view name don't fail
    // unconditionally, while column-existence checks against view
    // columns silently pass.
    out.entry(_v.name.value.clone()).or_default();
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
