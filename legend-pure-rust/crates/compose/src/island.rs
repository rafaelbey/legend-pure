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

//! Island grammar composer plugin infrastructure.
//!
//! Each island grammar provides an [`IslandComposer`] implementation that
//! renders island content back to Pure grammar text.

use legend_pure_parser_ast::island::{IslandContent, IslandExpression};

use crate::writer::IndentWriter;

/// Trait for island grammar composers.
///
/// Each island grammar plugin implements this to render its content
/// back to Pure grammar text.
pub trait IslandComposer {
    /// The tag this composer handles (matches [`IslandContent::tag()`]).
    fn tag(&self) -> &str;

    /// Compose the island content to the writer.
    fn compose(&self, w: &mut IndentWriter, content: &dyn IslandContent);
}

/// Returns the default set of built-in island composers.
///
/// Empty by default — DSL crates that own island grammars
/// (`dsl-graph`, `dsl-store`, `dsl-tds`) export their own helpers
/// (e.g. [`legend_pure_dsl_graph::compose::default_island_composers`])
/// which callers concatenate into the slice they pass to
/// [`compose_island_with`]. Core carries no DSL-specific composers.
#[must_use]
pub fn default_island_composers() -> Vec<Box<dyn IslandComposer>> {
    Vec::new()
}

/// Compose an island grammar expression by dispatching to the right
/// composer.
///
/// Reads the active composer set from the per-thread registry
/// populated by [`with_island_composers`]. If no registry was set
/// up, falls back to the empty default and emits a placeholder
/// comment for the island.
///
/// This indirection is what lets `compose_source_file` continue to
/// take just `&SourceFile` while DSL crates (`dsl-graph`,
/// `dsl-store`, `dsl-tds`) own the concrete composers — callers
/// wrap their `compose_source_file` call in
/// [`compose::section::compose_source_file_with`](crate::section::compose_source_file_with),
/// which sets the thread-local for the duration.
pub fn compose_island(w: &mut IndentWriter, island: &IslandExpression) {
    let tag = island.tag();
    THREAD_COMPOSERS.with(|cell| {
        let registry = cell.borrow();
        if let Some(composer) = registry.iter().find(|c| c.tag() == tag) {
            composer.compose(w, island.content.as_ref());
        } else {
            w.write(&format!("/* unknown island tag: '{tag}' */"));
        }
    });
}

/// Compose an island grammar expression using an explicit composer
/// list. Bypasses the thread-local registry — useful for DSL crate
/// tests that compose a single island without setting up a full
/// `compose_source_file_with` call.
pub fn compose_island_with(
    w: &mut IndentWriter,
    island: &IslandExpression,
    composers: &[Box<dyn IslandComposer>],
) {
    let tag = island.tag();
    if let Some(composer) = composers.iter().find(|c| c.tag() == tag) {
        composer.compose(w, island.content.as_ref());
    } else {
        w.write(&format!("/* unknown island tag: '{tag}' */"));
    }
}

// ---------------------------------------------------------------------------
// Per-thread composer registry — populated by `with_island_composers`
// during a `compose_source_file_with` call so the deep `compose_island`
// dispatcher can read it without threading the list through every layer.
// ---------------------------------------------------------------------------

thread_local! {
    static THREAD_COMPOSERS: std::cell::RefCell<Vec<Box<dyn IslandComposer>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with `composers` registered as the active per-thread
/// composer set. Replaces any previous registration on entry and
/// restores it on exit. Used by
/// [`compose::section::compose_source_file_with`](crate::section::compose_source_file_with).
pub fn with_island_composers<F, R>(composers: Vec<Box<dyn IslandComposer>>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = THREAD_COMPOSERS.with(|cell| cell.replace(composers));
    let result = f();
    THREAD_COMPOSERS.with(|cell| {
        let _ = cell.replace(prev);
    });
    result
}
