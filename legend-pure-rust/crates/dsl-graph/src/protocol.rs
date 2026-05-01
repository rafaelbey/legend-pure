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

//! `IslandProtocol` for graph-fetch.
//!
//! Tag: `""`. Converts `RootGraphFetchTree` AST into the v1 protocol's
//! `ClassInstance { _type: "rootGraphFetchTree", … }` value
//! specification, matching the Java engine's JSON output.
//!
//! Originally lived in
//! `legend-pure-parser-protocol::v1::convert`; moved here so core
//! protocol carries no graph-fetch knowledge. Plug-in callers
//! register via [`default_island_protocols`] when calling
//! [`legend_pure_parser_protocol::dispatch_island_convert`].

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_protocol::IslandProtocol;
use legend_pure_parser_protocol::v1::convert::convert_expression_typed;
use legend_pure_parser_protocol::v1::value_spec::{ClassInstance, ValueSpecification};

use crate::ast::{PropertyGraphFetchTree, RootGraphFetchTree, SubTypeGraphFetchTree, TAG};

/// Protocol converter for graph-fetch trees. Emits the canonical
/// `_type: "rootGraphFetchTree"` JSON shape.
pub struct GraphFetchIslandProtocol;

impl IslandProtocol for GraphFetchIslandProtocol {
    fn tag(&self) -> &str {
        TAG
    }

    fn convert(
        &self,
        content: &dyn IslandContent,
    ) -> legend_pure_parser_protocol::island_protocol::Result<ValueSpecification> {
        let tree = content
            .as_any()
            .downcast_ref::<RootGraphFetchTree>()
            .ok_or_else(|| {
                use serde::ser::Error as _;
                serde_json::Error::custom(format!(
                    "GraphFetchIslandProtocol: content with tag '{}' is not a RootGraphFetchTree",
                    content.tag()
                ))
            })?;
        Ok(convert_root_graph_fetch_tree(tree))
    }
}

/// Convenience helper — `vec![Box::new(GraphFetchIslandProtocol)]`
/// for callers that want the empty-tag default protocol converter.
#[must_use]
pub fn default_island_protocols() -> Vec<Box<dyn IslandProtocol>> {
    vec![Box::new(GraphFetchIslandProtocol)]
}

// ---------------------------------------------------------------------------
// Implementation — copied from
// legend-pure-parser-protocol::v1::convert with paths rewired.
// ---------------------------------------------------------------------------

fn convert_root_graph_fetch_tree(tree: &RootGraphFetchTree) -> ValueSpecification {
    let sub_trees: Vec<serde_json::Value> = tree
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = tree
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    ValueSpecification::ClassInstance(ClassInstance {
        type_name: "rootGraphFetchTree".to_string(),
        value: serde_json::json!({
            "_type": "rootGraphFetchTree",
            "class": tree.class.to_string(),
            "subTrees": sub_trees,
            "subTypeTrees": sub_type_trees,
        }),
        source_information: Some(source_information(&tree.source_info)),
    })
}

fn convert_property_graph_fetch_tree(prop: &PropertyGraphFetchTree) -> serde_json::Value {
    let sub_trees: Vec<serde_json::Value> = prop
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = prop
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    let mut obj = serde_json::json!({
        "_type": "propertyGraphFetchTree",
        "property": prop.property.to_string(),
        "subTrees": sub_trees,
        "subTypeTrees": sub_type_trees,
    });

    if !prop.parameters.is_empty() {
        let params: Vec<serde_json::Value> = prop
            .parameters
            .iter()
            .map(|e| {
                let vs = convert_expression_typed(e);
                serde_json::to_value(&vs).unwrap_or_default()
            })
            .collect();
        obj["parameters"] = serde_json::json!(params);
    }

    if let Some(alias) = &prop.alias {
        obj["alias"] = serde_json::json!(alias.to_string());
    }

    if let Some(sub_type) = &prop.sub_type {
        obj["subType"] = serde_json::json!(sub_type.to_string());
    }

    obj
}

fn convert_sub_type_graph_fetch_tree(sub: &SubTypeGraphFetchTree) -> serde_json::Value {
    let sub_trees: Vec<serde_json::Value> = sub
        .sub_trees
        .iter()
        .map(convert_property_graph_fetch_tree)
        .collect();

    let sub_type_trees: Vec<serde_json::Value> = sub
        .sub_type_trees
        .iter()
        .map(convert_sub_type_graph_fetch_tree)
        .collect();

    serde_json::json!({
        "_type": "subTypeGraphFetchTree",
        "subTypeClass": sub.sub_type_class.to_string(),
        "subTrees": sub_trees,
        "subTypeTrees": sub_type_trees,
    })
}

fn source_information(
    si: &legend_pure_parser_ast::source_info::SourceInfo,
) -> legend_pure_parser_protocol::v1::source_info::SourceInformation {
    legend_pure_parser_protocol::v1::source_info::SourceInformation::from(si)
}
