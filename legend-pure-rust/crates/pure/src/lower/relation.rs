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

//! Relation column / column-spec lowering: `~name`, `~name:Type[m]`,
//! `~[col, …]`, and the `@(cols)` relation-type literal.
//!
//! Step 4 of the lowering encapsulation plan. Owns:
//!
//! - [`lower_column`] — `~`-prefixed column-builder dispatch into
//!   `ColSpecLiteral` (single-column) or `ColSpecArrayLiteral` (array
//!   form) with the platform's three column-shape kinds (Plain / Func
//!   / Agg) inferred from each column's payload.
//! - [`classify_col_spec_kind`] — payload classifier mirroring the
//!   platform's `colSpec` vs `funcColSpec` vs `aggColSpec` grammar
//!   dispatch.
//! - [`lower_relation_columns`] — re-exposed for the `@(cols)`
//!   relation-type literal path in `lower/type_ref.rs`.
//! - [`lower_relation_columns_from_specs`] — `~[…]` array form, used
//!   by `lower_column`.
//! - The four `resolve_*_id` helpers (`RelationType`, `ColSpec`,
//!   `ColSpecArray`, `FuncColSpec`/`AggColSpec`/array variants) that
//!   look up the M3 metaclass IDs from `meta::pure::metamodel::relation`.
//!
//! `resolve_relation_type_id` and `lower_relation_columns` stay
//! `pub(super)` because `lower/type_ref.rs` reaches in for the
//! `@(cols)` relation-type-literal path.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::type_ref as ast_type;
use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::resolve::{self, ResolutionContext};
use crate::types::{
    ExprKind, Multiplicity, RelationColumnLowered, ResolvedType, TypeExpr, ValueSpec,
};

use super::typed;

/// Lowers `~name` / `~name:Type[mult]` / `~[name:Type[mult], …]` to
/// either a `ColSpecLiteral` (single-column form, `is_array=false`)
/// or a `ColSpecArrayLiteral` (array form, `is_array=true`).
///
/// Mirrors the platform's `PCT.grammarCharacters` annotations on
/// `meta::pure::functions::relation::colSpec` (`'~'`) vs
/// `colSpecArray` (`'~[,]'`). The two shapes have distinct M3
/// classifiers (`ColSpec` vs `ColSpecArray`) and different reflective
/// slots (`name:String[1]` vs `names:String[*]`), so downstream
/// overload narrowing — e.g. `ascending(column:ColSpec<T>[1])` —
/// fails to resolve when a single `~name` is mis-lowered to
/// `ColSpecArray`.
///
/// Captures column triples (`name`, resolved `type_element`,
/// `multiplicity`) at lowering time so the runtime allocator can
/// materialise the heap shape without re-resolving names.
///
/// Lambda-bearing `~name:x|$x+1` columns and the `funcColSpec*` /
/// `aggColSpec*` shapes are not yet wired through this lowerer —
/// those columns drop their lambda payload here. Add a follow-up if
/// a subsequent test forces them.
pub(super) fn lower_column(
    e: &ast_expr::ColumnBuilderExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ValueSpec> {
    let columns = lower_relation_columns_from_specs(&e.columns, ctx, errors);
    let kind = classify_col_spec_kind(&e.columns);
    let outer_id = match (e.is_array, kind) {
        (false, crate::types::ColSpecLiteralKind::Plain) => resolve_col_spec_id(ctx)?,
        (false, crate::types::ColSpecLiteralKind::Func) => {
            resolve_relation_class_id(ctx, "FuncColSpec")?
        }
        (false, crate::types::ColSpecLiteralKind::Agg) => {
            resolve_relation_class_id(ctx, "AggColSpec")?
        }
        (true, crate::types::ColSpecLiteralKind::Plain) => resolve_col_spec_array_id(ctx)?,
        (true, crate::types::ColSpecLiteralKind::Func) => {
            resolve_relation_class_id(ctx, "FuncColSpecArray")?
        }
        (true, crate::types::ColSpecLiteralKind::Agg) => {
            resolve_relation_class_id(ctx, "AggColSpecArray")?
        }
    };
    let type_expr = TypeExpr::Named {
        element: outer_id,
        type_arguments: vec![],
        multiplicity_arguments: Vec::new(),
        value_arguments: vec![],
        source_info: None,
    };
    if e.is_array {
        return Some(typed(
            ExprKind::ColSpecArrayLiteral { columns, kind },
            e.source_info.clone(),
            ResolvedType {
                type_expr,
                multiplicity: Multiplicity::PureOne,
            },
        ));
    }
    // Single-column form: take the first lowered column or synthesise
    // an `Any` placeholder if the lowerer dropped it (e.g. lambda
    // form, untyped name).
    let column = columns.into_iter().next().unwrap_or_else(|| {
        let name = e
            .columns
            .first()
            .map_or_else(|| SmolStr::new_static(""), |c| c.name.clone());
        RelationColumnLowered {
            name,
            type_element: crate::bootstrap::ANY_ID,
            multiplicity: Multiplicity::ZeroOrOne,
            init_lambda: None,
        }
    });
    Some(typed(
        ExprKind::ColSpecLiteral { column, kind },
        e.source_info.clone(),
        ResolvedType {
            type_expr,
            multiplicity: Multiplicity::PureOne,
        },
    ))
}

/// Classify a column-builder's columns into the three platform
/// shapes — plain (no lambda), `func` (one init lambda per column),
/// `agg` (init + reduce lambdas per column). Mixed shapes fall back
/// to `Plain` so the lowerer doesn't pretend to know which classifier
/// to pick. Mirrors the platform's
/// `colSpec` vs `funcColSpec` vs `aggColSpec` grammar dispatch.
fn classify_col_spec_kind(specs: &[ast_expr::ColumnSpec]) -> crate::types::ColSpecLiteralKind {
    use crate::types::ColSpecLiteralKind;
    if specs.is_empty() {
        return ColSpecLiteralKind::Plain;
    }
    let mut has_init = true;
    let mut has_agg = true;
    for c in specs {
        let is_lambda_init = matches!(&c.type_spec, Some(ast_expr::ColumnTypeSpec::Lambda(_)));
        if !is_lambda_init {
            has_init = false;
            has_agg = false;
            continue;
        }
        if c.extra_function.is_none() {
            has_agg = false;
        }
    }
    if has_agg {
        ColSpecLiteralKind::Agg
    } else if has_init {
        ColSpecLiteralKind::Func
    } else {
        ColSpecLiteralKind::Plain
    }
}

/// Resolves AST `RelationColumn`s into the lowered triple form. Used by
/// the `RelationLiteral` lowering path (`@(cols)`).
pub(super) fn lower_relation_columns(
    cols: &[ast_type::RelationColumn],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<RelationColumnLowered> {
    cols.iter()
        .filter_map(|c| {
            let type_expr = resolve::resolve_type_ref(&c.type_ref, ctx, errors)?;
            let TypeExpr::Named {
                element: type_element,
                ..
            } = type_expr
            else {
                return None;
            };
            let multiplicity = c
                .multiplicity
                .as_ref()
                .map_or(Multiplicity::ZeroOrOne, resolve::lower_multiplicity);
            Some(RelationColumnLowered {
                name: c.name.clone(),
                type_element,
                multiplicity,
                init_lambda: None,
            })
        })
        .collect()
}

/// Resolves AST `ColumnSpec`s into the lowered triple form. Used by the
/// `ColSpecArrayLiteral` lowering path (`~[cols]`).
///
/// Each input becomes a `RelationColumnLowered` regardless of shape:
///
/// - `name:Type[mult]` — typed column. Type resolves; multiplicity
///   parsed if present, else `[0..1]`.
/// - `name:lam|...` (lambda-bearing — `Func`/`Agg` `ColSpecLiteralKind`)
///   — synthesise a placeholder `(Any, [0..1])` for type/multiplicity
///   and lower the init lambda into `init_lambda` so the runtime
///   allocator can populate the `FuncColSpec.function` slot. Lambda
///   parameters with no declared type get `Any[1]` expectations — the
///   runtime evaluates `$x.col` via dynamic slot lookup on the
///   row-tuple heap object the per-row binding produces.
/// - `name` (bare) — placeholder type, no lambda.
fn lower_relation_columns_from_specs(
    cols: &[ast_expr::ColumnSpec],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<RelationColumnLowered> {
    cols.iter()
        .map(|c| {
            // Typed column → resolve and use as-is. No init lambda.
            if let Some(ast_expr::ColumnTypeSpec::Typed(type_ref, mult)) = &c.type_spec {
                let resolved = resolve::resolve_type_ref(type_ref, ctx, errors);
                if let Some(TypeExpr::Named {
                    element: type_element,
                    ..
                }) = resolved
                {
                    let multiplicity = mult
                        .as_ref()
                        .map_or(Multiplicity::ZeroOrOne, resolve::lower_multiplicity);
                    return RelationColumnLowered {
                        name: c.name.clone(),
                        type_element,
                        multiplicity,
                        init_lambda: None,
                    };
                }
                // Type didn't resolve to Named — fall through to the
                // placeholder so the column name survives.
            }
            // Lambda-bearing column → lower the init lambda with
            // synthetic Any-typed parameters. The runtime binds row
            // values dynamically via slot lookup on `$x.<col>`, so
            // compile-time inference for the param doesn't need to be
            // precise. Bare-name columns (no type_spec) skip the
            // lambda branch.
            let init_lambda = match &c.type_spec {
                Some(ast_expr::ColumnTypeSpec::Lambda(l)) => {
                    let any_param: (TypeExpr, Multiplicity) = (
                        TypeExpr::Named {
                            element: crate::bootstrap::ANY_ID,
                            type_arguments: vec![],
                            value_arguments: vec![],
                            multiplicity_arguments: vec![],
                            source_info: None,
                        },
                        Multiplicity::PureOne,
                    );
                    let expected: Vec<Option<(TypeExpr, Multiplicity)>> =
                        l.parameters.iter().map(|_| Some(any_param.clone())).collect();
                    super::lambda::lower_lambda_with_expected_types(
                        l,
                        Some(&expected),
                        ctx,
                        errors,
                    )
                }
                _ => None,
            };
            RelationColumnLowered {
                name: c.name.clone(),
                type_element: crate::bootstrap::ANY_ID,
                multiplicity: Multiplicity::ZeroOrOne,
                init_lambda,
            }
        })
        .collect()
}

pub(super) fn resolve_relation_type_id(
    ctx: &mut ResolutionContext<'_>,
) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("RelationType"),
    ])
}

fn resolve_col_spec_array_id(ctx: &mut ResolutionContext<'_>) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("ColSpecArray"),
    ])
}

fn resolve_col_spec_id(ctx: &mut ResolutionContext<'_>) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new("ColSpec"),
    ])
}

fn resolve_relation_class_id(
    ctx: &mut ResolutionContext<'_>,
    class_name: &str,
) -> Option<crate::ids::ElementId> {
    ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("relation"),
        SmolStr::new(class_name),
    ])
}
