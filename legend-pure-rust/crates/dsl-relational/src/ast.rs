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
//! body — `op_operation` in the Java grammar — is captured as an
//! opaque [`TokenSlice`] for round-trip; structural sub-grammar
//! parsing (binary / unary / variadic / DynaFunction / TableAliasColumn)
//! arrives in Stage 2. View bodies (filter / groupBy / distinct /
//! columnMappings) are captured the same way.
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

/// A `Table` declaration: `Table name ( <columns> )`.
///
/// Stage 1 ships only the column-list form. The optional
/// `milestoning ( … )` block defined in the Java grammar
/// (`milestoneSpec`) lands in Stage 3 — the parser currently rejects
/// any Table whose body opens with the `milestoning` keyword.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Table name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Column definitions, in source order.
    pub columns: Vec<ColumnDef>,
    /// Span of the entire `Table … ( … )` declaration.
    pub source_info: SourceInfo,
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
/// Stage 1 captures the `op_operation` body verbatim; Stage 2 swaps
/// `op_body` for a structural [`Operation`] sub-tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    /// Join name (relational identifier — bare or quoted).
    pub name: SpannedString,
    /// Verbatim token slice for the operation body (`{target}.x =
    /// other.y`, etc.).
    pub op_body: TokenSlice,
    /// Span of the entire `Join … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `Filter` declaration: `Filter name ( <op_operation> )`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    /// Filter name.
    pub name: SpannedString,
    /// Verbatim token slice for the operation body.
    pub op_body: TokenSlice,
    /// Span of the entire `Filter … ( … )` declaration.
    pub source_info: SourceInfo,
}

/// A `MultiGrainFilter` declaration: a Filter that the Java
/// processor flags for grain-aware joins.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiGrainFilter {
    /// MultiGrainFilter name.
    pub name: SpannedString,
    /// Verbatim token slice for the operation body.
    pub op_body: TokenSlice,
    /// Span of the entire `MultiGrainFilter … ( … )` declaration.
    pub source_info: SourceInfo,
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
