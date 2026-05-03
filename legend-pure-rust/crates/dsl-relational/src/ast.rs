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
//! (filter / groupBy / distinct / columnMappings) remain a
//! [`TokenSlice`] until Stage 5.
//!
//! What's intentionally absent from Stage 1:
//! - Stereotypes / tagged values on Database / Schema / Table / Column
//!   (Java grammar accepts them; deferred).
//! - Milestoning specs on Tables — Stage 3.
//! - The mapping-island grammar (`Class : Relational { … }` inside
//!   `###Mapping`) — Stages 5–7.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::{Annotated, PackageableElement};
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::{Identifier, Package};
use legend_pure_parser_lexer::Token;
use smol_str::SmolStr;

/// Section kind string this DSL claims (`###Relational`).
pub const SECTION_KIND: &str = "Relational";

// ---------------------------------------------------------------------------
// DatabaseDef
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// DatabaseInclude
// ---------------------------------------------------------------------------

/// `include pkg::OtherDb` — pulls another database's schemas / tables /
/// joins / filters into this one.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseInclude {
    /// FQN of the included database.
    pub included: PackageableElementPtr,
    /// Span of the entire `include …` directive.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// DatabaseElement
// ---------------------------------------------------------------------------

/// One element inside a [`DatabaseDef`] body, in source order.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// A `View` declaration: `View name ( <view-body> )`.
///
/// Stage 1 captures the body as an opaque [`TokenSlice`] — the
/// internal `(filterViewBlock)? (mappingBlockGroupBy)? (DISTINCTCMD)?
/// viewColumnMappingLines` shape is structurally parsed in a later
/// stage. Round-trip composition replays the captured tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// View name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Verbatim token slice for the view body. Composed back as
    /// space-separated token texts by [`compose`](crate::compose).
    pub body: TokenSlice,
    /// Span of the entire `View … ( … )` declaration.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Join / Filter / MultiGrainFilter
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// op_operation — structured Join / Filter / MultiGrainFilter body
// ---------------------------------------------------------------------------

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
            | OpExpr::Function { source_info, .. } => source_info,
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

// ---------------------------------------------------------------------------
// TokenSlice — Stage 1 verbatim capture
// ---------------------------------------------------------------------------

/// A run of tokens captured between matching parens, used by
/// Stage 1 for op-bodies and view bodies that haven't been
/// structurally parsed yet. Preserves token text + kind so the
/// composer can re-emit them; structural parsing in later stages
/// replaces these slices with proper AST nodes.
///
/// `Vec<Token>` is heavy (each Token carries `SmolStr` + SourceInfo),
/// but Stage 1 fixtures keep these slices short (a handful of tokens
/// per join / filter), and the slice goes away in Stage 2.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSlice {
    /// Captured tokens, in source order. Excludes the surrounding
    /// outer parens (the parser consumes those before / after the
    /// capture).
    pub tokens: Vec<Token>,
    /// Span covering the slice, useful for diagnostics that point
    /// inside the body before structural parsing lands.
    pub source_info: SourceInfo,
}

impl TokenSlice {
    /// Convenience: render the captured tokens as a single
    /// space-separated string. Used by the composer's round-trip
    /// path; the output isn't byte-for-byte identical to the
    /// original source (whitespace collapses to single spaces) but
    /// re-parses to the same token sequence per the
    /// `feedback_token_survival_round_trip` rule.
    #[must_use]
    pub fn render_with_spaces(&self) -> String {
        let mut out = String::new();
        for (i, t) in self.tokens.iter().enumerate() {
            if i > 0 && needs_space_between(&self.tokens[i - 1], t) {
                out.push(' ');
            }
            out.push_str(&t.text);
        }
        out
    }
}

/// Decide whether two adjacent tokens in a [`TokenSlice`] need a
/// separating space when emitted by the composer. The default policy
/// is "yes, unless one side is a punctuator that conventionally
/// hugs its neighbour". Keeps round-trip output compact and avoids
/// glueing identifiers like `Table.col` apart.
fn needs_space_between(prev: &Token, next: &Token) -> bool {
    use legend_pure_parser_lexer::TokenKind;
    let glue_left = matches!(
        prev.kind,
        TokenKind::Dot | TokenKind::LParen | TokenKind::LBracket
    );
    let glue_right = matches!(
        next.kind,
        TokenKind::Dot
            | TokenKind::Comma
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::Semicolon
    );
    !(glue_left || glue_right)
}

// Validators in later stages walk `db.elements` directly to find
// tables / views / joins / filters; keeping the Stage 1 surface lean
// avoids over-engineering iterator helpers nobody uses yet.
