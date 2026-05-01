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

//! Island grammar parser plugin infrastructure.
//!
//! Each island grammar (graph fetch, path, SQL, etc.) provides an
//! [`IslandParser`] implementation that can parse the content between
//! `#tag{` and `}#` tokens. The main parser dispatches to the
//! appropriate plugin based on the tag.
//!
//! # Adding a New Island Grammar
//!
//! 1. Define your AST types implementing [`IslandContent`]
//! 2. Implement [`IslandParser`] for your grammar
//! 3. Register it via [`default_island_parsers`] or pass to `parse_with_islands()`
//!
//! # Thread-Safety Requirements
//!
//! **All island parser implementations must be `Send + Sync`.**
//!
//! The CLI parallelizes parsing across multiple files using Rayon
//! (`files.par_iter().map(|path| parse(...))`), which means the same
//! `IslandParser` instance may be invoked concurrently from different
//! threads. The `Send + Sync` supertraits on [`IslandParser`] enforce
//! this at compile time.
//!
//! In practice, this means your implementation:
//! - **Must not** use `Rc`, `Cell`, `RefCell`, or other non-thread-safe types
//! - **Must not** hold mutable state between `parse()` calls
//! - **Should** be stateless (zero-sized structs are ideal, like [`GraphFetchIslandParser`](graph_fetch::GraphFetchIslandParser))
//! - If state is required, use `Arc<Mutex<...>>` or atomics
//!
//! The same requirement applies to [`IslandContent`] (the AST output), which
//! also requires `Send + Sync`.
//!
//! [`IslandContent`]: legend_pure_parser_ast::island::IslandContent

use legend_pure_parser_ast::island::IslandContent;

use crate::error::ParseError;
use crate::parser::ParserContext;

/// Trait for island grammar parsers.
///
/// Each island grammar provides a parser that can consume tokens between
/// `#tag{` and `}#` and produce a boxed [`IslandContent`].
///
/// The parser is called after the opening `#{` (or `#tag{`) has been consumed.
/// It must NOT consume the closing `}#` — the main parser handles that.
///
/// # Thread Safety
///
/// This trait requires `Send + Sync` because the CLI parallelizes file
/// parsing across threads. Implementations must be stateless or use
/// thread-safe interior mutability. The compiler will reject any
/// implementation that does not satisfy these bounds.
///
/// # Example
///
/// ```rust,ignore
/// use legend_pure_parser_parser::island::IslandParser;
///
/// // Good: zero-sized, stateless struct — trivially Send + Sync
/// struct MyIslandParser;
///
/// impl IslandParser for MyIslandParser {
///     fn tag(&self) -> &str { "myTag" }
///     fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError> {
///         // parse tokens from ctx...
///         # todo!()
///     }
/// }
/// ```
pub trait IslandParser: Send + Sync {
    /// The tag this parser handles.
    ///
    /// - `""` for `#{…}#` (graph fetch)
    /// - `">"` for `#>{…}#` (path, future)
    /// - `"sql"` for `#sql{…}#` (embedded SQL, future)
    fn tag(&self) -> &str;

    /// Parse island content from the parser context.
    ///
    /// Called after `#{` (or `#tag{`) has been consumed. The
    /// `ParserContext` provides access to the token cursor and
    /// expression parsing utilities.
    ///
    /// # Errors
    ///
    /// Returns `ParseError` if the island content is malformed.
    fn parse(&self, ctx: &mut ParserContext<'_>) -> Result<Box<dyn IslandContent>, ParseError>;
}

/// Returns the default set of built-in island parsers.
///
/// Empty by default — DSL crates that own island grammars (`dsl-graph`,
/// `dsl-store`, `dsl-tds`) export their own helpers (e.g.
/// [`legend_pure_dsl_graph::parser::default_island_parsers`]) which
/// callers concatenate into the slice they pass to
/// [`crate::parse_with_islands`]. Core carries no DSL-specific
/// island parsers.
#[must_use]
pub fn default_island_parsers() -> Vec<Box<dyn IslandParser>> {
    Vec::new()
}
