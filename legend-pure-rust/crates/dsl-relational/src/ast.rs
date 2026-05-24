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

//! Relational DSL AST.
//!
//! Top-level shape: a `###Relational` section contains zero or more
//! [`DatabaseDef`]s. Each definition has a name, an optional list of
//! [`DatabaseInclude`]s, and an ordered list of [`DatabaseElement`]s
//! (schemas, default-schema tables/views, joins, filters,
//! multi-grain filters).
//!
//! Stage 1 captures structural detail for top-level shells (Database,
//! Schema, Table, View, Join, Filter, MultiGrainFilter, include) and
//! Table column definitions. The Join / Filter / MultiGrainFilter
//! body — `op_operation` in the Java grammar — is structurally
//! parsed in Stage 2 as an [`OpExpr`] tree (booleans, comparisons,
//! `IS [NOT] NULL`, function calls, columns, literals). View bodies
//! are structurally parsed in Phase A: `filterViewBlock?`,
//! `mappingBlockGroupBy?`, `DISTINCTCMD?`, `viewColumnMappingLines`.
//!
//! What's intentionally absent from Stage 1:
//! - Stereotypes / tagged values on Database / Schema / Table / Column
//!   (Java grammar accepts them; deferred).
//! - Milestoning specs on Tables — Stage 3.
//! - The mapping-island grammar (`Class : Relational { … }` inside
//!   `###Mapping`) — Stages 5–7.

use legend_pure_dsl_mapping::ast::ForeignClassMappingBody;
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::{Annotated, PackageableElement};
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::{Identifier, Package};
use smol_str::SmolStr;

/// Section kind string this DSL claims (`###Relational`).
pub const SECTION_KIND: &str = "Relational";

/// A `Database` definition: `Database pkg::db ( includes* (schema | table |
/// join | filter | multiGrainFilter | view)* )`.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseDef {
    /// Package qualifier on the database name.
    pub package: Option<Package>,
    /// The database's simple name plus its name token's span.
    pub name: SpannedString,
    /// `include other::db` directives at the top of the body, in
    /// source order. Empty when none are written.
    pub includes: Vec<DatabaseInclude>,
    /// All other database-body elements (schemas, default-schema
    /// tables/views, joins, filters, multi-grain filters), in
    /// source order. Preserving order matters for round-trip
    /// composition.
    pub elements: Vec<DatabaseElement>,
    /// Span of the entire `Database … ( … )` declaration.
    pub source_info: SourceInfo,
}

impl Spanned for DatabaseDef {
    fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }
}

impl Annotated for DatabaseDef {
    fn stereotypes(&self) -> &[legend_pure_parser_ast::annotation::StereotypePtr] {
        &[]
    }
    fn tagged_values(&self) -> &[legend_pure_parser_ast::annotation::TaggedValue] {
        &[]
    }
}

impl PackageableElement for DatabaseDef {
    fn package(&self) -> Option<&Package> {
        self.package.as_ref()
    }
    fn name(&self) -> &Identifier {
        &self.name.value
    }
}

impl DSLElement for DatabaseDef {
    fn kind(&self) -> &str {
        SECTION_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn clone_box(&self) -> Box<dyn DSLElement> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn DSLElement) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

/// `include pkg::OtherDb` — pulls another database's schemas / tables /
/// joins / filters into this one.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseInclude {
    /// FQN of the included database.
    pub included: PackageableElementPtr,
    /// Span of the entire `include …` directive.
    pub source_info: SourceInfo,
}

/// One element inside a [`DatabaseDef`] body, in source order.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)] // size disparity is intentional — hot paths match by variant and avoid the allocation cost of a Box indirection
pub enum DatabaseElement {
    /// `Schema name ( table* | view* )`.
    Schema(Schema),
    /// Top-level `Table` (lives in the implicit default schema).
    Table(Table),
    /// Top-level `View` (lives in the implicit default schema).
    View(View),
    /// `Join name ( <op_operation> )`.
    Join(Join),
    /// `Filter name ( <op_operation> )`.
    Filter(Filter),
    /// `MultiGrainFilter name ( <op_operation> )`.
    MultiGrainFilter(MultiGrainFilter),
}

/// A `Schema` block: `Schema name ( (table | view)* )`.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    /// Schema name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Tables declared inside this schema, in source order.
    pub tables: Vec<Table>,
    /// Views declared inside this schema, in source order.
    pub views: Vec<View>,
    /// Span of the entire `Schema … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `Table` declaration: `Table name ( <milestoning>? <columns> )`.
///
/// The optional `milestoning ( … )` block (Java grammar's
/// `milestoneSpec`) lands inside the parens *before* the column list,
/// per the upstream grammar (RelationalParser.g4 `table` rule).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Table name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Optional milestoning block. `None` when the table has no
    /// `milestoning ( … )` clause.
    pub milestoning: Option<MilestoneSpec>,
    /// Column definitions, in source order.
    pub columns: Vec<ColumnDef>,
    /// Span of the entire `Table … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `milestoning ( … )` block on a table.
///
/// Java grammar: `milestoneSpec: MILESTONING '(' milestoningDefinitions? ')'`,
/// where `milestoningDefinitions` is one or more comma-separated
/// `milestoningDefinition: identifier '(' milestoningContent ')'`.
/// Stage 3 parses every definition but defers per-kind validation
/// (which keys belong to `business` vs `processing`) to Stage 4.
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneSpec {
    /// One or more flavour definitions (`business(...)`,
    /// `processing(...)`, snapshot variants), in source order.
    pub definitions: Vec<MilestoneDef>,
    /// Span covering `milestoning ( … )`.
    pub source_info: SourceInfo,
}

/// One milestoning flavour definition: `<kind> ( <field>, … )`.
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneDef {
    /// `business`, `processing`, or any extension flavour name —
    /// stored verbatim and validated against the known set in
    /// Stage 4. Holding it as a `SpannedString` lets the validator
    /// emit a precise diagnostic span if the kind is unknown.
    pub kind: SpannedString,
    /// Comma-separated `KEY=value` field list, in source order.
    /// Stage 3 accepts any keys; Stage 4 enforces the per-kind
    /// expected sets (`BUS_FROM`+`BUS_THRU`+optional flags for
    /// `business`, etc.).
    pub fields: Vec<MilestoneField>,
    /// Span covering the entire `<kind> ( … )` definition.
    pub source_info: SourceInfo,
}

/// One `KEY = value` pair inside a [`MilestoneDef`].
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneField {
    /// Key identifier — `BUS_FROM`, `BUS_THRU`, `BUS_SNAPSHOT_DATE`,
    /// `THRU_IS_INCLUSIVE`, `INFINITY_DATE`, `PROCESSING_IN`,
    /// `PROCESSING_OUT`, `OUT_IS_INCLUSIVE`,
    /// `PROCESSING_SNAPSHOT_DATE`, etc. Stored verbatim.
    pub key: SpannedString,
    /// Right-hand-side value.
    pub value: MilestoneValue,
    /// Span covering `key = value`.
    pub source_info: SourceInfo,
}

/// Right-hand side of a [`MilestoneField`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MilestoneValue {
    /// Bare identifier — typically a column name on the same table
    /// (`in_z`, `from_z`, …). Validated against the table's columns
    /// in Stage 4.
    Identifier(SpannedString),
    /// Date literal: `%2999-12-31`, `%2024-01-15T10:30:00`. Stored
    /// as the literal token text (preserves the leading `%` and the
    /// canonical spelling for round-trip).
    Date {
        /// Literal text including the leading `%`.
        literal: SmolStr,
        /// Span of the literal token.
        source_info: SourceInfo,
    },
    /// Boolean literal: `true` / `false`.
    Boolean {
        /// Parsed boolean value.
        value: bool,
        /// Span of the `true` / `false` token.
        source_info: SourceInfo,
    },
}

/// One column inside a [`Table`].
///
/// Java grammar:
///
/// ```text
/// columnDefinition:
///   relationalIdentifier
///   stereotypes? taggedValues?
///   identifier (GROUP_OPEN INTEGER (COMMA INTEGER)? GROUP_CLOSE)?
///   (PRIMARYKEY | NOT_NULL)?
/// ;
/// ```
///
/// Stage 1 captures name, type-name, optional `(size[, scale])`, and
/// the PK / NOT NULL trailing flags. Stereotypes / tagged values are
/// deferred (rare in fixtures; structural).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    /// Column name.
    pub name: SpannedString,
    /// Type name as written (`INT`, `VARCHAR`, `TIMESTAMP`, …) —
    /// Stage 1 stores the identifier; Stage 4's validators map it
    /// onto the relational metamodel `DataType` hierarchy.
    pub type_name: SpannedString,
    /// First parenthesized integer (`size`) — `VARCHAR(200)`'s `200`,
    /// `DECIMAL(10, 2)`'s `10`. `None` when omitted.
    pub size: Option<i64>,
    /// Second parenthesized integer (`scale`) — `DECIMAL(10, 2)`'s
    /// `2`. `None` when omitted (or when `size` itself is `None`).
    pub scale: Option<i64>,
    /// `PRIMARY KEY` flag.
    pub primary_key: bool,
    /// `NOT NULL` flag.
    pub not_null: bool,
    /// Span of the entire column definition (name through trailing
    /// flag if any).
    pub source_info: SourceInfo,
}

/// A `View` declaration: `View name ( <view-body> )`.
///
/// Java grammar (RelationalParser.g4):
/// ```text
/// view: VIEW relationalIdentifier '('
///         filterViewBlock?
///         mappingBlockGroupBy?
///         DISTINCTCMD?
///         viewColumnMappingLines
///       ')'
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// View name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Optional `~filter [(db1) joinSeq | [db2]]? <filterName>` clause.
    pub filter: Option<FilterViewBlock>,
    /// Optional `~groupBy(<joinCol>, …)` clause.
    pub group_by: Option<Vec<JoinColWithDbOrConstant>>,
    /// `~distinct` flag.
    pub distinct: bool,
    /// One or more `colName ([targetSetId])? : <joinCol>` lines.
    pub columns: Vec<ViewColumnMappingLine>,
    /// Span of the entire `View … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// `~filter (database joinSequence PIPE database)? identifier` —
/// optional filter binding on a [`View`].
///
/// Java grammar:
/// `filterViewBlock: MAPPING_FILTER (database joinSequence PIPE database)? identifier`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterViewBlock {
    /// Optional `[db1] joinSeq | [db2]` chain. When `None`, the
    /// filter resolves against the view's owning database.
    pub db_chain: Option<FilterViewDbChain>,
    /// Filter name. Unqualified — the bracket-enclosed `[db]`
    /// qualifier (when present) lives in `db_chain.second_db`, not on
    /// the filter name itself.
    pub filter_name: SpannedString,
    /// Span covering `~filter … <filter_name>`.
    pub source_info: SourceInfo,
}

/// `[db1] joinSequence | [db2]` chain inside a [`FilterViewBlock`].
#[derive(Debug, Clone, PartialEq)]
pub struct FilterViewDbChain {
    /// `[db1]` — database the join sequence walks from.
    pub first_db: PackageableElementPtr,
    /// `@a > @b > …` join sequence between the two databases.
    pub join_sequence: JoinSequence,
    /// `[db2]` — database the filter lives in.
    pub second_db: PackageableElementPtr,
    /// Span covering the chain.
    pub source_info: SourceInfo,
}

/// One `viewColumnMappingLine` row inside a [`View`] body.
///
/// Java grammar:
/// `viewColumnMappingLine: identifier (BRACKET_OPEN identifier BRACKET_CLOSE)? COLON joinColWithDbOrConstant`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewColumnMappingLine {
    /// Column name written in the view.
    pub column_name: SpannedString,
    /// Optional `[targetSetImplementationId]` — the bracket-enclosed
    /// identifier between the column name and the colon. Java's
    /// `RelationalGraphBuilder.visitViewColumnMappingLine` writes this
    /// as `targetSetImplementationId` on the synthesised
    /// `ColumnMapping`.
    pub target_set_id: Option<SpannedString>,
    /// Right-hand-side `joinColWithDbOrConstant` — column / join
    /// chain / literal whose value populates this view column.
    pub value: JoinColWithDbOrConstant,
    /// Span covering the entire row.
    pub source_info: SourceInfo,
}

/// A `Join` declaration: `Join name ( <op_operation> )`.
///
/// The `body` is the structurally parsed `op_operation` tree (Stage 2).
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    /// Join name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Structured op-expression body (`{target}.x = other.y`, etc.).
    pub body: OpExpr,
    /// Span of the entire `Join … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `Filter` declaration: `Filter name ( <op_operation> )`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    /// Filter name.
    pub name: SpannedString,
    /// Structured op-expression body.
    pub body: OpExpr,
    /// Span of the entire `Filter … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `MultiGrainFilter` declaration: a Filter that the Java
/// processor flags for grain-aware joins.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiGrainFilter {
    /// MultiGrainFilter name.
    pub name: SpannedString,
    /// Structured op-expression body.
    pub body: OpExpr,
    /// Span of the entire `MultiGrainFilter … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// Structured `op_operation` node — the Java grammar's
/// `op_operation`/`op_atomicOperation`/`op_function`/`op_column`
/// productions, collapsed into one tagged-union shape that mirrors
/// the Pure runtime `RelationalOperationElement` hierarchy at the
/// surface level.
///
/// Precedence: the parser is a hand-rolled climber. Boolean operators
/// (`and`, `or`) sit above comparisons (`=`, `>`, `<`, `>=`, `<=`,
/// `!=`, `<>`); a comparison takes a single col-or-constant on each
/// side per the Java grammar (no chained comparisons). `IS NULL` and
/// `IS NOT NULL` are postfix on a col-or-constant. Function calls
/// are atomic and take a comma-separated argument list of nested
/// op-expressions.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OpExpr {
    /// `lhs and rhs` / `lhs or rhs`. Right-recursive per Java
    /// (`op_boolean_operation_right: op_boolean_operator op_operation`).
    Bool {
        /// `and` or `or`.
        op: BoolOp,
        /// Left operand (atomic or already-grouped).
        lhs: Box<OpExpr>,
        /// Right operand — itself a full op_operation, so chained
        /// `a and b and c` parses as `Bool(and, a, Bool(and, b, c))`.
        rhs: Box<OpExpr>,
        /// Span covering both sides + the operator token.
        source_info: SourceInfo,
    },
    /// `lhs <op> rhs` — a single relational comparison. Java grammar
    /// allows only one comparison per atomic, so `lhs` and `rhs` are
    /// `colWithDbOrConstant`s, not nested booleans.
    Compare {
        /// One of `=`, `>`, `<`, `>=`, `<=`, `!=`, `<>`.
        op: BinOp,
        /// Left col-or-constant.
        lhs: Box<OpExpr>,
        /// Right col-or-constant.
        rhs: Box<OpExpr>,
        /// Span covering the whole comparison.
        source_info: SourceInfo,
    },
    /// `expr is null` / `expr is not null`. Postfix on a
    /// col-or-constant (cannot follow a boolean / comparison
    /// directly per the Java grammar).
    IsNull {
        /// The col-or-constant being null-tested.
        expr: Box<OpExpr>,
        /// `true` for `is not null`, `false` for `is null`.
        negated: bool,
        /// Span from the expr through the `null` keyword.
        source_info: SourceInfo,
    },
    /// `( inner )` — an explicit grouping in source. Preserved so
    /// the composer can round-trip the user's parenthesisation
    /// without re-deriving it from precedence rules.
    Group {
        /// Wrapped expression.
        inner: Box<OpExpr>,
        /// Span covering the parens and content.
        source_info: SourceInfo,
    },
    /// `[db?] name(arg1, arg2, …)` — `op_function` and the
    /// `tableAliasColumnWithScopeInfo` `name(joinCols)` form share
    /// this shape at the AST level. The args are nested op-exprs
    /// (col-or-constants for op_function, joinCols for the
    /// tableAlias form — Stage 5 may refine when fixtures force it).
    Function {
        /// Optional `[db]` qualifier preceding the name.
        db: Option<PackageableElementPtr>,
        /// Function or alias name.
        name: SpannedString,
        /// Argument list, in source order. Empty when called with
        /// `()`.
        args: Vec<OpExpr>,
        /// Span covering `[db]name(args)`.
        source_info: SourceInfo,
    },
    /// `[db?] column-form` — a column reference (target, alias, or
    /// alias-with-scope-info).
    Column(OpColumn),
    /// String / integer / float literal.
    Literal(OpLiteral),
    /// `[v1, v2, …]` — array literal used as a function argument
    /// (Java's `arrayOfFunctionArguments`). Most commonly seen as
    /// the second argument of `in(col, [v1, v2])`. Elements are
    /// nested op-exprs so arrays can contain columns, literals, or
    /// nested arrays.
    Array {
        /// Array elements, in source order.
        elements: Vec<OpExpr>,
        /// Span covering `[ … ]`.
        source_info: SourceInfo,
    },
}

impl OpExpr {
    /// Returns the span covering this node.
    #[must_use]
    pub fn source_info(&self) -> &SourceInfo {
        match self {
            OpExpr::Bool { source_info, .. }
            | OpExpr::Compare { source_info, .. }
            | OpExpr::IsNull { source_info, .. }
            | OpExpr::Group { source_info, .. }
            | OpExpr::Function { source_info, .. }
            | OpExpr::Array { source_info, .. } => source_info,
            OpExpr::Column(c) => c.source_info(),
            OpExpr::Literal(l) => l.source_info(),
        }
    }
}

/// One of the seven Java relational comparison operators.
///
/// Java grammar:
/// `op_operator: EQUAL | GREATERTHAN | LESSTHAN | GREATERTHANEQUAL |
/// LESSTHANEQUAL | TEST_NOT_EQUAL | NOT_EQUAL_2`. We keep `NotEq`
/// (`!=`) and `NotEq2` (`<>`) distinct so the composer can replay
/// the original spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `=`
    Eq,
    /// `>`
    Gt,
    /// `<`
    Lt,
    /// `>=`
    GtEq,
    /// `<=`
    LtEq,
    /// `!=`
    NotEq,
    /// `<>`
    NotEq2,
}

impl BinOp {
    /// Returns the canonical source spelling for this comparison.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BinOp::Eq => "=",
            BinOp::Gt => ">",
            BinOp::Lt => "<",
            BinOp::GtEq => ">=",
            BinOp::LtEq => "<=",
            BinOp::NotEq => "!=",
            BinOp::NotEq2 => "<>",
        }
    }
}

/// `and` / `or` boolean connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    /// `and`
    And,
    /// `or`
    Or,
}

impl BoolOp {
    /// Returns the canonical source spelling (`and` / `or`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BoolOp::And => "and",
            BoolOp::Or => "or",
        }
    }
}

/// One of the column-reference forms inside an [`OpExpr::Column`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OpColumn {
    /// `{target}.column [PRIMARY KEY]?` — the Filter-context form
    /// (`tableAliasColumn` in the Java grammar). The leading
    /// `{target}` literal is a special token in Pure relational
    /// syntax that refers to the join / filter's implicit subject
    /// table.
    Target {
        /// Column name following the `{target}.` prefix.
        column: SpannedString,
        /// `PRIMARY KEY` trailing flag (rare in op-expressions; kept
        /// for round-trip parity with Java).
        primary_key: bool,
        /// Span from `{target}` through the optional flag.
        source_info: SourceInfo,
    },
    /// `[db?] alias [.scope (.scope)?] [PRIMARY KEY]?` — the
    /// alias-with-optional-scope form
    /// (`tableAliasColumnWithScopeInfo`'s scope branch).
    ///
    /// Examples:
    /// - `tradeTable.prodId` — alias `tradeTable`, `scope = ["prodId"]`.
    /// - `tradeTable.qty PRIMARY KEY` — same with PK flag.
    /// - `[other::db]tradeTable.prodId` — db qualifier present.
    /// - `tradeTable` (alias alone) — `scope = []`.
    Aliased {
        /// Optional `[db]` prefix.
        db: Option<PackageableElementPtr>,
        /// Alias / table name.
        alias: SpannedString,
        /// Zero, one, or two scope-info segments (Java's
        /// `scopeInfoPart scopeInfoPart?`). Empty means "alias
        /// alone".
        scope: Vec<SpannedString>,
        /// `PRIMARY KEY` trailing flag.
        primary_key: bool,
        /// Span covering the entire reference.
        source_info: SourceInfo,
    },
}

impl OpColumn {
    /// Returns the span covering this column reference.
    #[must_use]
    pub fn source_info(&self) -> &SourceInfo {
        match self {
            OpColumn::Target { source_info, .. } | OpColumn::Aliased { source_info, .. } => {
                source_info
            }
        }
    }
}

/// Literal value inside an op-expression.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum OpLiteral {
    /// Quoted-string literal.
    String {
        /// Literal value, quotes stripped.
        value: SmolStr,
        /// Span of the literal token.
        source_info: SourceInfo,
    },
    /// Signed integer literal (Java's `INTEGER: ('+'|'-')? Digit+`).
    Integer {
        /// Parsed integer.
        value: i64,
        /// Span of the literal.
        source_info: SourceInfo,
    },
    /// Float literal.
    Float {
        /// Parsed float (string-preserved spelling stays in
        /// `source_info`'s underlying source).
        value: f64,
        /// Span of the literal.
        source_info: SourceInfo,
    },
}

impl OpLiteral {
    /// Returns the span covering this literal.
    #[must_use]
    pub fn source_info(&self) -> &SourceInfo {
        match self {
            OpLiteral::String { source_info, .. }
            | OpLiteral::Integer { source_info, .. }
            | OpLiteral::Float { source_info, .. } => source_info,
        }
    }
}

// Validators in later stages walk `db.elements` directly to find
// tables / views / joins / filters; keeping the Stage 1 surface lean
// avoids over-engineering iterator helpers nobody uses yet.

/// Class-mapping-body kind that
/// `RelationalClassMappingBodyParser` reports — also the verbatim
/// spelling consumers see in `Class : Relational { … }`.
pub const CLASS_MAPPING_BODY_KIND: &str = "Relational";

/// `Class : Relational { … }` body produced by the Stage-5 plug-in.
/// Lives in `dsl-mapping`'s `ClassMappingBody::Foreign` variant.
///
/// Java grammar (RelationalParser.g4 lines 165-265):
///
/// ```text
/// classMapping  : mappingBlock (mappingElements)? EOF ;
/// mappingBlock  : filterMappingBlock? DISTINCTCMD?
///                 mappingBlockGroupBy? primaryKey? mainTableBlock? ;
/// ```
///
/// Stage 5 covers the shell + all five `mappingBlock` headers + bare
/// property-mapping lines and the `scope(…)` wrapper. Embedded /
/// inline / otherwise property mapping values are deferred to Stage 6.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationalClassMappingBody {
    /// `~filter [db]( <joinSequence> | [db2])? <id>` — optional.
    pub filter: Option<FilterMappingBlock>,
    /// `~distinct` flag.
    pub distinct: bool,
    /// `~groupBy(<joinCol>, …)` — optional list.
    pub group_by: Option<Vec<JoinColWithDbOrConstant>>,
    /// `~primaryKey(<joinCol>, …)` — optional list.
    pub primary_key: Option<Vec<JoinColWithDbOrConstant>>,
    /// `~mainTable [db]<table>(<scope>)?` — optional declared
    /// main-table reference.
    pub main_table: Option<MainTableBlock>,
    /// Body mapping elements: bare property-mapping lines and / or
    /// `scope(…) (…)` wrappers, in source order.
    pub mapping_elements: Vec<MappingElement>,
    /// `AssociationMapping ( <singleMappingLines> )` — when present,
    /// the body is interpreted as a relational association mapping
    /// rather than a class mapping (Stage 7). Mutually exclusive
    /// with the regular `mappingBlock` headers + `mapping_elements`
    /// content, since real fixtures put one OR the other inside the
    /// body braces. Java grammar:
    ///
    /// ```text
    /// associationMapping : ASSOCIATION_MAPPING '(' propertyMappings ')' ;
    /// propertyMappings   : mappingElements ;
    /// ```
    pub association_mapping: Option<Vec<SingleMappingLine>>,
    /// Span of the entire `{ … }` body.
    pub source_info: SourceInfo,
}

/// `~filter [db] (<joinSequence> | [db2])? <filter_name>`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterMappingBlock {
    /// Leading `[db]` qualifier on the filter ref.
    pub db: PackageableElementPtr,
    /// Optional join-sequence + secondary `| [db2]` qualifier.
    pub join_sequence: Option<FilterMappingJoinSequence>,
    /// Trailing identifier — name of the registered Filter.
    pub filter_name: SpannedString,
    /// Span covering the entire `~filter …` clause.
    pub source_info: SourceInfo,
}

/// `(<group>)? <oneJoin> <oneJoinRight>* | [db]`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterMappingJoinSequence {
    /// Optional `(group_id)` prefix on the first join.
    pub group_id: Option<SpannedString>,
    /// First `@joinName`.
    pub head: OneJoin,
    /// Subsequent `> [db?] @joinName` segments.
    pub right: Vec<OneJoinRight>,
    /// Trailing `| [db]` qualifier.
    pub second_db: PackageableElementPtr,
    /// Span covering the whole sequence (head through the trailing db).
    pub source_info: SourceInfo,
}

/// `~mainTable [db]<table>(<scope>)?`.
#[derive(Debug, Clone, PartialEq)]
pub struct MainTableBlock {
    /// Leading `[db]` qualifier.
    pub db: PackageableElementPtr,
    /// `simpleScopeInfo` — table name + 0-2 scope segments.
    pub scope: SimpleScopeInfo,
    /// Span covering `~mainTable …`.
    pub source_info: SourceInfo,
}

/// `<table>(.scope (.scope)?)?` — used by mainTable + scope().
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleScopeInfo {
    /// Table name.
    pub table: SpannedString,
    /// Zero, one, or two scope-info segments after the table name.
    pub scope: Vec<SpannedString>,
    /// Span from the table name through the last scope segment.
    pub source_info: SourceInfo,
}

/// `(database? ((joinSequence (PIPE op_column)?) | op_column)) | constant`.
///
/// Captured as a tagged shape with optional `[db]` prefix, optional
/// `joinSequence` (followed by an optional `| op_column`), and the
/// terminal column / literal.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinColWithDbOrConstant {
    /// Optional `[db]` qualifier.
    pub db: Option<PackageableElementPtr>,
    /// Optional `@a > @b > …` join sequence.
    pub join: Option<JoinSequence>,
    /// Optional terminal column reference. Set when the source
    /// included an `op_column` (after the join sequence's `| col`
    /// piping form, or directly when no join sequence preceded it).
    pub column: Option<OpColumn>,
    /// Optional terminal constant literal.
    pub literal: Option<OpLiteral>,
    /// Span covering the whole expression.
    pub source_info: SourceInfo,
}

/// `oneJoin (oneJoinRight)*` — `@head > @right1 > @right2 ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinSequence {
    /// First `@joinName`.
    pub head: OneJoin,
    /// Subsequent `> [db?] @joinName` segments.
    pub right: Vec<OneJoinRight>,
    /// Span covering the whole sequence.
    pub source_info: SourceInfo,
}

/// `@joinName` — one element of a join sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct OneJoin {
    /// Name of the registered join.
    pub name: SpannedString,
    /// Span of `@joinName`.
    pub source_info: SourceInfo,
}

/// `> (group)? [db]? @joinName` — a non-head segment.
#[derive(Debug, Clone, PartialEq)]
pub struct OneJoinRight {
    /// Optional `(group_id)` on the right side.
    pub group_id: Option<SpannedString>,
    /// Optional `[db]` qualifier on the right side.
    pub db: Option<PackageableElementPtr>,
    /// `@joinName` on the right side.
    pub join: OneJoin,
    /// Span covering the entire `> … @join` segment.
    pub source_info: SourceInfo,
}

/// One element of `mappingElements`: a single mapping line or a
/// `scope(…)` wrapper.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)] // see DatabaseElement
pub enum MappingElement {
    /// Bare `prop : value` mapping line.
    Single(SingleMappingLine),
    /// `scope([db]<scope>) (prop : value, …)` wrapper.
    Scope(ScopedMapping),
}

/// `scope([db] simpleScopeInfo?) ( <singleMappingLines> )`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedMapping {
    /// `[db]` qualifier on the scope.
    pub db: PackageableElementPtr,
    /// Optional `simpleScopeInfo` — table name + 0-2 scope segments.
    pub scope: Option<SimpleScopeInfo>,
    /// Mapping lines inside the scope's body.
    pub mapping_lines: Vec<SingleMappingLine>,
    /// Span covering the entire `scope(…) (…)` expression.
    pub source_info: SourceInfo,
}

/// One `singleMappingLine` — bare or plus-form (`+localProperty`).
/// Embedded / inline / otherwise property mapping values are
/// deferred to Stage 6.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SingleMappingLine {
    /// `<property> [<srcId>(,<targetId>)?]? : transformer? joinCol`.
    NonePlus(NonePlusMappingLine),
    /// `+ <property> : <type>[<mult>] : transformer? joinCol`.
    Plus(PlusMappingLine),
}

/// Bare-form mapping line.
#[derive(Debug, Clone, PartialEq)]
pub struct NonePlusMappingLine {
    /// Property being mapped on the target class.
    pub property: SpannedString,
    /// Optional `[srcId]` or `[srcId, targetId]` tag pair.
    pub source_id: Option<SpannedString>,
    /// Optional second tag (when `[srcId, targetId]`).
    pub target_id: Option<SpannedString>,
    /// Right-hand-side mapping value: either `: transformer? joinCol`
    /// (Stage 5) or an embedded `( … )` body (Stage 6).
    pub value: NonePlusMappingValue,
    /// Span covering the entire line.
    pub source_info: SourceInfo,
}

/// RHS of a [`NonePlusMappingLine`].
///
/// Java grammar: `nonePlusSingleMappingLine : identifier
/// sourceAndTargetMappingId? (embeddedMapping | relationalMapping)`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)] // see DatabaseElement
pub enum NonePlusMappingValue {
    /// `: transformer? joinColWithDbOrConstant` — Stage 5 form.
    Relational(RelationalMapping),
    /// `( <body> ) (Inline | Otherwise)?` — Stage 6 embedded form.
    Embedded(EmbeddedMapping),
}

/// `( <primaryKey>? <singleMappingLines>? ) (<inline> | <otherwise>)?`.
///
/// Java grammar:
///
/// ```text
/// embeddedMapping :
///     '(' (primaryKey? singleMappingLines)? ')'
///     (otherwiseEmbeddedMapping | inline)?
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedMapping {
    /// Optional `~primaryKey(<joinCol>, …)` inside the embedded body.
    pub primary_key: Option<Vec<JoinColWithDbOrConstant>>,
    /// Nested mapping lines — each line itself can be embedded, so
    /// the AST is recursive.
    pub mapping_lines: Vec<SingleMappingLine>,
    /// Optional trailing `Inline [id]` or `Otherwise (…)`.
    pub trailer: Option<EmbeddedMappingTrailer>,
    /// Span covering the entire embedded form.
    pub source_info: SourceInfo,
}

/// Trailing alternative on an [`EmbeddedMapping`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EmbeddedMappingTrailer {
    /// `Inline [id]` — inline another class mapping by id.
    Inline(InlineRef),
    /// `Otherwise ( <otherwisePropertyMapping>, … )` — fallback
    /// property mappings reached when the primary embedded body
    /// doesn't apply.
    Otherwise(Vec<OtherwisePropertyMapping>),
}

/// `Inline [id]` — references another class mapping by its id.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineRef {
    /// Inlined class-mapping id.
    pub id: SpannedString,
    /// Span covering `Inline [id]`.
    pub source_info: SourceInfo,
}

/// `[<property>] : <otherwiseJoin>`.
#[derive(Debug, Clone, PartialEq)]
pub struct OtherwisePropertyMapping {
    /// Property the fallback applies to.
    pub property: SpannedString,
    /// `: <db>? <joinSequence>` — the fallback join.
    pub otherwise_join: OtherwiseJoin,
    /// Span covering `[prop] : …`.
    pub source_info: SourceInfo,
}

/// `<db>? <joinSequence>`.
#[derive(Debug, Clone, PartialEq)]
pub struct OtherwiseJoin {
    /// Optional `[db]` qualifier.
    pub db: Option<PackageableElementPtr>,
    /// Mandatory join sequence.
    pub join_sequence: JoinSequence,
    /// Span covering the whole otherwise-join.
    pub source_info: SourceInfo,
}

/// Plus-form mapping line: `+ <prop> : <typePath>[<lower>(..<upper>)?] : … `.
#[derive(Debug, Clone, PartialEq)]
pub struct PlusMappingLine {
    /// Property being mapped (declared local to this mapping).
    pub property: SpannedString,
    /// `: <typePath>[<lower>(..<upper>)?]` — local property declaration.
    pub local: LocalMappingProperty,
    /// `: transformer? joinColWithDbOrConstant`.
    pub mapping: RelationalMapping,
    /// Span covering the entire line.
    pub source_info: SourceInfo,
}

/// `: <typePath>[<lower>(..<upper>)?]`.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalMappingProperty {
    /// Property's declared type.
    pub type_path: PackageableElementPtr,
    /// Lower-bound multiplicity literal (`INTEGER | STAR`).
    pub mult_lower: SpannedString,
    /// Optional upper-bound multiplicity literal.
    pub mult_upper: Option<SpannedString>,
    /// Span covering the entire local-property declaration.
    pub source_info: SourceInfo,
}

/// `: <transformer>? <joinCol>` — bare relational mapping value.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationalMapping {
    /// `EnumerationMapping <id> :` — optional transformer prefix.
    pub transformer: Option<Transformer>,
    /// Terminal `joinColWithDbOrConstant`.
    pub value: JoinColWithDbOrConstant,
    /// Span covering the whole `: transformer? joinCol`.
    pub source_info: SourceInfo,
}

/// `EnumerationMapping <id> :`.
#[derive(Debug, Clone, PartialEq)]
pub struct Transformer {
    /// Enumeration-mapping identifier.
    pub enumeration_mapping: SpannedString,
    /// Span covering the entire transformer prefix.
    pub source_info: SourceInfo,
}

impl ForeignClassMappingBody for RelationalClassMappingBody {
    fn kind(&self) -> &str {
        CLASS_MAPPING_BODY_KIND
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn clone_box(&self) -> Box<dyn ForeignClassMappingBody> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn ForeignClassMappingBody) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
    fn compose(&self, out: &mut String) {
        crate::compose::write_relational_class_mapping_body(out, self);
    }
    fn referenced_stores(&self) -> Vec<PackageableElementPtr> {
        let mut out: Vec<PackageableElementPtr> = Vec::new();
        let mut seen: std::collections::HashSet<smol_str::SmolStr> =
            std::collections::HashSet::new();
        let record =
            |db: &PackageableElementPtr,
             out: &mut Vec<PackageableElementPtr>,
             seen: &mut std::collections::HashSet<smol_str::SmolStr>| {
                let key = ptr_fqn_key(db);
                if seen.insert(key) {
                    out.push(db.clone());
                }
            };
        // ~mainTable [db]
        if let Some(mt) = &self.main_table {
            record(&mt.db, &mut out, &mut seen);
        }
        // ~filter [db] / its `| [db]` chain
        if let Some(filter) = &self.filter {
            record(&filter.db, &mut out, &mut seen);
            if let Some(seq) = &filter.join_sequence {
                record(&seq.second_db, &mut out, &mut seen);
            }
        }
        // mappingElements: walk JoinColWithDbOrConstant.db + Otherwise db
        for elem in &self.mapping_elements {
            collect_db_refs_in_mapping_element(elem, &mut out, &mut seen);
        }
        // AssociationMapping body lines
        if let Some(lines) = &self.association_mapping {
            for line in lines {
                collect_db_refs_in_single_mapping_line(line, &mut out, &mut seen);
            }
        }
        // ~groupBy / ~primaryKey
        if let Some(jcs) = &self.group_by {
            for jc in jcs {
                if let Some(db) = &jc.db {
                    record(db, &mut out, &mut seen);
                }
            }
        }
        if let Some(jcs) = &self.primary_key {
            for jc in jcs {
                if let Some(db) = &jc.db {
                    record(db, &mut out, &mut seen);
                }
            }
        }
        out
    }
}

/// FQN key for the dedupe set inside `referenced_stores`.
fn ptr_fqn_key(p: &PackageableElementPtr) -> smol_str::SmolStr {
    let mut s = String::new();
    if let Some(pkg) = p.package.as_ref() {
        for seg in pkg.segments() {
            s.push_str(seg.as_str());
            s.push_str("::");
        }
    }
    s.push_str(p.name.as_str());
    smol_str::SmolStr::new(&s)
}

fn collect_db_refs_in_mapping_element(
    e: &MappingElement,
    out: &mut Vec<PackageableElementPtr>,
    seen: &mut std::collections::HashSet<smol_str::SmolStr>,
) {
    match e {
        MappingElement::Single(line) => {
            collect_db_refs_in_single_mapping_line(line, out, seen);
        }
        MappingElement::Scope(scope) => {
            // Scope's `[db]` is the contextual store for inner lines.
            let key = ptr_fqn_key(&scope.db);
            if seen.insert(key) {
                out.push(scope.db.clone());
            }
            for line in &scope.mapping_lines {
                collect_db_refs_in_single_mapping_line(line, out, seen);
            }
        }
    }
}

fn collect_db_refs_in_single_mapping_line(
    line: &SingleMappingLine,
    out: &mut Vec<PackageableElementPtr>,
    seen: &mut std::collections::HashSet<smol_str::SmolStr>,
) {
    match line {
        SingleMappingLine::Plus(p) => {
            collect_db_refs_in_join_col(&p.mapping.value, out, seen);
        }
        SingleMappingLine::NonePlus(np) => match &np.value {
            NonePlusMappingValue::Relational(rm) => {
                collect_db_refs_in_join_col(&rm.value, out, seen);
            }
            NonePlusMappingValue::Embedded(em) => {
                if let Some(jcs) = &em.primary_key {
                    for jc in jcs {
                        collect_db_refs_in_join_col(jc, out, seen);
                    }
                }
                for inner in &em.mapping_lines {
                    collect_db_refs_in_single_mapping_line(inner, out, seen);
                }
                if let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &em.trailer {
                    for m in maps {
                        if let Some(db) = &m.otherwise_join.db {
                            let key = ptr_fqn_key(db);
                            if seen.insert(key) {
                                out.push(db.clone());
                            }
                        }
                    }
                }
            }
        },
    }
}

fn collect_db_refs_in_join_col(
    jc: &JoinColWithDbOrConstant,
    out: &mut Vec<PackageableElementPtr>,
    seen: &mut std::collections::HashSet<smol_str::SmolStr>,
) {
    if let Some(db) = &jc.db {
        let key = ptr_fqn_key(db);
        if seen.insert(key) {
            out.push(db.clone());
        }
    }
    if let Some(seq) = &jc.join {
        for r in &seq.right {
            if let Some(db) = &r.db {
                let key = ptr_fqn_key(db);
                if seen.insert(key) {
                    out.push(db.clone());
                }
            }
        }
    }
    if let Some(OpColumn::Aliased { db: Some(db), .. }) = &jc.column {
        let key = ptr_fqn_key(db);
        if seen.insert(key) {
            out.push(db.clone());
        }
    }
}
