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

//! Phase B1+B2+B3: post-processor for the relational DSL.
//!
//! `RelationalExtension::define_bodies` runs this module against every
//! registered [`DatabaseDef`] to produce a [`ResolvedDatabase`] — a
//! per-database snapshot keyed by FQN. The snapshot is a side-table on
//! the extension; the parser AST is left untouched (the canonical
//! source of truth stays the AST, mirroring Java's
//! `DatabaseProcessor`'s read-once pattern).
//!
//! What this module currently resolves:
//!
//! - **Tables** (B1) — every Table on the database (top-level +
//!   schema-nested) with its column list. SQL type names are mapped
//!   to the [`PureColumnType`] enum (Java parity with
//!   `ColumnDataTypeFactory`), keeping the raw SQL spelling alongside
//!   for diagnostics.
//! - **Views / Joins / Filters / MultiGrainFilters by name** (B1).
//! - **`include` FQNs** (B1) — copied so consumers can walk the include
//!   graph without re-resolving.
//! - **Op-body column references** (B2) — every `OpColumn::Aliased`
//!   reference inside a Filter / Join / MultiGrainFilter body is bound
//!   to its `(database_fqn, table_name, column_index)` triple. Cross-db
//!   `[db]` qualifiers and transitively-included tables resolve through
//!   the snapshot map.
//!
//! - **View bodies** (B3) — every `ViewColumnMappingLine`'s value is
//!   resolved to an [`OpColumnBinding`]; constant literals carry a
//!   `None` binding. Each view tracks the distinct table names it
//!   references; views with a single referenced table get their
//!   `main_table` populated.
//!
//! What this module deliberately defers (Phase-B sub-items):
//!
//! - Class-mapping property mapping resolution (B4)
//! - Main-table inheritance through `extends` (B5)
//! - AssociationMapping source/target class population (B6)
//! - Implicit-db `@join` resolution (B7)
//! - Milestoning auto-rewrite (Phase C)

use std::collections::{HashMap, HashSet};

use smol_str::SmolStr;

use legend_pure_parser_ast::SourceInfo;

use crate::ast::{
    ColumnDef, DatabaseDef, DatabaseElement, JoinColWithDbOrConstant, OpColumn, OpExpr, View,
};

// ---------------------------------------------------------------------------
// PureColumnType — SQL → Pure primitive mapping
// ---------------------------------------------------------------------------

/// Pure primitive type that a SQL column resolves to.
///
/// Mirrors Java's `ColumnDataTypeFactory.COLUMN_TYPE_MAP`. The variants
/// are the Pure type names spelled exactly as they appear in
/// `meta::relational::metamodel::datatype::*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PureColumnType {
    /// `Float`.
    Float,
    /// `Double`.
    Double,
    /// `Integer`.
    Integer,
    /// `BigInt`.
    BigInt,
    /// `SmallInt`.
    SmallInt,
    /// `TinyInt`.
    TinyInt,
    /// `Char` — accepts a `(size)` constructor parameter.
    Char,
    /// `Varchar` — accepts a `(size)` constructor parameter.
    Varchar,
    /// `Binary` — accepts a `(size)` constructor parameter.
    Binary,
    /// `Varbinary` — accepts a `(size)` constructor parameter.
    Varbinary,
    /// `Timestamp`.
    Timestamp,
    /// `Date`.
    Date,
    /// `Decimal` — accepts `(precision, scale)`.
    Decimal,
    /// `Numeric` — accepts `(precision, scale)`.
    Numeric,
    /// `Distinct`.
    Distinct,
    /// `Other`.
    Other,
    /// `Bit`.
    Bit,
    /// `Real`.
    Real,
    /// `Array`.
    Array,
    /// `Json`.
    Json,
    /// `SemiStructured`.
    SemiStructured,
}

impl PureColumnType {
    /// Resolve a raw SQL type spelling (case-insensitive) to its Pure
    /// primitive type. Returns `None` for unknown names — Java's
    /// `ColumnDataTypeFactory` raises `ColumnDataTypeException` in
    /// that case; we surface the absence so the post-processor can
    /// emit a structured diagnostic later.
    #[must_use]
    pub fn from_sql_name(s: &str) -> Option<Self> {
        let upper = s.to_ascii_uppercase();
        match upper.as_str() {
            "FLOAT" => Some(Self::Float),
            "DOUBLE" => Some(Self::Double),
            "INT" | "INTEGER" => Some(Self::Integer),
            "BIGINT" => Some(Self::BigInt),
            "SMALLINT" => Some(Self::SmallInt),
            "TINYINT" => Some(Self::TinyInt),
            "CHAR" => Some(Self::Char),
            "VARCHAR" => Some(Self::Varchar),
            "BINARY" => Some(Self::Binary),
            "VARBINARY" => Some(Self::Varbinary),
            "TIMESTAMP" => Some(Self::Timestamp),
            "DATETIME" => Some(Self::Timestamp), // common SQL alias
            "DATE" => Some(Self::Date),
            "DECIMAL" => Some(Self::Decimal),
            "NUMERIC" => Some(Self::Numeric),
            "DISTINCT" => Some(Self::Distinct),
            "OTHER" => Some(Self::Other),
            "BIT" => Some(Self::Bit),
            "REAL" => Some(Self::Real),
            "ARRAY" => Some(Self::Array),
            "JSON" => Some(Self::Json),
            "SEMISTRUCTURED" => Some(Self::SemiStructured),
            _ => None,
        }
    }

    /// Pure type name as it appears in
    /// `meta::relational::metamodel::datatype::<name>`.
    #[must_use]
    pub fn pure_name(&self) -> &'static str {
        match self {
            Self::Float => "Float",
            Self::Double => "Double",
            Self::Integer => "Integer",
            Self::BigInt => "BigInt",
            Self::SmallInt => "SmallInt",
            Self::TinyInt => "TinyInt",
            Self::Char => "Char",
            Self::Varchar => "Varchar",
            Self::Binary => "Binary",
            Self::Varbinary => "Varbinary",
            Self::Timestamp => "Timestamp",
            Self::Date => "Date",
            Self::Decimal => "Decimal",
            Self::Numeric => "Numeric",
            Self::Distinct => "Distinct",
            Self::Other => "Other",
            Self::Bit => "Bit",
            Self::Real => "Real",
            Self::Array => "Array",
            Self::Json => "Json",
            Self::SemiStructured => "SemiStructured",
        }
    }
}

// ---------------------------------------------------------------------------
// Resolved snapshot types
// ---------------------------------------------------------------------------

/// One resolved column on a [`ResolvedTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumn {
    /// Column name as written in source.
    pub name: SmolStr,
    /// Raw SQL type spelling (e.g. `"VARCHAR"`, `"INT"`,
    /// `"TIMESTAMP"`). Kept for diagnostics that need to report what
    /// the user wrote.
    pub source_type: SmolStr,
    /// Resolved Pure primitive type, or `None` when the SQL spelling
    /// isn't recognised by [`PureColumnType::from_sql_name`].
    pub pure_type: Option<PureColumnType>,
    /// `(size)` or `(precision, ...)` constructor first parameter.
    pub size: Option<i64>,
    /// `(precision, scale)` second parameter — only meaningful for
    /// `Decimal` / `Numeric`.
    pub scale: Option<i64>,
    /// `PRIMARY KEY` flag.
    pub primary_key: bool,
    /// `NOT NULL` flag.
    pub not_null: bool,
}

/// One resolved Table on a [`ResolvedDatabase`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTable {
    /// Schema this table lives in, when nested. `None` for top-level
    /// (default-schema) tables.
    pub schema: Option<SmolStr>,
    /// Table name.
    pub name: SmolStr,
    /// Columns, in declaration order.
    pub columns: Vec<ResolvedColumn>,
    /// Column-name → index into `columns`. Built once at process
    /// time so column lookups are O(1).
    pub columns_by_name: HashMap<SmolStr, usize>,
}

impl ResolvedTable {
    /// Look up a column by name — `None` when no column with that
    /// name exists on this table.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&ResolvedColumn> {
        self.columns_by_name
            .get(name)
            .and_then(|i| self.columns.get(*i))
    }
}

/// Resolved binding for one `OpColumn::Aliased` reference inside an
/// op-body. Populated by Phase B2's `resolve_op_bodies` pass —
/// requires the full set of registered databases so cross-db `[db]`
/// qualifiers resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpColumnBinding {
    /// Database FQN where the table lives. When the AST's `[db]`
    /// qualifier was present, this is its FQN; otherwise it's the
    /// owning op-element's database FQN.
    pub database_fqn: SmolStr,
    /// Table simple name (the `alias` segment of the op-column ref).
    pub table_name: SmolStr,
    /// Column index into the resolved table's `columns` vector.
    /// `None` when the op-column had no scope segment (alias alone)
    /// or the column wasn't found.
    pub column_index: Option<usize>,
    /// `True` when `[db]@alias.col` couldn't be resolved to a known
    /// database in the snapshot map. Diagnostic-only — the V4
    /// validator already reports the underlying error.
    pub unresolved_database: bool,
    /// `True` when the `alias` segment didn't match any table
    /// visible to `database_fqn` (own + transitive includes).
    pub unresolved_table: bool,
    /// `True` when the table resolved but the scope's column name
    /// didn't match a known column.
    pub unresolved_column: bool,
    /// Source span of the op-column reference.
    pub source_info: SourceInfo,
}

impl OpColumnBinding {
    /// Look up the resolved column on the bound table. Returns
    /// `None` when the binding is unresolved or `snapshots` doesn't
    /// contain the bound database.
    #[must_use]
    pub fn resolved_column<'a>(
        &self,
        snapshots: &'a HashMap<SmolStr, ResolvedDatabase>,
    ) -> Option<&'a ResolvedColumn> {
        let snapshot = snapshots.get(&self.database_fqn)?;
        let table = snapshot.tables_by_name.get(&self.table_name)?;
        let idx = self.column_index?;
        table.columns.get(idx)
    }
}

/// Resolved op-body — the column bindings extracted from one Filter
/// / Join / MultiGrainFilter body, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOpBody {
    /// Owning element's simple name (e.g. `"trade_product"` for
    /// `Join trade_product (...)`).
    pub element_name: SmolStr,
    /// All `OpColumn::Aliased` bindings inside the body.
    pub bindings: Vec<OpColumnBinding>,
}

/// Resolved value-side binding for one `ViewColumnMappingLine` — the
/// column reference embedded in `JoinColWithDbOrConstant`'s `column`
/// field (after any join sequence). `None` when the line's value is
/// a constant literal or the column ref couldn't be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedViewColumn {
    /// View column name as written.
    pub column_name: SmolStr,
    /// Optional `[targetSetId]` qualifier on the column header.
    pub target_set_id: Option<SmolStr>,
    /// Resolved binding for the value's terminal column ref. `None`
    /// when the value is a constant literal.
    pub value_binding: Option<OpColumnBinding>,
}

/// Resolved view body — column bindings for every
/// `ViewColumnMappingLine`, plus the inferred main table (or `None`
/// when no consistent main table is detectable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedViewBody {
    /// View name as written.
    pub view_name: SmolStr,
    /// Resolved column lines, in source order.
    pub columns: Vec<ResolvedViewColumn>,
    /// All distinct table names referenced by this view's column
    /// values (and by the optional `~filter` chain). When the set
    /// has size 1 the view has a clean main table; size > 1 means
    /// either Java would error (multiple main tables) or the view
    /// pulls from joins.
    pub referenced_tables: Vec<SmolStr>,
    /// Inferred main table — populated when `referenced_tables`
    /// reduces to a single entry through the column chain. `None`
    /// when the view spans multiple unrelated tables (Java's
    /// `identifyMainTable` raises an error in that case; we record
    /// `None` and leave the user-facing diagnostic to a future
    /// validator).
    pub main_table: Option<SmolStr>,
}

/// Per-database resolved snapshot produced by [`process_database`].
///
/// Lives in extension state, not on the [`PureModel`]. Consumers
/// retrieve it via [`crate::compiler::RelationalExtension::resolved_databases`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDatabase {
    /// Database FQN (e.g. `"pkg::db"`).
    pub fqn: SmolStr,
    /// Tables indexed by simple name. Top-level tables and
    /// schema-nested tables share one namespace, mirroring
    /// `collect_visible_tables`'s key shape.
    pub tables_by_name: HashMap<SmolStr, ResolvedTable>,
    /// View names defined on this database, by simple name. Body
    /// resolution lands in B3.
    pub view_names: Vec<SmolStr>,
    /// Join names defined on this database.
    pub join_names: Vec<SmolStr>,
    /// Filter names defined on this database.
    pub filter_names: Vec<SmolStr>,
    /// `MultiGrainFilter` names defined on this database.
    pub multi_grain_filter_names: Vec<SmolStr>,
    /// `include other::Db` directives, keyed by target FQN — copied
    /// from the AST so consumers can walk the include graph without
    /// re-resolving.
    pub include_fqns: Vec<SmolStr>,
    /// Resolved op-bodies for every Filter on this database, in
    /// source order. Populated by [`resolve_op_bodies`] (Phase B2).
    pub filter_bodies: Vec<ResolvedOpBody>,
    /// Resolved op-bodies for every Join on this database.
    pub join_bodies: Vec<ResolvedOpBody>,
    /// Resolved op-bodies for every MultiGrainFilter on this database.
    pub multi_grain_filter_bodies: Vec<ResolvedOpBody>,
    /// Resolved view bodies for every View on this database.
    /// Populated by [`resolve_view_bodies`] (Phase B3).
    pub view_bodies: Vec<ResolvedViewBody>,
}

// ---------------------------------------------------------------------------
// Processor entry point
// ---------------------------------------------------------------------------

/// Build a [`ResolvedDatabase`] for one [`DatabaseDef`]. The walk is
/// strictly local: it does not chase `include` edges (the include
/// closure is the caller's job — see
/// [`crate::compiler::RelationalExtension::resolved_databases`] for
/// the per-database lookup pattern).
#[must_use]
pub fn process_database(def: &DatabaseDef) -> ResolvedDatabase {
    let mut tables_by_name: HashMap<SmolStr, ResolvedTable> = HashMap::new();
    let mut view_names: Vec<SmolStr> = Vec::new();
    let mut join_names: Vec<SmolStr> = Vec::new();
    let mut filter_names: Vec<SmolStr> = Vec::new();
    let mut multi_grain_filter_names: Vec<SmolStr> = Vec::new();

    for elem in &def.elements {
        match elem {
            DatabaseElement::Schema(s) => {
                for t in &s.tables {
                    let resolved = resolve_table(t, Some(s.name.value.clone()));
                    tables_by_name.insert(t.name.value.clone(), resolved);
                }
                for v in &s.views {
                    view_names.push(v.name.value.clone());
                }
            }
            DatabaseElement::Table(t) => {
                let resolved = resolve_table(t, None);
                tables_by_name.insert(t.name.value.clone(), resolved);
            }
            DatabaseElement::View(v) => {
                view_names.push(v.name.value.clone());
            }
            DatabaseElement::Join(j) => {
                join_names.push(j.name.value.clone());
            }
            DatabaseElement::Filter(f) => {
                filter_names.push(f.name.value.clone());
            }
            DatabaseElement::MultiGrainFilter(m) => {
                multi_grain_filter_names.push(m.name.value.clone());
            }
        }
    }

    let include_fqns: Vec<SmolStr> = def
        .includes
        .iter()
        .map(|inc| {
            let mut s = String::new();
            if let Some(pkg) = inc.included.package.as_ref() {
                for seg in pkg.segments() {
                    s.push_str(seg.as_str());
                    s.push_str("::");
                }
            }
            s.push_str(inc.included.name.as_str());
            SmolStr::new(&s)
        })
        .collect();

    ResolvedDatabase {
        fqn: database_fqn(def),
        tables_by_name,
        view_names,
        join_names,
        filter_names,
        multi_grain_filter_names,
        include_fqns,
        filter_bodies: Vec::new(),
        join_bodies: Vec::new(),
        multi_grain_filter_bodies: Vec::new(),
        view_bodies: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Phase B2 — op-column reference resolution
// ---------------------------------------------------------------------------

/// Walk every Filter / Join / MultiGrainFilter op-body on every
/// registered database, producing [`ResolvedOpBody`] rows that map
/// each `OpColumn::Aliased` reference to a `(database_fqn,
/// table_name, column_index)` binding. Cross-db `[db]` qualifiers
/// resolve through `defs_by_fqn`; missing-db / missing-table /
/// missing-column cases are recorded as flags on the binding (the
/// V4 validator emits the user-facing error).
///
/// Mutates `snapshots` in place — assumes the snapshots have already
/// been populated by [`process_database`].
pub fn resolve_op_bodies(
    snapshots: &mut HashMap<SmolStr, ResolvedDatabase>,
    defs_by_fqn: &HashMap<SmolStr, DatabaseDef>,
) {
    // Pre-compute visible-table sets per database so cross-db
    // resolution can answer "which tables can `dbX` see?" quickly.
    let visible_tables: HashMap<SmolStr, HashSet<SmolStr>> = defs_by_fqn
        .iter()
        .map(|(fqn, def)| (fqn.clone(), collect_visible_table_names(def, defs_by_fqn)))
        .collect();

    for (db_fqn, def) in defs_by_fqn {
        let mut filter_bodies: Vec<ResolvedOpBody> = Vec::new();
        let mut join_bodies: Vec<ResolvedOpBody> = Vec::new();
        let mut multi_grain_filter_bodies: Vec<ResolvedOpBody> = Vec::new();

        for elem in &def.elements {
            match elem {
                DatabaseElement::Filter(f) => {
                    filter_bodies.push(resolve_op_body(
                        &f.name.value,
                        &f.body,
                        db_fqn,
                        snapshots,
                        &visible_tables,
                    ));
                }
                DatabaseElement::Join(j) => {
                    join_bodies.push(resolve_op_body(
                        &j.name.value,
                        &j.body,
                        db_fqn,
                        snapshots,
                        &visible_tables,
                    ));
                }
                DatabaseElement::MultiGrainFilter(m) => {
                    multi_grain_filter_bodies.push(resolve_op_body(
                        &m.name.value,
                        &m.body,
                        db_fqn,
                        snapshots,
                        &visible_tables,
                    ));
                }
                _ => {}
            }
        }

        if let Some(snapshot) = snapshots.get_mut(db_fqn) {
            snapshot.filter_bodies = filter_bodies;
            snapshot.join_bodies = join_bodies;
            snapshot.multi_grain_filter_bodies = multi_grain_filter_bodies;
        }
    }
}

fn resolve_op_body(
    element_name: &SmolStr,
    body: &OpExpr,
    owning_db_fqn: &SmolStr,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    visible_tables: &HashMap<SmolStr, HashSet<SmolStr>>,
) -> ResolvedOpBody {
    let mut bindings: Vec<OpColumnBinding> = Vec::new();
    walk_op_expr_for_columns(
        body,
        owning_db_fqn,
        snapshots,
        visible_tables,
        &mut bindings,
    );
    ResolvedOpBody {
        element_name: element_name.clone(),
        bindings,
    }
}

fn walk_op_expr_for_columns(
    expr: &OpExpr,
    owning_db_fqn: &SmolStr,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    visible_tables: &HashMap<SmolStr, HashSet<SmolStr>>,
    out: &mut Vec<OpColumnBinding>,
) {
    match expr {
        OpExpr::Bool { lhs, rhs, .. } | OpExpr::Compare { lhs, rhs, .. } => {
            walk_op_expr_for_columns(lhs, owning_db_fqn, snapshots, visible_tables, out);
            walk_op_expr_for_columns(rhs, owning_db_fqn, snapshots, visible_tables, out);
        }
        OpExpr::IsNull { expr, .. } | OpExpr::Group { inner: expr, .. } => {
            walk_op_expr_for_columns(expr, owning_db_fqn, snapshots, visible_tables, out);
        }
        OpExpr::Function { args, .. } => {
            for a in args {
                walk_op_expr_for_columns(a, owning_db_fqn, snapshots, visible_tables, out);
            }
        }
        OpExpr::Column(OpColumn::Aliased {
            db,
            alias,
            scope,
            source_info,
            ..
        }) => {
            // Resolve target database. Explicit `[db]` overrides;
            // otherwise inherit from the owning element.
            let (target_db_fqn, unresolved_database) = match db {
                Some(d) => {
                    let fqn = packageable_fqn_from_ptr(d);
                    let exists = snapshots.contains_key(&fqn);
                    (fqn, !exists)
                }
                None => (owning_db_fqn.clone(), false),
            };

            // Resolve table existence + column index. When the table
            // resolves through an included database, record the home
            // db FQN on the binding so `resolved_column` finds it.
            let mut unresolved_table = false;
            let mut unresolved_column = false;
            let mut column_index: Option<usize> = None;
            let mut bound_db_fqn = target_db_fqn.clone();

            if !unresolved_database {
                let table_visible = visible_tables
                    .get(&target_db_fqn)
                    .is_some_and(|set| set.contains(&alias.value));
                if !table_visible {
                    unresolved_table = true;
                } else if let Some(scope_seg) = scope.first() {
                    // Walk visible snapshots to find the column AND
                    // the database where it lives.
                    if let Some((home_db, idx)) = lookup_column_via_includes(
                        snapshots,
                        &target_db_fqn,
                        &alias.value,
                        &scope_seg.value,
                    ) {
                        bound_db_fqn = home_db;
                        column_index = Some(idx);
                    } else {
                        unresolved_column = true;
                    }
                } else if let Some(home_db) =
                    lookup_table_home_db(snapshots, &target_db_fqn, &alias.value)
                {
                    // Alias-only ref (no scope) — still record the home db.
                    bound_db_fqn = home_db;
                }
            }

            out.push(OpColumnBinding {
                database_fqn: bound_db_fqn,
                table_name: alias.value.clone(),
                column_index,
                unresolved_database,
                unresolved_table,
                unresolved_column,
                source_info: source_info.clone(),
            });
        }
        OpExpr::Column(OpColumn::Target { .. }) | OpExpr::Literal(_) => {}
        OpExpr::Array { elements, .. } => {
            for e in elements {
                walk_op_expr_for_columns(e, owning_db_fqn, snapshots, visible_tables, out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase B3 — view body resolution + main-table inference
// ---------------------------------------------------------------------------

/// Walk every View on every registered database, producing a
/// [`ResolvedViewBody`] per view. Each view-column-mapping-line's
/// terminal column reference is resolved against the snapshot map
/// (with cross-db `[db]` qualifiers and include closures handled).
/// The view's main table is inferred when all column lines reference
/// the same table; otherwise `main_table` stays `None`.
///
/// Mutates `snapshots` in place.
pub fn resolve_view_bodies(
    snapshots: &mut HashMap<SmolStr, ResolvedDatabase>,
    defs_by_fqn: &HashMap<SmolStr, DatabaseDef>,
) {
    let visible_tables: HashMap<SmolStr, HashSet<SmolStr>> = defs_by_fqn
        .iter()
        .map(|(fqn, def)| (fqn.clone(), collect_visible_table_names(def, defs_by_fqn)))
        .collect();

    for (db_fqn, def) in defs_by_fqn {
        let mut view_bodies: Vec<ResolvedViewBody> = Vec::new();
        for elem in &def.elements {
            match elem {
                DatabaseElement::View(v) => {
                    view_bodies.push(resolve_view(v, db_fqn, snapshots, &visible_tables));
                }
                DatabaseElement::Schema(s) => {
                    for v in &s.views {
                        view_bodies.push(resolve_view(v, db_fqn, snapshots, &visible_tables));
                    }
                }
                _ => {}
            }
        }
        if let Some(snapshot) = snapshots.get_mut(db_fqn) {
            snapshot.view_bodies = view_bodies;
        }
    }
}

fn resolve_view(
    v: &View,
    owning_db_fqn: &SmolStr,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    visible_tables: &HashMap<SmolStr, HashSet<SmolStr>>,
) -> ResolvedViewBody {
    let mut columns: Vec<ResolvedViewColumn> = Vec::with_capacity(v.columns.len());
    let mut referenced: Vec<SmolStr> = Vec::new();
    let mut seen: HashSet<SmolStr> = HashSet::new();

    for line in &v.columns {
        let value_binding =
            resolve_view_column_value(&line.value, owning_db_fqn, snapshots, visible_tables);
        if let Some(b) = &value_binding {
            if !b.unresolved_table && seen.insert(b.table_name.clone()) {
                referenced.push(b.table_name.clone());
            }
        }
        columns.push(ResolvedViewColumn {
            column_name: line.column_name.value.clone(),
            target_set_id: line.target_set_id.as_ref().map(|t| t.value.clone()),
            value_binding,
        });
    }

    // Filter chain may reference additional tables — track those too.
    if let Some(filter) = &v.filter {
        if let Some(chain) = &filter.db_chain {
            // The join sequence's chain doesn't carry direct
            // table-name references at this layer; the joins point
            // at Join elements whose bodies were already resolved
            // by B2. We can revisit per-segment table inference in
            // B7 once implicit-db chains land.
            let _ = chain;
        }
    }

    let main_table = if referenced.len() == 1 {
        Some(referenced[0].clone())
    } else {
        None
    };

    ResolvedViewBody {
        view_name: v.name.value.clone(),
        columns,
        referenced_tables: referenced,
        main_table,
    }
}

fn resolve_view_column_value(
    jc: &JoinColWithDbOrConstant,
    owning_db_fqn: &SmolStr,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    visible_tables: &HashMap<SmolStr, HashSet<SmolStr>>,
) -> Option<OpColumnBinding> {
    let column = jc.column.as_ref()?;
    let OpColumn::Aliased {
        db,
        alias,
        scope,
        source_info,
        ..
    } = column
    else {
        // `{target}.col` form is illegal in view bodies (Java's grammar
        // only allows it inside Join op_columns). Skip.
        return None;
    };

    // Explicit `[db]` on the inner op_column overrides the outer
    // `JoinColWithDbOrConstant.db`; otherwise fall back to the
    // outer-level `[db]` and finally to the owning database.
    let target_db_ptr = db.as_ref().or(jc.db.as_ref());
    let (target_db_fqn, unresolved_database) = match target_db_ptr {
        Some(d) => {
            let fqn = packageable_fqn_from_ptr(d);
            let exists = snapshots.contains_key(&fqn);
            (fqn, !exists)
        }
        None => (owning_db_fqn.clone(), false),
    };

    let mut unresolved_table = false;
    let mut unresolved_column = false;
    let mut column_index: Option<usize> = None;
    let mut bound_db_fqn = target_db_fqn.clone();

    if !unresolved_database {
        let table_visible = visible_tables
            .get(&target_db_fqn)
            .is_some_and(|set| set.contains(&alias.value));
        if !table_visible {
            unresolved_table = true;
        } else if let Some(scope_seg) = scope.first() {
            if let Some((home_db, idx)) = lookup_column_via_includes(
                snapshots,
                &target_db_fqn,
                &alias.value,
                &scope_seg.value,
            ) {
                bound_db_fqn = home_db;
                column_index = Some(idx);
            } else {
                unresolved_column = true;
            }
        } else if let Some(home_db) = lookup_table_home_db(snapshots, &target_db_fqn, &alias.value)
        {
            bound_db_fqn = home_db;
        }
    }

    Some(OpColumnBinding {
        database_fqn: bound_db_fqn,
        table_name: alias.value.clone(),
        column_index,
        unresolved_database,
        unresolved_table,
        unresolved_column,
        source_info: source_info.clone(),
    })
}

fn packageable_fqn_from_ptr(
    p: &legend_pure_parser_ast::annotation::PackageableElementPtr,
) -> SmolStr {
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

/// Build the visible-table set for `def` (own tables + transitive
/// `include` closure). Schema-nested tables and top-level tables
/// share the same namespace.
fn collect_visible_table_names(
    def: &DatabaseDef,
    all_defs: &HashMap<SmolStr, DatabaseDef>,
) -> HashSet<SmolStr> {
    let mut out: HashSet<SmolStr> = HashSet::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_collect_tables(def, all_defs, &mut out, &mut visited);
    out
}

fn walk_collect_tables(
    def: &DatabaseDef,
    all_defs: &HashMap<SmolStr, DatabaseDef>,
    out: &mut HashSet<SmolStr>,
    visited: &mut HashSet<SmolStr>,
) {
    let fqn = database_fqn(def);
    if !visited.insert(fqn) {
        return;
    }
    for elem in &def.elements {
        match elem {
            DatabaseElement::Schema(s) => {
                for t in &s.tables {
                    out.insert(t.name.value.clone());
                }
                for v in &s.views {
                    out.insert(v.name.value.clone());
                }
            }
            DatabaseElement::Table(t) => {
                out.insert(t.name.value.clone());
            }
            DatabaseElement::View(v) => {
                out.insert(v.name.value.clone());
            }
            _ => {}
        }
    }
    for inc in &def.includes {
        let target_fqn = packageable_fqn_from_ptr(&inc.included);
        if let Some(included_def) = all_defs.get(&target_fqn) {
            walk_collect_tables(included_def, all_defs, out, visited);
        }
    }
}

/// Look up a column by walking `db_fqn`'s snapshot and the
/// snapshots of its transitively-included databases. Returns the
/// `(home_db_fqn, column_index)` of the first match.
fn lookup_column_via_includes(
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    db_fqn: &SmolStr,
    table_name: &str,
    column_name: &str,
) -> Option<(SmolStr, usize)> {
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_lookup_column(snapshots, db_fqn, table_name, column_name, &mut visited)
}

fn walk_lookup_column(
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    db_fqn: &SmolStr,
    table_name: &str,
    column_name: &str,
    visited: &mut HashSet<SmolStr>,
) -> Option<(SmolStr, usize)> {
    if !visited.insert(db_fqn.clone()) {
        return None;
    }
    let snapshot = snapshots.get(db_fqn)?;
    if let Some(t) = snapshot.tables_by_name.get(table_name) {
        if let Some(idx) = t.columns_by_name.get(column_name) {
            return Some((db_fqn.clone(), *idx));
        }
    }
    for include_fqn in &snapshot.include_fqns {
        if let Some(found) =
            walk_lookup_column(snapshots, include_fqn, table_name, column_name, visited)
        {
            return Some(found);
        }
    }
    None
}

/// Find the database FQN that owns `table_name`, walking the include
/// closure of `db_fqn`. Used by alias-only column refs that need to
/// pin the home db without a column lookup.
fn lookup_table_home_db(
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    db_fqn: &SmolStr,
    table_name: &str,
) -> Option<SmolStr> {
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_lookup_home_db(snapshots, db_fqn, table_name, &mut visited)
}

fn walk_lookup_home_db(
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    db_fqn: &SmolStr,
    table_name: &str,
    visited: &mut HashSet<SmolStr>,
) -> Option<SmolStr> {
    if !visited.insert(db_fqn.clone()) {
        return None;
    }
    let snapshot = snapshots.get(db_fqn)?;
    if snapshot.tables_by_name.contains_key(table_name) {
        return Some(db_fqn.clone());
    }
    for include_fqn in &snapshot.include_fqns {
        if let Some(found) = walk_lookup_home_db(snapshots, include_fqn, table_name, visited) {
            return Some(found);
        }
    }
    None
}

fn resolve_table(t: &crate::ast::Table, schema: Option<SmolStr>) -> ResolvedTable {
    let mut columns: Vec<ResolvedColumn> = Vec::with_capacity(t.columns.len());
    let mut columns_by_name: HashMap<SmolStr, usize> = HashMap::with_capacity(t.columns.len());
    for (i, c) in t.columns.iter().enumerate() {
        columns.push(resolve_column(c));
        columns_by_name.insert(c.name.value.clone(), i);
    }
    ResolvedTable {
        schema,
        name: t.name.value.clone(),
        columns,
        columns_by_name,
    }
}

fn resolve_column(c: &ColumnDef) -> ResolvedColumn {
    let pure_type = PureColumnType::from_sql_name(c.type_name.value.as_str());
    ResolvedColumn {
        name: c.name.value.clone(),
        source_type: c.type_name.value.clone(),
        pure_type,
        size: c.size,
        scale: c.scale,
        primary_key: c.primary_key,
        not_null: c.not_null,
    }
}

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
