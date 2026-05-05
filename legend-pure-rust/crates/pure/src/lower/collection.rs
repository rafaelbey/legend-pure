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

//! Collection-literal lowering: `[a, b, c]` → `ExprKind::Collection`.
//!
//! Step 4 of the lowering encapsulation plan; sibling slice to
//! `lower/literal.rs`.

use legend_pure_parser_ast::expression as ast_expr;

use crate::error::CompilationError;
use crate::resolve::ResolutionContext;
use crate::types::{ExprKind, ValueSpec};

use super::{lower_expression, untyped};

/// Lowers a collection literal `[a, b, c]`.
pub(super) fn lower_collection(
    coll: &ast_expr::CollectionExpr,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> ValueSpec {
    let elements = coll
        .elements
        .iter()
        .filter_map(|e| lower_expression(e, ctx, errors))
        .collect();
    untyped(ExprKind::Collection { elements }, coll.source_info.clone())
}
