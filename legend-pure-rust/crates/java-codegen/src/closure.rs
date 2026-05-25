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

use crate::model::{Bootstrap, Options, ResolvedFn};
use crate::naming::{pure_fqn_segments, pure_fqn_string};
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
/// types of every requested function plus any explicitly-requested
/// `--classes` / `--associations` seeds.
///
/// `extra_class_seeds` are emitted unconditionally even if their FQN
/// would normally be filtered as a platform class — the user opted in by
/// name, so we honour it. Transitive types reached *through* those seeds
/// still go through the platform filter.
pub(crate) fn reachable_types(
    model: &PureModel,
    fns: &[ResolvedFn],
    extra_class_seeds: &[ElementId],
    bootstrap: Bootstrap,
    opts: &Options,
) -> ReachableSet {
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut queue: Vec<ElementId> = Vec::new();
    let mut classes: Vec<ElementId> = Vec::new();
    let mut enums: Vec<ElementId> = Vec::new();
    // Explicit seeds bypass the platform filter (only the seeds
    // themselves; transitive walking from them still filters).
    let bypass_filter: HashSet<ElementId> = extra_class_seeds.iter().copied().collect();

    for resolved in fns {
        if let Element::Function(f) = model.get_element(resolved.element_id) {
            for param in f.parameters.iter() {
                seed_from_type_expr(&param.type_expr, &mut queue);
            }
            seed_from_type_expr(&f.return_type, &mut queue);
        }
    }
    for id in extra_class_seeds {
        queue.push(*id);
    }

    while let Some(id) = queue.pop() {
        if !visited.insert(id) {
            continue;
        }
        // Externally-bound types are emitted by another module — skip
        // walking + emission entirely. The reference site renders to
        // the imported Java FQN via `Options::external_bindings` in
        // `types.rs::render_named`, so we don't need to chase
        // transitive types reached *through* an external class either:
        // that closure already lives in the importing JAR.
        let pure_fqn = pure_fqn_string(model, id);
        if opts.external_bindings.contains_key(&pure_fqn) {
            continue;
        }
        // Skip the M3 metamodel — we render those as opaque `Object` and
        // do not auto-generate interfaces for them in v1, unless the
        // caller explicitly asked for this id via `--classes` /
        // `--associations`.
        let segments = pure_fqn_segments(model, id);
        if !bypass_filter.contains(&id) && is_platform_class(&segments) {
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

    // Drop the universal-supertype `Any` and the bottom type `Nil`
    // from the reachable set — the hand-written
    // `org.finos.legend.pure.rust.proxy.Any` interface stands in for
    // Any, and Nil is rendered as Java `Void` directly. Identity
    // comparison via the canonical bootstrap IDs (FQN-string match
    // would miss because M3 bootstrap classes carry
    // `parent_package = root` and so `pure_fqn_segments` returns just
    // the leaf name).
    classes.retain(|id| !bootstrap.is_any(*id) && !bootstrap.is_nil(*id));

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
        TypeExpr::GenericTypeOperation {
            left: a, right: b, ..
        } => {
            seed_from_type_expr(a, queue);
            seed_from_type_expr(b, queue);
        }
        TypeExpr::Generic(_) | TypeExpr::Unresolved => {}
    }
}
