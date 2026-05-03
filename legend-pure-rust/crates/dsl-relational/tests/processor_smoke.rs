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

//! Phase B1: post-processor scaffolding smoke tests.
//!
//! Drives the relational extension's full lifecycle (`declare` →
//! `define_bodies`) and pins the shape of the per-database resolved
//! snapshot produced by `process_database`. Each test runs against a
//! single fixture and inspects the resolved state through the public
//! accessor.

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_dsl_relational::processor::PureColumnType;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use smol_str::SmolStr;

fn parse_source(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "processor_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .expect("source must parse")
}

fn run_lifecycle(source: &str) -> RelationalExtension {
    let file = parse_source(source);
    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.declare(&mut declare_ctx);

    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.define_bodies(&mut define_ctx);

    extension
}

// ---------------------------------------------------------------------------
// PureColumnType — SQL type mapping
// ---------------------------------------------------------------------------

#[test]
fn pure_column_type_maps_known_sql_names() {
    assert_eq!(
        PureColumnType::from_sql_name("INT"),
        Some(PureColumnType::Integer)
    );
    assert_eq!(
        PureColumnType::from_sql_name("INTEGER"),
        Some(PureColumnType::Integer)
    );
    assert_eq!(
        PureColumnType::from_sql_name("varchar"),
        Some(PureColumnType::Varchar)
    );
    assert_eq!(
        PureColumnType::from_sql_name("DECIMAL"),
        Some(PureColumnType::Decimal)
    );
    assert_eq!(
        PureColumnType::from_sql_name("Timestamp"),
        Some(PureColumnType::Timestamp)
    );
    assert_eq!(
        PureColumnType::from_sql_name("DATETIME"),
        Some(PureColumnType::Timestamp)
    );
}

#[test]
fn pure_column_type_returns_none_for_unknown_sql_name() {
    assert_eq!(PureColumnType::from_sql_name("WIDGET"), None);
    assert_eq!(PureColumnType::from_sql_name(""), None);
}

#[test]
fn pure_column_type_pure_name_round_trip() {
    for sql in [
        "FLOAT",
        "DOUBLE",
        "INT",
        "BIGINT",
        "SMALLINT",
        "TINYINT",
        "CHAR",
        "VARCHAR",
        "BINARY",
        "VARBINARY",
        "TIMESTAMP",
        "DATE",
        "DECIMAL",
        "NUMERIC",
        "BIT",
        "REAL",
        "ARRAY",
        "JSON",
        "SEMISTRUCTURED",
    ] {
        let resolved = PureColumnType::from_sql_name(sql).unwrap_or_else(|| {
            panic!("expected {sql} to resolve");
        });
        // Pure type name should be a non-empty PascalCase string.
        assert!(
            !resolved.pure_name().is_empty(),
            "pure_name empty for {sql}"
        );
    }
}

// ---------------------------------------------------------------------------
// process_database — column resolution
// ---------------------------------------------------------------------------

#[test]
fn resolves_top_level_table_columns_with_pure_types() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeTable (
            id INT PRIMARY KEY,
            name VARCHAR(200) NOT NULL,
            price DECIMAL(10, 2),
            ts TIMESTAMP
          )
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved
        .get("pkg::db")
        .expect("pkg::db missing from resolved");
    assert_eq!(db.fqn.as_str(), "pkg::db");
    assert_eq!(db.tables_by_name.len(), 1);

    let t = db
        .tables_by_name
        .get("tradeTable")
        .expect("tradeTable missing");
    assert!(t.schema.is_none(), "expected top-level table");
    assert_eq!(t.columns.len(), 4);

    let id = t.column("id").expect("id column missing");
    assert_eq!(id.source_type.as_str(), "INT");
    assert_eq!(id.pure_type, Some(PureColumnType::Integer));
    assert!(id.primary_key);
    assert!(!id.not_null);
    assert!(id.size.is_none());

    let name = t.column("name").expect("name column missing");
    assert_eq!(name.source_type.as_str(), "VARCHAR");
    assert_eq!(name.pure_type, Some(PureColumnType::Varchar));
    assert_eq!(name.size, Some(200));
    assert!(name.not_null);
    assert!(!name.primary_key);

    let price = t.column("price").expect("price column missing");
    assert_eq!(price.pure_type, Some(PureColumnType::Decimal));
    assert_eq!(price.size, Some(10));
    assert_eq!(price.scale, Some(2));

    let ts = t.column("ts").expect("ts column missing");
    assert_eq!(ts.pure_type, Some(PureColumnType::Timestamp));
}

#[test]
fn resolves_schema_nested_tables() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema sales
          (
            Table tradeTable (id INT PRIMARY KEY)
            View activeTrades (id : tradeTable.id)
          )
          Schema reference
          (
            Table productTable (id INT PRIMARY KEY)
          )
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.tables_by_name.len(), 2);
    let t = db
        .tables_by_name
        .get("tradeTable")
        .expect("tradeTable missing");
    assert_eq!(t.schema.as_deref(), Some("sales"));
    let p = db
        .tables_by_name
        .get("productTable")
        .expect("productTable missing");
    assert_eq!(p.schema.as_deref(), Some("reference"));
    assert_eq!(db.view_names, vec![SmolStr::new("activeTrades")]);
}

#[test]
fn unknown_sql_type_resolves_to_none_pure_type() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, mystery WIDGET)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    let t = db.tables_by_name.get("t").expect("t missing");
    let mystery = t.column("mystery").expect("mystery missing");
    assert_eq!(mystery.source_type.as_str(), "WIDGET");
    assert_eq!(mystery.pure_type, None);
}

// ---------------------------------------------------------------------------
// process_database — joins / filters / mgf / includes
// ---------------------------------------------------------------------------

#[test]
fn collects_join_filter_multigrain_names() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, fk INT)
          Table u (id INT PRIMARY KEY)
          Join j1 (t.fk = u.id)
          Filter f1 (t.id > 0)
          MultiGrainFilter mgf1 (t.id > 5)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.join_names, vec![SmolStr::new("j1")]);
    assert_eq!(db.filter_names, vec![SmolStr::new("f1")]);
    assert_eq!(db.multi_grain_filter_names, vec![SmolStr::new("mgf1")]);
}

#[test]
fn collects_include_fqns_in_source_order() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::base
        (
          Table b (id INT PRIMARY KEY)
        )

        ###Relational
        Database pkg::other::shared
        (
          Table s (id INT PRIMARY KEY)
        )

        ###Relational
        Database pkg::derived
        (
          include pkg::base
          include pkg::other::shared
          Table local (id INT PRIMARY KEY)
        )
    "});
    let resolved = extension.resolved_databases();
    let derived = resolved.get("pkg::derived").expect("missing");
    assert_eq!(
        derived.include_fqns,
        vec![
            SmolStr::new("pkg::base"),
            SmolStr::new("pkg::other::shared")
        ]
    );
}

// ---------------------------------------------------------------------------
// Multi-database: each registered DB gets its own snapshot
// ---------------------------------------------------------------------------

#[test]
fn multi_database_each_resolved_independently() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::a
        (
          Table ta (id INT PRIMARY KEY)
        )

        ###Relational
        Database pkg::b
        (
          Table tb (id BIGINT PRIMARY KEY)
        )
    "});
    let resolved = extension.resolved_databases();
    assert_eq!(resolved.len(), 2);
    let a = resolved.get("pkg::a").expect("missing pkg::a");
    let b = resolved.get("pkg::b").expect("missing pkg::b");
    assert!(a.tables_by_name.contains_key("ta"));
    assert!(b.tables_by_name.contains_key("tb"));
    assert!(!a.tables_by_name.contains_key("tb"));

    let tb_id = b
        .tables_by_name
        .get("tb")
        .expect("tb missing")
        .column("id")
        .expect("id missing");
    assert_eq!(tb_id.pure_type, Some(PureColumnType::BigInt));
}

// ---------------------------------------------------------------------------
// Phase B2 — op-body column reference resolution
// ---------------------------------------------------------------------------

#[test]
fn resolves_filter_op_columns_to_local_table_columns() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, qty FLOAT(10, 2), name VARCHAR(50))
          Filter f1 (t.id = 1 and t.qty > 0)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.filter_bodies.len(), 1);
    let body = &db.filter_bodies[0];
    assert_eq!(body.element_name.as_str(), "f1");
    assert_eq!(body.bindings.len(), 2);

    // Both bindings point at table 't' with locally-scoped column refs.
    for b in &body.bindings {
        assert_eq!(b.database_fqn.as_str(), "pkg::db");
        assert_eq!(b.table_name.as_str(), "t");
        assert!(!b.unresolved_database);
        assert!(!b.unresolved_table);
        assert!(!b.unresolved_column);
    }

    // Specific column indices: 'id' is column[0], 'qty' is column[1].
    let col0 = body.bindings[0].resolved_column(&resolved).unwrap();
    let col1 = body.bindings[1].resolved_column(&resolved).unwrap();
    assert_eq!(col0.name.as_str(), "id");
    assert_eq!(col0.pure_type, Some(PureColumnType::Integer));
    assert_eq!(col1.name.as_str(), "qty");
    assert_eq!(col1.pure_type, Some(PureColumnType::Float));
}

#[test]
fn resolves_join_op_columns_across_two_tables() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table src (id INT PRIMARY KEY, fk INT)
          Table dst (id INT PRIMARY KEY, name VARCHAR(50))
          Join sToD (src.fk = dst.id)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.join_bodies.len(), 1);
    let body = &db.join_bodies[0];
    assert_eq!(body.element_name.as_str(), "sToD");
    assert_eq!(body.bindings.len(), 2);

    let lhs = &body.bindings[0];
    let rhs = &body.bindings[1];
    assert_eq!(lhs.table_name.as_str(), "src");
    assert_eq!(rhs.table_name.as_str(), "dst");

    let lhs_col = lhs.resolved_column(&resolved).unwrap();
    let rhs_col = rhs.resolved_column(&resolved).unwrap();
    assert_eq!(lhs_col.name.as_str(), "fk");
    assert_eq!(rhs_col.name.as_str(), "id");
}

#[test]
fn resolves_op_columns_across_includes() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::base
        (
          Table sharedT (id INT PRIMARY KEY, fk INT)
        )

        ###Relational
        Database pkg::main
        (
          include pkg::base
          Table localT (id INT PRIMARY KEY)
          Filter f (sharedT.id = localT.id)
        )
    "});
    let resolved = extension.resolved_databases();
    let main = resolved.get("pkg::main").expect("missing");
    assert_eq!(main.filter_bodies.len(), 1);
    let body = &main.filter_bodies[0];
    assert_eq!(body.bindings.len(), 2);

    // First binding (sharedT.id) — table lives in pkg::base via include.
    let shared = &body.bindings[0];
    assert_eq!(shared.table_name.as_str(), "sharedT");
    assert!(!shared.unresolved_table); // visible via include
    // Column index resolves through the include closure.
    let col = shared.resolved_column(&resolved).unwrap();
    assert_eq!(col.name.as_str(), "id");
}

#[test]
fn resolves_explicit_db_qualifier_to_other_database() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::other
        (
          Table foreign (id INT PRIMARY KEY, name VARCHAR(50))
        )

        ###Relational
        Database pkg::main
        (
          Table local (id INT PRIMARY KEY)
          Filter f ([pkg::other]foreign.id = local.id)
        )
    "});
    let resolved = extension.resolved_databases();
    let main = resolved.get("pkg::main").expect("missing");
    let body = &main.filter_bodies[0];

    // First binding has explicit `[pkg::other]` qualifier.
    let foreign = &body.bindings[0];
    assert_eq!(foreign.database_fqn.as_str(), "pkg::other");
    assert_eq!(foreign.table_name.as_str(), "foreign");
    assert!(!foreign.unresolved_database);
    let col = foreign.resolved_column(&resolved).unwrap();
    assert_eq!(col.name.as_str(), "id");
    assert_eq!(col.pure_type, Some(PureColumnType::Integer));

    // Second binding falls back to the owning DB.
    let local = &body.bindings[1];
    assert_eq!(local.database_fqn.as_str(), "pkg::main");
    assert_eq!(local.table_name.as_str(), "local");
}

#[test]
fn flags_unresolved_table_in_op_body() {
    // V4 raises a hard error; the resolver still produces a binding
    // with `unresolved_table = true` for downstream consumers.
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Filter f (missing.id = t.id)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.filter_bodies[0];
    let missing = &body.bindings[0];
    assert!(missing.unresolved_table);
    assert!(missing.column_index.is_none());
}

#[test]
fn flags_unresolved_database_in_op_body() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Filter f ([pkg::nope]t.id = t.id)
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.filter_bodies[0];
    let missing = &body.bindings[0];
    assert!(missing.unresolved_database);
}

#[test]
fn collects_multi_grain_filter_bodies() {
    let extension = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, region VARCHAR(2))
          MultiGrainFilter byRegion (t.region = 'US')
        )
    "});
    let resolved = extension.resolved_databases();
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.multi_grain_filter_bodies.len(), 1);
    let body = &db.multi_grain_filter_bodies[0];
    assert_eq!(body.element_name.as_str(), "byRegion");
    assert_eq!(body.bindings.len(), 1);
    let col = body.bindings[0].resolved_column(&resolved).unwrap();
    assert_eq!(col.name.as_str(), "region");
}

#[test]
fn resolved_databases_empty_before_define_bodies() {
    // Run only the declare pass — `resolved_databases` must be empty
    // until `define_bodies` populates it.
    let file = parse_source(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
        )
    "});
    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.declare(&mut declare_ctx);

    assert!(extension.resolved_databases().is_empty());
}
