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

//! Stage-3 [`CompilerExtension`] for the Mapping DSL.
//!
//! Phases:
//!
//! - `declare()` — walks every source file's `###Mapping` sections,
//!   downcasts each `Element::DSLElement` to [`MappingDef`], and
//!   registers it in extension-private state keyed by FQN. Duplicates
//!   produce a `DuplicateElement` diagnostic.
//! - `validate()` — runs the structural validators that don't require
//!   lambda lowering:
//!   1. Class-mapping target class must resolve as a `Class`.
//!   2. `~src` source class (if present) must resolve as a `Class`.
//!   3. Property names in property mappings must exist on the target
//!      class (or its supertypes).
//!   4. Mapping-include FQNs must reference another registered
//!      mapping.
//!   5. Mapping-include graph must be acyclic.
//!   6. `extends [superId]` must reference a sibling class-mapping ID
//!      within the same mapping or one of its (transitive) includes.
//!   7. Class-mapping IDs within a single mapping must be unique
//!      (whether explicit `[id]` or implicit class-name default).
//!
//! Phases not yet wired (`define_signatures` / `define_bodies`):
//!
//! - The Java `PureInstanceSetImplementationProcessor` lowers filter
//!   and transform expressions with `src` bound to the source class,
//!   then runs Pass 2 type inference. The Rust analog requires
//!   exposing both `lower::lower_expression` and a public wrapper
//!   that builds a `ResolutionContext` from a small input set
//!   (model + auto-imports + bindings). That plumbing is a separate
//!   sub-stage (call it Stage 3.5); the public `infer_function_body`
//!   surface (this stage's other deliverable) provides the inference
//!   half of the contract once the lowering wrapper lands.
//! - As a consequence, the following Java validator rules are
//!   **deferred until 3.5** and do not fire here:
//!     * "Filter must return Boolean[1]"
//!     * "Transform return type must be subtype of property type"
//!     * "Transform multiplicity must subsume property multiplicity"
//!
//!   The structural rules above still catch the bulk of real user
//!   errors (unknown property, missing class, cyclic include).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx, ValidateCtx};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element as ModelElement, PureModel};
use legend_pure_parser_pure::types::TypeExpr;
use smol_str::SmolStr;

use crate::ast::{ClassMapping, ClassMappingBody, MappingDef, PureClassMappingBody};

/// Compiler extension for the `###Mapping` DSL.
///
/// Construct one per
/// [`compile_with_extensions`](legend_pure_parser_pure::pipeline::compile_with_extensions)
/// invocation; do not share across compilations because the
/// extension's internal map is reset per call.
#[derive(Default)]
pub struct MappingExtension {
    /// Mappings collected during `declare`, keyed by FQN
    /// (`"pkg::sub::Name"`). Per-extension state — not stored in
    /// `PureModel`. Use [`Self::mappings`] to inspect after compile.
    mappings: RefCell<HashMap<SmolStr, RegisteredMapping>>,
}

/// One registered mapping alongside its bookkeeping FQN.
#[derive(Debug, Clone)]
pub struct RegisteredMapping {
    /// The original AST node.
    pub def: MappingDef,
    /// FQN this mapping is registered under.
    pub fqn: SmolStr,
}

impl MappingExtension {
    /// Construct an empty extension.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all registered mappings keyed by FQN. Useful for
    /// tests, codegen, and downstream stages.
    #[must_use]
    pub fn mappings(&self) -> HashMap<SmolStr, RegisteredMapping> {
        self.mappings.borrow().clone()
    }
}

impl CompilerExtension for MappingExtension {
    fn name(&self) -> &'static str {
        "dsl-mapping"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let mut registry = self.mappings.borrow_mut();
        for source_file in ctx.source_files {
            for section in &source_file.sections {
                if section.kind.as_str() != crate::ast::SECTION_KIND {
                    continue;
                }
                for elem in &section.elements {
                    let AstElement::DSLElement(boxed) = elem else {
                        continue;
                    };
                    let Some(m) = boxed.as_any().downcast_ref::<MappingDef>() else {
                        continue;
                    };

                    let fqn = build_fqn(m);
                    if let Some(prev) = registry.insert(
                        fqn.clone(),
                        RegisteredMapping {
                            def: m.clone(),
                            fqn: fqn.clone(),
                        },
                    ) {
                        ctx.errors.push(CompilationError {
                            message: format!("Duplicate mapping '{fqn}'"),
                            source_info: m.source_info.clone(),
                            kind: CompilationErrorKind::DuplicateElement { name: fqn },
                        });
                        // Restore prior registration so subsequent
                        // passes still see the earlier definition.
                        registry.insert(prev.fqn.clone(), prev);
                    }
                }
            }
        }
    }

    fn validate(&self, ctx: &mut ValidateCtx<'_>) {
        let registry = self.mappings.borrow();
        check_include_dag(&registry, ctx.errors);
        for (_, reg) in registry.iter() {
            validate_mapping(&reg.def, &registry, ctx.model, ctx.errors);
        }
    }
}

// ---------------------------------------------------------------------------
// Per-mapping validation
// ---------------------------------------------------------------------------

fn validate_mapping(
    m: &MappingDef,
    registry: &HashMap<SmolStr, RegisteredMapping>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Includes: each FQN must reference a registered mapping.
    for inc in &m.includes {
        let fqn = ptr_fqn(&inc.included);
        if !registry.contains_key(&fqn) {
            errors.push(CompilationError {
                message: format!("Mapping include '{fqn}' does not resolve to a known mapping"),
                source_info: inc.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: fqn },
            });
        }
    }

    // Per class mapping: class resolves, src class resolves, properties
    // exist on the target class (or its supertypes), super-id resolves
    // against the visible class-mapping ID set.
    let visible_ids = visible_class_mapping_ids(m, registry);
    for cm in &m.class_mappings {
        validate_class_mapping(cm, m, &visible_ids, model, errors);
    }

    // Class-mapping IDs must be unique within a single mapping. The
    // effective ID is the explicit `[id]` if present, otherwise the
    // class FQN. Duplicates make `extends [id]` and downstream
    // routing ambiguous, mirroring the Java validator's behaviour.
    let mut seen_ids: HashMap<SmolStr, &ClassMapping> = HashMap::new();
    let mapping_fqn = build_fqn(m);
    for cm in &m.class_mappings {
        let effective_id = cm.id.clone().unwrap_or_else(|| ptr_fqn(&cm.class));
        if let Some(prev) = seen_ids.insert(effective_id.clone(), cm) {
            errors.push(CompilationError {
                message: format!(
                    "Duplicate class-mapping ID '{effective_id}' in mapping '{mapping_fqn}' \
                     (first defined at {}:{})",
                    prev.source_info.start_line, prev.source_info.start_column
                ),
                source_info: cm.source_info.clone(),
                kind: CompilationErrorKind::DuplicateElement { name: effective_id },
            });
        }
    }
}

fn validate_class_mapping(
    cm: &ClassMapping,
    owner: &MappingDef,
    visible_ids: &HashSet<SmolStr>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Target class must resolve to a Class.
    let class_fqn = ptr_fqn(&cm.class);
    let class_id = match resolve_class(model, &class_fqn) {
        Some(id) => Some(id),
        None => {
            errors.push(CompilationError {
                message: format!("Class mapping target '{class_fqn}' does not resolve to a Class"),
                source_info: cm.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: class_fqn.clone(),
                },
            });
            None
        }
    };

    // Super-mapping reference: when present, must match a known ID.
    if let Some(super_id) = &cm.extends
        && !visible_ids.contains(super_id)
    {
        errors.push(CompilationError {
            message: format!(
                "Super class-mapping ID '{super_id}' on '{}' is not declared in this mapping or any included mapping",
                build_fqn(owner)
            ),
            source_info: cm.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: super_id.clone(),
            },
        });
    }

    // Body-shape rules.
    let ClassMappingBody::Pure(body) = &cm.body;
    validate_pure_body(body, &class_fqn, class_id, model, errors);
}

fn validate_pure_body(
    body: &PureClassMappingBody,
    target_class_fqn: &str,
    target_class_id: Option<ElementId>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // ~src must resolve to a Class.
    if let Some(src) = &body.src_class {
        let src_fqn = ptr_fqn(src);
        if resolve_class(model, &src_fqn).is_none() {
            errors.push(CompilationError {
                message: format!("~src class '{src_fqn}' does not resolve to a Class"),
                source_info: src.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: src_fqn },
            });
        }
    }

    // Property names — only checkable if the target class resolved.
    if let Some(class_id) = target_class_id {
        for pm in &body.property_mappings {
            if !class_has_property(model, class_id, pm.property_name.as_str()) {
                errors.push(CompilationError {
                    message: format!(
                        "Class '{target_class_fqn}' has no property '{}'",
                        pm.property_name
                    ),
                    source_info: pm.source_info.clone(),
                    kind: CompilationErrorKind::UnknownProperty {
                        type_name: SmolStr::new(target_class_fqn),
                        property_name: pm.property_name.clone(),
                    },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Visible class-mapping IDs (current mapping + transitively included)
// ---------------------------------------------------------------------------

fn visible_class_mapping_ids(
    m: &MappingDef,
    registry: &HashMap<SmolStr, RegisteredMapping>,
) -> HashSet<SmolStr> {
    let mut out = HashSet::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    let mut queue: VecDeque<&MappingDef> = VecDeque::new();
    queue.push_back(m);
    visited.insert(build_fqn(m));

    while let Some(cur) = queue.pop_front() {
        for cm in &cur.class_mappings {
            // The default class-mapping ID is the class FQN; an
            // explicit `[id]` overrides it. Both shapes are reachable
            // via `extends [...]`.
            let default_id = SmolStr::new(cm.class.name.as_str());
            out.insert(default_id);
            if let Some(id) = &cm.id {
                out.insert(id.clone());
            }
        }
        for inc in &cur.includes {
            let fqn = ptr_fqn(&inc.included);
            if visited.contains(&fqn) {
                continue;
            }
            if let Some(reg) = registry.get(&fqn) {
                visited.insert(fqn);
                queue.push_back(&reg.def);
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Mapping include DAG (cycle detection — Kahn's algorithm)
// ---------------------------------------------------------------------------

fn check_include_dag(
    registry: &HashMap<SmolStr, RegisteredMapping>,
    errors: &mut Vec<CompilationError>,
) {
    // Edge: m -> included.  In-degree counts incoming edges per node.
    // Restricted to registered FQNs — unresolved includes are flagged
    // separately by `validate_mapping`.
    let mut in_degree: HashMap<&str, usize> =
        registry.keys().map(|k| (k.as_str(), 0_usize)).collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for (fqn, reg) in registry {
        for inc in &reg.def.includes {
            let included_fqn = ptr_fqn(&inc.included);
            if let Some((tgt_key, _)) = registry.get_key_value(&included_fqn) {
                edges
                    .entry(fqn.as_str())
                    .or_default()
                    .push(tgt_key.as_str());
                *in_degree.entry(tgt_key.as_str()).or_insert(0) += 1;
            }
        }
    }

    // Kahn: start with all nodes whose in-degree is 0.
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter_map(|(k, deg)| (*deg == 0).then_some(*k))
        .collect();
    let mut visited = 0_usize;
    while let Some(n) = queue.pop_front() {
        visited += 1;
        if let Some(out_edges) = edges.get(n) {
            for &m in out_edges {
                let entry = in_degree.entry(m).or_insert(0);
                *entry = entry.saturating_sub(1);
                if *entry == 0 {
                    queue.push_back(m);
                }
            }
        }
    }

    // Anything still with non-zero in-degree participates in a cycle.
    if visited < registry.len() {
        for (fqn, deg) in &in_degree {
            if *deg > 0
                && let Some(reg) = registry.get(*fqn)
            {
                errors.push(CompilationError {
                    message: format!("Cyclic mapping include detected involving '{fqn}'"),
                    source_info: reg.def.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: SmolStr::new(*fqn),
                    },
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Class / property lookup
// ---------------------------------------------------------------------------

fn resolve_class(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    let id = model.resolve_by_path(&segments)?;
    matches!(model.get_element(id), ModelElement::Class(_)).then_some(id)
}

/// Walks the class's properties (including supertypes via `super_types`)
/// looking for `prop_name`. Mirrors the dsl-graph helper at
/// `crates/dsl-graph/src/compiler.rs::class_has_property`.
fn class_has_property(model: &PureModel, class_id: ElementId, prop_name: &str) -> bool {
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut stack: Vec<ElementId> = vec![class_id];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let ModelElement::Class(c) = model.get_element(id) else {
            continue;
        };
        if c.properties.iter().any(|p| p.name == prop_name)
            || c.qualified_properties.iter().any(|q| q.name == prop_name)
        {
            return true;
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                stack.push(*element);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// FQN helpers
// ---------------------------------------------------------------------------

fn build_fqn(m: &MappingDef) -> SmolStr {
    if let Some(pkg) = m.package() {
        SmolStr::new(format!("{pkg}::{}", m.name.value))
    } else {
        m.name.value.clone()
    }
}

fn ptr_fqn(p: &PackageableElementPtr) -> SmolStr {
    if let Some(pkg) = &p.package {
        SmolStr::new(format!("{pkg}::{}", p.name))
    } else {
        p.name.clone()
    }
}
