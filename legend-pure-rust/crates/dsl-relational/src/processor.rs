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

//! Phase B1: post-processor scaffolding for the relational DSL.
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
//! - **Tables** — every Table on the database (top-level + schema-nested)
//!   with its column list. SQL type names are mapped to the
//!   [`PureColumnType`] enum (Java parity with `ColumnDataTypeFactory`),
//!   keeping the raw SQL spelling alongside for diagnostics.
//! - **Views / Joins / Filters / MultiGrainFilters** — by name, for
//!   contextual lookups. Body resolution (column-ref → `ResolvedColumn`,
//!   join-tree-node construction) lands in B2.
//!
//! What this module deliberately defers (Phase-B sub-items):
//!
//! - Op-body column references → `ResolvedColumn` ElementId (B2)
//! - View body resolution + view-cycle detection (B3)
//! - Class-mapping property mapping resolution (B4)
//! - Main-table inheritance through `extends` (B5)
//! - AssociationMapping source/target class population (B6)
//! - Implicit-db `@join` resolution (B7)
//! - Milestoning auto-rewrite (Phase C)

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::ast::{ColumnDef, DatabaseDef, DatabaseElement};

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
    }
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
