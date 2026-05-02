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

//! Stage-3 (+ 3.5) [`CompilerExtension`] for the Mapping DSL.
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
//! Type-check rules added in Stage 3.5 (this revision):
//!
//! 8. `~filter` expression must infer to `Boolean[1]` with `src` bound
//!    to the resolved source class.
//! 9. Each transform expression must infer to a type compatible with
//!    its target property's declared type (subtype check).
//! 10. Each transform's inferred multiplicity must fit within the
//!     target property's declared multiplicity (range subsumption).
//!
//! These rules use `legend_pure_parser_pure::extension::lower_and_infer_expression`
//! and the public `is_subtype` / `is_multiplicity_compatible` helpers
//! exposed from the `pure` crate's `resolve` module. When inference
//! fails (e.g. a structurally invalid expression), the wrapper pushes
//! the underlying lower/infer errors and the rule short-circuits with
//! no further follow-up — the user already has a primary diagnostic
//! to act on.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{
    CompilerExtension, DeclareCtx, ValidateCtx, lower_and_infer_expression,
};
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element as ModelElement, PureModel};
use legend_pure_parser_pure::resolve::{is_multiplicity_compatible, is_subtype};
use legend_pure_parser_pure::types::{Multiplicity, ResolvedType, TypeExpr};
use smol_str::SmolStr;

use crate::ast::{
    AggregateSpecification, AggregationAwareClassMappingBody, AggregationFunctionSpec,
    ClassMapping, ClassMappingBody, EnumSourceValue, EnumerationClassMappingBody, MappingDef,
    NestedClassMapping, OperationClassMappingBody, PureClassMappingBody, PurePropertyMapping,
    XStoreClassMappingBody, XStorePropertyMapping,
};

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
            validate_mapping(&reg.def, &registry, ctx.model, ctx.auto_imports, ctx.errors);
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
    auto_imports: &[SmolStr],
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
    let visible_enum_mappings = visible_enum_mapping_targets(m, registry);
    for cm in &m.class_mappings {
        validate_class_mapping(
            cm,
            m,
            &visible_ids,
            &visible_enum_mappings,
            model,
            auto_imports,
            errors,
        );
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

    // One-root-per-class invariant. When a class is mapped by more
    // than one set-implementation directly within this mapping,
    // exactly one of those set-implementations must be marked root
    // with `*`. Mirrors Java's check (see TestRoot.testRootError):
    //   "The class 'X' is mapped by N set implementations and has
    //    M roots. There should be exactly one root set
    //    implementation for the class, and it should be marked
    //    with a '*'."
    //
    // Per-Mapping, *not* transitive across includes: TestRoot
    // .testRootWithInclude proves that two distinct included
    // mappings each contributing their own root is fine — the rule
    // applies only to the directly-defined class_mappings of one
    // Mapping. XStore bodies target Associations rather than
    // Classes, so they're excluded from the count.
    let mut by_class: HashMap<SmolStr, (usize, usize, &ClassMapping)> = HashMap::new();
    for cm in &m.class_mappings {
        if matches!(cm.body, ClassMappingBody::XStore(_)) {
            continue;
        }
        let class_fqn = ptr_fqn(&cm.class);
        let entry = by_class.entry(class_fqn).or_insert((0, 0, cm));
        entry.0 += 1;
        if cm.is_root {
            entry.1 += 1;
        }
    }
    for (class_fqn, (count, roots, first_cm)) in &by_class {
        if *count > 1 && *roots != 1 {
            errors.push(CompilationError {
                message: format!(
                    "The class '{class_fqn}' is mapped by {count} set implementations and has \
                     {roots} roots. There should be exactly one root set implementation for \
                     the class, and it should be marked with a '*'."
                ),
                // Pin the diagnostic to the first class-mapping for
                // this class — gives the user a stable file:line
                // hook into the offending block.
                source_info: first_cm.source_info.clone(),
                kind: CompilationErrorKind::DuplicateElement {
                    name: class_fqn.clone(),
                },
            });
        }
    }
}

fn validate_class_mapping(
    cm: &ClassMapping,
    owner: &MappingDef,
    visible_ids: &HashSet<SmolStr>,
    visible_enum_mappings: &HashMap<SmolStr, SmolStr>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    errors: &mut Vec<CompilationError>,
) {
    // Super-mapping reference: when present, must match a known ID.
    // (Independent of body kind — applies to Pure, Enumeration, …)
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

    // Target resolution + body-shape rules dispatch on the body kind.
    // Pure bodies want a Class target; Enumeration bodies want an
    // Enumeration target.
    let target_fqn = ptr_fqn(&cm.class);
    match &cm.body {
        ClassMappingBody::Pure(body) => {
            let class_id = resolve_class(model, &target_fqn);
            if class_id.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "Class mapping target '{target_fqn}' does not resolve to a Class"
                    ),
                    source_info: cm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: target_fqn.clone(),
                    },
                });
            }
            validate_pure_body(
                body,
                &target_fqn,
                class_id,
                visible_enum_mappings,
                model,
                auto_imports,
                errors,
            );
        }
        ClassMappingBody::Enumeration(body) => {
            let enum_id = resolve_enumeration(model, &target_fqn);
            if enum_id.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "EnumerationMapping target '{target_fqn}' does not resolve to an Enumeration"
                    ),
                    source_info: cm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: target_fqn.clone(),
                    },
                });
            }
            validate_enumeration_body(body, &target_fqn, enum_id, model, errors);
        }
        ClassMappingBody::Operation(body) => {
            // Operation bodies still want a Class target — the
            // function's job is to produce a SetImplementation[*]
            // for that class.
            if resolve_class(model, &target_fqn).is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "Class mapping target '{target_fqn}' does not resolve to a Class"
                    ),
                    source_info: cm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: target_fqn.clone(),
                    },
                });
            }
            validate_operation_body(body, visible_ids, model, errors);
        }
        ClassMappingBody::AggregationAware(body) => {
            // AggregationAware bodies also want a Class target —
            // each nested mapping (main + per-view aggregate)
            // inherits this class.
            let class_id = resolve_class(model, &target_fqn);
            if class_id.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "Class mapping target '{target_fqn}' does not resolve to a Class"
                    ),
                    source_info: cm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: target_fqn.clone(),
                    },
                });
            }
            validate_aggregation_aware_body(
                body,
                &target_fqn,
                class_id,
                visible_ids,
                visible_enum_mappings,
                model,
                auto_imports,
                errors,
            );
        }
        ClassMappingBody::XStore(body) => {
            // XStore reinterprets the outer FQN as an *Association*,
            // not a Class — that's the only point in the dispatch
            // where the Java's `classMapping` rule produces a
            // non-class set-implementation. The validator looks
            // properties up against the association's `properties`.
            let assoc_id = resolve_association(model, &target_fqn);
            if assoc_id.is_none() {
                errors.push(CompilationError {
                    message: format!(
                        "XStore mapping target '{target_fqn}' does not resolve to an Association"
                    ),
                    source_info: cm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: target_fqn.clone(),
                    },
                });
            }
            validate_xstore_body(body, &target_fqn, assoc_id, visible_ids, model, errors);
        }
    }
}

fn validate_pure_body(
    body: &PureClassMappingBody,
    target_class_fqn: &str,
    target_class_id: Option<ElementId>,
    visible_enum_mappings: &HashMap<SmolStr, SmolStr>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    errors: &mut Vec<CompilationError>,
) {
    // ~src must resolve to a Class.
    let src_class_id: Option<ElementId> = body.src_class.as_ref().and_then(|src| {
        let src_fqn = ptr_fqn(src);
        let resolved = resolve_class(model, &src_fqn);
        if resolved.is_none() {
            errors.push(CompilationError {
                message: format!("~src class '{src_fqn}' does not resolve to a Class"),
                source_info: src.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: src_fqn },
            });
        }
        resolved
    });

    // The "src" binding visible to filter / transform expressions —
    // only built when ~src resolved. Without it the inference would
    // fail with a "$src not found" cascade for every property
    // mapping, which would obscure the real diagnostic (the missing
    // ~src). When ~src is absent, we still type-check expressions
    // with no bindings.
    let src_binding: Vec<(SmolStr, TypeExpr, Multiplicity)> = src_class_id
        .map(|id| {
            vec![(
                SmolStr::new("src"),
                TypeExpr::Named {
                    element: id,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                Multiplicity::PureOne,
            )]
        })
        .unwrap_or_default();

    // ~filter must return Boolean[1].
    if let Some(filter) = &body.filter
        && let Some(filter_ty) =
            lower_and_infer_expression(model, auto_imports, filter, &src_binding, errors)
        && !is_boolean_one(model, &filter_ty)
    {
        errors.push(CompilationError {
            message: format!(
                "~filter must return Boolean[1] for class mapping of '{target_class_fqn}', \
                 got {}",
                describe_type(&filter_ty, model)
            ),
            source_info: filter.source_info().clone(),
            // TODO(error-kinds): replace UnsupportedExpression with a
            // dedicated `TypeMismatch { context, expected, actual }`
            // variant. UnsupportedExpression means "shape we don't
            // support yet" — a Boolean-vs-not-Boolean type mismatch
            // is a typed expression that's wrong, not unsupported.
            // The variant addition is a cross-crate change touching
            // error categorizers + surveyor reports; tracked
            // separately. For now the message string carries the
            // intent.
            kind: CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new_static("FilterReturnType"),
            },
        });
    }

    // Property mappings: name resolution + (when target resolves) type
    // and multiplicity compatibility for the transform.
    if let Some(class_id) = target_class_id {
        for pm in &body.property_mappings {
            validate_property_mapping(
                pm,
                target_class_fqn,
                class_id,
                visible_enum_mappings,
                model,
                auto_imports,
                &src_binding,
                errors,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_property_mapping(
    pm: &PurePropertyMapping,
    target_class_fqn: &str,
    target_class_id: ElementId,
    visible_enum_mappings: &HashMap<SmolStr, SmolStr>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    src_binding: &[(SmolStr, TypeExpr, Multiplicity)],
    errors: &mut Vec<CompilationError>,
) {
    // Look up the target property by name (with supertype walk).
    let prop_meta = find_property_type(model, target_class_id, pm.property_name.as_str());
    let Some((prop_type, prop_mult)) = prop_meta else {
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
        return;
    };

    // Inline `EnumerationMapping <name>` transformer rule. Mirrors
    // Java's TestModelMapping.testM2MMappingWithInvalidEnumerationMapping:
    //
    //   "Property : [state] is of type : [my::State] but enumeration
    //    mapping : [OptionMapping] is defined on enumeration : [my::Option]."
    //
    // Rules:
    //   1. The transformer name must reference an EnumerationMapping
    //      with a matching `mapping_name` somewhere in this Mapping
    //      or transitively included mappings.
    //   2. The property's declared type must be that EnumerationMapping's
    //      target enumeration. (When the property is typed as something
    //      other than the transformer's target enum, error.)
    //
    // We don't enforce that the property type IS an Enumeration as a
    // separate rule — the matching check subsumes it: if the property
    // type doesn't match the transformer's enum target, we error
    // regardless of whether the property is an Enum or a Class.
    if let Some(transformer_name) = &pm.transformer {
        match visible_enum_mappings.get(transformer_name) {
            None => {
                errors.push(CompilationError {
                    message: format!(
                        "EnumerationMapping '{transformer_name}' referenced from property \
                         '{}.{}' is not declared in this mapping or any included mapping",
                        target_class_fqn, pm.property_name
                    ),
                    source_info: pm.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: transformer_name.clone(),
                    },
                });
            }
            Some(target_enum_fqn) => {
                let prop_type_fqn = match &prop_type {
                    TypeExpr::Named { element, .. } => Some(element_fqn(model, *element)),
                    _ => None,
                };
                if prop_type_fqn.as_deref() != Some(target_enum_fqn.as_str()) {
                    let prop_type_str = prop_type_fqn.as_deref().unwrap_or("<non-named-type>");
                    errors.push(CompilationError {
                        message: format!(
                            "Property : [{}] is of type : [{prop_type_str}] but enumeration \
                             mapping : [{transformer_name}] is defined on enumeration : \
                             [{target_enum_fqn}].",
                            pm.property_name
                        ),
                        source_info: pm.source_info.clone(),
                        // TODO(error-kinds): same TypeMismatch refactor
                        // as the Stage-3.5 rules — wants its own kind.
                        kind: CompilationErrorKind::UnsupportedExpression {
                            kind: SmolStr::new_static("EnumerationMappingTypeMismatch"),
                        },
                    });
                }
            }
        }
    }

    // Lower + infer the transform with `src` bound.
    let Some(transform_ty) =
        lower_and_infer_expression(model, auto_imports, &pm.transform, src_binding, errors)
    else {
        // Underlying lower/infer errors were pushed by the wrapper;
        // skip the type/multiplicity follow-up rules — the user has a
        // primary diagnostic already.
        return;
    };

    // Subtype check (only meaningful for Named type expressions; other
    // shapes — generic, function-type, relation — are passed through
    // without complaint, mirroring the Java validator's behaviour for
    // less-common transform shapes).
    if let (
        TypeExpr::Named {
            element: prop_eid, ..
        },
        TypeExpr::Named {
            element: tx_eid, ..
        },
    ) = (&prop_type, &transform_ty.type_expr)
        && !is_subtype(*tx_eid, *prop_eid, model)
    {
        errors.push(CompilationError {
            message: format!(
                "Transform for property '{}.{}' has type {} which is not a subtype \
                 of the property's declared type {}",
                target_class_fqn,
                pm.property_name,
                describe_type(&transform_ty, model),
                describe_type_expr(&prop_type, model)
            ),
            source_info: pm.transform.source_info().clone(),
            // TODO(error-kinds): same as the filter rule above —
            // wants a dedicated `TypeMismatch` variant.
            kind: CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new_static("TransformReturnType"),
            },
        });
    }

    // Multiplicity check: transform's inferred multiplicity must fit
    // within the property's declared multiplicity range.
    if !is_multiplicity_compatible(Some(&transform_ty.multiplicity), &prop_mult) {
        errors.push(CompilationError {
            message: format!(
                "Transform for property '{}.{}' has multiplicity {} which is not compatible \
                 with the property's declared multiplicity {}",
                target_class_fqn,
                pm.property_name,
                describe_multiplicity(&transform_ty.multiplicity),
                describe_multiplicity(&prop_mult)
            ),
            source_info: pm.transform.source_info().clone(),
            // TODO(error-kinds): a multiplicity mismatch deserves its
            // own kind (`MultiplicityMismatch` or sibling of
            // `TypeMismatch`). Same cross-crate refactor as above.
            kind: CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new_static("TransformMultiplicity"),
            },
        });
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

/// Walk an element's parent-package chain to build its FQN as a
/// double-colon-joined string (e.g. `"my::test::State"`). Used by
/// the inline `EnumerationMapping` transformer rule to compare
/// against the user-written enumeration FQN. Skips the unnamed
/// root package.
fn element_fqn(model: &PureModel, id: ElementId) -> String {
    let ElementId::InstanceId { .. } = id else {
        // Packages don't appear in TypeExpr::Named.element in
        // practice (those are types, not packages); fall through
        // to the simple name on the off-chance a caller passes one.
        return model.element_name(id).to_string();
    };
    let node = model.get_node(id);
    let mut segments: Vec<SmolStr> = vec![node.name.clone()];
    let mut current = Some(node.parent_package);
    while let Some(pkg_id) = current {
        let pkg = model.get_package(pkg_id);
        if pkg.parent.is_none() {
            break; // root package has no name to contribute
        }
        segments.push(pkg.name.clone());
        current = pkg.parent;
    }
    segments.reverse();
    segments
        .iter()
        .map(SmolStr::as_str)
        .collect::<Vec<_>>()
        .join("::")
}

/// Build a `mapping_name → target enum FQN` lookup for every
/// EnumerationMapping reachable from `m` via own class_mappings +
/// transitively included mappings. Used by the inline
/// `EnumerationMapping <name>` transformer rule on Pure property
/// mappings to resolve the local name into a target enumeration.
fn visible_enum_mapping_targets(
    m: &MappingDef,
    registry: &HashMap<SmolStr, RegisteredMapping>,
) -> HashMap<SmolStr, SmolStr> {
    let mut out: HashMap<SmolStr, SmolStr> = HashMap::new();
    let mut visited: HashSet<SmolStr> = HashSet::new();
    let mut queue: VecDeque<&MappingDef> = VecDeque::new();
    queue.push_back(m);
    visited.insert(build_fqn(m));

    while let Some(cur) = queue.pop_front() {
        for cm in &cur.class_mappings {
            if matches!(cm.body, ClassMappingBody::Enumeration(_))
                && let Some(name) = &cm.mapping_name
            {
                // First-write-wins: if two mappings (one direct, one
                // included) declare the same name, the directly-defined
                // one is queued first and takes precedence.
                out.entry(name.clone())
                    .or_insert_with(|| ptr_fqn(&cm.class));
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
// Stage-4 — Enumeration body validation
// ---------------------------------------------------------------------------

fn validate_enumeration_body(
    body: &EnumerationClassMappingBody,
    target_enum_fqn: &str,
    target_enum_id: Option<ElementId>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Source-value type homogeneity: across all value mappings, the
    // source values must share the same kind. Mirrors the Java rule
    // in `MappingValidator.validateEnumerationMappings` ("only one
    // source Type is allowed for an Enumeration Mapping").
    //
    // `EnumSourceKind` derives `Eq`, so two enum-refs to *different*
    // source enumerations also count as mismatched (matching the Java
    // rule which compares source `Type`, not just kind).
    let mut anchor: Option<EnumSourceKind> = None;
    'outer: for vm in &body.value_mappings {
        for sv in &vm.source_values {
            let kind = source_kind(sv);
            match &anchor {
                None => anchor = Some(kind),
                Some(prev) if prev == &kind => {}
                Some(prev) => {
                    errors.push(CompilationError {
                        message: format!(
                            "EnumerationMapping for '{target_enum_fqn}' mixes source value \
                             kinds: {} and {} — all source values must share the same kind",
                            describe_source_kind(prev),
                            describe_source_kind(&kind)
                        ),
                        source_info: source_value_si(sv).clone(),
                        // TODO(error-kinds): warrants a dedicated
                        // `TypeMismatch` variant; same refactor as the
                        // Stage-3.5 type-check rules.
                        kind: CompilationErrorKind::UnsupportedExpression {
                            kind: SmolStr::new_static("EnumSourceKindMismatch"),
                        },
                    });
                    // Stop after the first mismatch to avoid cascade
                    // noise — the user fixes one source first.
                    break 'outer;
                }
            }
        }
    }

    // Per-target-value resolution: each enum_value_name must exist on
    // the target enumeration. Skip when target didn't resolve.
    let Some(enum_id) = target_enum_id else {
        return;
    };
    let value_set = enumeration_value_names(model, enum_id);
    for vm in &body.value_mappings {
        if !value_set.contains(vm.enum_value_name.as_str()) {
            errors.push(CompilationError {
                message: format!(
                    "Enumeration '{target_enum_fqn}' has no value '{}'",
                    vm.enum_value_name
                ),
                source_info: vm.source_info.clone(),
                // Reuses UnknownProperty kind — semantically the
                // closest existing variant. TODO(error-kinds):
                // see the same TODO sites at the type-mismatch rules.
                kind: CompilationErrorKind::UnknownProperty {
                    type_name: SmolStr::new(target_enum_fqn),
                    property_name: vm.enum_value_name.clone(),
                },
            });
        }

        // Source enum-refs themselves must resolve.
        for sv in &vm.source_values {
            if let EnumSourceValue::EnumRef {
                enumeration,
                value_name,
                source_info,
            } = sv
            {
                let src_enum_fqn = ptr_fqn(enumeration);
                match resolve_enumeration(model, &src_enum_fqn) {
                    None => {
                        errors.push(CompilationError {
                            message: format!(
                                "Source enum reference '{src_enum_fqn}.{value_name}' does not \
                                 resolve to an Enumeration"
                            ),
                            source_info: source_info.clone(),
                            kind: CompilationErrorKind::UnresolvedElement { path: src_enum_fqn },
                        });
                    }
                    Some(src_id) => {
                        let src_values = enumeration_value_names(model, src_id);
                        if !src_values.contains(value_name.as_str()) {
                            errors.push(CompilationError {
                                message: format!(
                                    "Enumeration '{src_enum_fqn}' has no value '{value_name}'"
                                ),
                                source_info: source_info.clone(),
                                kind: CompilationErrorKind::UnknownProperty {
                                    type_name: SmolStr::new(src_enum_fqn),
                                    property_name: value_name.clone(),
                                },
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Discriminator for source-value kinds used in the homogeneity check.
/// Derives `Eq` so two enum-refs to different source enumerations
/// compare unequal (the Java rule treats different source `Type`s as
/// a mismatch).
#[derive(Debug, Clone, Eq, PartialEq)]
enum EnumSourceKind {
    String,
    Integer,
    Enum(SmolStr),
}

fn source_kind(sv: &EnumSourceValue) -> EnumSourceKind {
    match sv {
        EnumSourceValue::String { .. } => EnumSourceKind::String,
        EnumSourceValue::Integer { .. } => EnumSourceKind::Integer,
        EnumSourceValue::EnumRef { enumeration, .. } => EnumSourceKind::Enum(ptr_fqn(enumeration)),
    }
}

fn describe_source_kind(k: &EnumSourceKind) -> String {
    match k {
        EnumSourceKind::String => "String".to_string(),
        EnumSourceKind::Integer => "Integer".to_string(),
        EnumSourceKind::Enum(fqn) => format!("enum '{fqn}'"),
    }
}

fn source_value_si(sv: &EnumSourceValue) -> &SourceInfo {
    match sv {
        EnumSourceValue::String { source_info, .. }
        | EnumSourceValue::Integer { source_info, .. }
        | EnumSourceValue::EnumRef { source_info, .. } => source_info,
    }
}

fn enumeration_value_names(model: &PureModel, enum_id: ElementId) -> HashSet<String> {
    let ModelElement::Enumeration(e) = model.get_element(enum_id) else {
        return HashSet::new();
    };
    e.values.iter().map(|v| v.name.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Stage-5 — Operation body validation
// ---------------------------------------------------------------------------

fn validate_operation_body(
    body: &OperationClassMappingBody,
    visible_ids: &HashSet<SmolStr>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // 1. Operation function path must resolve to a Function in the
    //    model via *exact-FQN* lookup. Mirrors Java's
    //    `OperationGraphBuilder.visitMapping` which always emits the
    //    mangled FQN (e.g. `a__SetImplementation_MANY_`) into the
    //    `operation` ImportStub. Accepting a simple-name fallback
    //    here would silently pick the first overload registered in
    //    the package — possibly the wrong function entirely — so
    //    we deliberately don't.
    let fn_fqn = ptr_fqn(&body.operation);
    let segments: Vec<SmolStr> = fn_fqn.split("::").map(SmolStr::new).collect();
    let resolved = model
        .resolve_by_path(&segments)
        .filter(|id| matches!(model.get_element(*id), ModelElement::Function(_)));
    if resolved.is_none() {
        errors.push(CompilationError {
            message: format!("Operation function '{fn_fqn}' does not resolve to a known function"),
            source_info: body.operation.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement { path: fn_fqn },
        });
    }
    // TODO(stage-5+): once the function is resolvable, also check its
    // signature is `OperationSetImplementation[1] -> SetImplementation[*]`.
    // The metamodel constraint is at mapping.pure:108 — same shape
    // every operation function must satisfy. Not done here because
    // the FunctionType-against-target shape comparison overlaps with
    // the lower-and-infer machinery the Stage-3.5 rules rely on; a
    // dedicated helper belongs in `legend_pure_parser_pure::resolve`.

    // 2. Each parameter ID must reference a class-mapping ID visible
    //    in this mapping (its own class mappings + transitively
    //    included). Mirrors `OperationSetImplementationProcessor.process`
    //    which throws `"The SetImplementation '<id>' can't be found
    //    in the mapping '<m>'"` when a parameter doesn't resolve.
    for param in &body.parameters {
        if !visible_ids.contains(&param.id) {
            errors.push(CompilationError {
                message: format!(
                    "Operation parameter '{}' does not reference any class-mapping ID in \
                     this mapping or any included mapping",
                    param.id
                ),
                source_info: param.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: param.id.clone(),
                },
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Stage-6 — AggregationAware body validation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn validate_aggregation_aware_body(
    body: &AggregationAwareClassMappingBody,
    target_class_fqn: &str,
    target_class_id: Option<ElementId>,
    visible_ids: &HashSet<SmolStr>,
    visible_enum_mappings: &HashMap<SmolStr, SmolStr>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    errors: &mut Vec<CompilationError>,
) {
    // 1. Java's AggregationAwareValidator iterates per
    //    `aggregateValues` entry and rejects mapFn/aggregateFn whose
    //    return type isn't a DataType (primitive type / enumeration).
    //    Mirror that here: lower each function with `this` bound to
    //    the outer class, infer its return type, then check it's a
    //    DataType. The aggregateFn additionally has `mapped` bound
    //    to the mapFn's return shape (Java wraps the mapped value
    //    into a synthesized `mapped` parameter — same idea here).
    for view in &body.views {
        validate_model_operation(
            &view.model_operation,
            target_class_fqn,
            target_class_id,
            model,
            auto_imports,
            errors,
        );
        // 2. Recursively validate the per-view aggregate mapping.
        validate_nested_class_mapping(
            &view.aggregate_mapping,
            target_class_fqn,
            target_class_id,
            visible_ids,
            visible_enum_mappings,
            model,
            auto_imports,
            errors,
        );
    }
    // 3. Recursively validate the main mapping fall-through.
    validate_nested_class_mapping(
        &body.main_mapping,
        target_class_fqn,
        target_class_id,
        visible_ids,
        visible_enum_mappings,
        model,
        auto_imports,
        errors,
    );
}

fn validate_model_operation(
    spec: &AggregateSpecification,
    target_class_fqn: &str,
    target_class_id: Option<ElementId>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    errors: &mut Vec<CompilationError>,
) {
    // Without a resolved target class we can't bind `this`; the
    // outer "target doesn't resolve" diagnostic already fired, so
    // just skip the type-check follow-up here to avoid a $this-not-
    // found cascade.
    let Some(class_id) = target_class_id else {
        return;
    };
    let this_binding: Vec<(SmolStr, TypeExpr, Multiplicity)> = vec![(
        SmolStr::new("this"),
        TypeExpr::Named {
            element: class_id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        },
        Multiplicity::PureOne,
    )];

    for av in &spec.aggregate_values {
        validate_aggregate_value(
            av,
            target_class_fqn,
            model,
            auto_imports,
            &this_binding,
            errors,
        );
    }
}

fn validate_aggregate_value(
    av: &AggregationFunctionSpec,
    target_class_fqn: &str,
    model: &PureModel,
    auto_imports: &[SmolStr],
    this_binding: &[(SmolStr, TypeExpr, Multiplicity)],
    errors: &mut Vec<CompilationError>,
) {
    // mapFn: must return a DataType (primitive / enumeration).
    let mapped_ty =
        lower_and_infer_expression(model, auto_imports, &av.map_fn, this_binding, errors);
    if let Some(ty) = &mapped_ty
        && !is_data_type(model, ty)
    {
        errors.push(CompilationError {
            message: format!(
                "AggregationAware ~mapFn for class '{target_class_fqn}' must return a DataType \
                 (primitive type / enumeration), got {}",
                describe_type(ty, model)
            ),
            source_info: av.map_fn.source_info().clone(),
            // TODO(error-kinds): see the same TypeMismatch TODO at
            // the Stage-3.5 filter rule — wants its own variant.
            kind: CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new_static("AggregateMapFnReturnType"),
            },
        });
    }

    // aggregateFn: bound `mapped` is the mapFn's return shape (or
    // `Any[*]` if mapFn inference failed). Java synthesises the
    // bound from the mapFn's return type.
    let mapped_binding: Vec<(SmolStr, TypeExpr, Multiplicity)> = match mapped_ty {
        Some(ref ty) => vec![(
            SmolStr::new("mapped"),
            ty.type_expr.clone(),
            Multiplicity::ZeroOrMany,
        )],
        None => Vec::new(),
    };
    let agg_ty = lower_and_infer_expression(
        model,
        auto_imports,
        &av.aggregate_fn,
        &mapped_binding,
        errors,
    );
    if let Some(ty) = &agg_ty
        && !is_data_type(model, ty)
    {
        errors.push(CompilationError {
            message: format!(
                "AggregationAware ~aggregateFn for class '{target_class_fqn}' must return a \
                 DataType (primitive type / enumeration), got {}",
                describe_type(ty, model)
            ),
            source_info: av.aggregate_fn.source_info().clone(),
            kind: CompilationErrorKind::UnsupportedExpression {
                kind: SmolStr::new_static("AggregateAggregateFnReturnType"),
            },
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_nested_class_mapping(
    nested: &NestedClassMapping,
    target_class_fqn: &str,
    target_class_id: Option<ElementId>,
    visible_ids: &HashSet<SmolStr>,
    visible_enum_mappings: &HashMap<SmolStr, SmolStr>,
    model: &PureModel,
    auto_imports: &[SmolStr],
    errors: &mut Vec<CompilationError>,
) {
    // Reuse the body-kind-specific validators. The nested mapping
    // inherits the outer class FQN/id so we don't re-resolve it.
    match &nested.body {
        ClassMappingBody::Pure(body) => {
            validate_pure_body(
                body,
                target_class_fqn,
                target_class_id,
                visible_enum_mappings,
                model,
                auto_imports,
                errors,
            );
        }
        ClassMappingBody::Enumeration(body) => {
            // Enumeration nested under AggregationAware doesn't make
            // sense (the outer target is a Class, not an Enumeration)
            // — Java's grammar accepts it textually but the validator
            // would reject the outer target shape elsewhere. Nothing
            // useful to add here beyond what the Stage-4 validator
            // already does.
            validate_enumeration_body(body, target_class_fqn, None, model, errors);
        }
        ClassMappingBody::Operation(body) => {
            validate_operation_body(body, visible_ids, model, errors);
        }
        ClassMappingBody::AggregationAware(body) => {
            // Recursive case: AggregationAware nested under
            // AggregationAware. Unusual but the grammar allows it.
            validate_aggregation_aware_body(
                body,
                target_class_fqn,
                target_class_id,
                visible_ids,
                visible_enum_mappings,
                model,
                auto_imports,
                errors,
            );
        }
        ClassMappingBody::XStore(_) => {
            // XStore nested under AggregationAware is semantically
            // unreachable: the outer target is a Class (validated
            // up-front in validate_aggregation_aware_body) and
            // XStore expects an Association. Emit a single pointed
            // diagnostic rather than running the XStore body rules
            // with a Class FQN they'd misuse in their messages.
            errors.push(CompilationError {
                message: format!(
                    "XStore body is not valid inside an AggregationAware ~mainMapping or \
                     ~aggregateMapping (target '{target_class_fqn}' is a Class, not an \
                     Association)"
                ),
                source_info: nested.source_info.clone(),
                kind: CompilationErrorKind::UnsupportedExpression {
                    kind: SmolStr::new_static("XStoreNestedUnderAggregationAware"),
                },
            });
        }
    }
}

fn is_data_type(model: &PureModel, ty: &ResolvedType) -> bool {
    let TypeExpr::Named { element, .. } = ty.type_expr else {
        return false;
    };
    matches!(
        model.get_element(element),
        ModelElement::PrimitiveType(_) | ModelElement::Enumeration(_)
    )
}

// ---------------------------------------------------------------------------
// Stage-7 — XStore body validation
// ---------------------------------------------------------------------------

fn validate_xstore_body(
    body: &XStoreClassMappingBody,
    target_assoc_fqn: &str,
    target_assoc_id: Option<ElementId>,
    visible_ids: &HashSet<SmolStr>,
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Collect the association's property names once. When the
    // outer target didn't resolve, treat the property set as empty
    // so the per-entry property-existence check still emits a
    // pointed diagnostic per entry rather than silently passing.
    let property_names: HashSet<String> = target_assoc_id
        .map(|id| association_property_names(model, id))
        .unwrap_or_default();

    for pm in &body.property_mappings {
        validate_xstore_property_mapping(
            pm,
            target_assoc_fqn,
            &property_names,
            target_assoc_id,
            visible_ids,
            errors,
        );
    }
}

fn validate_xstore_property_mapping(
    pm: &XStorePropertyMapping,
    target_assoc_fqn: &str,
    property_names: &HashSet<String>,
    target_assoc_id: Option<ElementId>,
    visible_ids: &HashSet<SmolStr>,
    errors: &mut Vec<CompilationError>,
) {
    // Property name must exist on the association — only when the
    // association resolved. Without a resolved association the
    // outer "doesn't resolve" diagnostic already fired, so skip
    // the cascade.
    if target_assoc_id.is_some() && !property_names.contains(pm.property_name.as_str()) {
        errors.push(CompilationError {
            message: format!(
                "Association '{target_assoc_fqn}' has no property '{}'",
                pm.property_name
            ),
            source_info: pm.source_info.clone(),
            kind: CompilationErrorKind::UnknownProperty {
                type_name: SmolStr::new(target_assoc_fqn),
                property_name: pm.property_name.clone(),
            },
        });
    }

    // Source / target set-impl IDs are required at validate time
    // even though the parser accepts the bare `propName : expr`
    // form. Mirrors Java's split: M3Parser.g4 makes
    // `sourceAndTargetMappingId` optional syntactically, but
    // XStoreProcessor's `MappingValidator.validateId(...
    // ._sourceSetImplementationId(), ...)` throws when the id is
    // null. Without the IDs the crossExpression has nothing to bind
    // `$this`/`$that` to, so the structural problem belongs at
    // validate time, not later.
    match (&pm.source_set_impl_id, &pm.target_set_impl_id) {
        (None, None) => {
            errors.push(CompilationError {
                message: format!(
                    "XStore property mapping '{}.{}' requires source and target \
                     set-implementation IDs (`propName[srcId, tgtId] : crossExpr`)",
                    target_assoc_fqn, pm.property_name
                ),
                source_info: pm.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: pm.property_name.clone(),
                },
            });
        }
        (None, Some(_)) | (Some(_), None) => {
            errors.push(CompilationError {
                message: format!(
                    "XStore property mapping '{}.{}' requires both source and target \
                     set-implementation IDs (`propName[srcId, tgtId] : crossExpr`); only one \
                     was supplied",
                    target_assoc_fqn, pm.property_name
                ),
                source_info: pm.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement {
                    path: pm.property_name.clone(),
                },
            });
        }
        (Some(_), Some(_)) => {} // both present — fall through to visibility check
    }

    // Each present ID must reference a class-mapping ID visible in
    // this mapping or its (transitive) includes. Mirrors
    // `MappingValidator.validateId` which the Java XStoreProcessor
    // invokes per property mapping.
    if let Some(src_id) = &pm.source_set_impl_id
        && !visible_ids.contains(src_id)
    {
        errors.push(CompilationError {
            message: format!(
                "XStore source set-implementation '{src_id}' on '{}.{}' is not declared in \
                 this mapping or any included mapping",
                target_assoc_fqn, pm.property_name
            ),
            source_info: pm.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: src_id.clone(),
            },
        });
    }
    if let Some(tgt_id) = &pm.target_set_impl_id
        && !visible_ids.contains(tgt_id)
    {
        errors.push(CompilationError {
            message: format!(
                "XStore target set-implementation '{tgt_id}' on '{}.{}' is not declared in \
                 this mapping or any included mapping",
                target_assoc_fqn, pm.property_name
            ),
            source_info: pm.source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement {
                path: tgt_id.clone(),
            },
        });
    }
    // TODO(stage-7+): once src/tgt set-impls are resolved, lower the
    // crossExpression with `this`/`that` bindings derived from each
    // referenced set-impl's class. Java's XStoreProcessor builds
    // those VariableExpressions and lowers the lambda; this Rust
    // port intentionally stops at structural validation for Stage 7.
}

fn association_property_names(model: &PureModel, assoc_id: ElementId) -> HashSet<String> {
    let ModelElement::Association(a) = model.get_element(assoc_id) else {
        return HashSet::new();
    };
    a.properties.iter().map(|p| p.name.to_string()).collect()
}

fn resolve_association(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    let id = model.resolve_by_path(&segments)?;
    matches!(model.get_element(id), ModelElement::Association(_)).then_some(id)
}

// ---------------------------------------------------------------------------
// Class / Enumeration / property lookup
// ---------------------------------------------------------------------------

fn resolve_class(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    let id = model.resolve_by_path(&segments)?;
    matches!(model.get_element(id), ModelElement::Class(_)).then_some(id)
}

fn resolve_enumeration(model: &PureModel, fqn: &str) -> Option<ElementId> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if segments.is_empty() || segments.iter().any(smol_str::SmolStr::is_empty) {
        return None;
    }
    let id = model.resolve_by_path(&segments)?;
    matches!(model.get_element(id), ModelElement::Enumeration(_)).then_some(id)
}

/// Walks the class (and its supertypes via `super_types`) looking for
/// a simple property named `prop_name`. Returns the property's
/// `(type, multiplicity)` for downstream type-compatibility checks.
/// Qualified properties carry a function-type rather than a direct
/// type, so they're returned as `None` here — those are skipped in
/// the type-compatibility rules.
fn find_property_type(
    model: &PureModel,
    class_id: ElementId,
    prop_name: &str,
) -> Option<(TypeExpr, Multiplicity)> {
    let mut visited: HashSet<ElementId> = HashSet::new();
    let mut stack: Vec<ElementId> = vec![class_id];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let ModelElement::Class(c) = model.get_element(id) else {
            continue;
        };
        if let Some(p) = c.properties.iter().find(|p| p.name == prop_name) {
            return Some((p.type_expr.clone(), p.multiplicity.clone()));
        }
        // Qualified properties resolve as "exists" but we don't
        // type-check transforms against them.
        if c.qualified_properties.iter().any(|q| q.name == prop_name) {
            return None;
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                stack.push(*element);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Type / multiplicity diagnostics
// ---------------------------------------------------------------------------

fn is_boolean_one(model: &PureModel, ty: &ResolvedType) -> bool {
    if ty.multiplicity != Multiplicity::PureOne {
        return false;
    }
    let TypeExpr::Named { element, .. } = ty.type_expr else {
        return false;
    };
    matches!(model.get_element(element), ModelElement::PrimitiveType(_))
        && model.element_name(element) == "Boolean"
}

fn describe_type(ty: &ResolvedType, model: &PureModel) -> String {
    format!(
        "{}[{}]",
        describe_type_expr(&ty.type_expr, model),
        describe_multiplicity(&ty.multiplicity)
    )
}

fn describe_type_expr(expr: &TypeExpr, model: &PureModel) -> String {
    match expr {
        TypeExpr::Named { element, .. } => model.element_name(*element).to_string(),
        TypeExpr::Generic(name) => name.to_string(),
        TypeExpr::FunctionType { .. } => "<FunctionType>".to_string(),
        TypeExpr::Relation(_) => "<RelationType>".to_string(),
        TypeExpr::AlgebraUnion(a, b) => format!(
            "{}|{}",
            describe_type_expr(a, model),
            describe_type_expr(b, model)
        ),
        TypeExpr::Unresolved => "<unresolved>".to_string(),
    }
}

fn describe_multiplicity(m: &Multiplicity) -> String {
    match m {
        Multiplicity::PureOne => "1".to_string(),
        Multiplicity::ZeroOrOne => "0..1".to_string(),
        Multiplicity::ZeroOrMany => "*".to_string(),
        Multiplicity::OneOrMany => "1..*".to_string(),
        Multiplicity::Range { lower, upper } => match upper {
            Some(u) => format!("{lower}..{u}"),
            None => format!("{lower}..*"),
        },
        Multiplicity::Variable(name) => name.to_string(),
    }
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
