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

//! `IslandComposer` for graph-fetch (`#{ Type { fields } }#`).
//!
//! Tag: `""`. Pretty-prints a graph-fetch tree to canonical, multi-
//! line, indented Pure source so the round-trip
//! parse → compose → parse is stable.
//!
//! Originally lived in
//! `legend-pure-parser-compose::island::GraphFetchIslandComposer`;
//! moved here so core compose carries no graph-fetch knowledge.
//! Plug-in callers register via [`default_island_composers`] (returns
//! `vec![Box::new(GraphFetchIslandComposer)]`) when calling
//! [`legend_pure_parser_compose::island::compose_island_with`].

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_compose::expression::{compose_element_ptr, compose_expression};
use legend_pure_parser_compose::identifier::maybe_quote;
use legend_pure_parser_compose::island::IslandComposer;
use legend_pure_parser_compose::writer::IndentWriter;

use crate::ast::{PropertyGraphFetchTree, RootGraphFetchTree, SubTypeGraphFetchTree, TAG};

/// Composer for graph-fetch tree syntax. Produces multi-line, indented
/// output matching the Java engine's pretty-rendering mode.
pub struct GraphFetchIslandComposer;

impl IslandComposer for GraphFetchIslandComposer {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn compose(&self, w: &mut IndentWriter, content: &dyn IslandContent) {
        if let Some(tree) = content.as_any().downcast_ref::<RootGraphFetchTree>() {
            compose_graph_fetch_tree(w, tree);
        }
    }
}

/// Convenience helper — `vec![Box::new(GraphFetchIslandComposer)]` for
/// callers that want the empty-tag default island composer registered.
#[must_use]
pub fn default_island_composers() -> Vec<Box<dyn IslandComposer>> {
    vec![Box::new(GraphFetchIslandComposer)]
}

fn compose_graph_fetch_tree(w: &mut IndentWriter, tree: &RootGraphFetchTree) {
    w.write_line("#{");
    w.push_indent();

    compose_element_ptr(w, &tree.class);
    w.write_line("{");
    w.push_indent();

    compose_graph_fetch_children(w, &tree.sub_trees, &tree.sub_type_trees);

    w.pop_indent();
    w.write_line("}");

    w.pop_indent();
    w.write("}#");
}

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
        w.newline();
    }

    for sub in sub_type_trees {
        compose_sub_type_tree(w, sub);
        idx += 1;
        if idx < total {
            w.write(",");
        }
        w.newline();
    }
}

fn compose_property_tree(w: &mut IndentWriter, prop: &PropertyGraphFetchTree) {
    if let Some(alias) = &prop.alias {
        w.write("'");
        w.write(alias);
        w.write("':");
    }

    w.write(&maybe_quote(&prop.property));

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

    if let Some(sub_type) = &prop.sub_type {
        w.write("->subType(@");
        compose_element_ptr(w, sub_type);
        w.write(")");
    }

    if !prop.sub_trees.is_empty() || !prop.sub_type_trees.is_empty() {
        w.write_line("{");
        w.push_indent();
        compose_graph_fetch_children(w, &prop.sub_trees, &prop.sub_type_trees);
        w.pop_indent();
        w.write("}");
    }
}

fn compose_sub_type_tree(w: &mut IndentWriter, sub: &SubTypeGraphFetchTree) {
    w.write("subType(@");
    compose_element_ptr(w, &sub.sub_type_class);
    w.write(")");

    if !sub.sub_trees.is_empty() || !sub.sub_type_trees.is_empty() {
        w.write_line("{");
        w.push_indent();
        compose_graph_fetch_children(w, &sub.sub_trees, &sub.sub_type_trees);
        w.pop_indent();
        w.write("}");
    }
}
