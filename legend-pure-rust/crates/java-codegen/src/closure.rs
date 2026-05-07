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

//! BFS over the property graph from a seed set of requested-function
//! parameter and return types. Produces the de-duplicated lists of user
//! `Class` and `Enumeration` elements we need to emit Java sources for.

use std::collections::HashSet;

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::TypeExpr;

use crate::model::ResolvedFn;
use crate::naming::pure_fqn_segments;
use crate::types::is_platform_class;

/// User types reachable from the seed set, deduplicated and ordered.
#[derive(Debug, Default)]
pub(crate) struct ReachableSet {
    /// Reachable Class element IDs in BFS order.
    pub(crate) classes: Vec<ElementId>,
    /// Reachable Enumeration element IDs in BFS order.
    pub(crate) enums: Vec<ElementId>,
    /// Set form for O(1) lookup during interface emission (e.g. to decide
    /// whether to emit a supertype `extends` or skip it).
    pub(crate) class_set: HashSet<ElementId>,
}

impl ReachableSet {
    /// True if a class with the given id will be emitted as a generated
    /// Java interface.
    pub(crate) fn contains_class(&self, id: ElementId) -> bool {
        self.class_set.contains(&id)
    }
}

/// Build the reachability set starting from the parameter and return
/// types of every requested function.
pub(crate) fn reachable_types(model: &PureModel, fns: &[ResolvedFn]) -> ReachableSet {
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut queue: Vec<ElementId> = Vec::new();
    let mut classes: Vec<ElementId> = Vec::new();
    let mut enums: Vec<ElementId> = Vec::new();

    for resolved in fns {
        if let Element::Function(f) = model.get_element(resolved.element_id) {
            for param in f.parameters.iter() {
                seed_from_type_expr(&param.type_expr, &mut queue);
            }
            seed_from_type_expr(&f.return_type, &mut queue);
        }
    }

    while let Some(id) = queue.pop() {
        if !visited.insert(id) {
            continue;
        }
        // Skip the M3 metamodel — we render those as opaque `Object` and
        // do not auto-generate interfaces for them in v1.
        let segments = pure_fqn_segments(model, id);
        if is_platform_class(&segments) {
            continue;
        }
        match model.get_element(id) {
            Element::Class(class) => {
                classes.push(id);
                for super_ty in &class.super_types {
                    seed_from_type_expr(super_ty, &mut queue);
                }
                for prop in &class.properties {
                    seed_from_type_expr(&prop.type_expr, &mut queue);
                }
                for qp in &class.qualified_properties {
                    seed_from_type_expr(&qp.return_type, &mut queue);
                    for param in qp.parameters.iter() {
                        seed_from_type_expr(&param.type_expr, &mut queue);
                    }
                }
                // The injected property visible from `id` is the OTHER
                // end of the association — `1 - prop_idx_pointing_to_self`.
                // We follow that end's type to walk the graph correctly.
                for (assoc_id, prop_idx_pointing_to_self) in model.association_properties(id) {
                    if let Element::Association(assoc) = model.get_element(*assoc_id)
                        && assoc.properties.len() == 2
                        && let Some(injected) = assoc.properties.get(1 - *prop_idx_pointing_to_self)
                    {
                        seed_from_type_expr(&injected.type_expr, &mut queue);
                    }
                }
            }
            Element::Enumeration(_) => {
                enums.push(id);
            }
            // Primitives, packages, etc., are not seeds for further walking.
            _ => {}
        }
    }

    let class_set: HashSet<ElementId> = classes.iter().copied().collect();
    ReachableSet {
        classes,
        enums,
        class_set,
    }
}

fn seed_from_type_expr(ty: &TypeExpr, queue: &mut Vec<ElementId>) {
    match ty {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            queue.push(*element);
            for nested in type_arguments {
                seed_from_type_expr(nested, queue);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (p, _) in parameters {
                seed_from_type_expr(p, queue);
            }
            seed_from_type_expr(return_type, queue);
        }
        TypeExpr::Relation(cols) => {
            for col in cols {
                seed_from_type_expr(&col.type_expr, queue);
            }
        }
        TypeExpr::AlgebraUnion(a, b) => {
            seed_from_type_expr(a, queue);
            seed_from_type_expr(b, queue);
        }
        TypeExpr::Generic(_) | TypeExpr::Unresolved => {}
    }
}
