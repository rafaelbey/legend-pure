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

//! Phase B1+B2+B3+B4+B5+B6+C (and B7 + A3' in compiler.rs):
//! post-processor for the relational DSL.
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
//! - **Class-mapping property mappings** (B4) — for every registered
//!   `Class : Relational { ... }` body, each `SingleMappingLine`'s
//!   value is resolved to a [`ResolvedClassMappingProperty`]. The
//!   mapping's primary database flows from `~mainTable [db]`;
//!   scope-wrapped lines pick up the scope's `[db]`. Embedded
//!   mappings are tagged but not recursed.
//! - **Main-table inheritance through `extends`** (B5) — when a
//!   class mapping has `extends [parentId]` and no `~mainTable` of
//!   its own, the inheritance walker fills `effective_main_table`
//!   and `effective_primary_database` from the nearest ancestor
//!   that declares one. Cycles terminate cleanly.
//! - **AssociationMapping bodies** (B6) — bodies whose AST has
//!   `association_mapping = Some(lines)` are tagged with
//!   `ResolvedClassMappingKind::Association` and the two property
//!   lines (one per association end) get the same value-side
//!   resolution as class-mapping properties. Each end's
//!   `[srcId, tgtId]` tags are preserved for downstream consumers.
//!
//! - **Milestoning auto-rewrite** (Phase C) — for every class mapping
//!   whose effective main table declares a `milestoning(...)` spec,
//!   `apply_milestoning_synthesis` populates
//!   `synthesized_milestoning` with the embedded property mapping
//!   Java's `MilestoningPropertyMappingProcessor` would emit. Maps
//!   `BUS_FROM`/`BUS_THRU` → `from`/`thru`, `PROCESSING_IN`/
//!   `PROCESSING_OUT` → `in`/`out`, and snapshot variants both onto
//!   the single `*_SNAPSHOT_DATE` column.
//!
//! What this module deliberately defers:
//!
//! - Embedded-body recursion (depends on broader plumbing)
//!
//! Phase B7 (implicit-db `@join` resolution) and A3'
//! (JoinTreeNodeValidation) live in `compiler.rs` since they emit
//! validator diagnostics rather than building parallel resolved
//! state.

use std::collections::{HashMap, HashSet};

use smol_str::SmolStr;

use legend_pure_parser_ast::SourceInfo;

use crate::ast::{
    ColumnDef, DatabaseDef, DatabaseElement, JoinColWithDbOrConstant, MappingElement,
    MilestoneSpec, MilestoneValue, NonePlusMappingValue, OpColumn, OpExpr,
    RelationalClassMappingBody, SingleMappingLine, View,
};

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
            // common SQL alias for TIMESTAMP
            "TIMESTAMP" | "DATETIME" => Some(Self::Timestamp),
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
    /// Resolved milestoning spec when the table declares
    /// `milestoning(...)`. Phase C uses this to drive synthesised
    /// property mappings on class mappings that target this table.
    pub milestoning: Option<ResolvedMilestoning>,
}

/// Java-parity milestoning kind, derived from the `MilestoneDef.kind`
/// keyword (`business`, `processing`, etc.). Snapshot variants are
/// flagged separately so synthesis can pick the right property →
/// column mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MilestoningKind {
    /// `business( BUS_FROM=col, BUS_THRU=col, ... )`. Java
    /// `MilestoningStereotypeEnum.businesstemporal`.
    Business,
    /// `business( BUS_SNAPSHOT_DATE=col, ... )`. Java relational
    /// `BusinessSnapshotMilestoning`.
    BusinessSnapshot,
    /// `processing( PROCESSING_IN=col, PROCESSING_OUT=col, ... )`.
    /// Java `MilestoningStereotypeEnum.processingtemporal`.
    Processing,
    /// `processing( PROCESSING_SNAPSHOT_DATE=col, ... )`. Java
    /// relational `ProcessingSnapshotMilestoning`.
    ProcessingSnapshot,
}

/// Resolved milestoning spec on a [`ResolvedTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMilestoning {
    /// One entry per `MilestoneDef` on the table — typically one
    /// (e.g. `business`) or two (`business` + `processing` for
    /// bitemporal). Each entry carries its kind + identifier-valued
    /// fields.
    pub definitions: Vec<ResolvedMilestoningDef>,
}

/// One milestoning flavour entry — `business(...)`, `processing(...)`,
/// etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMilestoningDef {
    /// Kind tag derived from the `MilestoneDef.kind` keyword + which
    /// fields are set.
    pub kind: MilestoningKind,
    /// Identifier-valued field bindings (e.g. `BUS_FROM=col` →
    /// `(key="BUS_FROM", column_name="col", column_index=Some(i))`).
    /// Date / boolean literal fields are filtered out — they have no
    /// column to bind.
    pub fields: Vec<ResolvedMilestoningField>,
}

/// One identifier-valued milestoning field binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMilestoningField {
    /// Field key (e.g. `"BUS_FROM"`, `"PROCESSING_IN"`).
    pub key: SmolStr,
    /// Column name on the table.
    pub column_name: SmolStr,
    /// Column index into [`ResolvedTable::columns`]. `None` when the
    /// referenced column couldn't be resolved.
    pub column_index: Option<usize>,
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

/// One resolved property mapping inside a relational class-mapping
/// body. Mirrors the shape of [`crate::ast::SingleMappingLine`] but
/// with the value's terminal column reference resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedClassMappingProperty {
    /// Property being mapped on the target class.
    pub property_name: SmolStr,
    /// Optional `[srcId]` tag.
    pub source_id: Option<SmolStr>,
    /// Optional `[srcId, targetId]` second tag.
    pub target_id: Option<SmolStr>,
    /// Resolved value-side payload — the kind tells whether the
    /// line was bare (Single), a `+local` Plus line, or an Embedded
    /// (whose recursion is deferred to a later phase).
    pub kind: ResolvedClassMappingPropertyKind,
}

/// Shape of a resolved class-mapping property's RHS.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolvedClassMappingPropertyKind {
    /// Bare `prop : <joinCol>` with the terminal column ref bound.
    /// `binding = None` when the value was a literal constant or
    /// resolution failed.
    Single {
        /// Resolved binding for the value's terminal column ref.
        binding: Option<OpColumnBinding>,
    },
    /// `+prop : Type[mult] : <joinCol>` plus-form local property.
    Plus {
        /// Resolved binding for the value's terminal column ref.
        binding: Option<OpColumnBinding>,
    },
    /// Embedded `(<inner-mappings>) (Inline | Otherwise)?` form —
    /// recursion deferred. Java's `RelationalPropertyMappingProcessor`
    /// recurses into the embedded body; our scaffolding tracks the
    /// shape so a later phase can fill it in.
    Embedded,
}

/// Whether a `ResolvedClassMapping` represents a regular class
/// mapping or an `AssociationMapping ( ... )` body. Set during
/// resolution; consumers can switch on it to enforce
/// association-only constraints (e.g. exactly two property
/// mapping lines per Java's grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResolvedClassMappingKind {
    /// Regular class mapping body.
    Class,
    /// `AssociationMapping (...)` body — exactly two property lines
    /// (one per association end).
    Association,
}

/// Resolved class-mapping snapshot produced by Phase B4 + B5 + B6.
/// Lives in extension state, accessible via
/// [`crate::compiler::RelationalExtension::resolved_class_mappings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedClassMapping {
    /// FQN of the enclosing `Mapping`.
    pub mapping_fqn: SmolStr,
    /// Class-mapping id — the `[id]` if explicitly set in source,
    /// otherwise the class's FQN.
    pub class_mapping_id: SmolStr,
    /// Whether this is a regular class mapping or an
    /// `AssociationMapping (...)` body.
    pub kind: ResolvedClassMappingKind,
    /// `extends [superId]` — captured verbatim from the AST.
    pub extends: Option<SmolStr>,
    /// Database FQN declared on `~mainTable [db]<table>`. `None` when
    /// the body has no `~mainTable` block.
    pub primary_database: Option<SmolStr>,
    /// Table name declared on `~mainTable`. `None` when the body has
    /// no `~mainTable` block.
    pub main_table: Option<SmolStr>,
    /// Effective main-table table name after walking the `extends`
    /// chain (Phase B5). When `main_table` is set on this body
    /// itself, the values match. Otherwise it's inherited from the
    /// nearest ancestor that declares one. `None` if no ancestor
    /// declares a main table or the chain has a cycle.
    pub effective_main_table: Option<SmolStr>,
    /// Effective primary database after walking the `extends`
    /// chain. Same inheritance rule as `effective_main_table`.
    pub effective_primary_database: Option<SmolStr>,
    /// Inferred main table — populated when all property values
    /// reference the same table. Independent of the explicit
    /// `~mainTable` declaration.
    pub inferred_main_table: Option<SmolStr>,
    /// Distinct table names referenced by property values, in
    /// source order. Useful for the multiple-main-tables diagnostic.
    pub referenced_tables: Vec<SmolStr>,
    /// Resolved property mappings, top-level only — Embedded
    /// recursion is deferred. Scope-wrapped lines are flattened
    /// into this list (the scope's `[db]` is honoured during
    /// resolution but the wrapping is not preserved here).
    pub properties: Vec<ResolvedClassMappingProperty>,
    /// Synthesised milestoning embedded mapping (Phase C). Populated
    /// when the effective main table declares a `milestoning(...)`
    /// spec — Java's
    /// `MilestoningPropertyMappingProcessor.createMilestoningPropertyMapping`
    /// inserts a synthetic embedded `RelationalInstanceSetImplementation`
    /// at id `<parent>_milestoning`. We carry the parallel data here.
    pub synthesized_milestoning: Option<SynthesizedMilestoningMapping>,
}

/// Synthesized embedded milestoning mapping produced by Phase C.
///
/// Java parity: `EmbeddedRelationalInstanceSetImplementation` with
/// `id = "<parent>_milestoning"`, `_property` named `milestoning`,
/// and one `RelationalPropertyMapping` per Pure milestoning property
/// (`from`, `thru`, `in`, `out`) bound to the corresponding
/// `MilestoneSpec` field's column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizedMilestoningMapping {
    /// `<parent_class_mapping_id>_milestoning`.
    pub id: SmolStr,
    /// Parent class mapping id this milestoning embedded belongs to.
    pub source_set_implementation_id: SmolStr,
    /// `(property_name, OpColumnBinding)` pairs in canonical order
    /// (`in` before `out` before `from` before `thru`, matching
    /// Java's `compareTemporalDatePropertyNames`). Each binding
    /// resolves to a column on the effective main table.
    pub property_bindings: Vec<(SmolStr, OpColumnBinding)>,
}

/// Per-database resolved snapshot produced by [`process_database`].
///
/// Lives in extension state, not on the `PureModel`. Consumers
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
        if let Some(b) = &value_binding
            && !b.unresolved_table
            && seen.insert(b.table_name.clone())
        {
            referenced.push(b.table_name.clone());
        }
        columns.push(ResolvedViewColumn {
            column_name: line.column_name.value.clone(),
            target_set_id: line.target_set_id.as_ref().map(|t| t.value.clone()),
            value_binding,
        });
    }

    // Filter chain may reference additional tables — track those too.
    if let Some(filter) = &v.filter
        && let Some(chain) = &filter.db_chain
    {
        // The join sequence's chain doesn't carry direct
        // table-name references at this layer; the joins point
        // at Join elements whose bodies were already resolved
        // by B2. We can revisit per-segment table inference in
        // B7 once implicit-db chains land.
        let _ = chain;
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
    if let Some(t) = snapshot.tables_by_name.get(table_name)
        && let Some(idx) = t.columns_by_name.get(column_name)
    {
        return Some((db_fqn.clone(), *idx));
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
    let milestoning = t
        .milestoning
        .as_ref()
        .map(|spec| resolve_milestoning(spec, &columns_by_name));
    ResolvedTable {
        schema,
        name: t.name.value.clone(),
        columns,
        columns_by_name,
        milestoning,
    }
}

fn resolve_milestoning(
    spec: &MilestoneSpec,
    columns_by_name: &HashMap<SmolStr, usize>,
) -> ResolvedMilestoning {
    let mut definitions: Vec<ResolvedMilestoningDef> = Vec::with_capacity(spec.definitions.len());
    for def in &spec.definitions {
        let mut fields: Vec<ResolvedMilestoningField> = Vec::new();
        let mut keys: HashSet<SmolStr> = HashSet::new();
        for f in &def.fields {
            keys.insert(f.key.value.clone());
            if let MilestoneValue::Identifier(id) = &f.value {
                fields.push(ResolvedMilestoningField {
                    key: f.key.value.clone(),
                    column_name: id.value.clone(),
                    column_index: columns_by_name.get(&id.value).copied(),
                });
            }
        }
        let kind = milestoning_kind(def.kind.value.as_str(), &keys);
        definitions.push(ResolvedMilestoningDef { kind, fields });
    }
    ResolvedMilestoning { definitions }
}

/// Map the `MilestoneDef.kind` keyword + the set of declared field
/// keys onto a [`MilestoningKind`]. Snapshot variants are detected
/// when a `*_SNAPSHOT_DATE` field is present (mirrors Java's
/// relational metamodel which has dedicated `*SnapshotMilestoning`
/// classes for those shapes).
fn milestoning_kind(kw: &str, keys: &HashSet<SmolStr>) -> MilestoningKind {
    match kw {
        "business" => {
            if keys.contains("BUS_SNAPSHOT_DATE") {
                MilestoningKind::BusinessSnapshot
            } else {
                MilestoningKind::Business
            }
        }
        "processing" => {
            if keys.contains("PROCESSING_SNAPSHOT_DATE") {
                MilestoningKind::ProcessingSnapshot
            } else {
                MilestoningKind::Processing
            }
        }
        // Unknown kind keyword → default to Business as a fallback.
        // Stage-3 validators reject unknown kinds with a structured
        // diagnostic; we never reach this branch on a clean compile.
        _ => MilestoningKind::Business,
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

/// Resolve every `SingleMappingLine` in `body` to a
/// [`ResolvedClassMappingProperty`]. The class-mapping's contextual
/// database is determined by the `~mainTable [db]` block when present
/// (overrides per-line `[db]` for inheriting cases); each scope-wrapped
/// line picks up the scope's `[db]` instead.
///
/// `defs_by_fqn` is required to know which databases exist for
/// cross-db `[db]` qualifier validation. Resolution flags
/// (`unresolved_database`, `unresolved_table`, `unresolved_column`)
/// remain as on the binding — the V4 / E validators emit user-facing
/// diagnostics; this resolver produces the parallel structure.
///
/// `effective_main_table` and `effective_primary_database` are not
/// populated here — they require the full set of class mappings for
/// the `extends` chain walk. Use [`apply_extends_inheritance`] on
/// the resolved set after this returns.
pub fn resolve_class_mapping(
    body: &RelationalClassMappingBody,
    mapping_fqn: &SmolStr,
    class_mapping_id: &SmolStr,
    extends: Option<&SmolStr>,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    defs_by_fqn: &HashMap<SmolStr, DatabaseDef>,
) -> ResolvedClassMapping {
    let visible_tables: HashMap<SmolStr, HashSet<SmolStr>> = defs_by_fqn
        .iter()
        .map(|(fqn, def)| (fqn.clone(), collect_visible_table_names(def, defs_by_fqn)))
        .collect();

    // Primary database + main table from `~mainTable`.
    let primary_database = body
        .main_table
        .as_ref()
        .map(|mt| packageable_fqn_from_ptr(&mt.db));
    let main_table = body
        .main_table
        .as_ref()
        .map(|mt| mt.scope.table.value.clone());

    let mut properties: Vec<ResolvedClassMappingProperty> = Vec::new();
    let mut referenced: Vec<SmolStr> = Vec::new();
    let mut seen: HashSet<SmolStr> = HashSet::new();

    let kind = if body.association_mapping.is_some() {
        ResolvedClassMappingKind::Association
    } else {
        ResolvedClassMappingKind::Class
    };

    // Choose the line set to walk based on body kind. Association
    // bodies use `body.association_mapping`; class bodies use
    // `body.mapping_elements` (with scope-wrapped lines flattened).
    let owning_db_fqn = primary_database.clone().unwrap_or_else(|| SmolStr::new(""));
    if let Some(assoc_lines) = &body.association_mapping {
        for line in assoc_lines {
            let prop =
                resolve_single_mapping_line(line, &owning_db_fqn, snapshots, &visible_tables);
            record_binding(&prop, &mut referenced, &mut seen);
            properties.push(prop);
        }
    } else {
        for elem in &body.mapping_elements {
            match elem {
                MappingElement::Single(line) => {
                    let prop = resolve_single_mapping_line(
                        line,
                        &owning_db_fqn,
                        snapshots,
                        &visible_tables,
                    );
                    record_binding(&prop, &mut referenced, &mut seen);
                    properties.push(prop);
                }
                MappingElement::Scope(scope) => {
                    // Scope sets a per-block contextual db.
                    let scope_db_fqn = packageable_fqn_from_ptr(&scope.db);
                    for line in &scope.mapping_lines {
                        let prop = resolve_single_mapping_line(
                            line,
                            &scope_db_fqn,
                            snapshots,
                            &visible_tables,
                        );
                        record_binding(&prop, &mut referenced, &mut seen);
                        properties.push(prop);
                    }
                }
            }
        }
    }

    let inferred_main_table = if referenced.len() == 1 {
        Some(referenced[0].clone())
    } else {
        None
    };

    let effective_main_table = main_table.clone();
    let effective_primary_database = primary_database.clone();

    ResolvedClassMapping {
        mapping_fqn: mapping_fqn.clone(),
        class_mapping_id: class_mapping_id.clone(),
        kind,
        extends: extends.cloned(),
        primary_database,
        main_table,
        effective_main_table,
        effective_primary_database,
        inferred_main_table,
        referenced_tables: referenced,
        properties,
        synthesized_milestoning: None,
    }
}

/// Phase B5 — apply `extends`-chain inheritance to the resolved
/// class mappings. Mirrors Java's
/// `RelationalInstanceSetImplementationProcessor`'s main-table
/// inheritance: when a class mapping has `extends [parentId]` and
/// no `~mainTable` of its own, walk up the chain to inherit the
/// nearest ancestor's main table + primary database.
///
/// Cycles in the `extends` graph terminate the walk silently
/// (the validator emits a structured error elsewhere).
pub fn apply_extends_inheritance(class_mappings: &mut [ResolvedClassMapping]) {
    // Index by (mapping_fqn, class_mapping_id) for fast parent lookup.
    let by_id: HashMap<(SmolStr, SmolStr), usize> = class_mappings
        .iter()
        .enumerate()
        .map(|(i, cm)| ((cm.mapping_fqn.clone(), cm.class_mapping_id.clone()), i))
        .collect();

    // Pre-compute the inherited values per index (reading current
    // state — no in-place mutation during walk). Then write back.
    let mut effective: Vec<(Option<SmolStr>, Option<SmolStr>)> =
        Vec::with_capacity(class_mappings.len());
    for cm in class_mappings.iter() {
        let mut visited: HashSet<(SmolStr, SmolStr)> = HashSet::new();
        let mut current = cm;
        let mut main_table = current.main_table.clone();
        let mut primary_db = current.primary_database.clone();
        while main_table.is_none() {
            let Some(parent_id) = &current.extends else {
                break;
            };
            let key = (current.mapping_fqn.clone(), parent_id.clone());
            if !visited.insert(key.clone()) {
                break;
            }
            let Some(idx) = by_id.get(&key) else {
                break;
            };
            current = &class_mappings[*idx];
            if current.main_table.is_some() {
                main_table.clone_from(&current.main_table);
                primary_db.clone_from(&current.primary_database);
            }
        }
        effective.push((main_table, primary_db));
    }

    for (cm, (mt, db)) in class_mappings.iter_mut().zip(effective) {
        cm.effective_main_table = mt;
        cm.effective_primary_database = db;
    }
}

fn record_binding(
    prop: &ResolvedClassMappingProperty,
    referenced: &mut Vec<SmolStr>,
    seen: &mut HashSet<SmolStr>,
) {
    let binding = match &prop.kind {
        ResolvedClassMappingPropertyKind::Single { binding }
        | ResolvedClassMappingPropertyKind::Plus { binding } => binding,
        ResolvedClassMappingPropertyKind::Embedded => return,
    };
    if let Some(b) = binding
        && !b.unresolved_table
        && seen.insert(b.table_name.clone())
    {
        referenced.push(b.table_name.clone());
    }
}

fn resolve_single_mapping_line(
    line: &SingleMappingLine,
    owning_db_fqn: &SmolStr,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
    visible_tables: &HashMap<SmolStr, HashSet<SmolStr>>,
) -> ResolvedClassMappingProperty {
    match line {
        SingleMappingLine::NonePlus(np) => {
            let kind = match &np.value {
                NonePlusMappingValue::Relational(rm) => {
                    let binding =
                        resolve_join_col_value(&rm.value, owning_db_fqn, snapshots, visible_tables);
                    ResolvedClassMappingPropertyKind::Single { binding }
                }
                NonePlusMappingValue::Embedded(_) => ResolvedClassMappingPropertyKind::Embedded,
            };
            ResolvedClassMappingProperty {
                property_name: np.property.value.clone(),
                source_id: np.source_id.as_ref().map(|s| s.value.clone()),
                target_id: np.target_id.as_ref().map(|t| t.value.clone()),
                kind,
            }
        }
        SingleMappingLine::Plus(p) => {
            let binding =
                resolve_join_col_value(&p.mapping.value, owning_db_fqn, snapshots, visible_tables);
            ResolvedClassMappingProperty {
                property_name: p.property.value.clone(),
                source_id: None,
                target_id: None,
                kind: ResolvedClassMappingPropertyKind::Plus { binding },
            }
        }
    }
}

/// Resolve a single `JoinColWithDbOrConstant`'s terminal column ref
/// against the snapshot map. Same machinery as the view resolver
/// (B3) — refactor candidate but kept inline so view / class-mapping
/// shapes can diverge later.
fn resolve_join_col_value(
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
        return None;
    };

    let target_db_ptr = db.as_ref().or(jc.db.as_ref());
    let (target_db_fqn, mut unresolved_database) = match target_db_ptr {
        Some(d) => {
            let fqn = packageable_fqn_from_ptr(d);
            let exists = snapshots.contains_key(&fqn);
            (fqn, !exists)
        }
        None => (owning_db_fqn.clone(), false),
    };
    if target_db_fqn.is_empty() {
        // No `~mainTable [db]` declared and no explicit `[db]` on the
        // column — we can't resolve. Mark database-unresolved so
        // consumers see a structured signal.
        unresolved_database = true;
    }

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

/// For each class mapping whose effective main table declares a
/// `milestoning(...)` spec, synthesise the embedded property mapping
/// Java's `MilestoningPropertyMappingProcessor.createMilestoningPropertyMapping`
/// inserts. The resulting [`SynthesizedMilestoningMapping`] lives on
/// `ResolvedClassMapping.synthesized_milestoning`.
///
/// Property → field mapping (Java parity):
/// - `business` → `from` ← `BUS_FROM`, `thru` ← `BUS_THRU`
/// - `business` snapshot → `from` ← `BUS_SNAPSHOT_DATE`, `thru` ← `BUS_SNAPSHOT_DATE`
/// - `processing` → `in` ← `PROCESSING_IN`, `out` ← `PROCESSING_OUT`
/// - `processing` snapshot → `in` ← `PROCESSING_SNAPSHOT_DATE`, `out` ← `PROCESSING_SNAPSHOT_DATE`
/// - bitemporal (both `business` + `processing` defs) → all four
///
/// Property names are emitted in canonical order (`in`, `out`, `from`,
/// `thru`) to match Java's
/// `MilestoningStereotypeEnum.compareTemporalDatePropertyNames`.
///
/// Children of an extending parent skip synthesis when the parent
/// also has a milestoned main table — Java's
/// `shouldCreateMilestoningPropertyMapping` checks
/// `superSetImplementationId == null`. Children with their own
/// non-milestoned main table (or whose extends chain has no
/// milestoning) get their own synthesised mapping.
pub fn apply_milestoning_synthesis(
    class_mappings: &mut [ResolvedClassMapping],
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
) {
    let mut by_id: HashMap<(SmolStr, SmolStr), bool> = HashMap::new();
    for cm in class_mappings.iter() {
        by_id.insert(
            (cm.mapping_fqn.clone(), cm.class_mapping_id.clone()),
            class_mapping_has_milestoned_main_table(cm, snapshots),
        );
    }

    for cm in class_mappings.iter_mut() {
        if let Some(parent_id) = &cm.extends {
            let parent_key = (cm.mapping_fqn.clone(), parent_id.clone());
            if by_id.get(&parent_key).copied().unwrap_or(false) {
                continue;
            }
        }
        let Some(synthesised) = synthesise_milestoning_for_class_mapping(cm, snapshots) else {
            continue;
        };
        cm.synthesized_milestoning = Some(synthesised);
    }
}

fn class_mapping_has_milestoned_main_table(
    cm: &ResolvedClassMapping,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
) -> bool {
    let Some(main_table) = &cm.effective_main_table else {
        return false;
    };
    let Some(db_fqn) = &cm.effective_primary_database else {
        return false;
    };
    let Some(snapshot) = snapshots.get(db_fqn) else {
        return false;
    };
    let Some(table) = snapshot.tables_by_name.get(main_table) else {
        return false;
    };
    table.milestoning.is_some()
}

fn synthesise_milestoning_for_class_mapping(
    cm: &ResolvedClassMapping,
    snapshots: &HashMap<SmolStr, ResolvedDatabase>,
) -> Option<SynthesizedMilestoningMapping> {
    let main_table_name = cm.effective_main_table.as_ref()?;
    let db_fqn = cm.effective_primary_database.as_ref()?;
    let snapshot = snapshots.get(db_fqn)?;
    let table = snapshot.tables_by_name.get(main_table_name)?;
    let milestoning = table.milestoning.as_ref()?;

    let mut by_key: HashMap<SmolStr, &ResolvedMilestoningField> = HashMap::new();
    let mut has_business = false;
    let mut has_business_snapshot = false;
    let mut has_processing = false;
    let mut has_processing_snapshot = false;
    for def in &milestoning.definitions {
        match def.kind {
            MilestoningKind::Business => has_business = true,
            MilestoningKind::BusinessSnapshot => has_business_snapshot = true,
            MilestoningKind::Processing => has_processing = true,
            MilestoningKind::ProcessingSnapshot => has_processing_snapshot = true,
        }
        for f in &def.fields {
            by_key.insert(f.key.clone(), f);
        }
    }

    let mut bindings: Vec<(SmolStr, OpColumnBinding)> = Vec::new();

    if has_processing {
        push_synth_binding(
            &mut bindings,
            "in",
            by_key.get("PROCESSING_IN").copied(),
            db_fqn,
            main_table_name,
            cm,
        );
        push_synth_binding(
            &mut bindings,
            "out",
            by_key.get("PROCESSING_OUT").copied(),
            db_fqn,
            main_table_name,
            cm,
        );
    } else if has_processing_snapshot
        && let Some(field) = by_key.get("PROCESSING_SNAPSHOT_DATE").copied()
    {
        push_synth_binding(
            &mut bindings,
            "in",
            Some(field),
            db_fqn,
            main_table_name,
            cm,
        );
        push_synth_binding(
            &mut bindings,
            "out",
            Some(field),
            db_fqn,
            main_table_name,
            cm,
        );
    }
    if has_business {
        push_synth_binding(
            &mut bindings,
            "from",
            by_key.get("BUS_FROM").copied(),
            db_fqn,
            main_table_name,
            cm,
        );
        push_synth_binding(
            &mut bindings,
            "thru",
            by_key.get("BUS_THRU").copied(),
            db_fqn,
            main_table_name,
            cm,
        );
    } else if has_business_snapshot && let Some(field) = by_key.get("BUS_SNAPSHOT_DATE").copied() {
        push_synth_binding(
            &mut bindings,
            "from",
            Some(field),
            db_fqn,
            main_table_name,
            cm,
        );
        push_synth_binding(
            &mut bindings,
            "thru",
            Some(field),
            db_fqn,
            main_table_name,
            cm,
        );
    }

    if bindings.is_empty() {
        return None;
    }

    Some(SynthesizedMilestoningMapping {
        id: SmolStr::new(format!("{}_milestoning", cm.class_mapping_id.as_str())),
        source_set_implementation_id: cm.class_mapping_id.clone(),
        property_bindings: bindings,
    })
}

fn push_synth_binding(
    out: &mut Vec<(SmolStr, OpColumnBinding)>,
    property_name: &str,
    field: Option<&ResolvedMilestoningField>,
    db_fqn: &SmolStr,
    table_name: &SmolStr,
    cm: &ResolvedClassMapping,
) {
    let Some(field) = field else {
        return;
    };
    let source_info = cm
        .properties
        .first()
        .and_then(|p| match &p.kind {
            ResolvedClassMappingPropertyKind::Single { binding } => {
                binding.as_ref().map(|b| b.source_info.clone())
            }
            ResolvedClassMappingPropertyKind::Plus { binding } => {
                binding.as_ref().map(|b| b.source_info.clone())
            }
            ResolvedClassMappingPropertyKind::Embedded => None,
        })
        .unwrap_or_else(|| SourceInfo::new("", 0, 0, 0, 0));
    out.push((
        SmolStr::new(property_name),
        OpColumnBinding {
            database_fqn: db_fqn.clone(),
            table_name: table_name.clone(),
            column_index: field.column_index,
            unresolved_database: false,
            unresolved_table: false,
            unresolved_column: field.column_index.is_none(),
            source_info,
        },
    ));
}
