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
//! Phase B' (this revision) — predicate `Boolean[1]` return-type
//! validation:
//!
//!   6. Every Filter / Join / MultiGrainFilter `op_operation` body
//!      must reduce to `Boolean` (or `Any` for unmodeled
//!      DynaFunctions). Inferred by
//!      [`crate::op_typer::infer_op_type`] using the owning
//!      database's resolved-table snapshot for column-type lookups.
//!      Java parity: `DatabaseProcessor` / `RelationalOperationElementProcessor`
//!      validate the predicate's `_genericType` after expression
//!      inference; we take the narrow path (classify-by-shape +
//!      column-type lookup) since full DynaFunction → Pure-function
//!      lowering is RT-1 / INT-1 territory.
//!
//! See [`crates/dsl-mapping/src/compiler.rs`] for the trait-shape
//! template these validators follow.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement as _;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx, ValidateCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{
    DSLInstance, Element as ModelElement, ElementNode, PureModel,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Stable name used to key Database payloads in `Element::DSLInstance`.
pub const DATABASE_DSL_NAME: &str = "RelationalDatabase";

/// Stable name used to key Relational class-mapping payloads in
/// `Element::DSLInstance`. One row per `pkg::Class : Relational
/// { ~mainTable [db]Schema.Table ... }` line. Materialised as a
/// SIDECAR — the row is invisible to top-level navigation; its job
/// is to carry the data the runtime needs to patch the
/// `RootRelationalInstanceSetImplementation` heap row that the
/// Mapping populator already creates.
pub const RELATIONAL_CLASS_MAPPING_DSL_NAME: &str = "RelationalClassMapping";

/// FQN of the M3 metaclass the sidecar elements claim as classifier.
/// Mirrors the heap-row classifier the Mapping populator assigns to
/// `body_kind="Relational"` class-mappings.
pub const RELATIONAL_CLASS_MAPPING_CLASSIFIER_FQN: &str =
    "meta::relational::mappings::RootRelationalInstanceSetImplementation";

/// FQN of the M3 metaclass `Database` instances are typed against.
pub const DATABASE_CLASSIFIER_FQN: &str = "meta::relational::metamodel::Database";

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
    /// Resolved relational class mappings built during Pass 2b
    /// (Phase B4). One entry per registered `Class : Relational { ... }`
    /// body, in registration order. See
    /// [`crate::processor::ResolvedClassMapping`] and
    /// [`Self::resolved_class_mappings`].
    resolved_class_mappings: RefCell<Vec<crate::processor::ResolvedClassMapping>>,
    /// Per-mapping include FQNs captured during `declare()`. Keyed
    /// by mapping FQN, value is the list of FQNs in that mapping's
    /// `includes`. Used by Phase E4 (cross-mapping inline + extends
    /// id resolution) to walk the include closure when looking up
    /// class-mapping ids.
    mapping_includes: RefCell<HashMap<SmolStr, Vec<SmolStr>>>,
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
    /// FQN of the class this mapping implements — taken verbatim from
    /// the AST `ClassMapping.class` pointer. Used by per-property
    /// validators that look up the class on `ctx.model` to inspect
    /// declared properties' types.
    class_fqn: SmolStr,
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

    /// Snapshot of the per-database resolved state built during Pass 2b.
    /// Keyed by database FQN. Empty until [`CompilerExtension::define_bodies`]
    /// runs — call this only post-compile.
    #[must_use]
    pub fn resolved_databases(&self) -> HashMap<SmolStr, crate::processor::ResolvedDatabase> {
        self.resolved_databases.borrow().clone()
    }

    /// Snapshot of the resolved relational class mappings built during
    /// Pass 2b (Phase B4). One entry per registered class mapping,
    /// in registration order. Empty until
    /// [`CompilerExtension::define_bodies`] runs.
    #[must_use]
    pub fn resolved_class_mappings(&self) -> Vec<crate::processor::ResolvedClassMapping> {
        self.resolved_class_mappings.borrow().clone()
    }

    /// Snapshot of all registered databases **as graph elements** —
    /// walks `model.elements()` for `Element::DSLInstance` entries
    /// keyed `"RelationalDatabase"` and decodes each payload.
    ///
    /// Survives `.purem` slice/merge: every database registered during
    /// the original compile reappears here after a fresh model is
    /// built from a serialised slice. Decoding errors are dropped
    /// silently.
    ///
    /// Relational class mappings (`Class : Relational { ... }` bodies
    /// inside `###Mapping` sections) are covered by the Mapping
    /// extension's own snapshot — `body_kind == "Relational"` —
    /// because they live in `MappingDef.class_mappings`, not in
    /// `DatabaseDef`. This reader handles only the Database side.
    #[must_use]
    pub fn databases_from_model(model: &PureModel) -> Vec<(SmolStr, DatabaseSnapshot)> {
        let mut out = Vec::new();
        for chunk in &model.chunks {
            for (_, element) in chunk.elements.iter() {
                let ModelElement::DSLInstance(d) = element else {
                    continue;
                };
                if d.dsl_name.as_str() != DATABASE_DSL_NAME {
                    continue;
                }
                if let Ok(snapshot) = DatabaseSnapshot::decode(&d.data) {
                    out.push((snapshot.fqn.clone(), snapshot));
                }
            }
        }
        out
    }
}

/// Compiled, serializable form of a Database — what survives a `.purem`
/// round-trip via `Element::DSLInstance.data`.
///
/// Captures only **identifying** structural data: FQN, include FQNs,
/// per-element kind/name summary (schema → tables/views; top-level
/// table/view names; join/filter/multi-grain-filter names). Lossless
/// `DatabaseDef` round-trip is follow-up work; the snapshot is enough
/// to demonstrate the architectural round-trip.
///
/// Decoupled from `legend-pure-parser-ast`'s non-serde types so the
/// architectural rule "ast crate has no serde" stays intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatabaseSnapshot {
    /// FQN of the database, e.g. `"model::test::MyDb"`.
    pub fqn: SmolStr,
    /// FQNs of databases included via `include`.
    pub includes: Vec<SmolStr>,
    /// Schema names with their table/view counts.
    pub schemas: Vec<SchemaSnapshot>,
    /// Top-level (default-schema) table names, in source order.
    pub default_tables: Vec<SmolStr>,
    /// Top-level (default-schema) view names, in source order.
    pub default_views: Vec<SmolStr>,
    /// Join names, in source order.
    pub joins: Vec<SmolStr>,
    /// Filter names, in source order.
    pub filters: Vec<SmolStr>,
    /// MultiGrainFilter names, in source order.
    pub multi_grain_filters: Vec<SmolStr>,
}

/// Per-schema summary inside a [`DatabaseSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    /// Schema name.
    pub name: SmolStr,
    /// Table names in this schema, in source order.
    pub tables: Vec<SmolStr>,
    /// View names in this schema, in source order.
    pub views: Vec<SmolStr>,
}

/// Sidecar payload carrying the data needed to wire a relational
/// class-mapping's `mainTableAlias` slot at runtime hydration time.
///
/// Encoded into `Element::DSLInstance.data` under
/// [`RELATIONAL_CLASS_MAPPING_DSL_NAME`]. Read at evaluator setup by
/// `RelationalClassMappingDSLPopulator` in `dsl-relational-runtime`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationalClassMappingSnapshot {
    /// FQN of the enclosing `Mapping`, e.g. `"my::test::MyMapping"`.
    /// Used by the populator to locate the parent Mapping heap row.
    pub mapping_fqn: SmolStr,
    /// Class-mapping id — the `[id]` if explicitly set, otherwise the
    /// class FQN (same convention as `visible_class_mapping_ids`).
    pub class_mapping_id: SmolStr,
    /// `~mainTable [db]Schema.Table` reference. `None` when the body
    /// has no `~mainTable` block.
    pub main_table: Option<MainTableRef>,
}

/// `[db]Schema.Table` triple extracted from a `~mainTable` block.
///
/// Schema defaults to `"default"` for the `~mainTable [db]Table`
/// shape (top-level table outside any `Schema` block). When the
/// source has `~mainTable [db]Schema.Table`, the schema is captured
/// literally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MainTableRef {
    /// Database FQN extracted from the `[db]` qualifier.
    pub database_fqn: SmolStr,
    /// Schema name (`"default"` for the unqualified form).
    pub schema_name: SmolStr,
    /// Table name.
    pub table_name: SmolStr,
}

impl RelationalClassMappingSnapshot {
    /// Encode for storage in `Element::DSLInstance.data`.
    ///
    /// # Errors
    /// Returns the underlying Postcard error on encode failure.
    pub fn encode(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Decode a payload produced by [`Self::encode`].
    ///
    /// # Errors
    /// Returns the underlying Postcard error on corrupt or truncated
    /// input.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

fn extract_main_table_ref(body: &RelationalClassMappingBody) -> Option<MainTableRef> {
    let mt = body.main_table.as_ref()?;
    let database_fqn = packageable_fqn(&mt.db);
    // `~mainTable [db]Schema.Table` parses to `scope.table = "Schema"`
    // and `scope.scope = ["Table"]`. `~mainTable [db]Table` parses to
    // `scope.table = "Table"` and `scope.scope = []`.
    let (schema_name, table_name) = if mt.scope.scope.is_empty() {
        (SmolStr::new("default"), mt.scope.table.value.clone())
    } else {
        (
            mt.scope.table.value.clone(),
            mt.scope.scope[0].value.clone(),
        )
    };
    Some(MainTableRef {
        database_fqn,
        schema_name,
        table_name,
    })
}

impl DatabaseSnapshot {
    /// Build a snapshot from an in-memory `DatabaseDef` AST.
    #[must_use]
    pub fn from_def(def: &DatabaseDef, fqn: SmolStr) -> Self {
        let includes = def
            .includes
            .iter()
            .map(|inc| ptr_fqn(&inc.included))
            .collect();
        let mut schemas = Vec::new();
        let mut default_tables = Vec::new();
        let mut default_views = Vec::new();
        let mut joins = Vec::new();
        let mut filters = Vec::new();
        let mut multi_grain_filters = Vec::new();
        for elem in &def.elements {
            match elem {
                DatabaseElement::Schema(s) => schemas.push(SchemaSnapshot {
                    name: s.name.value.clone(),
                    tables: s.tables.iter().map(|t| t.name.value.clone()).collect(),
                    views: s.views.iter().map(|v| v.name.value.clone()).collect(),
                }),
                DatabaseElement::Table(t) => default_tables.push(t.name.value.clone()),
                DatabaseElement::View(v) => default_views.push(v.name.value.clone()),
                DatabaseElement::Join(j) => joins.push(j.name.value.clone()),
                DatabaseElement::Filter(f) => filters.push(f.name.value.clone()),
                DatabaseElement::MultiGrainFilter(m) => {
                    multi_grain_filters.push(m.name.value.clone());
                }
            }
        }
        Self {
            fqn,
            includes,
            schemas,
            default_tables,
            default_views,
            joins,
            filters,
            multi_grain_filters,
        }
    }

    /// Encode for storage in `Element::DSLInstance.data`.
    ///
    /// # Errors
    /// Postcard never fails on well-typed inputs in practice, but the
    /// error is propagated rather than panicking.
    pub fn encode(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Decode a payload produced by [`Self::encode`].
    ///
    /// # Errors
    /// Returns the underlying Postcard error on corrupt or truncated
    /// input.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

/// FQN string for a `PackageableElementPtr` (database includes use
/// the same shape Mapping uses internally).
fn ptr_fqn(p: &legend_pure_parser_ast::annotation::PackageableElementPtr) -> SmolStr {
    if let Some(pkg) = &p.package {
        SmolStr::new(format!("{pkg}::{}", p.name))
    } else {
        p.name.clone()
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
        let mut mapping_includes = self.mapping_includes.borrow_mut();
        mapping_includes.clear();
        for source in ctx.source_files {
            for section in &source.sections {
                match section.kind.as_str() {
                    "Relational" => {
                        // Pass 1 created the slice's chunk; allocate
                        // `Element::DSLInstance` rows there alongside
                        // the M3 elements parsed from the same file.
                        let chunk_id = (ctx.model.chunks.len().saturating_sub(1)) as u16;
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
                                    kind: CompilationErrorKind::DuplicateElement {
                                        name: fqn.clone(),
                                    },
                                });
                                continue;
                            }

                            // Dual-write 1/2 — extension RefCell.
                            by_fqn.insert(fqn.clone(), RegisteredDatabase { def: (*db).clone() });

                            // Dual-write 2/2 — model graph as
                            // `Element::DSLInstance`. Same pattern as
                            // the Diagram + Mapping pilots.
                            let snapshot = DatabaseSnapshot::from_def(db, fqn.clone());
                            let data = match snapshot.encode() {
                                Ok(bytes) => bytes,
                                Err(e) => {
                                    ctx.errors.push(CompilationError {
                                        message: format!(
                                            "Failed to encode DatabaseSnapshot for '{fqn}': {e}"
                                        ),
                                        source_info: db.source_info.clone(),
                                        kind: CompilationErrorKind::DuplicateElement {
                                            name: fqn.clone(),
                                        },
                                    });
                                    continue;
                                }
                            };

                            let pkg_path: Vec<SmolStr> = db
                                .package()
                                .map(|p| p.segments().into_iter().cloned().collect())
                                .unwrap_or_default();
                            let package_id = if pkg_path.is_empty() {
                                ctx.model.root_package
                            } else {
                                ctx.model.get_or_create_package(&pkg_path)
                            };
                            let Some(chunk) = ctx.model.chunks.get_mut(chunk_id as usize) else {
                                continue;
                            };
                            let local_idx = chunk.alloc_element(
                                ElementNode {
                                    name: db.name.value.clone(),
                                    source_info: db.source_info.clone(),
                                    name_source_info: db.name.source_info.clone(),
                                    parent_package: package_id,
                                },
                                ModelElement::DSLInstance(DSLInstance {
                                    dsl_name: SmolStr::new(DATABASE_DSL_NAME),
                                    classifier_fqn: SmolStr::new(DATABASE_CLASSIFIER_FQN),
                                    data,
                                }),
                            );
                            let id = ElementId::InstanceId {
                                chunk_id,
                                local_idx,
                            };
                            ctx.model.register_element(package_id, id);
                        }
                    }
                    "Mapping" => {
                        // Sidecar DSLInstance rows for relational
                        // class-mappings land in the same chunk Pass 1
                        // just created for this source file. Same
                        // pattern as the Database arm above.
                        let chunk_id = (ctx.model.chunks.len().saturating_sub(1)) as u16;
                        for element in &section.elements {
                            let AstElement::DSLElement(boxed) = element else {
                                continue;
                            };
                            let Some(mapping) = boxed.as_any().downcast_ref::<MappingDef>() else {
                                continue;
                            };
                            let mapping_fqn = mapping_fqn(mapping);
                            // Capture this mapping's include FQNs so the
                            // E4 cross-mapping resolver can walk the
                            // include closure when looking up class-
                            // mapping ids. Done once per Mapping section
                            // entry; later entries for the same FQN
                            // overwrite (the dsl-mapping side surfaces
                            // duplicate-Mapping diagnostics already).
                            mapping_includes.insert(
                                mapping_fqn.clone(),
                                mapping
                                    .includes
                                    .iter()
                                    .map(|inc| packageable_fqn(&inc.included))
                                    .collect(),
                            );
                            for cm in &mapping.class_mappings {
                                let ClassMappingBody::Foreign(boxed) = &cm.body else {
                                    continue;
                                };
                                let Some(body) =
                                    boxed.as_any().downcast_ref::<RelationalClassMappingBody>()
                                else {
                                    continue;
                                };
                                let class_fqn = packageable_fqn(&cm.class);
                                let class_mapping_id =
                                    cm.id.clone().unwrap_or_else(|| class_fqn.clone());
                                relational_class_mappings.push(RegisteredRelationalClassMapping {
                                    mapping_fqn: mapping_fqn.clone(),
                                    class_mapping_id: class_mapping_id.clone(),
                                    class_fqn,
                                    extends: cm.extends.clone(),
                                    class_mapping_source_info: cm.source_info.clone(),
                                    body: body.clone(),
                                });

                                // Sidecar `Element::DSLInstance` row
                                // carrying the data the
                                // `RelationalClassMappingDSLPopulator`
                                // reads at evaluator setup to patch
                                // `RootRelationalInstanceSetImplementation.mainTableAlias`
                                // onto the class-mapping heap row the
                                // Mapping populator allocates. Sidecar
                                // means: invisible to package
                                // navigation, but routes through the
                                // standard chunk-element machinery so
                                // `.purem` slice/merge preserves it.
                                let snapshot = RelationalClassMappingSnapshot {
                                    mapping_fqn: mapping_fqn.clone(),
                                    class_mapping_id: class_mapping_id.clone(),
                                    main_table: extract_main_table_ref(body),
                                };
                                let data = match snapshot.encode() {
                                    Ok(bytes) => bytes,
                                    Err(e) => {
                                        ctx.errors.push(CompilationError {
                                            message: format!(
                                                "Failed to encode RelationalClassMappingSnapshot for \
                                                 '{mapping_fqn}.{class_mapping_id}': {e}"
                                            ),
                                            source_info: cm.source_info.clone(),
                                            kind: CompilationErrorKind::UnresolvedElement {
                                                path: class_mapping_id.clone(),
                                            },
                                        });
                                        continue;
                                    }
                                };
                                let Some(chunk) = ctx.model.chunks.get_mut(chunk_id as usize)
                                else {
                                    continue;
                                };
                                let synthetic_name =
                                    SmolStr::new(format!("{mapping_fqn}.{class_mapping_id}"));
                                let _local_idx = chunk.alloc_element(
                                    ElementNode {
                                        name: synthetic_name,
                                        source_info: cm.source_info.clone(),
                                        name_source_info: cm.source_info.clone(),
                                        parent_package: ctx.model.root_package,
                                    },
                                    ModelElement::DSLInstance(DSLInstance {
                                        dsl_name: SmolStr::new(RELATIONAL_CLASS_MAPPING_DSL_NAME),
                                        classifier_fqn: SmolStr::new(
                                            RELATIONAL_CLASS_MAPPING_CLASSIFIER_FQN,
                                        ),
                                        data,
                                    }),
                                );
                                // Note: NOT calling `register_element`.
                                // Class-mappings aren't packageable; the
                                // sidecar lives in the chunk arena
                                // only, discoverable via element
                                // iteration but invisible to FQN
                                // navigation.
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
        // Phase B3: resolve view body column refs + infer main tables.
        crate::processor::resolve_view_bodies(&mut resolved, &defs_by_fqn);
        // Phase B4: resolve class-mapping property values.
        let class_mappings = self.relational_class_mappings.borrow();
        let mut resolved_cms = self.resolved_class_mappings.borrow_mut();
        resolved_cms.clear();
        for reg in class_mappings.iter() {
            resolved_cms.push(crate::processor::resolve_class_mapping(
                &reg.body,
                &reg.mapping_fqn,
                &reg.class_mapping_id,
                reg.extends.as_ref(),
                &resolved,
                &defs_by_fqn,
            ));
        }
        // Phase B5: inherit main-table / primary-database through the
        // `extends` chain.
        crate::processor::apply_extends_inheritance(&mut resolved_cms);
        // Phase C: synthesise milestoning embedded mappings for class
        // mappings whose effective main table declares a
        // `milestoning(...)` spec.
        crate::processor::apply_milestoning_synthesis(&mut resolved_cms, &resolved);
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
        let mapping_includes = self.mapping_includes.borrow();
        validate_relational_class_mappings(&class_mappings, &dbs, &mapping_includes, ctx.errors);
        // Phase D: repo-boundary visibility for `include` and `[db]`
        // qualifiers. No-op when `model.repo_visibility` is empty (so
        // existing tests that build a model without descriptors stay
        // green).
        validate_repo_visibility(&dbs, ctx.model, ctx.errors);
        // Phase A3' (post-B2): JoinTreeNode parity — chained
        // `@a > @b > ...` join sequences must share end-tables. Reads
        // the resolved snapshots from `define_bodies` so it can pull
        // each Join's distinct table set without re-walking the AST.
        let resolved = self.resolved_databases.borrow();
        let resolved_cms = self.resolved_class_mappings.borrow();
        validate_join_tree_chains(&class_mappings, &resolved, &resolved_cms, ctx.errors);
        // Phase A4: RelationalAssociationImplementationValidator
        // parity — every property line on an AssociationMapping body
        // must (a) reference a join sequence, and (b) form a chain
        // from the source class mapping's main table to the target's.
        validate_association_mapping_joins(&class_mappings, &resolved_cms, &resolved, ctx.errors);
        // Phase A5: RelationalInstanceSetImplementationValidator
        // parity — per-property checks against the target class's
        // declared property types: data-type properties forbid
        // `[targetId]`; enum properties require an EnumerationMapping
        // transformer; class-typed properties require a join sequence
        // (Java errors with "Mapping Error! The target type:'X' is
        // not a data type but the relationalOperation is not a join").
        validate_class_mapping_property_types(&class_mappings, ctx.model, ctx.errors);
        // Phase A6: Inline-target subtype check — when an embedded
        // mapping carries an `Inline[setId]` trailer, the inline
        // target's class must be a subtype of the property's
        // declared target class. Java parity:
        // "The inlineSetImplementationId '...' is implementing the
        // class 'X' which is not a subType of 'Y'".
        validate_inline_target_subtypes(&class_mappings, &mapping_includes, ctx.model, ctx.errors);
        // Phase A7: AssociationMapping target identity. An
        // AssociationMapping body's class FQN must resolve to an
        // `Association` element on the model (not a Class), and
        // each association can be mapped at most once per Mapping
        // (Java parity: TestAssociationMappingValidation).
        validate_association_mapping_targets(&class_mappings, ctx.model, ctx.errors);
        // Phase B': Filter / Join / MultiGrainFilter predicate
        // bodies must return Boolean[1]. Uses the resolved
        // snapshots already populated by `define_bodies` for
        // column-type lookups; un-modeled DynaFunctions and
        // unresolved column refs type as `Any` (silent, no false
        // positives).
        validate_predicate_return_types(&dbs, &resolved, ctx.errors);
    }

    /// Surface Relational-DSL reference sites to the IDE's reference
    /// index.
    ///
    /// Covered today:
    /// - **Database include target** — `include other::Db` →
    ///   jumps to the included Database element.
    ///
    /// Schema/table/column refs and the join-tree FQNs follow once
    /// the relational AST tracks their per-segment spans. Today they
    /// flow as `db.schema.table.col` strings without per-segment
    /// `SourceInfo`, so we can't yet surface a clickable region per
    /// segment.
    fn walk_references(
        &self,
        model: &legend_pure_parser_pure::model::PureModel,
        visit: &mut dyn FnMut(legend_pure_parser_pure::refs::Reference),
    ) {
        use legend_pure_parser_ast::element::PackageableElement;
        let dbs = self.databases.borrow();
        for reg in dbs.values() {
            for include in &reg.def.includes {
                let target_id = if let Some(pkg) = include.included.package() {
                    model.resolve_in_package(pkg, include.included.name())
                } else {
                    model.resolve_by_path(std::slice::from_ref(include.included.name()))
                };
                let Some(target_id) = target_id else { continue };
                if matches!(
                    target_id,
                    legend_pure_parser_pure::ids::ElementId::Package(_)
                ) {
                    continue;
                }
                let target = model.get_node(target_id).name_source_info.clone();
                visit(legend_pure_parser_pure::refs::Reference {
                    range: include.included.source_info.clone(),
                    kind: legend_pure_parser_pure::refs::RefKind::TypeRef,
                    target_element: Some(target_id),
                    target,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase B' — predicate Boolean[1] validation
// ---------------------------------------------------------------------------

/// Walk every Filter / Join / MultiGrainFilter on every registered
/// database; classify the body's top-level result via
/// [`crate::op_typer::infer_op_type`]; emit a diagnostic when the
/// body is neither `Boolean` nor `Any`.
///
/// Java parity: the Filter / Join predicate inference in
/// `DatabaseProcessor` / `RelationalOperationElementProcessor` —
/// they error on a non-`Boolean[1]` body. We take the narrow path
/// (allowing `Any` for un-modeled DynaFunctions and unresolved
/// column refs) so this validator can't false-positive while full
/// DynaFunction lowering stays deferred.
fn validate_predicate_return_types(
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    use crate::op_typer::OpTypeScope;

    for (db_fqn, reg) in dbs {
        let scope = resolved
            .get(db_fqn)
            .map_or_else(OpTypeScope::empty, |snap| {
                OpTypeScope::from_tables(&snap.tables_by_name)
            });
        for elem in &reg.def.elements {
            match elem {
                DatabaseElement::Filter(f) => check_predicate(
                    &f.body,
                    &f.name.value,
                    "Filter",
                    db_fqn.as_str(),
                    scope,
                    &f.source_info,
                    errors,
                ),
                DatabaseElement::Join(j) => check_predicate(
                    &j.body,
                    &j.name.value,
                    "Join",
                    db_fqn.as_str(),
                    scope,
                    &j.source_info,
                    errors,
                ),
                DatabaseElement::MultiGrainFilter(m) => check_predicate(
                    &m.body,
                    &m.name.value,
                    "MultiGrainFilter",
                    db_fqn.as_str(),
                    scope,
                    &m.source_info,
                    errors,
                ),
                _ => {}
            }
        }
    }
}

fn check_predicate(
    body: &OpExpr,
    name: &SmolStr,
    kind: &str,
    db_fqn: &str,
    scope: crate::op_typer::OpTypeScope<'_>,
    source_info: &SourceInfo,
    errors: &mut Vec<CompilationError>,
) {
    use crate::op_typer::{OpType, infer_op_type};

    let ty = infer_op_type(body, scope);
    if matches!(ty, OpType::Boolean | OpType::Any) {
        return;
    }
    let actual = match ty {
        OpType::Boolean | OpType::Any => unreachable!(),
        OpType::Numeric => "Numeric",
        OpType::String => "String",
        OpType::Date => "Date",
    };
    errors.push(CompilationError {
        message: format!(
            "{kind} predicate '{name}' in database '{db_fqn}' must return Boolean[1], \
             got {actual}"
        ),
        source_info: source_info.clone(),
        // `TypeMismatch` is the documented variant for "expression is
        // supported but its inferred type contradicts the slot's
        // declared shape" — exactly this predicate-return-type rule.
        // Context label distinguishes Filter / Join / MultiGrainFilter
        // for downstream consumers that bucket on the kind.
        kind: CompilationErrorKind::TypeMismatch {
            context: SmolStr::new(format!("{kind} predicate")),
            target: SmolStr::new(format!("{db_fqn}::{name}")),
            expected: SmolStr::new_static("Boolean[1]"),
            actual: SmolStr::new(actual),
        },
    });
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
            // Advance the parent's index BEFORE recursing. The `while let
            // Some((current, idx)) = stack.last().cloned()` above guarantees
            // stack is non-empty here; bail with unreachable! to keep clippy
            // happy without restructuring the loop.
            let Some(last) = stack.last_mut() else {
                unreachable!("stack non-empty by loop guard");
            };
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
/// E3. Inline trailer must reference a class-mapping id reachable
///     from the enclosing `Mapping` — same Mapping or any
///     transitively-included one. Java's processor walks the
///     include graph via `getClassMappingsByIdIncludeEmbedded`; we
///     mirror with `mapping_includes` (captured at declare time)
///     plus the same-Mapping `ids_by_mapping` index.
fn validate_relational_class_mappings(
    class_mappings: &[RegisteredRelationalClassMapping],
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
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

        // A8: extends rules on class mappings (Java parity:
        // `TestExtendGrammar.testExtendInvalidSetCannotBeSelf` +
        // `testExtendInvalidIdWithInclude`).
        if let Some(parent_id) = &reg.extends {
            // A8a: a class mapping cannot extend itself.
            if parent_id == &reg.class_mapping_id {
                errors.push(CompilationError {
                    message: format!(
                        "Class mapping '{}' cannot extend itself",
                        reg.class_mapping_id
                    ),
                    source_info: reg.class_mapping_source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: reg.class_mapping_id.clone(),
                        reason: SmolStr::new("class mapping extends itself"),
                    },
                });
            } else {
                // A8b: extends id must resolve to a class mapping
                // visible from this Mapping — itself or any
                // transitively-included Mapping.
                let visible =
                    visible_class_mapping_ids(&reg.mapping_fqn, &ids_by_mapping, mapping_includes);
                let known = visible.contains(parent_id);
                if !known {
                    errors.push(CompilationError {
                        message: format!(
                            "Class mapping '{}' extends unknown id '{}' (no class mapping with \
                             that id in '{}')",
                            reg.class_mapping_id, parent_id, reg.mapping_fqn,
                        ),
                        source_info: reg.class_mapping_source_info.clone(),
                        kind: CompilationErrorKind::UnresolvedElement {
                            path: parent_id.clone(),
                        },
                    });
                }
            }
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
                // E3: Inline trailer must reference a mapping id
                // visible from the enclosing Mapping — itself or any
                // transitively-included Mapping.
                if let Some(EmbeddedMappingTrailer::Inline(inline)) = &em.trailer {
                    let known_ids = visible_class_mapping_ids(
                        &reg.mapping_fqn,
                        &ids_by_mapping,
                        mapping_includes,
                    );
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
                    // A5b (RelationalInstanceSetImplementationValidator
                    // parity): an Otherwise embedded body must declare
                    // at least one inner property mapping line — Java
                    // errors with "Invalid Otherwise mapping found:
                    // '<prop>' property has no embedded mappings
                    // defined, please use a property mapping with
                    // Join instead."
                    if em.mapping_lines.is_empty() {
                        errors.push(CompilationError {
                            message: format!(
                                "Invalid Otherwise mapping for property '{}' in class mapping '{}': \
                                 the embedded body has no property mappings; use a property mapping \
                                 with a Join instead",
                                np.property.value, reg.class_mapping_id,
                            ),
                            source_info: em.source_info.clone(),
                            kind: CompilationErrorKind::InvalidAssociation {
                                name: SmolStr::new(format!(
                                    "Otherwise '{}' in '{}'",
                                    np.property.value, reg.class_mapping_id
                                )),
                                reason: SmolStr::new(
                                    "embedded body cannot be empty when Otherwise is present",
                                ),
                            },
                        });
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
    if let Some(filter_block) = &reg.body.filter
        && let Some(seq) = &filter_block.join_sequence
    {
        check_filter_mapping_join_sequence(
            seq,
            &filter_block.db,
            dbs,
            &reg.class_mapping_id,
            errors,
        );
    }

    // Phase B7: contextual db for implicit-db join refs flows from
    // `~mainTable [db]` when present. Scope-wrapped lines override
    // with the scope's `[db]`.
    let contextual_db: Option<&legend_pure_parser_ast::annotation::PackageableElementPtr> =
        reg.body.main_table.as_ref().map(|mt| &mt.db);

    // 2. Walk every JoinColWithDbOrConstant + OtherwiseJoin in the
    //    body, including implicit-db cases that fall back to the
    //    contextual db.
    let body = &reg.body;
    for elem in &body.mapping_elements {
        walk_mapping_element_for_joins(elem, contextual_db, dbs, &reg.class_mapping_id, errors);
    }
    if let Some(lines) = &body.association_mapping {
        for line in lines {
            walk_single_mapping_line_for_joins(
                line,
                contextual_db,
                dbs,
                &reg.class_mapping_id,
                errors,
            );
        }
    }

    // 3. ~groupBy / ~primaryKey at the body level — use the
    //    mapping's contextual db.
    if let Some(jcs) = &body.group_by {
        for jc in jcs {
            walk_join_col_for_joins(jc, contextual_db, dbs, &reg.class_mapping_id, errors);
        }
    }
    if let Some(jcs) = &body.primary_key {
        for jc in jcs {
            walk_join_col_for_joins(jc, contextual_db, dbs, &reg.class_mapping_id, errors);
        }
    }
}

fn walk_mapping_element_for_joins(
    e: &crate::ast::MappingElement,
    contextual_db: Option<&legend_pure_parser_ast::annotation::PackageableElementPtr>,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match e {
        crate::ast::MappingElement::Single(line) => {
            walk_single_mapping_line_for_joins(line, contextual_db, dbs, owner, errors);
        }
        crate::ast::MappingElement::Scope(s) => {
            // Scope overrides the contextual db for its lines.
            let scope_db = Some(&s.db);
            for line in &s.mapping_lines {
                walk_single_mapping_line_for_joins(line, scope_db, dbs, owner, errors);
            }
        }
    }
}

fn walk_single_mapping_line_for_joins(
    line: &SingleMappingLine,
    contextual_db: Option<&legend_pure_parser_ast::annotation::PackageableElementPtr>,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match line {
        SingleMappingLine::Plus(p) => {
            walk_join_col_for_joins(&p.mapping.value, contextual_db, dbs, owner, errors);
        }
        SingleMappingLine::NonePlus(np) => match &np.value {
            NonePlusMappingValue::Relational(rm) => {
                walk_join_col_for_joins(&rm.value, contextual_db, dbs, owner, errors);
            }
            NonePlusMappingValue::Embedded(em) => {
                if let Some(jcs) = &em.primary_key {
                    for jc in jcs {
                        walk_join_col_for_joins(jc, contextual_db, dbs, owner, errors);
                    }
                }
                for inner in &em.mapping_lines {
                    walk_single_mapping_line_for_joins(inner, contextual_db, dbs, owner, errors);
                }
                if let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &em.trailer {
                    for m in maps {
                        match &m.otherwise_join.db {
                            Some(db) => check_join_sequence_against_db(
                                &m.otherwise_join.join_sequence,
                                db,
                                dbs,
                                owner,
                                errors,
                            ),
                            None => {
                                if let Some(ctx_db) = contextual_db {
                                    check_join_sequence_against_db(
                                        &m.otherwise_join.join_sequence,
                                        ctx_db,
                                        dbs,
                                        owner,
                                        errors,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        },
    }
}

fn walk_join_col_for_joins(
    jc: &JoinColWithDbOrConstant,
    contextual_db: Option<&legend_pure_parser_ast::annotation::PackageableElementPtr>,
    dbs: &HashMap<SmolStr, RegisteredDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let Some(seq) = &jc.join else {
        return;
    };
    // Explicit `[db]` on the joinColWithDbOrConstant takes precedence;
    // otherwise fall back to the contextual db when one is in scope.
    let resolved_db = jc.db.as_ref().or(contextual_db);
    if let Some(db) = resolved_db {
        check_join_sequence_against_db(seq, db, dbs, owner, errors);
    } else {
        // No explicit AND no contextual db — validate per-segment
        // explicit `[db]`s only. Without a contextual db, the
        // implicit-db case stays unresolved (a future post-processor
        // pass could pick this up).
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
    if let Some(reg) = dbs.get(&db_fqn) {
        Ok(collect_visible_joins(&reg.def, dbs))
    } else {
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
    if let Some(filter) = &v.filter
        && let Some(chain) = &filter.db_chain
    {
        check_join_sequence_against_db(&chain.join_sequence, &chain.first_db, dbs, &owner, errors);
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
    if let Some(filter) = &v.filter
        && let Some(chain) = &filter.db_chain
    {
        check_db_ref_visibility(&chain.first_db, use_site, visible, databases, errors);
        check_db_ref_visibility(&chain.second_db, use_site, visible, databases, errors);
        walk_join_sequence_for_db_refs(&chain.join_sequence, use_site, visible, databases, errors);
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
    if let Some(OpColumn::Aliased { db: Some(db), .. }) = &jc.column {
        check_db_ref_visibility(db, use_site, visible, databases, errors);
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

// ---------------------------------------------------------------------------
// Phase A3' — JoinTreeNodeValidation parity
// ---------------------------------------------------------------------------

/// Validate that every chained `@a > @b > ...` join sequence in a
/// class-mapping body shares end-tables. Mirrors Java's
/// [`JoinTreeNodeValidation.validateJoinTreeNode`][1]:
///
/// - Each `Join`'s op-body references some table set (typically two
///   tables); we extract that set from B2's resolved op-bodies.
/// - Walk the chain starting from the contextual source table
///   (`~mainTable` / `scope` table). For each `@joinName`, the join
///   must contain the current source table in its set; we then
///   "follow" to the OTHER table in the set, which becomes the new
///   source for the next iteration.
/// - When a chain ends with `| <op_column>`, the chain's final
///   source table must match the column's alias.
///
/// Errors as `UnresolvedElement` for "join doesn't contain source"
/// and `InvalidProperty` for "join doesn't connect to target". The
/// V4 + B2 paths already report missing-join / missing-table cases;
/// this validator runs after those and assumes the resolved data is
/// consistent.
///
/// [1]: legend-pure-store/legend-pure-store-relational/.../v1/validator/JoinTreeNodeValidation.java
fn validate_join_tree_chains(
    class_mappings: &[RegisteredRelationalClassMapping],
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    _resolved_cms: &[crate::processor::ResolvedClassMapping],
    errors: &mut Vec<CompilationError>,
) {
    for reg in class_mappings {
        // Class mapping's contextual main table comes from `~mainTable`.
        // Without one, we can't anchor the chain — skip (the V4 / E
        // validators surface the missing-context as needed).
        let Some(main_table) = &reg.body.main_table else {
            continue;
        };
        let source_db_fqn = packageable_fqn(&main_table.db);
        let source_table = main_table.scope.table.value.clone();

        let owner = SmolStr::new(format!("Class mapping '{}'", reg.class_mapping_id.as_str()));

        // Walk every JoinColWithDbOrConstant + Otherwise in the body.
        for elem in &reg.body.mapping_elements {
            walk_jc_for_chain(
                elem,
                &source_db_fqn,
                &source_table,
                resolved,
                &owner,
                errors,
            );
        }
        if let Some(lines) = &reg.body.association_mapping {
            for line in lines {
                walk_line_for_chain(
                    line,
                    &source_db_fqn,
                    &source_table,
                    resolved,
                    &owner,
                    errors,
                );
            }
        }
    }
}

fn walk_jc_for_chain(
    e: &crate::ast::MappingElement,
    source_db_fqn: &SmolStr,
    source_table: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match e {
        crate::ast::MappingElement::Single(line) => {
            walk_line_for_chain(line, source_db_fqn, source_table, resolved, owner, errors);
        }
        crate::ast::MappingElement::Scope(s) => {
            // Scope optionally overrides the source via simpleScopeInfo.
            let scope_db_fqn = packageable_fqn(&s.db);
            let scope_source_table = s
                .scope
                .as_ref()
                .map_or_else(|| source_table.clone(), |info| info.table.value.clone());
            for line in &s.mapping_lines {
                walk_line_for_chain(
                    line,
                    &scope_db_fqn,
                    &scope_source_table,
                    resolved,
                    owner,
                    errors,
                );
            }
        }
    }
}

fn walk_line_for_chain(
    line: &SingleMappingLine,
    source_db_fqn: &SmolStr,
    source_table: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match line {
        SingleMappingLine::Plus(p) => {
            check_chain_in_join_col(
                &p.mapping.value,
                source_db_fqn,
                source_table,
                resolved,
                owner,
                errors,
            );
        }
        SingleMappingLine::NonePlus(np) => match &np.value {
            NonePlusMappingValue::Relational(rm) => {
                check_chain_in_join_col(
                    &rm.value,
                    source_db_fqn,
                    source_table,
                    resolved,
                    owner,
                    errors,
                );
            }
            NonePlusMappingValue::Embedded(em) => {
                if let Some(jcs) = &em.primary_key {
                    for jc in jcs {
                        check_chain_in_join_col(
                            jc,
                            source_db_fqn,
                            source_table,
                            resolved,
                            owner,
                            errors,
                        );
                    }
                }
                for inner in &em.mapping_lines {
                    walk_line_for_chain(
                        inner,
                        source_db_fqn,
                        source_table,
                        resolved,
                        owner,
                        errors,
                    );
                }
                if let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &em.trailer {
                    for m in maps {
                        let oj_db = m
                            .otherwise_join
                            .db
                            .as_ref()
                            .map_or_else(|| source_db_fqn.clone(), packageable_fqn);
                        check_chain_in_sequence(
                            &m.otherwise_join.join_sequence,
                            &oj_db,
                            source_table,
                            None,
                            resolved,
                            owner,
                            errors,
                        );
                    }
                }
            }
        },
    }
}

fn check_chain_in_join_col(
    jc: &JoinColWithDbOrConstant,
    source_db_fqn: &SmolStr,
    source_table: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let Some(seq) = &jc.join else {
        return;
    };
    let chain_db = jc
        .db
        .as_ref()
        .map_or_else(|| source_db_fqn.clone(), packageable_fqn);
    // Trailing column after `|` — its alias is the chain's claimed
    // target table.
    let target_alias = jc.column.as_ref().and_then(|c| match c {
        OpColumn::Aliased { alias, .. } => Some(alias.value.clone()),
        OpColumn::Target { .. } => None,
    });
    check_chain_in_sequence(
        seq,
        &chain_db,
        source_table,
        target_alias.as_ref(),
        resolved,
        owner,
        errors,
    );
}

fn check_chain_in_sequence(
    seq: &JoinSequence,
    chain_db: &SmolStr,
    source_table: &SmolStr,
    target_table: Option<&SmolStr>,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let mut current_source = source_table.clone();
    let mut current_db = chain_db.clone();

    let head = follow_join(
        &seq.head,
        &current_db,
        &current_source,
        resolved,
        owner,
        errors,
    );
    if let Some(next) = head {
        current_source = next;
    } else {
        // Couldn't follow — error already pushed; abort the rest of
        // the chain so we don't cascade misleading messages.
        return;
    }

    for r in &seq.right {
        if let Some(rdb) = &r.db {
            current_db = packageable_fqn(rdb);
        }
        let next = follow_join(
            &r.join,
            &current_db,
            &current_source,
            resolved,
            owner,
            errors,
        );
        if let Some(next) = next {
            current_source = next;
        } else {
            return;
        }
    }

    if let Some(target) = target_table
        && &current_source != target
    {
        errors.push(CompilationError {
            message: format!(
                "{owner}: join chain ends at table '{current_source}' \
                 but the trailing column references table '{target}'"
            ),
            source_info: seq.source_info.clone(),
            kind: CompilationErrorKind::InvalidAssociation {
                name: owner.clone(),
                reason: SmolStr::new(format!(
                    "chain ends at '{current_source}', trailing column at '{target}'"
                )),
            },
        });
    }
}

/// Look up `@join`'s table set in the resolved database snapshot;
/// if `source_table` is in the set, return the OTHER table in the
/// pair. Reports an `UnresolvedElement` when the join's table set
/// doesn't contain `source_table`.
fn follow_join(
    one_join: &OneJoin,
    db_fqn: &SmolStr,
    source_table: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) -> Option<SmolStr> {
    let join_tables = resolve_join_tables(db_fqn, &one_join.name.value, resolved)?;
    if !join_tables.contains(source_table) {
        // Skip when the join is unknown or has < 2 distinct tables —
        // those cases are surfaced by validator E / V4. The chain
        // walker only fires when the join exists but doesn't include
        // the source.
        if join_tables.len() >= 2 {
            errors.push(CompilationError {
                message: format!(
                    "{owner}: join @{} does not contain source table '{}'",
                    one_join.name.value, source_table
                ),
                source_info: one_join.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: SmolStr::new(format!("join @{}", one_join.name.value)),
                },
            });
        }
        return None;
    }
    // Return the OTHER table.
    join_tables.into_iter().find(|t| t != source_table)
}

/// Distinct table names referenced by a Join's resolved op-body,
/// walking the include closure. Returns `None` when the join isn't
/// found in any visible database.
fn resolve_join_tables(
    db_fqn: &SmolStr,
    join_name: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
) -> Option<Vec<SmolStr>> {
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_resolve_join_tables(db_fqn, join_name, resolved, &mut visited)
}

fn walk_resolve_join_tables(
    db_fqn: &SmolStr,
    join_name: &SmolStr,
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    visited: &mut HashSet<SmolStr>,
) -> Option<Vec<SmolStr>> {
    if !visited.insert(db_fqn.clone()) {
        return None;
    }
    let snapshot = resolved.get(db_fqn)?;
    if let Some(body) = snapshot
        .join_bodies
        .iter()
        .find(|b| &b.element_name == join_name)
    {
        let mut tables: Vec<SmolStr> = Vec::new();
        let mut seen: HashSet<SmolStr> = HashSet::new();
        for binding in &body.bindings {
            if !binding.unresolved_table && seen.insert(binding.table_name.clone()) {
                tables.push(binding.table_name.clone());
            }
        }
        return Some(tables);
    }
    for include_fqn in &snapshot.include_fqns {
        if let Some(tables) = walk_resolve_join_tables(include_fqn, join_name, resolved, visited) {
            return Some(tables);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Phase A4 — RelationalAssociationImplementationValidator parity
// ---------------------------------------------------------------------------

/// For every AssociationMapping body (Java's
/// `RelationalAssociationImplementation`), validate each property
/// mapping line:
///
/// 1. The line's `[srcId, tgtId]` must each resolve to a registered
///    class mapping in the same `Mapping` (E2 already enforces the
///    arity-of-2; here we focus on identity).
/// 2. Both source and target class mappings must declare an
///    `effective_main_table` (own or inherited via `extends`).
/// 3. The line's value must carry a join sequence (Java errors with
///    "Mapping Error: expected a join" for bare-column / literal
///    values on association lines).
/// 4. The join chain must start at the source's main table and end
///    at the target's main table — same connectivity check as A3'
///    but anchored to a known source/target pair.
fn validate_association_mapping_joins(
    class_mappings: &[RegisteredRelationalClassMapping],
    resolved_cms: &[crate::processor::ResolvedClassMapping],
    resolved: &HashMap<SmolStr, crate::processor::ResolvedDatabase>,
    errors: &mut Vec<CompilationError>,
) {
    // Build (mapping_fqn, class_mapping_id) → effective main table /
    // primary db for fast source/target resolution.
    let mut by_id: HashMap<(SmolStr, SmolStr), (Option<SmolStr>, Option<SmolStr>)> = HashMap::new();
    for cm in resolved_cms {
        by_id.insert(
            (cm.mapping_fqn.clone(), cm.class_mapping_id.clone()),
            (
                cm.effective_main_table.clone(),
                cm.effective_primary_database.clone(),
            ),
        );
    }

    for reg in class_mappings {
        let Some(lines) = &reg.body.association_mapping else {
            continue;
        };

        let owner = SmolStr::new(format!(
            "AssociationMapping '{}'",
            reg.class_mapping_id.as_str()
        ));

        for line in lines {
            let (property_name, source_id, target_id, value, line_si) = match line {
                SingleMappingLine::NonePlus(np) => (
                    np.property.value.clone(),
                    np.source_id.as_ref().map(|s| s.value.clone()),
                    np.target_id.as_ref().map(|t| t.value.clone()),
                    match &np.value {
                        NonePlusMappingValue::Relational(rm) => Some(&rm.value),
                        NonePlusMappingValue::Embedded(_) => None,
                    },
                    np.source_info.clone(),
                ),
                SingleMappingLine::Plus(p) => (
                    p.property.value.clone(),
                    None,
                    None,
                    Some(&p.mapping.value),
                    p.source_info.clone(),
                ),
            };

            // (1) Source / target ids resolve to class mappings.
            let Some((source_table, source_db)) = source_id
                .as_ref()
                .and_then(|id| by_id.get(&(reg.mapping_fqn.clone(), id.clone())).cloned())
            else {
                if let Some(id) = &source_id {
                    errors.push(CompilationError {
                        message: format!(
                            "{owner}: property '{property_name}' source set-implementation id \
                             '{id}' does not match any class mapping in '{}'",
                            reg.mapping_fqn
                        ),
                        source_info: line_si.clone(),
                        kind: CompilationErrorKind::UnresolvedElement { path: id.clone() },
                    });
                }
                continue;
            };
            let Some((target_table, target_db)) = target_id
                .as_ref()
                .and_then(|id| by_id.get(&(reg.mapping_fqn.clone(), id.clone())).cloned())
            else {
                if let Some(id) = &target_id {
                    errors.push(CompilationError {
                        message: format!(
                            "{owner}: property '{property_name}' target set-implementation id \
                             '{id}' does not match any class mapping in '{}'",
                            reg.mapping_fqn
                        ),
                        source_info: line_si.clone(),
                        kind: CompilationErrorKind::UnresolvedElement { path: id.clone() },
                    });
                }
                continue;
            };

            // (2) Both class mappings must declare a main table.
            let (Some(source_table), Some(source_db)) = (source_table, source_db) else {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' source class mapping has no \
                         resolvable main table"
                    ),
                    source_info: line_si.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: owner.clone(),
                        reason: SmolStr::new("source class mapping has no main table"),
                    },
                });
                continue;
            };
            let (Some(target_table), Some(_target_db)) = (target_table, target_db) else {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' target class mapping has no \
                         resolvable main table"
                    ),
                    source_info: line_si.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: owner.clone(),
                        reason: SmolStr::new("target class mapping has no main table"),
                    },
                });
                continue;
            };

            // (3) Line must carry a join sequence (Java's
            // "Mapping Error: expected a join").
            let Some(value) = value else {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' must specify a join sequence \
                         (Java parity: 'Mapping Error: expected a join')"
                    ),
                    source_info: line_si.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: owner.clone(),
                        reason: SmolStr::new("expected a join"),
                    },
                });
                continue;
            };
            let Some(seq) = &value.join else {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' must specify a join sequence \
                         (Java parity: 'Mapping Error: expected a join')"
                    ),
                    source_info: value.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: owner.clone(),
                        reason: SmolStr::new("expected a join"),
                    },
                });
                continue;
            };

            // (4) Chain must connect source main table → target main table.
            let chain_db = value
                .db
                .as_ref()
                .map(packageable_fqn)
                .unwrap_or(source_db.clone());
            check_chain_in_sequence(
                seq,
                &chain_db,
                &source_table,
                Some(&target_table),
                resolved,
                &owner,
                errors,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Phase A5 — RelationalInstanceSetImplementationValidator parity
// (per-property type-driven checks)
// ---------------------------------------------------------------------------

/// For every relational class mapping (NOT association), inspect each
/// property mapping line and apply the property-type checks Java's
/// `RelationalInstanceSetImplementationValidator.validatePropertyMappings`
/// runs:
///
/// - **Data-type with `[targetId]`** (Java: "The property 'X' returns
///   a data type and thus should not have a targetId").
/// - **Enum without EnumerationMapping** (Java: "Missing an
///   EnumerationMapping for the enum property 'X'. Enum properties
///   require an EnumerationMapping ...").
/// - **Class-typed property without join sequence** (Java: "Mapping
///   Error! The target type:'X' is not a data type but the
///   relationalOperation is not a join").
///
/// Reads the property's declared type via
/// `legend_pure_parser_pure::model::PureModel`. When the class can't
/// be resolved on the model (test fixtures that don't compile real
/// classes), the validator no-ops — the existing per-class-mapping
/// validators already surface the missing-class case.
fn validate_class_mapping_property_types(
    class_mappings: &[RegisteredRelationalClassMapping],
    model: &legend_pure_parser_pure::model::PureModel,
    errors: &mut Vec<CompilationError>,
) {
    use legend_pure_parser_pure::model::Element as ModelElement;
    use legend_pure_parser_pure::types::TypeExpr;

    for reg in class_mappings {
        // Skip association bodies — A4 handles those.
        if reg.body.association_mapping.is_some() {
            continue;
        }
        let Some(class_id) = resolve_class_by_fqn(model, &reg.class_fqn) else {
            // Class not resolvable on the model (fixtures without
            // pure source, etc.) — no per-property type info to
            // check against.
            continue;
        };

        let owner = SmolStr::new(format!("Class mapping '{}'", reg.class_mapping_id.as_str()));

        // Walk top-level + scope-wrapped lines (both flatten into
        // class-mapping property scope).
        let visit_line = |line: &SingleMappingLine, errors: &mut Vec<CompilationError>| {
            check_property_line(line, class_id, model, &reg.class_mapping_id, &owner, errors);
        };
        for elem in &reg.body.mapping_elements {
            match elem {
                crate::ast::MappingElement::Single(line) => visit_line(line, errors),
                crate::ast::MappingElement::Scope(s) => {
                    for line in &s.mapping_lines {
                        visit_line(line, errors);
                    }
                }
            }
        }

        // Suppress warnings — TypeExpr / ModelElement are referenced
        // through `check_property_line` below.
        let _ = std::marker::PhantomData::<(TypeExpr, ModelElement)>;
    }
}

fn check_property_line(
    line: &SingleMappingLine,
    class_id: legend_pure_parser_pure::ids::ElementId,
    model: &legend_pure_parser_pure::model::PureModel,
    class_mapping_id: &SmolStr,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    use crate::ast::NonePlusMappingValue;

    // Plus lines declare a *local* property — by definition not on
    // the underlying class, so the class's declared property types
    // don't apply. Java's validator skips them too (the local
    // property's type comes from the line's own LocalMappingProperty).
    let SingleMappingLine::NonePlus(np) = line else {
        return;
    };

    let property_name = &np.property.value;

    // Resolve the property's declared type by walking the class
    // hierarchy.
    let Some(prop_type) = find_property_type(model, class_id, property_name) else {
        // No such property on the class. dsl-mapping's validators
        // surface the unknown-property error; we just skip type
        // checks here.
        return;
    };

    let target_kind = classify_type_target(model, &prop_type);

    match &np.value {
        NonePlusMappingValue::Relational(rm) => {
            check_relational_value_against_type(
                np,
                rm,
                target_kind,
                class_mapping_id,
                owner,
                errors,
            );
        }
        NonePlusMappingValue::Embedded(_) => {
            // Embedded mappings always target a class (the embedded
            // class) — Java's validator recurses into them. Property
            // mappings inside the embedded body get visited at the
            // top level of the per-class-mapping walk anyway, so
            // nothing extra to do here.
        }
    }
}

/// Categorise a property's resolved [`TypeExpr`] into the three
/// buckets Java's validator distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyTargetKind {
    /// `Integer`, `String`, `Date`, …
    DataType,
    /// `meta::pure::metamodel::type::Enumeration<…>`.
    Enumeration,
    /// Any other class.
    Class,
    /// Type didn't resolve to a known element — bail out of the
    /// per-property-type checks for this line.
    Unknown,
}

fn classify_type_target(
    model: &legend_pure_parser_pure::model::PureModel,
    ty: &legend_pure_parser_pure::types::TypeExpr,
) -> PropertyTargetKind {
    use legend_pure_parser_pure::model::Element as ModelElement;
    use legend_pure_parser_pure::types::TypeExpr;
    let TypeExpr::Named { element, .. } = ty else {
        return PropertyTargetKind::Unknown;
    };
    match model.try_get_element(*element) {
        Some(ModelElement::PrimitiveType(_)) => PropertyTargetKind::DataType,
        Some(ModelElement::Enumeration(_)) => PropertyTargetKind::Enumeration,
        Some(ModelElement::Class(_)) => PropertyTargetKind::Class,
        _ => PropertyTargetKind::Unknown,
    }
}

fn check_relational_value_against_type(
    np: &crate::ast::NonePlusMappingLine,
    rm: &crate::ast::RelationalMapping,
    target_kind: PropertyTargetKind,
    class_mapping_id: &SmolStr,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    let property_name = &np.property.value;

    match target_kind {
        PropertyTargetKind::DataType => {
            // (1) `[srcId, tgtId]` is illegal for data-type-typed
            // properties — Java errors with "should not have a
            // targetId".
            if let Some(target_id) = &np.target_id {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' returns a data type and \
                         thus should not have a targetId ('{}')",
                        target_id.value,
                    ),
                    source_info: target_id.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: class_mapping_id.clone(),
                        reason: SmolStr::new(format!(
                            "data-type property '{property_name}' carries a targetId"
                        )),
                    },
                });
            }
        }
        PropertyTargetKind::Enumeration => {
            // (2) Enum properties require an EnumerationMapping
            // transformer — Java errors with "Missing an
            // EnumerationMapping for the enum property".
            if rm.transformer.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' is an enum-typed property and \
                         requires an EnumerationMapping transformer (Java parity: \
                         'Missing an EnumerationMapping for the enum property')"
                    ),
                    source_info: np.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: class_mapping_id.clone(),
                        reason: SmolStr::new(format!(
                            "enum property '{property_name}' missing EnumerationMapping"
                        )),
                    },
                });
            }
        }
        PropertyTargetKind::Class => {
            // (3) Class-typed properties must be mapped to a join
            // sequence (Java: "The target type:'X' is not a data
            // type but the relationalOperation is not a join").
            if rm.value.join.is_none() && rm.value.literal.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "{owner}: property '{property_name}' targets a class but its \
                         relational operation is not a join sequence (Java parity: \
                         'is not a data type but the relationalOperation is not a join')"
                    ),
                    source_info: rm.value.source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: class_mapping_id.clone(),
                        reason: SmolStr::new(format!(
                            "class-typed property '{property_name}' missing join"
                        )),
                    },
                });
            }
        }
        PropertyTargetKind::Unknown => {}
    }
}

/// Resolve a class FQN to its model `ElementId`, returning `None`
/// when no such class exists. Mirrors dsl-mapping's `resolve_class`
/// helper — kept inline so dsl-relational doesn't need to reach into
/// dsl-mapping internals.
fn resolve_class_by_fqn(
    model: &legend_pure_parser_pure::model::PureModel,
    fqn: &str,
) -> Option<legend_pure_parser_pure::ids::ElementId> {
    use legend_pure_parser_pure::model::Element as ModelElement;
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(SmolStr::is_empty) {
        return None;
    }
    let id = model.resolve_by_path(&segments)?;
    matches!(model.try_get_element(id)?, ModelElement::Class(_)).then_some(id)
}

/// Walk a class and its supertypes looking for a simple property
/// named `prop_name`. Returns the property's declared type when found.
/// Qualified properties resolve as "exists" but their
/// function-typed shape isn't useful for the per-property checks
/// here — return `None` so the caller skips them.
fn find_property_type(
    model: &legend_pure_parser_pure::model::PureModel,
    class_id: legend_pure_parser_pure::ids::ElementId,
    prop_name: &str,
) -> Option<legend_pure_parser_pure::types::TypeExpr> {
    use legend_pure_parser_pure::model::Element as ModelElement;
    use legend_pure_parser_pure::types::TypeExpr;
    let mut visited: HashSet<legend_pure_parser_pure::ids::ElementId> = HashSet::new();
    let mut stack: Vec<legend_pure_parser_pure::ids::ElementId> = vec![class_id];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(ModelElement::Class(c)) = model.try_get_element(id) else {
            continue;
        };
        if let Some(p) = c.properties.iter().find(|p| p.name == prop_name) {
            return Some(p.type_expr.clone());
        }
        if c.qualified_properties.iter().any(|q| q.name == prop_name) {
            return None;
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                stack.push(*element);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Phase A6 — Inline-target subtype check
// ---------------------------------------------------------------------------

/// For every embedded class-mapping body that ends with an
/// `Inline[setId]` trailer, validate that the inline target's class
/// is a subtype of the property's declared target class. Java parity:
///
///   "Mapping Error! The inlineSetImplementationId 'X' is
///    implementing the class 'Y' which is not a subType of 'Z'
///    (return type of the mapped property 'P')"
///
/// Walks recursively into embedded bodies so deeply-nested Inline
/// trailers are caught. E3 already validates the inline id resolves
/// to a class mapping in the same Mapping; A6 layers the subtype
/// check on top.
fn validate_inline_target_subtypes(
    class_mappings: &[RegisteredRelationalClassMapping],
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
    model: &legend_pure_parser_pure::model::PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Build a (mapping_fqn, class_mapping_id) → class_fqn lookup so
    // an Inline target's id resolves to the class FQN it implements.
    let mut by_id: HashMap<(SmolStr, SmolStr), SmolStr> = HashMap::new();
    for reg in class_mappings {
        by_id.insert(
            (reg.mapping_fqn.clone(), reg.class_mapping_id.clone()),
            reg.class_fqn.clone(),
        );
    }

    for reg in class_mappings {
        // Skip association bodies — they don't carry embedded
        // mappings on association ends in our grammar.
        if reg.body.association_mapping.is_some() {
            continue;
        }
        let Some(class_id) = resolve_class_by_fqn(model, &reg.class_fqn) else {
            continue;
        };
        let owner = SmolStr::new(format!("Class mapping '{}'", reg.class_mapping_id.as_str()));

        for elem in &reg.body.mapping_elements {
            walk_for_inline_subtypes(
                elem,
                class_id,
                &reg.mapping_fqn,
                &by_id,
                mapping_includes,
                model,
                &owner,
                errors,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)] // mirrors walk_line_for_inline_subtypes; refactoring needs a shared context struct
fn walk_for_inline_subtypes(
    e: &crate::ast::MappingElement,
    enclosing_class_id: legend_pure_parser_pure::ids::ElementId,
    mapping_fqn: &SmolStr,
    by_id: &HashMap<(SmolStr, SmolStr), SmolStr>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
    model: &legend_pure_parser_pure::model::PureModel,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    match e {
        crate::ast::MappingElement::Single(line) => {
            walk_line_for_inline_subtypes(
                line,
                enclosing_class_id,
                mapping_fqn,
                by_id,
                mapping_includes,
                model,
                owner,
                errors,
            );
        }
        crate::ast::MappingElement::Scope(s) => {
            for line in &s.mapping_lines {
                walk_line_for_inline_subtypes(
                    line,
                    enclosing_class_id,
                    mapping_fqn,
                    by_id,
                    mapping_includes,
                    model,
                    owner,
                    errors,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)] // see walk_for_inline_subtypes
fn walk_line_for_inline_subtypes(
    line: &SingleMappingLine,
    enclosing_class_id: legend_pure_parser_pure::ids::ElementId,
    mapping_fqn: &SmolStr,
    by_id: &HashMap<(SmolStr, SmolStr), SmolStr>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
    model: &legend_pure_parser_pure::model::PureModel,
    owner: &SmolStr,
    errors: &mut Vec<CompilationError>,
) {
    use legend_pure_parser_pure::types::TypeExpr;

    let SingleMappingLine::NonePlus(np) = line else {
        return;
    };
    let NonePlusMappingValue::Embedded(em) = &np.value else {
        return;
    };

    // (a) Recurse into the embedded body: nested mapping lines may
    // themselves carry Inline trailers. The "enclosing class" for
    // the recursion is the embedded property's TARGET class.
    let property_name = &np.property.value;
    let nested_class_id = find_property_type(model, enclosing_class_id, property_name)
        .as_ref()
        .and_then(|t| match t {
            TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        })
        .filter(|id| {
            matches!(
                model.try_get_element(*id),
                Some(legend_pure_parser_pure::model::Element::Class(_))
            )
        });
    if let Some(nested_id) = nested_class_id {
        for inner in &em.mapping_lines {
            walk_line_for_inline_subtypes(
                inner,
                nested_id,
                mapping_fqn,
                by_id,
                mapping_includes,
                model,
                owner,
                errors,
            );
        }
    }

    // (b) If this embedded body has an Inline trailer, run the
    // subtype check against the property's declared target class.
    let Some(EmbeddedMappingTrailer::Inline(inline)) = &em.trailer else {
        return;
    };
    let Some(property_target_id) = nested_class_id else {
        // No class-typed property → no Inline check applies (E3
        // already validates the inline id resolution; class-property
        // checks are A5's job).
        return;
    };
    let Some(inline_class_fqn) =
        find_class_mapping_via_includes(mapping_fqn, &inline.id.value, by_id, mapping_includes)
    else {
        // E3 emits the unresolved diagnostic.
        return;
    };
    let Some(inline_class_id) = resolve_class_by_fqn(model, inline_class_fqn) else {
        return;
    };

    if !is_subtype_of(model, inline_class_id, property_target_id) {
        let target_fqn = SmolStr::new(
            legend_pure_parser_pure::purem::fqn_path::element_fqn_path(model, property_target_id)
                .join("::"),
        );
        errors.push(CompilationError {
            message: format!(
                "{owner}: Inline target '{}' implements class '{}' which is not a subtype of \
                 '{}' (return type of the mapped property '{}')",
                inline.id.value, inline_class_fqn, target_fqn, property_name
            ),
            source_info: inline.source_info.clone(),
            kind: CompilationErrorKind::InvalidAssociation {
                name: owner.clone(),
                reason: SmolStr::new(format!(
                    "Inline target '{inline_class_fqn}' is not a subtype of '{target_fqn}'"
                )),
            },
        });
    }
}

/// Walk the supertype graph from `sub` looking for `sup`. Returns
/// `true` when `sub == sup` or any transitive `super_types` chain
/// reaches `sup`. Cycles terminate via `visited`.
fn is_subtype_of(
    model: &legend_pure_parser_pure::model::PureModel,
    sub: legend_pure_parser_pure::ids::ElementId,
    sup: legend_pure_parser_pure::ids::ElementId,
) -> bool {
    use legend_pure_parser_pure::model::Element as ModelElement;
    use legend_pure_parser_pure::types::TypeExpr;
    if sub == sup {
        return true;
    }
    let mut visited: HashSet<legend_pure_parser_pure::ids::ElementId> = HashSet::new();
    let mut stack: Vec<legend_pure_parser_pure::ids::ElementId> = vec![sub];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(ModelElement::Class(c)) = model.try_get_element(id) else {
            continue;
        };
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                if *element == sup {
                    return true;
                }
                stack.push(*element);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Phase A7 — AssociationMapping target identity + duplicate detection
// ---------------------------------------------------------------------------

/// For every relational class-mapping body whose AST is an
/// `AssociationMapping (...)` shape, validate two Java-parity rules
/// from `TestAssociationMappingValidation`:
///
/// 1. **Target must be an Association element on the model.** When
///    `class_fqn` resolves to a `Class` (or any non-Association),
///    error.
/// 2. **At most one AssociationMapping per (Mapping, Association)
///    pair.** Two `pkg::Foo : Relational { AssociationMapping(...) }`
///    bodies in the same Mapping that target the same association
///    error as duplicates.
fn validate_association_mapping_targets(
    class_mappings: &[RegisteredRelationalClassMapping],
    model: &legend_pure_parser_pure::model::PureModel,
    errors: &mut Vec<CompilationError>,
) {
    use legend_pure_parser_pure::model::Element as ModelElement;

    let mut seen: HashSet<(SmolStr, SmolStr)> = HashSet::new();
    for reg in class_mappings {
        if reg.body.association_mapping.is_none() {
            continue;
        }
        let key = (reg.mapping_fqn.clone(), reg.class_fqn.clone());
        if !seen.insert(key) {
            errors.push(CompilationError {
                message: format!(
                    "Duplicate AssociationMapping for '{}' in mapping '{}'",
                    reg.class_fqn, reg.mapping_fqn,
                ),
                source_info: reg.class_mapping_source_info.clone(),
                kind: CompilationErrorKind::DuplicateElement {
                    name: reg.class_fqn.clone(),
                },
            });
            continue;
        }
        // Resolve the target on the model and ensure it's an
        // Association (not a Class, Enumeration, etc.).
        let segments: Vec<SmolStr> = reg
            .class_fqn
            .as_str()
            .split("::")
            .map(SmolStr::new)
            .collect();
        if segments.is_empty() || segments.iter().any(SmolStr::is_empty) {
            continue;
        }
        let Some(id) = model.resolve_by_path(&segments) else {
            // dsl-mapping's element-resolution path emits the
            // "association not found" diagnostic; we don't double up.
            continue;
        };
        match model.try_get_element(id) {
            Some(ModelElement::Association(_)) => {}
            Some(ModelElement::Class(_)) => {
                errors.push(CompilationError {
                    message: format!(
                        "AssociationMapping body for '{}' but '{}' is a Class, not an \
                         Association — class mappings must use a regular `Class : Relational \
                         {{ (...) }}` body",
                        reg.class_fqn, reg.class_fqn,
                    ),
                    source_info: reg.class_mapping_source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: reg.class_fqn.clone(),
                        reason: SmolStr::new(
                            "AssociationMapping body targets a Class, not an Association",
                        ),
                    },
                });
            }
            _ => {
                errors.push(CompilationError {
                    message: format!(
                        "AssociationMapping body for '{}' but '{}' is not an Association",
                        reg.class_fqn, reg.class_fqn,
                    ),
                    source_info: reg.class_mapping_source_info.clone(),
                    kind: CompilationErrorKind::InvalidAssociation {
                        name: reg.class_fqn.clone(),
                        reason: SmolStr::new(
                            "AssociationMapping body targets a non-Association element",
                        ),
                    },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase E4 — cross-mapping include resolution helpers
// ---------------------------------------------------------------------------

/// Set of class-mapping ids visible from `mapping_fqn` — its own ids
/// unioned with every transitively-included mapping's ids. Mirrors
/// Java's
/// `org.finos.legend.pure.m2.dsl.mapping.Mapping.getClassMappingsByIdIncludeEmbedded`.
fn visible_class_mapping_ids(
    mapping_fqn: &SmolStr,
    ids_by_mapping: &HashMap<SmolStr, HashSet<SmolStr>>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
) -> HashSet<SmolStr> {
    let mut out: HashSet<SmolStr> = HashSet::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_visible_ids(
        mapping_fqn,
        ids_by_mapping,
        mapping_includes,
        &mut out,
        &mut visited,
    );
    out
}

fn walk_visible_ids(
    mapping_fqn: &SmolStr,
    ids_by_mapping: &HashMap<SmolStr, HashSet<SmolStr>>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
    out: &mut HashSet<SmolStr>,
    visited: &mut HashSet<SmolStr>,
) {
    if !visited.insert(mapping_fqn.clone()) {
        return;
    }
    if let Some(ids) = ids_by_mapping.get(mapping_fqn) {
        out.extend(ids.iter().cloned());
    }
    if let Some(includes) = mapping_includes.get(mapping_fqn) {
        for inc_fqn in includes {
            walk_visible_ids(inc_fqn, ids_by_mapping, mapping_includes, out, visited);
        }
    }
}

/// Find the class FQN that an inline target id resolves to, walking
/// the include closure. Returns `None` when the id isn't visible
/// from `mapping_fqn` or any of its transitively-included mappings.
fn find_class_mapping_via_includes<'a>(
    mapping_fqn: &SmolStr,
    target_id: &SmolStr,
    by_id: &'a HashMap<(SmolStr, SmolStr), SmolStr>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
) -> Option<&'a SmolStr> {
    let mut visited: HashSet<SmolStr> = HashSet::new();
    walk_find_class_mapping(
        mapping_fqn,
        target_id,
        by_id,
        mapping_includes,
        &mut visited,
    )
}

fn walk_find_class_mapping<'a>(
    mapping_fqn: &SmolStr,
    target_id: &SmolStr,
    by_id: &'a HashMap<(SmolStr, SmolStr), SmolStr>,
    mapping_includes: &HashMap<SmolStr, Vec<SmolStr>>,
    visited: &mut HashSet<SmolStr>,
) -> Option<&'a SmolStr> {
    if !visited.insert(mapping_fqn.clone()) {
        return None;
    }
    if let Some(class_fqn) = by_id.get(&(mapping_fqn.clone(), target_id.clone())) {
        return Some(class_fqn);
    }
    if let Some(includes) = mapping_includes.get(mapping_fqn) {
        for inc_fqn in includes {
            if let Some(found) =
                walk_find_class_mapping(inc_fqn, target_id, by_id, mapping_includes, visited)
            {
                return Some(found);
            }
        }
    }
    None
}
