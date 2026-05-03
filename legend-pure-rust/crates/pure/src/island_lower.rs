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

//! Plug-in trait for lowering inline-island expressions to AST.
//!
//! Each DSL crate that owns an island grammar (today: `dsl-tds`, with
//! `dsl-graph` and `dsl-store` as future candidates) supplies an
//! [`IslandLowerer`] implementation. The lowerer in
//! [`crate::lower`] dispatches by tag and recurses into the resulting
//! synthetic AST — keeping the `pure` crate ignorant of any DSL's
//! content shape.
//!
//! # Why a synthetic AST node, not a `ValueSpec`?
//!
//! Returning an [`ast_expr::Expression`] keeps DSL crates from needing
//! access to `pure`'s internal `ResolutionContext` and lowering
//! machinery. The trait contract becomes "given parsed island content,
//! emit the AST you'd like the compiler to lower in your stead." The
//! main lowerer then recursively runs `lower_expression` on the
//! synthetic tree, so existing function-application / arrow-call /
//! type-reference paths handle the heavy lifting.
//!
//! # Inline-island parser/composer/protocol/lowerer parallel
//!
//! - **Parser** (`legend_pure_parser_parser::IslandParser`) — `&str` →
//!   `Box<dyn IslandContent>` at parse time.
//! - **Composer** (`legend_pure_parser_compose::island::IslandComposer`)
//!   — `&dyn IslandContent` → grammar text at compose time.
//! - **Protocol** (`legend_pure_parser_protocol::IslandProtocol`) —
//!   `&dyn IslandContent` → JSON at v1-protocol-emit time.
//! - **Lowerer** (this trait) — `&dyn IslandContent` → synthetic AST
//!   at compile time, which the main lowerer then resolves.
//!
//! All four dispatch by tag so a single DSL crate can supply all four
//! impls without coupling to `pure`'s internals.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::island::{IslandContent, IslandExpression};
use legend_pure_parser_ast::source_info::SourceInfo;

/// Lower one island grammar's [`IslandContent`] to a synthetic AST
/// expression that the main [`crate::lower::lower_expression`] should
/// process in place of the island.
///
/// # Thread safety
///
/// `Send + Sync` — implementations must be stateless or use
/// thread-safe interior mutability, mirroring the same constraint on
/// [`legend_pure_parser_parser::IslandParser`]. The CLI parallelises
/// compilation across files.
pub trait IslandLowerer: Send + Sync {
    /// The island tag this lowerer handles. Must match the
    /// [`IslandContent::tag()`] of every content node it accepts.
    fn tag(&self) -> &str;

    /// Build a synthetic AST expression that the main lowerer will
    /// recurse into. Returning `None` means the lowerer declined this
    /// content (e.g. failed downcast, malformed shape) — the caller
    /// will treat the island as unlowerable and emit the existing
    /// "Island expression lowering not yet implemented" diagnostic.
    fn lower_to_ast(
        &self,
        content: &dyn IslandContent,
        source_info: &SourceInfo,
    ) -> Option<ast_expr::Expression>;
}

/// Look up an [`IslandLowerer`] for `island.tag()` in `lowerers` and
/// dispatch. Returns the synthesised AST or `None` if no lowerer
/// claims the tag (or the matched lowerer declined the content).
#[must_use]
pub fn dispatch_island_lower(
    island: &IslandExpression,
    lowerers: &[Box<dyn IslandLowerer>],
) -> Option<ast_expr::Expression> {
    let tag = island.content.tag();
    for lowerer in lowerers {
        if lowerer.tag() == tag {
            return lowerer.lower_to_ast(island.content.as_ref(), &island.source_info);
        }
    }
    None
}
