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

//! Narrow return-type inference for relational op-expressions.
//!
//! The relational DSL parses Filter / Join / MultiGrainFilter bodies
//! into the structural [`OpExpr`] tree. This module classifies the
//! tree by its top-level **result kind** (`Boolean`, `Numeric`,
//! `String`, `Date`, `Any`) without translating to Pure functions —
//! enough to enforce the Java-parity invariant that a predicate
//! must return `Boolean[1]`.
//!
//! Full DynaFunction lowering (every relational pseudo-function
//! resolved to a Pure `FunctionApplication`) is deferred to the
//! engine-side integration (RT-1 / INT-1 in
//! `legend-engine-rust/docs/integration/`). Until that lands, we
//! keep a small allow-list of DynaFunctions whose result shape is
//! known to be `Boolean[1]` — anything else stays `Any` so we don't
//! false-positive on user code.

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::ast::{OpColumn, OpExpr, OpLiteral};
use crate::processor::{PureColumnType, ResolvedTable};

/// Classification of an op-expression's result.
///
/// Coarser than Pure's full type lattice — sufficient for the
/// predicate-return-type rule. Extend only when a new validator
/// needs to distinguish, say, `Numeric` from `Date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OpType {
    /// Boolean — and / or / comparisons / IS NULL / boolean-shaped
    /// DynaFunctions / boolean column refs.
    Boolean,
    /// Integer / Float / Decimal / Numeric / BigInt / SmallInt /
    /// TinyInt / Real / Bit column refs and integer / float
    /// literals.
    Numeric,
    /// `Char` / `Varchar` column refs and string literals.
    String,
    /// `Date` / `Timestamp` column refs.
    Date,
    /// Unknown — used for unresolved column refs, unrecognised
    /// DynaFunctions, `{target}.col` (no scope binding), and
    /// `Array` literals. The predicate validator accepts `Any` as a
    /// no-op so unresolved shapes don't produce false positives.
    Any,
}

/// Lookup scope passed to [`infer_op_type`].
///
/// Today carries only the owning database's resolved-table map
/// (used to type column refs by their declared SQL type). Cross-db
/// `[db]` qualifiers, include traversal, and `{target}` binding are
/// deferred — see module doc.
#[derive(Clone, Copy)]
pub struct OpTypeScope<'a> {
    /// Resolved tables visible directly on the owning database.
    /// `None` when the snapshot is missing (defensive — should not
    /// happen post-`define_bodies` but the typer must stay total).
    pub tables_by_name: Option<&'a HashMap<SmolStr, ResolvedTable>>,
}

impl<'a> OpTypeScope<'a> {
    /// Build a scope from a `ResolvedDatabase`'s `tables_by_name`
    /// map.
    #[must_use]
    pub fn from_tables(tables: &'a HashMap<SmolStr, ResolvedTable>) -> Self {
        Self {
            tables_by_name: Some(tables),
        }
    }

    /// Empty scope — every column ref types as `Any`. Useful when
    /// the database's snapshot isn't available yet.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tables_by_name: None,
        }
    }
}

/// DynaFunctions whose Java relational metamodel return type is
/// `Boolean[1]`. Names mirror the Java `DynaFunctionRegistration`
/// allow-list. Anything not in this list types as [`OpType::Any`].
///
/// Sourced from Java's relational `DynaFunctionRegistration` /
/// `RelationalOperationElement` boolean-shape entries. Conservative
/// by design — adding a new DynaFunction here only widens the set
/// of predicates we'll accept clean; it cannot accidentally produce
/// new errors.
pub const KNOWN_BOOLEAN_DYNAFUNCTIONS: &[&str] = &[
    "and",
    "or",
    "not",
    "equal",
    "lessThan",
    "lessThanEqual",
    "greaterThan",
    "greaterThanEqual",
    "notEqual",
    "isNull",
    "isNotNull",
    "in",
    "notIn",
    "like",
    "exists",
    "matchExpression",
    "regexpLike",
    "between",
];

/// Walk an [`OpExpr`] and classify its top-level result.
///
/// - `Bool`, `Compare`, `IsNull` always return [`OpType::Boolean`].
/// - `Group` propagates its inner type.
/// - `Function`: [`OpType::Boolean`] iff the name is in
///   [`KNOWN_BOOLEAN_DYNAFUNCTIONS`]; otherwise [`OpType::Any`].
/// - `Column(Aliased)` resolves the column on the scope's table
///   map; missed lookups return [`OpType::Any`].
/// - `Column(Target)`: [`OpType::Any`] — `{target}` binding is
///   context-dependent and not threaded today (out of scope).
/// - `Literal(String|Integer|Float)`: matching primitive type.
/// - `Array`: [`OpType::Any`] (rarely sits at top-level; collection
///   semantics belong to the surrounding `in(...)` shape).
#[must_use]
pub fn infer_op_type(expr: &OpExpr, scope: OpTypeScope<'_>) -> OpType {
    match expr {
        OpExpr::Bool { .. } | OpExpr::Compare { .. } | OpExpr::IsNull { .. } => OpType::Boolean,
        OpExpr::Group { inner, .. } => infer_op_type(inner, scope),
        OpExpr::Function { name, .. } => {
            if KNOWN_BOOLEAN_DYNAFUNCTIONS
                .iter()
                .any(|n| *n == name.value.as_str())
            {
                OpType::Boolean
            } else {
                OpType::Any
            }
        }
        OpExpr::Column(OpColumn::Aliased {
            alias, scope: cols, ..
        }) => {
            let Some(tables) = scope_tables(scope) else {
                return OpType::Any;
            };
            let Some(table) = tables.get(&alias.value) else {
                return OpType::Any;
            };
            // Only `alias.column` shape (a single scope segment) is
            // typed today. Bare alias refs ("the whole row") don't
            // appear at top-level of a predicate in practice; for
            // safety they default to `Any`.
            let Some(col_seg) = cols.first() else {
                return OpType::Any;
            };
            let Some(col) = table.column(col_seg.value.as_str()) else {
                return OpType::Any;
            };
            col.pure_type.map_or(OpType::Any, classify_column_type)
        }
        OpExpr::Column(OpColumn::Target { .. }) => OpType::Any,
        OpExpr::Literal(OpLiteral::String { .. }) => OpType::String,
        OpExpr::Literal(OpLiteral::Integer { .. } | OpLiteral::Float { .. }) => OpType::Numeric,
        OpExpr::Array { .. } => OpType::Any,
    }
}

fn scope_tables<'a>(scope: OpTypeScope<'a>) -> Option<&'a HashMap<SmolStr, ResolvedTable>> {
    scope.tables_by_name
}

fn classify_column_type(pct: PureColumnType) -> OpType {
    match pct {
        PureColumnType::Bit => OpType::Boolean,
        PureColumnType::Float
        | PureColumnType::Double
        | PureColumnType::Integer
        | PureColumnType::BigInt
        | PureColumnType::SmallInt
        | PureColumnType::TinyInt
        | PureColumnType::Decimal
        | PureColumnType::Numeric
        | PureColumnType::Real => OpType::Numeric,
        PureColumnType::Char | PureColumnType::Varchar => OpType::String,
        PureColumnType::Date | PureColumnType::Timestamp => OpType::Date,
        PureColumnType::Binary
        | PureColumnType::Varbinary
        | PureColumnType::Distinct
        | PureColumnType::Other
        | PureColumnType::Array
        | PureColumnType::Json
        | PureColumnType::SemiStructured => OpType::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinOp;
    use legend_pure_parser_ast::source_info::SourceInfo;

    fn si() -> SourceInfo {
        SourceInfo::new("<test>", 0, 0, 0, 0)
    }

    fn lit_int(n: i64) -> OpExpr {
        OpExpr::Literal(OpLiteral::Integer {
            value: n,
            source_info: si(),
        })
    }

    fn lit_str(s: &str) -> OpExpr {
        OpExpr::Literal(OpLiteral::String {
            value: SmolStr::new(s),
            source_info: si(),
        })
    }

    #[test]
    fn compare_is_boolean() {
        let expr = OpExpr::Compare {
            op: BinOp::Eq,
            lhs: Box::new(lit_int(1)),
            rhs: Box::new(lit_int(2)),
            source_info: si(),
        };
        assert_eq!(infer_op_type(&expr, OpTypeScope::empty()), OpType::Boolean);
    }

    #[test]
    fn string_literal_is_string() {
        assert_eq!(
            infer_op_type(&lit_str("hi"), OpTypeScope::empty()),
            OpType::String
        );
    }

    #[test]
    fn integer_literal_is_numeric() {
        assert_eq!(
            infer_op_type(&lit_int(0), OpTypeScope::empty()),
            OpType::Numeric
        );
    }

    #[test]
    fn unknown_dynafunction_is_any() {
        let expr = OpExpr::Function {
            db: None,
            name: legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new("madeUpFn"),
                source_info: si(),
            },
            args: vec![],
            source_info: si(),
        };
        assert_eq!(infer_op_type(&expr, OpTypeScope::empty()), OpType::Any);
    }

    #[test]
    fn known_boolean_dynafunction_is_boolean() {
        let expr = OpExpr::Function {
            db: None,
            name: legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new("equal"),
                source_info: si(),
            },
            args: vec![],
            source_info: si(),
        };
        assert_eq!(infer_op_type(&expr, OpTypeScope::empty()), OpType::Boolean);
    }

    #[test]
    fn target_column_is_any() {
        let expr = OpExpr::Column(OpColumn::Target {
            column: legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new("month"),
                source_info: si(),
            },
            primary_key: false,
            source_info: si(),
        });
        assert_eq!(infer_op_type(&expr, OpTypeScope::empty()), OpType::Any);
    }
}
