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

//! Plug-in trait for converting island AST content into protocol JSON.
//!
//! Section-level analog: `legend_pure_parser_pure::extension::CompilerExtension`.
//! Inline-island parser/composer analogs:
//! `legend_pure_parser_parser::IslandParser` and the composer-side
//! `IslandComposer`. This module completes the plug-in triad on the
//! protocol side.
//!
//! Each DSL crate that owns an island grammar (`dsl-graph`,
//! `dsl-store`, `dsl-tds`) implements [`IslandProtocol`] keyed on
//! the same tag the parser/composer use, and converts the typed
//! [`IslandContent`] into the protocol's
//! [`ValueSpecification`](crate::v1::value_spec::ValueSpecification).
//!
//! The current `convert_island_expression` path (in `v1::convert`)
//! still performs a hardcoded downcast to graph-fetch types as a
//! transitional compatibility shim. Once the graph-fetch protocol
//! converter is moved into `dsl-graph` (planned commit #7), the
//! shim is removed and *all* island conversion routes through
//! [`dispatch_island_convert`].

use legend_pure_parser_ast::island::{IslandContent, IslandExpression};

use crate::v1::value_spec::ValueSpecification;

/// Result alias matching the rest of the protocol crate's surface.
pub type Result<T> = std::result::Result<T, serde_json::Error>;

/// Plug-in for converting one island grammar's [`IslandContent`]
/// into a protocol-side [`ValueSpecification`].
///
/// Implementations live in DSL crates next to that DSL's
/// `IslandParser` and `IslandComposer` impls. The `tag()` value
/// must match the tag the parser and composer use.
///
/// # Example
///
/// ```rust,ignore
/// pub struct GraphFetchProtocol;
/// impl IslandProtocol for GraphFetchProtocol {
///     fn tag(&self) -> &str { "" }
///     fn convert(&self, content: &dyn IslandContent) -> Result<ValueSpecification> {
///         let tree = content
///             .as_any()
///             .downcast_ref::<RootGraphFetchTree>()
///             .ok_or_else(|| /* error */)?;
///         /* convert tree to ValueSpecification */
///     }
/// }
/// ```
pub trait IslandProtocol: Send + Sync {
    /// The island tag this converter handles. Same string the
    /// matching `IslandParser::tag()` and `IslandComposer::tag()`
    /// return — `""` for graph fetch, `">"` for
    /// RelationStoreAccessor, `"TDS"` for TDS, etc.
    fn tag(&self) -> &str;

    /// Convert one [`IslandContent`] (already downcast-ready via
    /// [`IslandContent::as_any`]) into a protocol value
    /// specification. Implementations downcast to their concrete
    /// content type and emit the corresponding protocol JSON shape.
    ///
    /// # Errors
    /// Returns `serde_json::Error` if the protocol value cannot be
    /// constructed (typically a downcast mismatch or a nested
    /// expression conversion failure).
    fn convert(&self, content: &dyn IslandContent) -> Result<ValueSpecification>;
}

/// Look up an [`IslandProtocol`] for `island.tag()` in `protocols`
/// and convert.
///
/// Returns `Ok(Some(value))` if a matching converter was found and
/// produced a value, `Ok(None)` if no converter matches (caller
/// decides whether to fall back to a built-in path), or `Err` if a
/// matching converter ran but failed.
///
/// # Errors
/// Propagates the matching converter's error verbatim.
pub fn dispatch_island_convert(
    island: &IslandExpression,
    protocols: &[&dyn IslandProtocol],
) -> Result<Option<ValueSpecification>> {
    let tag = island.tag();
    for p in protocols {
        if p.tag() == tag {
            return p.convert(island.content.as_ref()).map(Some);
        }
    }
    Ok(None)
}
