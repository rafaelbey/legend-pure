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

use legend_pure_parser_ast::island::{
    IslandContent, IslandExpression, PropertyGraphFetchTree, RootGraphFetchTree,
    SubTypeGraphFetchTree,
};

use crate::expression::{compose_element_ptr, compose_expression};
use crate::identifier::maybe_quote;
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
#[must_use]
pub fn default_island_composers() -> Vec<Box<dyn IslandComposer>> {
    vec![Box::new(GraphFetchIslandComposer)]
}

/// Compose an island grammar expression by dispatching to the right composer.
///
/// Falls back to a `/* unknown island */` comment if no composer matches.
pub fn compose_island(w: &mut IndentWriter, island: &IslandExpression) {
    compose_island_with(w, island, &default_island_composers());
}

/// Compose an island grammar expression using a custom set of composers.
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
// Graph Fetch Island Composer
// ---------------------------------------------------------------------------

/// Composer for graph fetch tree syntax: `#{Type{field1,field2{sub}}}#`.
///
/// Produces the **compact** canonical form matching Java's roundtrip output:
/// no whitespace between tokens except where syntactically required.
pub struct GraphFetchIslandComposer;

impl IslandComposer for GraphFetchIslandComposer {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        ""
    }

    fn compose(&self, w: &mut IndentWriter, content: &dyn IslandContent) {
        if let Some(tree) = content.as_any().downcast_ref::<RootGraphFetchTree>() {
            compose_graph_fetch_tree(w, tree);
        }
    }
}

/// Compose a root graph fetch tree: `#{Type{...}}#`.
fn compose_graph_fetch_tree(w: &mut IndentWriter, tree: &RootGraphFetchTree) {
    w.write("#{");
    compose_element_ptr(w, &tree.class);
    w.write("{");
    compose_graph_fetch_children(w, &tree.sub_trees, &tree.sub_type_trees);
    w.write("}}#");
}

/// Compose the children (property trees + subtype trees) of a graph fetch node.
fn compose_graph_fetch_children(
    w: &mut IndentWriter,
    sub_trees: &[PropertyGraphFetchTree],
    sub_type_trees: &[SubTypeGraphFetchTree],
) {
    let total = sub_trees.len() + sub_type_trees.len();
    let mut idx = 0;

    for prop in sub_trees {
        compose_property_tree(w, prop);
        idx += 1;
        if idx < total {
            w.write(",");
        }
    }

    for sub in sub_type_trees {
        compose_sub_type_tree(w, sub);
        idx += 1;
        if idx < total {
            w.write(",");
        }
    }
}

/// Compose a property field: `name`, `'alias':name`, `name(args)`, or `name{subs}`.
fn compose_property_tree(w: &mut IndentWriter, prop: &PropertyGraphFetchTree) {
    // Optional alias: 'alias':
    if let Some(alias) = &prop.alias {
        w.write("'");
        w.write(alias);
        w.write("':");
    }

    w.write(&maybe_quote(&prop.property));

    // Optional qualified property parameters: (args)
    if !prop.parameters.is_empty() {
        w.write("(");
        for (i, arg) in prop.parameters.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            compose_expression(w, arg);
        }
        w.write(")");
    }

    // Optional subtype cast: ->subType(@Type)
    if let Some(sub_type) = &prop.sub_type {
        w.write("->subType(@");
        compose_element_ptr(w, sub_type);
        w.write(")");
    }

    // Optional sub-tree: {children}
    if !prop.sub_trees.is_empty() || !prop.sub_type_trees.is_empty() {
        w.write("{");
        compose_graph_fetch_children(w, &prop.sub_trees, &prop.sub_type_trees);
        w.write("}");
    }
}

/// Compose a subtype tree: `->subType(@Type){children}`.
fn compose_sub_type_tree(w: &mut IndentWriter, sub: &SubTypeGraphFetchTree) {
    w.write("->subType(@");
    compose_element_ptr(w, &sub.sub_type_class);
    w.write(")");

    if !sub.sub_trees.is_empty() || !sub.sub_type_trees.is_empty() {
        w.write("{");
        compose_graph_fetch_children(w, &sub.sub_trees, &sub.sub_type_trees);
        w.write("}");
    }
}
