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
/// Two independently optional bindings:
///
/// - `tables_by_name` — the owning database's resolved-table map; used
///   to type explicit alias refs (`tradeTable.col`).
/// - `target_subject` — the implicit `{target}` subject table, used to
///   type `{target}.col` refs against the column's declared SQL type.
///   Plan-generation context decides what the subject is (e.g. the
///   class-mapping's main table for a MultiGrainFilter, the other
///   end of a Join chain, …). When unset, `{target}.col` defaults to
///   `Any` — the existing conservative behaviour.
///
/// Cross-db `[db]` qualifiers and include traversal of the table map
/// remain deferred — see module doc.
#[derive(Clone, Copy)]
pub struct OpTypeScope<'a> {
    /// Resolved tables visible directly on the owning database.
    /// `None` when the snapshot is missing (defensive — should not
    /// happen post-`define_bodies` but the typer must stay total).
    pub tables_by_name: Option<&'a HashMap<SmolStr, ResolvedTable>>,
    /// Implicit subject table bound to `{target}`. `None` at the
    /// Filter / Join / MultiGrainFilter declaration site (where the
    /// subject is not yet decided); set by downstream consumers
    /// (plan generation, class-mapping application) once the binding
    /// is known.
    pub target_subject: Option<&'a ResolvedTable>,
}

impl<'a> OpTypeScope<'a> {
    /// Build a scope from a `ResolvedDatabase`'s `tables_by_name`
    /// map. `{target}` stays unbound.
    #[must_use]
    pub fn from_tables(tables: &'a HashMap<SmolStr, ResolvedTable>) -> Self {
        Self {
            tables_by_name: Some(tables),
            target_subject: None,
        }
    }

    /// Empty scope — every column ref types as `Any`. Useful when
    /// the database's snapshot isn't available yet.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tables_by_name: None,
            target_subject: None,
        }
    }

    /// Augment a scope with an implicit `{target}` subject. The
    /// subject's columns become reachable through
    /// `OpColumn::Target { column, .. }` refs. Returns a new
    /// `OpTypeScope` value (the original is `Copy`).
    ///
    /// Java parity: at plan-generation time, the surrounding
    /// Filter / Join / MGF chain pins the subject before any SQL
    /// emission walks the body — `{target}` resolves to a definite
    /// table at that point.
    #[must_use]
    pub fn with_target_subject(mut self, subject: &'a ResolvedTable) -> Self {
        self.target_subject = Some(subject);
        self
    }
}

/// DynaFunctions whose Java relational metamodel return type is
/// `Boolean[1]`. Names mirror the canonical Java classifier
/// `meta::relational::functions::sqlQueryToString::isBooleanOperation`
/// at `legend-engine-xt-relationalStore-core-pure/.../sqlQueryToString/
/// dbExtension.pure:803` — the single source of truth for which
/// DynaFunctions resolve to a SQL predicate.
///
/// Conservative by design: widening this list can only relax
/// predicate-shape errors; it cannot introduce new failures. When
/// SQL emission lands (plan T1.4), this list will gain a sibling
/// table mapping each name to its SQL template — until then the
/// full 104-entry registry lives upstream at `getDynaFunctionToSqlDefault`
/// (extensionDefaults.pure:180).
pub const KNOWN_BOOLEAN_DYNAFUNCTIONS: &[&str] = &[
    // Logical
    "and",
    "or",
    "not",
    // Comparison
    "equal",
    "notEqual",
    "notEqualAnsi",
    "lessThan",
    "lessThanEqual",
    "greaterThan",
    "greaterThanEqual",
    // String predicates
    "startsWith",
    "endsWith",
    "contains",
    "matches",
    // Null + emptiness
    "isNull",
    "isNotNull",
    "isEmpty",
    "isNotEmpty",
    // Character-class
    "isAlphaNumeric",
    "isNumeric",
    // Set membership + existence
    "in",
    "exists",
    "isDistinct",
];

/// Walk an [`OpExpr`] and classify its top-level result.
///
/// - `Bool`, `Compare`, `IsNull` always return [`OpType::Boolean`].
/// - `Group` propagates its inner type.
/// - `Function`: [`OpType::Boolean`] iff the name is in
///   [`KNOWN_BOOLEAN_DYNAFUNCTIONS`]; otherwise [`OpType::Any`].
/// - `Column(Aliased)` resolves the column on the scope's table
///   map; missed lookups return [`OpType::Any`].
/// - `Column(Target)` resolves against `scope.target_subject` when
///   set; falls back to [`OpType::Any`] when the subject is unbound
///   (declaration-site context) or the column is not on the
///   subject (defensive — caller is expected to validate column
///   existence separately).
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
        OpExpr::Column(OpColumn::Target { column, .. }) => {
            // `{target}.col` types against the implicit subject when
            // the caller pinned one; otherwise the declaration-site
            // fallback to `Any` (no false positives on un-pinned
            // bodies).
            let Some(subject) = scope.target_subject else {
                return OpType::Any;
            };
            let Some(col) = subject.column(column.value.as_str()) else {
                return OpType::Any;
            };
            col.pure_type.map_or(OpType::Any, classify_column_type)
        }
        OpExpr::Array { .. } => OpType::Any,
        OpExpr::Literal(OpLiteral::String { .. }) => OpType::String,
        OpExpr::Literal(OpLiteral::Integer { .. } | OpLiteral::Float { .. }) => OpType::Numeric,
    }
}

#[allow(clippy::elidable_lifetime_names)] // explicit 'a links scope.tables_by_name to caller's borrow lifetime; elision drops the connection
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

    // -----------------------------------------------------------------
    // `{target}` structural tracking (T3.2): when the caller pins an
    // implicit subject, `{target}.col` types against the subject's
    // declared column SQL type. When no subject is pinned, the column
    // ref still defaults to `Any` (declaration-site safety net).
    // -----------------------------------------------------------------

    fn col(name: &str, pct: PureColumnType) -> crate::processor::ResolvedColumn {
        crate::processor::ResolvedColumn {
            name: SmolStr::new(name),
            source_type: SmolStr::new("<test>"),
            pure_type: Some(pct),
            size: None,
            scale: None,
            primary_key: false,
            not_null: false,
        }
    }

    fn subject(name: &str, cols: Vec<crate::processor::ResolvedColumn>) -> ResolvedTable {
        let columns_by_name: HashMap<SmolStr, usize> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        ResolvedTable {
            schema: None,
            name: SmolStr::new(name),
            columns: cols,
            columns_by_name,
            milestoning: None,
        }
    }

    fn target_col(name: &str) -> OpExpr {
        OpExpr::Column(OpColumn::Target {
            column: legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new(name),
                source_info: si(),
            },
            primary_key: false,
            source_info: si(),
        })
    }

    #[test]
    fn target_column_types_against_pinned_subject_string_column() {
        let t = subject("tradeTable", vec![col("name", PureColumnType::Varchar)]);
        let scope = OpTypeScope::empty().with_target_subject(&t);
        assert_eq!(infer_op_type(&target_col("name"), scope), OpType::String);
    }

    #[test]
    fn target_column_types_against_pinned_subject_numeric_column() {
        let t = subject(
            "tradeTable",
            vec![
                col("qty", PureColumnType::Integer),
                col("name", PureColumnType::Varchar),
            ],
        );
        let scope = OpTypeScope::empty().with_target_subject(&t);
        assert_eq!(infer_op_type(&target_col("qty"), scope), OpType::Numeric);
    }

    #[test]
    fn target_column_unknown_column_on_subject_falls_back_to_any() {
        // Mirror Java's behaviour: an unrecognised column name doesn't
        // produce a false-positive type — separate column-existence
        // validation owns that diagnostic. `infer_op_type` just stays
        // total and conservative.
        let t = subject("tradeTable", vec![col("qty", PureColumnType::Integer)]);
        let scope = OpTypeScope::empty().with_target_subject(&t);
        assert_eq!(
            infer_op_type(&target_col("not_a_column"), scope),
            OpType::Any
        );
    }

    #[test]
    fn target_column_without_subject_stays_any() {
        // Existing behaviour preserved when no subject is pinned —
        // ensures the typer can still be called from declaration-site
        // validators (which have no implicit subject) without
        // regression.
        let scope = OpTypeScope::empty();
        assert_eq!(infer_op_type(&target_col("month"), scope), OpType::Any);
    }

    #[test]
    fn aliased_column_unaffected_by_target_subject() {
        // Pinning a subject must not affect aliased refs — they
        // resolve via `tables_by_name`, not the subject. Belt-and-
        // braces: regression-protect the separation.
        let t = subject("tradeTable", vec![col("name", PureColumnType::Varchar)]);
        let scope = OpTypeScope::empty().with_target_subject(&t);
        // No tables_by_name → aliased lookup fails → Any.
        let aliased = OpExpr::Column(OpColumn::Aliased {
            db: None,
            alias: legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new("tradeTable"),
                source_info: si(),
            },
            scope: vec![legend_pure_parser_ast::annotation::SpannedString {
                value: SmolStr::new("name"),
                source_info: si(),
            }],
            primary_key: false,
            source_info: si(),
        });
        assert_eq!(infer_op_type(&aliased, scope), OpType::Any);
    }

    #[test]
    fn target_subject_drives_compare_predicate_inference() {
        // The motivating use case: `{target}.col = literal` types as
        // Boolean. The Compare wrapper already types as Boolean
        // unconditionally — but verify the subject lookup happens
        // inside the comparison's operand without panicking.
        use crate::ast::BinOp;
        let t = subject("tradeTable", vec![col("qty", PureColumnType::Integer)]);
        let scope = OpTypeScope::empty().with_target_subject(&t);
        let cmp = OpExpr::Compare {
            op: BinOp::Eq,
            lhs: Box::new(target_col("qty")),
            rhs: Box::new(lit_int(0)),
            source_info: si(),
        };
        assert_eq!(infer_op_type(&cmp, scope), OpType::Boolean);
    }
}
