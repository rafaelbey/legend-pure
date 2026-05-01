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

//! TDS AST.
//!
//! [`TDSExpr`] is the root content node implementing
//! [`IslandContent`]. It captures the syntactic shape only — type
//! resolution and CSV typing happen later in the [`crate::compiler`]
//! pass (commit #14).

use std::any::Any;

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

/// The island tag this DSL claims.
pub const TAG: &str = "TDS";

/// `#TDS\n cols\n rows\n#`.
///
/// Header line is parsed into [`TDSColumn`]s; subsequent lines are
/// parsed into row vectors of [`TDSCell`]. Cell values keep their
/// raw token text — typing and CSV parsing happen at compile time.
#[derive(Debug, Clone, PartialEq)]
pub struct TDSExpr {
    /// Column specifications from the header line.
    pub columns: Vec<TDSColumn>,
    /// Data rows. Each row's length should match `columns.len()`,
    /// but the parser does not enforce this — that check belongs to
    /// the compiler extension.
    pub rows: Vec<Vec<TDSCell>>,
    /// Source location of the entire `#TDS … #` island.
    pub source_info: SourceInfo,
}

/// One column specifier from the header line.
///
/// Three shapes the parser accepts:
///
/// - `name`            — bare identifier, no type or multiplicity.
/// - `name:Type`       — typed column without multiplicity.
/// - `name:Type[mult]` — typed column with multiplicity.
///
/// `name` may also be a single-quoted string literal (`'col with
/// space'`), in which case the leading/trailing quotes are stripped.
#[derive(Debug, Clone, PartialEq)]
pub struct TDSColumn {
    /// Column name (unquoted).
    pub name: SmolStr,
    /// Optional type reference. `None` means "untyped" — the
    /// compiler extension may infer from data, matching Java's
    /// behaviour.
    pub type_ref: Option<TDSColumnType>,
    /// Source location of the column spec.
    pub source_info: SourceInfo,
}

/// A typed column annotation: `Type` or `Type[mult]`.
#[derive(Debug, Clone, PartialEq)]
pub struct TDSColumnType {
    /// Qualified type name as a `::`-joined FQN string, e.g.
    /// `"Integer"`, `"meta::pure::Float"`, `"Numeric(10,4)"`.
    /// Type-parameter / arity arguments are kept in the raw string
    /// for later parsing by the compiler extension.
    pub name: SmolStr,
    /// Optional multiplicity (`[1]`, `[0..1]`, `[*]`, `[0..*]`,
    /// etc.). Stored as the raw bracket contents (no surrounding
    /// brackets) for the compiler extension to parse.
    pub multiplicity: Option<SmolStr>,
    /// Source location of the type annotation.
    pub source_info: SourceInfo,
}

/// One cell value in a data row. Raw token text — typing happens
/// later when the column type is known.
#[derive(Debug, Clone, PartialEq)]
pub struct TDSCell {
    /// Raw token text. May be a literal (`"42"`, `"'foo'"`,
    /// `"true"`) or a bare identifier (`"A"`).
    pub raw: SmolStr,
    /// Source location of the cell token.
    pub source_info: SourceInfo,
}

impl IslandContent for TDSExpr {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn IslandContent> {
        Box::new(self.clone())
    }

    fn eq_content(&self, other: &dyn IslandContent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}
