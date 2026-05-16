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
use legend_pure_parser_pure::extension::{CompileExtensionScope, CompilerExtension};
use legend_pure_parser_pure::model::PureModel;
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

fn run_lifecycle(source: &str) -> PureModel {
    let file = parse_source(source);
    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    // Take the model's scope, thread it through declare + define_bodies,
    // restore it so `RelationalExtension::resolved_databases(&model)`
    // can read the populated state.
    let mut scope = std::mem::take(&mut bootstrap.compile_scope);
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: Some(&mut scope),
    };
    extension.declare(&mut declare_ctx);

    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: Some(&mut scope),
    };
    extension.define_bodies(&mut define_ctx);

    bootstrap.compile_scope = scope;
    bootstrap
}

/// Lifecycle helper that registers BOTH the relational section parser
/// and a Mapping section parser with the relational class-mapping body
/// hook. Used by Phase B4 fixtures that need a `Class : Relational
/// { ... }` body to be registered + resolved.
fn run_lifecycle_with_mapping(source: &str) -> PureModel {
    use legend_pure_dsl_mapping::parser::MappingSectionParser;
    use legend_pure_dsl_relational::parser::RelationalClassMappingBodyParser;

    let file = legend_pure_parser_parser::parse_with_sections(
        source,
        "processor_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![
            Box::new(RelationalSectionParser),
            Box::new(MappingSectionParser::with_body_parsers(vec![Box::new(
                RelationalClassMappingBodyParser,
            )])),
        ],
    )
    .expect("source must parse");
    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let mut scope = std::mem::take(&mut bootstrap.compile_scope);
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: Some(&mut scope),
    };
    extension.declare(&mut declare_ctx);

    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: Some(&mut scope),
    };
    extension.define_bodies(&mut define_ctx);

    bootstrap.compile_scope = scope;
    bootstrap
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, mystery WIDGET)
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.join_names, vec![SmolStr::new("j1")]);
    assert_eq!(db.filter_names, vec![SmolStr::new("f1")]);
    assert_eq!(db.multi_grain_filter_names, vec![SmolStr::new("mgf1")]);
}

#[test]
fn collects_include_fqns_in_source_order() {
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, qty FLOAT(10, 2), name VARCHAR(50))
          Filter f1 (t.id = 1 and t.qty > 0)
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table src (id INT PRIMARY KEY, fk INT)
          Table dst (id INT PRIMARY KEY, name VARCHAR(50))
          Join sToD (src.fk = dst.id)
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
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
    let resolved = RelationalExtension::resolved_databases(&model);
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
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Filter f (missing.id = t.id)
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.filter_bodies[0];
    let missing = &body.bindings[0];
    assert!(missing.unresolved_table);
    assert!(missing.column_index.is_none());
}

#[test]
fn flags_unresolved_database_in_op_body() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
          Filter f ([pkg::nope]t.id = t.id)
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.filter_bodies[0];
    let missing = &body.bindings[0];
    assert!(missing.unresolved_database);
}

#[test]
fn collects_multi_grain_filter_bodies() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, region VARCHAR(2))
          MultiGrainFilter byRegion (t.region = 'US')
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.multi_grain_filter_bodies.len(), 1);
    let body = &db.multi_grain_filter_bodies[0];
    assert_eq!(body.element_name.as_str(), "byRegion");
    assert_eq!(body.bindings.len(), 1);
    let col = body.bindings[0].resolved_column(&resolved).unwrap();
    assert_eq!(col.name.as_str(), "region");
}

// ---------------------------------------------------------------------------
// Phase B3 — view body resolution + main-table inference
// ---------------------------------------------------------------------------

#[test]
fn resolves_view_with_single_main_table() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema s
          (
            Table t (id INT PRIMARY KEY, qty FLOAT(10, 2))
            View v (id : t.id, qty : t.qty)
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    assert_eq!(db.view_bodies.len(), 1);
    let body = &db.view_bodies[0];
    assert_eq!(body.view_name.as_str(), "v");
    assert_eq!(body.columns.len(), 2);
    assert_eq!(body.referenced_tables, vec![SmolStr::new("t")]);
    assert_eq!(body.main_table.as_deref(), Some("t"));

    // Each column line resolves through to a ResolvedColumn.
    let id_line = &body.columns[0];
    assert_eq!(id_line.column_name.as_str(), "id");
    let id_binding = id_line.value_binding.as_ref().expect("id binding missing");
    let id_col = id_binding.resolved_column(&resolved).unwrap();
    assert_eq!(id_col.name.as_str(), "id");
    assert_eq!(id_col.pure_type, Some(PureColumnType::Integer));
}

#[test]
fn detects_view_with_multiple_main_tables() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema s
          (
            Table a (id INT PRIMARY KEY)
            Table b (id INT PRIMARY KEY)
            View v (one : a.id, two : b.id)
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.view_bodies[0];
    assert_eq!(body.referenced_tables.len(), 2);
    assert!(
        body.main_table.is_none(),
        "expected None for multi-table view"
    );
}

#[test]
fn resolves_view_with_target_set_id() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema s
          (
            Table t (id INT PRIMARY KEY)
            View v (id[OrderSet] : t.id)
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.view_bodies[0];
    let line = &body.columns[0];
    assert_eq!(line.target_set_id.as_deref(), Some("OrderSet"));
    let col = line
        .value_binding
        .as_ref()
        .unwrap()
        .resolved_column(&resolved)
        .unwrap();
    assert_eq!(col.name.as_str(), "id");
}

#[test]
fn resolves_view_referencing_table_via_explicit_db() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::other
        (
          Table foreign (id INT PRIMARY KEY)
        )

        ###Relational
        Database pkg::main
        (
          Schema s
          (
            Table local (id INT PRIMARY KEY)
            View v (orderId : [pkg::other]foreign.id)
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let main = resolved.get("pkg::main").expect("missing");
    let body = &main.view_bodies[0];
    let line = &body.columns[0];
    let binding = line
        .value_binding
        .as_ref()
        .expect("view col binding missing");
    assert_eq!(binding.database_fqn.as_str(), "pkg::other");
    assert_eq!(binding.table_name.as_str(), "foreign");
    let col = binding.resolved_column(&resolved).unwrap();
    assert_eq!(col.name.as_str(), "id");
}

#[test]
fn view_with_constant_value_has_no_binding() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Schema s
          (
            Table t (id INT PRIMARY KEY)
            View v (id : t.id, label : 'static')
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let body = &db.view_bodies[0];
    assert_eq!(body.columns.len(), 2);
    assert!(body.columns[0].value_binding.is_some());
    // Literal-valued columns have no column binding.
    assert!(body.columns[1].value_binding.is_none());
    assert_eq!(body.columns[1].column_name.as_str(), "label");
}

// ---------------------------------------------------------------------------
// Phase B4 — class-mapping property resolution
// ---------------------------------------------------------------------------

#[test]
fn resolves_class_mapping_with_main_table_and_property_values() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeTable (id INT PRIMARY KEY, qty FLOAT(10, 2))
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; qty : Float[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeTable
            (id : tradeTable.id, qty : tradeTable.qty)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    assert_eq!(resolved.len(), 1);
    let cm = &resolved[0];
    assert_eq!(cm.mapping_fqn.as_str(), "pkg::TradeMap");
    assert_eq!(cm.primary_database.as_deref(), Some("pkg::db"));
    assert_eq!(cm.main_table.as_deref(), Some("tradeTable"));
    assert_eq!(cm.inferred_main_table.as_deref(), Some("tradeTable"));
    assert_eq!(cm.referenced_tables, vec![SmolStr::new("tradeTable")]);
    assert_eq!(cm.properties.len(), 2);

    // Both properties resolve to columns on tradeTable.
    let dbs = RelationalExtension::resolved_databases(&model);
    let id_prop = &cm.properties[0];
    assert_eq!(id_prop.property_name.as_str(), "id");
    let id_binding = match &id_prop.kind {
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Single {
            binding,
        } => binding.as_ref(),
        _ => panic!("expected Single kind"),
    };
    let id_col = id_binding
        .expect("id binding missing")
        .resolved_column(&dbs)
        .expect("resolved id column");
    assert_eq!(id_col.name.as_str(), "id");
}

#[test]
fn flags_inconsistent_main_tables_in_class_mapping() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table a (id INT PRIMARY KEY)
          Table b (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::T { x : Integer[1]; y : Integer[1]; }

        ###Mapping
        Mapping pkg::M
        (
          pkg::T : Relational
          {
            ~mainTable [pkg::db]a
            (x : a.id, y : b.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let cm = &resolved[0];
    assert_eq!(cm.referenced_tables.len(), 2);
    // ~mainTable says 'a', but property values reference both 'a' and
    // 'b' — inferred is None, declared main_table stays as 'a'.
    assert_eq!(cm.main_table.as_deref(), Some("a"));
    assert!(cm.inferred_main_table.is_none());
}

#[test]
fn resolves_scope_wrapped_lines_with_scope_db() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::other
        (
          Table foreign (id INT PRIMARY KEY, name VARCHAR(50))
        )

        ###Relational
        Database pkg::main
        (
          Table local (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::T { x : Integer[1]; y : Integer[1]; }

        ###Mapping
        Mapping pkg::M
        (
          pkg::T : Relational
          {
            ~mainTable [pkg::main]local
            (
              x : local.id,
              scope([pkg::other])
              (
                y : foreign.id
              )
            )
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let cm = &resolved[0];
    // Two properties resolved: x against pkg::main, y against pkg::other.
    assert_eq!(cm.properties.len(), 2);
    let dbs = RelationalExtension::resolved_databases(&model);

    let x_binding = match &cm.properties[0].kind {
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Single {
            binding,
        } => binding.as_ref().expect("x binding"),
        _ => panic!("expected Single"),
    };
    assert_eq!(x_binding.database_fqn.as_str(), "pkg::main");

    let y_binding = match &cm.properties[1].kind {
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Single {
            binding,
        } => binding.as_ref().expect("y binding"),
        _ => panic!("expected Single"),
    };
    assert_eq!(y_binding.database_fqn.as_str(), "pkg::other");
    let y_col = y_binding.resolved_column(&dbs).expect("y col");
    assert_eq!(y_col.name.as_str(), "id");
}

#[test]
fn embedded_property_marked_as_embedded_kind() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY, addr_id INT)
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]t
            (
              id : t.id,
              address (
                ~primaryKey(t.addr_id)
                line : t.addr_id
              )
            )
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let cm = &resolved[0];
    assert_eq!(cm.properties.len(), 2);
    assert!(matches!(
        cm.properties[1].kind,
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Embedded
    ));
    assert_eq!(cm.properties[1].property_name.as_str(), "address");
}

#[test]
fn class_mapping_without_main_table_marks_db_unresolved_on_lines() {
    // Java parity: a class mapping with no `~mainTable` and no per-line
    // `[db]` qualifier can't pin the database. Each binding records
    // `unresolved_database`.
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            (id : t.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let cm = &resolved[0];
    assert!(cm.primary_database.is_none());
    let binding = match &cm.properties[0].kind {
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Single {
            binding,
        } => binding.as_ref().expect("binding"),
        _ => panic!("expected Single"),
    };
    assert!(binding.unresolved_database);
}

// ---------------------------------------------------------------------------
// Phase B5 — main-table inheritance through `extends`
// ---------------------------------------------------------------------------

#[test]
fn child_inherits_main_table_from_parent_via_extends() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table parentT (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Parent { id : Integer[1]; }
        Class pkg::Child extends pkg::Parent {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::Parent[parentMap] : Relational
          {
            ~mainTable [pkg::db]parentT
            (id : parentT.id)
          }

          pkg::Child[childMap] extends [parentMap] : Relational
          {
            (id : parentT.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    assert_eq!(resolved.len(), 2);
    let parent = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "parentMap")
        .expect("parentMap missing");
    let child = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "childMap")
        .expect("childMap missing");

    assert_eq!(parent.main_table.as_deref(), Some("parentT"));
    assert_eq!(parent.effective_main_table.as_deref(), Some("parentT"));

    // Child has no `~mainTable` of its own — inherits from parent.
    assert!(child.main_table.is_none());
    assert_eq!(child.effective_main_table.as_deref(), Some("parentT"));
    assert_eq!(child.effective_primary_database.as_deref(), Some("pkg::db"));
    assert_eq!(child.extends.as_deref(), Some("parentMap"));
}

#[test]
fn child_with_own_main_table_does_not_inherit() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table parentT (id INT PRIMARY KEY)
          Table childT (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Parent { id : Integer[1]; }
        Class pkg::Child extends pkg::Parent {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::Parent[parentMap] : Relational
          {
            ~mainTable [pkg::db]parentT
            (id : parentT.id)
          }

          pkg::Child[childMap] extends [parentMap] : Relational
          {
            ~mainTable [pkg::db]childT
            (id : childT.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let child = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "childMap")
        .expect("childMap missing");
    // Child declares its own — effective stays as childT.
    assert_eq!(child.main_table.as_deref(), Some("childT"));
    assert_eq!(child.effective_main_table.as_deref(), Some("childT"));
}

#[test]
fn extends_inheritance_walks_three_level_chain() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table rootT (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::A { id : Integer[1]; }
        Class pkg::B extends pkg::A {}
        Class pkg::C extends pkg::B {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::A[aMap] : Relational
          {
            ~mainTable [pkg::db]rootT
            (id : rootT.id)
          }

          pkg::B[bMap] extends [aMap] : Relational
          {
            (id : rootT.id)
          }

          pkg::C[cMap] extends [bMap] : Relational
          {
            (id : rootT.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let c = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "cMap")
        .expect("cMap missing");
    // C → B → A → rootT.
    assert!(c.main_table.is_none());
    assert_eq!(c.effective_main_table.as_deref(), Some("rootT"));
}

#[test]
fn extends_chain_with_no_ancestor_main_table_stays_none() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Parent { id : Integer[1]; }
        Class pkg::Child extends pkg::Parent {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::Parent[parentMap] : Relational
          {
            (id : t.id)
          }

          pkg::Child[childMap] extends [parentMap] : Relational
          {
            (id : t.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let child = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "childMap")
        .expect("childMap missing");
    // Neither parent nor child declares ~mainTable — effective stays None.
    assert!(child.effective_main_table.is_none());
    assert!(child.effective_primary_database.is_none());
}

#[test]
fn extends_inheritance_terminates_on_cycle() {
    // Hypothetical cycle: A extends B, B extends A (Java errors via
    // G1, but the inheritance walker should still terminate).
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::A { id : Integer[1]; }
        Class pkg::B {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::A[aMap] extends [bMap] : Relational
          {
            (id : t.id)
          }

          pkg::B[bMap] extends [aMap] : Relational
          {
            (id : t.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    // Walker terminates without panicking and leaves both with None
    // effective main table.
    let a = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "aMap")
        .expect("aMap missing");
    let b = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "bMap")
        .expect("bMap missing");
    assert!(a.effective_main_table.is_none());
    assert!(b.effective_main_table.is_none());
}

// ---------------------------------------------------------------------------
// Phase B6 — AssociationMapping body resolution
// ---------------------------------------------------------------------------

#[test]
fn resolves_association_mapping_with_two_ends() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (id INT PRIMARY KEY, prodId INT)
          Table prodT (id INT PRIMARY KEY)
          Join tradeProd (tradeT.prodId = prodT.id)
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }
        Class pkg::Product { id : Integer[1]; }
        Association pkg::TradeProd
        {
          trade : pkg::Trade[1];
          product : pkg::Product[1];
        }

        ###Mapping
        Mapping pkg::M
        (
          pkg::Trade[tradeMap] : Relational
          {
            ~mainTable [pkg::db]tradeT
            (id : tradeT.id)
          }

          pkg::Product[prodMap] : Relational
          {
            ~mainTable [pkg::db]prodT
            (id : prodT.id)
          }

          pkg::TradeProd : Relational
          {
            AssociationMapping
            (
              trade[tradeMap, prodMap] : [pkg::db]@tradeProd | tradeT.id,
              product[prodMap, tradeMap] : [pkg::db]@tradeProd | prodT.id
            )
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let assoc = resolved
        .iter()
        .find(|cm| {
            matches!(
                cm.kind,
                legend_pure_dsl_relational::processor::ResolvedClassMappingKind::Association
            )
        })
        .expect("association mapping missing");
    assert_eq!(assoc.properties.len(), 2);
    assert_eq!(assoc.properties[0].property_name.as_str(), "trade");
    assert_eq!(assoc.properties[1].property_name.as_str(), "product");
    // Both ends carry [srcId, tgtId].
    assert_eq!(assoc.properties[0].source_id.as_deref(), Some("tradeMap"));
    assert_eq!(assoc.properties[0].target_id.as_deref(), Some("prodMap"));
    assert_eq!(assoc.properties[1].source_id.as_deref(), Some("prodMap"));
    assert_eq!(assoc.properties[1].target_id.as_deref(), Some("tradeMap"));

    // Each end's value is bound to a column.
    let dbs = RelationalExtension::resolved_databases(&model);
    let trade_binding = match &assoc.properties[0].kind {
        legend_pure_dsl_relational::processor::ResolvedClassMappingPropertyKind::Single {
            binding,
        } => binding.as_ref().expect("binding"),
        _ => panic!("expected Single"),
    };
    let trade_col = trade_binding.resolved_column(&dbs).expect("col");
    assert_eq!(trade_col.name.as_str(), "id");
    assert_eq!(trade_binding.table_name.as_str(), "tradeT");
}

#[test]
fn association_mapping_kind_distinguishes_from_class_mapping() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table t (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::A { id : Integer[1]; }
        Class pkg::B { id : Integer[1]; }
        Association pkg::AB
        {
          a : pkg::A[1];
          b : pkg::B[1];
        }

        ###Mapping
        Mapping pkg::M
        (
          pkg::A[aMap] : Relational
          {
            ~mainTable [pkg::db]t
            (id : t.id)
          }

          pkg::B[bMap] : Relational
          {
            ~mainTable [pkg::db]t
            (id : t.id)
          }

          pkg::AB : Relational
          {
            AssociationMapping
            (
              a[aMap, bMap] : [pkg::db]t.id,
              b[bMap, aMap] : [pkg::db]t.id
            )
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let class_mappings = resolved
        .iter()
        .filter(|cm| {
            matches!(
                cm.kind,
                legend_pure_dsl_relational::processor::ResolvedClassMappingKind::Class
            )
        })
        .count();
    let association_mappings = resolved
        .iter()
        .filter(|cm| {
            matches!(
                cm.kind,
                legend_pure_dsl_relational::processor::ResolvedClassMappingKind::Association
            )
        })
        .count();
    assert_eq!(class_mappings, 2);
    assert_eq!(association_mappings, 1);
}

// ---------------------------------------------------------------------------
// Phase C — milestoning auto-rewrite
// ---------------------------------------------------------------------------

#[test]
fn synthesizes_from_thru_for_business_milestoned_main_table() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
            id INT PRIMARY KEY,
            fromZ DATE,
            thruZ DATE
          )
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]tradeT
            (id : tradeT.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let cm = &resolved[0];
    let synth = cm
        .synthesized_milestoning
        .as_ref()
        .expect("expected synthesized milestoning");
    assert_eq!(
        synth.id.as_str(),
        format!("{}_milestoning", cm.class_mapping_id.as_str())
    );
    assert_eq!(
        synth.source_set_implementation_id.as_str(),
        cm.class_mapping_id.as_str()
    );
    let names: Vec<&str> = synth
        .property_bindings
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, vec!["from", "thru"]);

    // Each binding points at the milestoning column on the main table.
    let dbs = RelationalExtension::resolved_databases(&model);
    let from_col = synth.property_bindings[0]
        .1
        .resolved_column(&dbs)
        .expect("from column");
    let thru_col = synth.property_bindings[1]
        .1
        .resolved_column(&dbs)
        .expect("thru column");
    assert_eq!(from_col.name.as_str(), "fromZ");
    assert_eq!(thru_col.name.as_str(), "thruZ");
}

#[test]
fn synthesizes_in_out_for_processing_milestoned_main_table() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table eventT (
            milestoning ( processing (PROCESSING_IN=inZ, PROCESSING_OUT=outZ) )
            id INT PRIMARY KEY,
            inZ DATE,
            outZ DATE
          )
        )

        ###Pure
        Class pkg::Event { id : Integer[1]; }

        ###Mapping
        Mapping pkg::EventMap
        (
          pkg::Event : Relational
          {
            ~mainTable [pkg::db]eventT
            (id : eventT.id)
          }
        )
    "});
    let synth = RelationalExtension::resolved_class_mappings(&model)[0]
        .synthesized_milestoning
        .clone()
        .expect("expected synthesized milestoning");
    let names: Vec<&str> = synth
        .property_bindings
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, vec!["in", "out"]);
}

#[test]
fn synthesizes_all_four_for_bitemporal_main_table() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table biT (
            milestoning (
              business (BUS_FROM=fromZ, BUS_THRU=thruZ),
              processing (PROCESSING_IN=inZ, PROCESSING_OUT=outZ)
            )
            id INT PRIMARY KEY,
            fromZ DATE,
            thruZ DATE,
            inZ DATE,
            outZ DATE
          )
        )

        ###Pure
        Class pkg::Bi { id : Integer[1]; }

        ###Mapping
        Mapping pkg::BiMap
        (
          pkg::Bi : Relational
          {
            ~mainTable [pkg::db]biT
            (id : biT.id)
          }
        )
    "});
    let synth = RelationalExtension::resolved_class_mappings(&model)[0]
        .synthesized_milestoning
        .clone()
        .expect("expected synthesized milestoning");
    // Canonical order: in, out, from, thru.
    let names: Vec<&str> = synth
        .property_bindings
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, vec!["in", "out", "from", "thru"]);
}

#[test]
fn synthesis_skipped_for_non_milestoned_table() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table plain (id INT PRIMARY KEY)
        )

        ###Pure
        Class pkg::Trade { id : Integer[1]; }

        ###Mapping
        Mapping pkg::TradeMap
        (
          pkg::Trade : Relational
          {
            ~mainTable [pkg::db]plain
            (id : plain.id)
          }
        )
    "});
    let cm = &RelationalExtension::resolved_class_mappings(&model)[0];
    assert!(cm.synthesized_milestoning.is_none());
}

#[test]
fn synthesis_skipped_when_parent_already_milestoned() {
    // Java's `shouldCreateMilestoningPropertyMapping` skips children
    // when the parent's super_set_impl_id is non-null and the parent
    // is milestoned. The child inherits the synthesised mapping via
    // the parent.
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table tradeT (
            milestoning ( business (BUS_FROM=fromZ, BUS_THRU=thruZ) )
            id INT PRIMARY KEY,
            fromZ DATE,
            thruZ DATE
          )
        )

        ###Pure
        Class pkg::Parent { id : Integer[1]; }
        Class pkg::Child extends pkg::Parent {}

        ###Mapping
        Mapping pkg::M
        (
          *pkg::Parent[parentMap] : Relational
          {
            ~mainTable [pkg::db]tradeT
            (id : tradeT.id)
          }

          pkg::Child[childMap] extends [parentMap] : Relational
          {
            (id : tradeT.id)
          }
        )
    "});
    let resolved = RelationalExtension::resolved_class_mappings(&model);
    let parent = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "parentMap")
        .expect("parentMap missing");
    let child = resolved
        .iter()
        .find(|cm| cm.class_mapping_id.as_str() == "childMap")
        .expect("childMap missing");
    // Parent gets the synthesised mapping.
    assert!(parent.synthesized_milestoning.is_some());
    // Child skips synthesis since the parent already carries it.
    assert!(child.synthesized_milestoning.is_none());
}

#[test]
fn synthesizes_for_business_snapshot_milestoning() {
    let model = run_lifecycle_with_mapping(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table snapT (
            milestoning ( business (BUS_SNAPSHOT_DATE=snapZ) )
            id INT PRIMARY KEY,
            snapZ DATE
          )
        )

        ###Pure
        Class pkg::Snap { id : Integer[1]; }

        ###Mapping
        Mapping pkg::SnapMap
        (
          pkg::Snap : Relational
          {
            ~mainTable [pkg::db]snapT
            (id : snapT.id)
          }
        )
    "});
    let synth = RelationalExtension::resolved_class_mappings(&model)[0]
        .synthesized_milestoning
        .clone()
        .expect("expected synthesized milestoning");
    let names: Vec<&str> = synth
        .property_bindings
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    // Snapshot maps both 'from' and 'thru' to the same column.
    assert_eq!(names, vec!["from", "thru"]);
    let dbs = RelationalExtension::resolved_databases(&model);
    let from_col = synth.property_bindings[0]
        .1
        .resolved_column(&dbs)
        .expect("from column");
    let thru_col = synth.property_bindings[1]
        .1
        .resolved_column(&dbs)
        .expect("thru column");
    assert_eq!(from_col.name.as_str(), "snapZ");
    assert_eq!(thru_col.name.as_str(), "snapZ");
}

#[test]
fn resolved_table_carries_milestoning_definitions() {
    let model = run_lifecycle(indoc! {r"
        ###Relational
        Database pkg::db
        (
          Table biT (
            milestoning (
              business (BUS_FROM=fromZ, BUS_THRU=thruZ, INFINITY_DATE=%9999-12-31),
              processing (PROCESSING_IN=inZ, PROCESSING_OUT=outZ)
            )
            id INT PRIMARY KEY,
            fromZ DATE,
            thruZ DATE,
            inZ DATE,
            outZ DATE
          )
        )
    "});
    let resolved = RelationalExtension::resolved_databases(&model);
    let db = resolved.get("pkg::db").expect("missing");
    let table = db.tables_by_name.get("biT").expect("biT missing");
    let milestoning = table.milestoning.as_ref().expect("expected milestoning");
    assert_eq!(milestoning.definitions.len(), 2);
    // The date-literal `INFINITY_DATE` field is filtered out — only
    // identifier-valued fields produce ResolvedMilestoningField rows.
    let business_def = &milestoning.definitions[0];
    let business_keys: Vec<&str> = business_def.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(business_keys, vec!["BUS_FROM", "BUS_THRU"]);
    // Each business field has a resolved column index.
    for f in &business_def.fields {
        assert!(f.column_index.is_some(), "expected column resolved");
    }
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
    let mut scope = std::mem::take(&mut bootstrap.compile_scope);
    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
        scope: Some(&mut scope),
    };
    extension.declare(&mut declare_ctx);
    bootstrap.compile_scope = scope;

    assert!(RelationalExtension::resolved_databases(&bootstrap).is_empty());
}
