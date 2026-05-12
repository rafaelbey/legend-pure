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

//! [`CompilerExtension`] for the TDS inline-island.
//!
//! Like dsl-store, TDS islands are *expression-level* (not section-
//! level), so the extension walks function bodies in `define_bodies`
//! and validates each `#TDS … #` it finds:
//!
//! 1. Each column's row count must match `columns.len()` (per-row
//!    arity).
//! 2. If the column has a `type_ref`, that type's qualified name
//!    must resolve to a Class, `PrimitiveType`, or Enumeration in the
//!    [`PureModel`].
//!
//! Lowering of the island into a real [`PureModel`] value is
//! deferred — the core lowering pass still reports
//! `UnsupportedExpression { kind: "Island" }`. This commit
//! implements *validation* only.

use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::expression::{Expression, MemberAccess};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DefineCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element as ModelElement, PureModel};
use smol_str::SmolStr;

use crate::ast::TDSExpr;
use crate::csv;
use crate::lower::{column_overrides, reconstruct_csv};

/// Compiler extension for the TDS inline-island DSL.
#[derive(Default)]
pub struct TDSExtension;

impl TDSExtension {
    /// Construct a fresh extension.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CompilerExtension for TDSExtension {
    fn name(&self) -> &'static str {
        "dsl-tds"
    }

    fn define_bodies(&self, ctx: &mut DefineCtx<'_>) {
        for source_file in ctx.source_files {
            walk_source_file(source_file, ctx.model, ctx.errors);
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

fn resolve_fqn(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    model.resolve_by_path(&segments)
}

fn is_type(model: &PureModel, fqn: &str) -> bool {
    // Strip any `(args)` suffix (e.g. `Numeric(10,4)`) — the inner
    // arity arguments aren't part of the resolvable type name.
    let bare = fqn.split('(').next().unwrap_or(fqn);
    resolve_fqn(model, bare).is_some_and(|id| {
        matches!(
            model.get_element(id),
            ModelElement::Class(_) | ModelElement::PrimitiveType(_) | ModelElement::Enumeration(_)
        )
    })
}

// ---------------------------------------------------------------------------
// Source-file traversal
// ---------------------------------------------------------------------------

fn walk_source_file(sf: &SourceFile, model: &PureModel, errors: &mut Vec<CompilationError>) {
    for elem in sf.all_elements() {
        if let AstElement::Function(f) = elem {
            for expr in &f.body {
                walk_expression(expr, model, errors);
            }
        }
    }
}

fn walk_expression(expr: &Expression, model: &PureModel, errors: &mut Vec<CompilationError>) {
    match expr {
        Expression::Island(isl) => validate_island(isl, model, errors),
        Expression::FunctionApplication(app) => {
            for arg in &app.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::ArrowFunction(arrow) => {
            walk_expression(&arrow.target, model, errors);
            for arg in &arrow.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::MemberAccess(MemberAccess::Simple(m)) => {
            walk_expression(&m.target, model, errors);
        }
        Expression::MemberAccess(MemberAccess::Qualified(m)) => {
            walk_expression(&m.target, model, errors);
            for arg in &m.arguments {
                walk_expression(arg, model, errors);
            }
        }
        Expression::Lambda(lam) => {
            for e in &lam.body {
                walk_expression(e, model, errors);
            }
        }
        Expression::Let(l) => walk_expression(&l.value, model, errors),
        Expression::Collection(c) => {
            for e in &c.elements {
                walk_expression(e, model, errors);
            }
        }
        Expression::Slice(s) => {
            if let Some(start) = &s.start {
                walk_expression(start, model, errors);
            }
            walk_expression(&s.stop, model, errors);
            if let Some(step) = &s.step {
                walk_expression(step, model, errors);
            }
        }
        Expression::Group(inner) => walk_expression(inner, model, errors),
        Expression::Arithmetic(a) => {
            walk_expression(&a.left, model, errors);
            walk_expression(&a.right, model, errors);
        }
        Expression::Comparison(c) => {
            walk_expression(&c.left, model, errors);
            walk_expression(&c.right, model, errors);
        }
        Expression::Logical(l) => {
            walk_expression(&l.left, model, errors);
            walk_expression(&l.right, model, errors);
        }
        Expression::Bitwise(b) => {
            walk_expression(&b.left, model, errors);
            walk_expression(&b.right, model, errors);
        }
        Expression::Not(n) => walk_expression(&n.operand, model, errors),
        Expression::UnaryMinus(u) => walk_expression(&u.operand, model, errors),
        Expression::BitwiseNot(b) => walk_expression(&b.operand, model, errors),
        Expression::Copy(c) => {
            for kv in &c.assignments {
                walk_expression(&kv.value, model, errors);
            }
        }
        Expression::NavigationPath(nav) => {
            for step in &nav.path {
                for arg in &step.parameters {
                    walk_expression(arg, model, errors);
                }
            }
        }
        Expression::NewInstance(_)
        | Expression::Column(_)
        | Expression::PackageableElementRef(_)
        | Expression::TypeReferenceExpr(_)
        | Expression::MultiplicityReferenceExpr(_)
        | Expression::UnitInstance(_)
        | Expression::Variable(_)
        | Expression::Literal(_) => {}
    }
}

fn validate_island(isl: &IslandExpression, model: &PureModel, errors: &mut Vec<CompilationError>) {
    let Some(tds) = isl.content.as_any().downcast_ref::<TDSExpr>() else {
        return;
    };

    // Per-row arity check: each row must have exactly columns.len() cells.
    let expected = tds.columns.len();
    for (idx, row) in tds.rows.iter().enumerate() {
        if row.len() != expected {
            errors.push(CompilationError {
                message: format!(
                    "TDS row {} has {} cell(s); expected {} (one per declared column)",
                    idx + 1,
                    row.len(),
                    expected
                ),
                source_info: tds.source_info.clone(),
                kind: CompilationErrorKind::UnsupportedExpression {
                    kind: SmolStr::new_static("TDS"),
                },
            });
        }
    }

    // Type-resolution check: every column with an explicit type_ref
    // must resolve to a Type-like element.
    for col in &tds.columns {
        if let Some(ty) = &col.type_ref
            && !is_type(model, ty.name.as_str())
        {
            errors.push(CompilationError {
                message: format!(
                    "TDS column '{}' references unresolved type '{}'",
                    col.name, ty.name
                ),
                source_info: ty.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: ty.name.clone(),
                },
            });
        }
    }

    // CSV-level validation: run the same shared parse the lowerer
    // uses, with the column overrides derived from explicit
    // `name:Type[mult]` annotations. Surfaces:
    //
    // - Type mismatch — a non-empty cell that doesn't parse against
    //   the declared primitive type (e.g. `v:Integer` with cell
    //   `"hello"`).
    // - Multiplicity mismatch — an empty cell under a column declared
    //   `[1]` / `[1..*]` / `[1..n]`.
    //
    // Cell-level errors are positioned at the offending cell when we
    // can identify it; otherwise they fall back to the island span.
    // Skips when the per-row arity check above has already fired,
    // since `parse_and_infer` would just re-report that.
    let arity_clean = tds.rows.iter().all(|row| row.len() == tds.columns.len());
    if arity_clean {
        let canonical_csv = reconstruct_csv(tds);
        let overrides = column_overrides(&tds.columns);
        if let Err(e) = csv::parse_and_infer(&canonical_csv, &overrides) {
            // The CSV's line numbering: header on line 1, data row N
            // on line N+1. Map back to the offending TDS cell's
            // source_info when we have it.
            let target_si = if e.line >= 2 {
                let row_idx = e.line - 2;
                let col_idx = e.column.saturating_sub(1);
                tds.rows
                    .get(row_idx)
                    .and_then(|r| r.get(col_idx))
                    .map_or_else(|| tds.source_info.clone(), |c| c.source_info.clone())
            } else {
                tds.source_info.clone()
            };
            errors.push(CompilationError {
                message: e.message,
                source_info: target_si,
                kind: CompilationErrorKind::UnsupportedExpression {
                    kind: SmolStr::new_static("TDS"),
                },
            });
        }
    }
}
