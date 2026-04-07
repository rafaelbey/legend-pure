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

//! Compiler pipeline: AST → `PureModel`.
//!
//! Orchestrates the multi-pass compilation of parsed AST source files into
//! a fully resolved [`PureModel`].
//!
//! # Pipeline Phases
//!
//! 1. **Declaration (Pass 1)** — Iterate all AST elements, assign `ElementId`s,
//!    allocate shells, build the global package tree.
//! 2. **Topological Sort (Pass 1.5)** — Build DAG from hard dependencies
//!    (supertypes), topologically sort via Kahn's algorithm.
//!    Cyclic inheritance = compilation error.
//! 3. **Definition (Pass 2)** — Hydrate shells in topological order,
//!    resolving soft dependencies to existing shells.
//! 4. **Freeze** — Call `rebuild_derived_indexes()`.
//! 5. **Validation (Pass 3)** — Read-only pass on the frozen model.

use std::collections::{HashMap, VecDeque};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::element as ast;
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::Package as AstPackage;
use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, ElementNode, ModelChunk, PureModel};
use crate::nodes::class::{self, Class};
use crate::nodes::enumeration::{EnumValue, Enumeration};
use crate::nodes::function::Function;
use crate::nodes::measure::Measure;
use crate::nodes::profile::Profile;
use crate::nodes::association::Association;
use crate::nodes::unit::Unit;
use crate::resolve::{self, ResolutionContext};
use crate::types::{Multiplicity, TypeExpr};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compiles a list of parsed source files into a `PureModel`.
///
/// This is the main entry point for the compiler pipeline.
///
/// # Errors
///
/// Returns compilation errors for unresolved types, cyclic inheritance, etc.
pub fn compile(source_files: &[SourceFile]) -> Result<PureModel, Vec<CompilationError>> {
    let mut model = PureModel::new();

    // Chunk 0 — bootstrap primitives
    let bootstrap_chunk = bootstrap::create_bootstrap_chunk(model.root_package);
    model.chunks.push(bootstrap_chunk);

    // Register bootstrap elements in the root package
    for local_idx in 0..model.chunks[0].nodes.len() {
        let eid = ElementId { chunk_id: 0, local_idx };
        model.register_element(model.root_package, eid);
    }

    let mut errors = Vec::new();

    // ---- Pass 1: Declaration ----
    let (declarations, unit_mappings) = pass_declare(source_files, &mut model, &mut errors);

    // ---- Pass 1.5: Topological Sort ----
    let sorted = pass_topo_sort(&declarations, source_files, &model, &mut errors);

    // ---- Pass 2: Definition ----
    pass_define(&sorted, source_files, &declarations, &unit_mappings, &mut model, &mut errors);

    // ---- Freeze ----
    model.rebuild_derived_indexes();

    // ---- Pass 3: Validation ----
    errors.extend(crate::validate::validate(&model));

    if errors.is_empty() {
        Ok(model)
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Declaration — a record of what was declared
// ---------------------------------------------------------------------------

/// A single declared element, linking its AST source to its assigned ID.
#[derive(Debug, Clone)]
struct Declaration {
    /// Assigned element ID.
    id: ElementId,
    /// Index into the `source_files` array.
    file_idx: usize,
    /// Index of the section within the source file.
    section_idx: usize,
    /// Index of the element within the section.
    element_idx: usize,
}

/// Tracks unit `ElementId`s allocated for a measure during Pass 1.
#[derive(Debug, Clone)]
struct UnitMapping {
    /// The canonical unit's ElementId, if present.
    canonical: Option<ElementId>,
    /// Non-canonical unit ElementIds, in order.
    non_canonical: Vec<ElementId>,
}

// ---------------------------------------------------------------------------
// Pass 1: Declaration
// ---------------------------------------------------------------------------

/// Pass 1 — assigns `ElementId`s, allocates element shells, and builds the
/// package tree.
///
/// Returns a map from fully qualified name to Declaration, and a map from
/// measure `ElementId` to its allocated unit `ElementId`s.
fn pass_declare(
    source_files: &[SourceFile],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (HashMap<SmolStr, Declaration>, HashMap<ElementId, UnitMapping>) {
    let mut declarations = HashMap::new();
    let mut unit_mappings: HashMap<ElementId, UnitMapping> = HashMap::new();
    #[allow(clippy::cast_possible_truncation)] // chunks.len() is bounded by u16 in practice
    let chunk_id = model.chunks.len() as u16;
    let mut chunk = ModelChunk::new(chunk_id);

    for (file_idx, source_file) in source_files.iter().enumerate() {
        for (section_idx, section) in source_file.sections.iter().enumerate() {
            for (element_idx, element) in section.elements.iter().enumerate() {
                let name = ast_element_name(element);
                let source_info = ast_element_source(element);

                // Resolve package path
                let pkg_path = ast_element_package_path(element);
                let package_id = if pkg_path.is_empty() {
                    model.root_package
                } else {
                    model.get_or_create_package(&pkg_path)
                };

                // Build fully qualified name
                let fqn = build_fqn(&pkg_path, &name);

                // Check for duplicates
                if declarations.contains_key(&fqn) {
                    errors.push(CompilationError {
                        message: format!("Duplicate element: '{fqn}'"),
                        source_info: source_info.clone(),
                        kind: CompilationErrorKind::DuplicateElement { name: fqn.clone() },
                    });
                    continue;
                }

                // Allocate shell
                let shell = create_shell(element);
                let local_idx = chunk.alloc_element(
                    ElementNode {
                        name: name.clone(),
                        source_info: source_info.clone(),
                        parent_package: package_id,
                    },
                    shell,
                );

                let id = ElementId { chunk_id, local_idx };
                model.register_element(package_id, id);

                declarations.insert(fqn.clone(), Declaration {
                    id,
                    file_idx,
                    section_idx,
                    element_idx,
                });

                // For Measures: allocate Unit shells now
                if let ast::Element::Measure(measure_def) = element {
                    let mapping = allocate_unit_shells(
                        measure_def, id, &fqn, chunk_id, package_id,
                        &mut chunk, model, &mut declarations,
                        file_idx, section_idx, element_idx,
                    );
                    unit_mappings.insert(id, mapping);
                }
            }
        }
    }

    model.chunks.push(chunk);
    (declarations, unit_mappings)
}

/// Allocates `Element::Unit` shells for each unit in a measure definition.
///
/// Units get their own `ElementId` so they can be referenced in type positions
/// (e.g., `prop: Kilogram[1]`). Each unit is registered in the same package
/// as the parent measure.
fn allocate_unit_shells(
    measure_def: &ast::MeasureDef,
    measure_id: ElementId,
    measure_fqn: &str,
    chunk_id: u16,
    package_id: crate::ids::PackageId,
    chunk: &mut ModelChunk,
    model: &mut PureModel,
    declarations: &mut HashMap<SmolStr, Declaration>,
    file_idx: usize,
    section_idx: usize,
    element_idx: usize,
) -> UnitMapping {
    let mut canonical = None;
    let mut non_canonical = Vec::new();

    // Helper to allocate a single unit
    let mut alloc_unit = |unit_def: &ast::UnitDef| -> ElementId {
        let unit_name = &unit_def.name;
        let unit_fqn = SmolStr::new(format!("{measure_fqn}~{unit_name}"));

        let unit_shell = Element::Unit(Unit {
            measure: measure_id,
            conversion_expression: None, // Hydrated in Pass 2
        });

        let local_idx = chunk.alloc_element(
            ElementNode {
                name: unit_name.clone(),
                source_info: unit_def.source_info.clone(),
                parent_package: package_id,
            },
            unit_shell,
        );

        let unit_id = ElementId { chunk_id, local_idx };
        model.register_element(package_id, unit_id);

        declarations.insert(unit_fqn, Declaration {
            id: unit_id,
            file_idx,
            section_idx,
            element_idx,
        });

        unit_id
    };

    // Canonical unit
    if let Some(ref canon) = measure_def.canonical_unit {
        canonical = Some(alloc_unit(canon));
    }

    // Non-canonical units
    for unit_def in &measure_def.non_canonical_units {
        non_canonical.push(alloc_unit(unit_def));
    }

    UnitMapping { canonical, non_canonical }
}


// ---------------------------------------------------------------------------
// Pass 1.5: Topological Sort (Hard Dependencies)
// ---------------------------------------------------------------------------

/// Pass 1.5 — builds a dependency DAG from supertypes and sorts via Kahn's algorithm.
///
/// Returns an ordered list of element IDs safe for definition.
fn pass_topo_sort(
    declarations: &HashMap<SmolStr, Declaration>,
    source_files: &[SourceFile],
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) -> Vec<ElementId> {
    // Build adjacency list: id → list of hard dependencies (supertypes)
    let mut in_degree: HashMap<ElementId, usize> = HashMap::new();
    let mut dependents: HashMap<ElementId, Vec<ElementId>> = HashMap::new();

    for decl in declarations.values() {
        in_degree.entry(decl.id).or_insert(0);
    }

    for decl in declarations.values() {
        let element = get_ast_element(source_files, decl);
        let hard_deps = extract_hard_dependencies(element, declarations, model);

        for dep_id in hard_deps {
            dependents.entry(dep_id).or_default().push(decl.id);
            *in_degree.entry(decl.id).or_insert(0) += 1;
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<ElementId> = in_degree.iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut sorted = Vec::with_capacity(declarations.len());

    while let Some(id) = queue.pop_front() {
        sorted.push(id);
        if let Some(deps) = dependents.get(&id) {
            for &dep_id in deps {
                let deg = in_degree.get_mut(&dep_id).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(dep_id);
                }
            }
        }
    }

    // Check for cycles
    if sorted.len() < declarations.len() {
        let cyclic: Vec<_> = in_degree.iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(&id, _)| id)
            .collect();

        for id in &cyclic {
            let node = model.get_node(*id);
            errors.push(CompilationError {
                message: format!(
                    "Cyclic inheritance detected involving '{}'",
                    node.name
                ),
                source_info: node.source_info.clone(),
                kind: CompilationErrorKind::CyclicInheritance {
                    element_name: node.name.clone(),
                },
            });
        }
    }

    sorted
}

/// Extracts hard dependencies (supertypes) from an AST element.
fn extract_hard_dependencies(
    element: &ast::Element,
    declarations: &HashMap<SmolStr, Declaration>,
    _model: &PureModel,
) -> Vec<ElementId> {
    match element {
        ast::Element::Class(class_def) => {
            class_def.super_types.iter()
                .filter_map(|type_ref| {
                    let fqn = SmolStr::new(type_ref.full_path());
                    declarations.get(&fqn).map(|d| d.id)
                })
                .collect()
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Pass 2: Definition
// ---------------------------------------------------------------------------

/// Pass 2 — hydrates shells in topological order.
fn pass_define(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    declarations: &HashMap<SmolStr, Declaration>,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) {
    // Build reverse lookup: ElementId → Declaration
    let id_to_decl: HashMap<ElementId, &Declaration> = declarations.values()
        .map(|d| (d.id, d))
        .collect();

    // Build resolution context: FQN → ElementId (for resolve.rs)
    let decl_ids: HashMap<SmolStr, ElementId> = declarations.iter()
        .map(|(fqn, d)| (fqn.clone(), d.id))
        .collect();

    for &id in sorted {
        let Some(decl) = id_to_decl.get(&id) else { continue };
        let ast_element = get_ast_element(source_files, decl);

        let ctx = ResolutionContext {
            declarations: &decl_ids,
            model,
        };

        let hydrated = hydrate_element(ast_element, id, unit_mappings, &ctx, errors);

        // Replace the shell in the chunk
        let chunk = &mut model.chunks[id.chunk_id as usize];
        *chunk.elements.get_mut(id.local_idx) = hydrated;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates an empty shell for an AST element.
fn create_shell(element: &ast::Element) -> Element {
    match element {
        ast::Element::Class(_) => Element::Class(Class {
            type_parameters: vec![],
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }),
        ast::Element::Enumeration(_) => Element::Enumeration(Enumeration {
            values: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }),
        ast::Element::Function(_) => Element::Function(Function {
            parameters: vec![],
            return_type: TypeExpr::Named {
                element: bootstrap::ANY_ID,
                type_arguments: vec![],
                value_arguments: vec![],
            },
            return_multiplicity: Multiplicity::PureOne,
            body: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }),
        ast::Element::Profile(_) => Element::Profile(Profile {
            stereotypes: vec![],
            tags: vec![],
        }),
        ast::Element::Association(_) => Element::Association(Association {
            properties: vec![],
            qualified_properties: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }),
        ast::Element::Measure(_) => Element::Measure(Measure {
            canonical_unit: None,
            non_canonical_units: vec![],
        }),
    }
}

/// Hydrates an AST element into its full Pure representation.
///
/// This resolves type references (properties, parameters, return types),
/// annotations (stereotypes, tagged values), and structural fields.
/// Expression bodies remain as placeholders — full expression lowering
/// is deferred to Phase 4+.
#[allow(clippy::too_many_lines)]
fn hydrate_element(
    element: &ast::Element,
    element_id: ElementId,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Element {
    match element {
        ast::Element::Class(class_def) => {
            // Super types
            let super_types: Vec<TypeExpr> = class_def.super_types.iter()
                .filter_map(|type_ref| resolve::resolve_type_ref(type_ref, ctx, errors))
                .collect();

            // Properties
            let properties = lower_properties(&class_def.properties, ctx, errors);

            // Qualified properties
            let qualified_properties = lower_qualified_properties(
                &class_def.qualified_properties, ctx, errors,
            );

            // Constraints
            let constraints = lower_constraints(&class_def.constraints);

            // Annotations
            let stereotypes = resolve::resolve_stereotypes(
                &class_def.stereotypes, ctx, errors,
            );
            let tagged_values = resolve::resolve_tagged_values(
                &class_def.tagged_values, ctx, errors,
            );

            Element::Class(Class {
                type_parameters: class_def.type_parameters.clone(),
                super_types,
                properties,
                qualified_properties,
                constraints,
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Enumeration(enum_def) => {
            let stereotypes = resolve::resolve_stereotypes(
                &enum_def.stereotypes, ctx, errors,
            );
            let tagged_values = resolve::resolve_tagged_values(
                &enum_def.tagged_values, ctx, errors,
            );

            Element::Enumeration(Enumeration {
                values: enum_def.values.iter()
                    .map(|v| {
                        let v_stereos = resolve::resolve_stereotypes(
                            &v.stereotypes, ctx, errors,
                        );
                        let v_tvs = resolve::resolve_tagged_values(
                            &v.tagged_values, ctx, errors,
                        );
                        EnumValue {
                            name: v.name.clone(),
                            source_info: v.source_info.clone(),
                            stereotypes: v_stereos,
                            tagged_values: v_tvs,
                        }
                    })
                    .collect(),
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Profile(prof_def) => {
            Element::Profile(Profile {
                stereotypes: prof_def.stereotypes.iter()
                    .map(|s| s.value.clone())
                    .collect(),
                tags: prof_def.tags.iter()
                    .map(|t| t.value.clone())
                    .collect(),
            })
        }
        ast::Element::Function(func_def) => {
            let parameters = lower_parameters(&func_def.parameters, ctx, errors);
            let return_type = resolve::resolve_type_spec(
                &func_def.return_type, ctx, errors,
            ).unwrap_or(TypeExpr::Named {
                element: bootstrap::ANY_ID,
                type_arguments: vec![],
                value_arguments: vec![],
            });
            let return_multiplicity = resolve::lower_multiplicity(
                &func_def.return_multiplicity,
            );
            let stereotypes = resolve::resolve_stereotypes(
                &func_def.stereotypes, ctx, errors,
            );
            let tagged_values = resolve::resolve_tagged_values(
                &func_def.tagged_values, ctx, errors,
            );

            Element::Function(Function {
                parameters,
                return_type,
                return_multiplicity,
                body: vec![], // Expression lowering deferred
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Association(assoc_def) => {
            let properties = lower_properties(&assoc_def.properties, ctx, errors);
            let qualified_properties = lower_qualified_properties(
                &assoc_def.qualified_properties, ctx, errors,
            );
            let stereotypes = resolve::resolve_stereotypes(
                &assoc_def.stereotypes, ctx, errors,
            );
            let tagged_values = resolve::resolve_tagged_values(
                &assoc_def.tagged_values, ctx, errors,
            );

            Element::Association(Association {
                properties,
                qualified_properties,
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Measure(_measure_def) => {
            // Unit ElementIds were allocated in Pass 1; look them up
            let mapping = unit_mappings.get(&element_id);
            Element::Measure(Measure {
                canonical_unit: mapping.and_then(|m| m.canonical),
                non_canonical_units: mapping
                    .map(|m| m.non_canonical.clone())
                    .unwrap_or_default(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Property Lowering Helpers
// ---------------------------------------------------------------------------

/// Lowers AST properties to Pure properties.
fn lower_properties(
    props: &[ast::Property],
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::Property> {
    props.iter()
        .filter_map(|p| {
            let type_expr = resolve::resolve_type_spec(&p.type_ref, ctx, errors)?;
            let multiplicity = resolve::lower_multiplicity(&p.multiplicity);
            let aggregation = p.aggregation.map(lower_aggregation_kind);
            let stereotypes = resolve::resolve_stereotypes(&p.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(
                &p.tagged_values, ctx, errors,
            );

            Some(class::Property {
                name: p.name.clone(),
                source_info: p.source_info.clone(),
                type_expr,
                multiplicity,
                aggregation,
                default_value: None, // Expression lowering deferred
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers AST qualified properties to Pure qualified properties.
fn lower_qualified_properties(
    qprops: &[ast::QualifiedProperty],
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::QualifiedProperty> {
    qprops.iter()
        .filter_map(|qp| {
            let return_type = resolve::resolve_type_spec(&qp.return_type, ctx, errors)?;
            let return_multiplicity = resolve::lower_multiplicity(&qp.return_multiplicity);
            let parameters = lower_parameters(&qp.parameters, ctx, errors);
            let stereotypes = resolve::resolve_stereotypes(&qp.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(
                &qp.tagged_values, ctx, errors,
            );

            Some(class::QualifiedProperty {
                name: qp.name.clone(),
                source_info: qp.source_info.clone(),
                parameters,
                return_type,
                return_multiplicity,
                body: vec![], // Expression lowering deferred
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers AST parameters to Pure parameters.
fn lower_parameters(
    params: &[legend_pure_parser_ast::annotation::Parameter],
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params.iter()
        .filter_map(|p| {
            // Skip untyped lambda params — they need type inference (Phase 5+)
            let type_ref = p.type_ref.as_ref()?;
            let mult = p.multiplicity.as_ref()?;
            let type_expr = resolve::resolve_type_ref(type_ref, ctx, errors)?;
            let multiplicity = resolve::lower_multiplicity(mult);

            Some(crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity,
                source_info: p.source_info.clone(),
            })
        })
        .collect()
}

/// Lowers AST constraints to Pure constraints.
///
/// Constraint expressions are placeholder — full expression lowering is Phase 4+.
fn lower_constraints(constraints: &[ast::Constraint]) -> Vec<class::Constraint> {
    constraints.iter()
        .map(|c| class::Constraint {
            name: c.name.clone(),
            source_info: c.source_info.clone(),
            function: resolve::placeholder_expression(
                c.function_definition.source_info(),
            ),
            enforcement_level: c.enforcement_level.clone(),
            external_id: c.external_id.clone(),
            message: c.message.as_ref().map(|m| {
                resolve::placeholder_expression(m.source_info())
            }),
        })
        .collect()
}

/// Converts an AST `AggregationKind` to the Pure equivalent.
fn lower_aggregation_kind(kind: ast::AggregationKind) -> class::AggregationKind {
    match kind {
        ast::AggregationKind::None => class::AggregationKind::None,
        ast::AggregationKind::Shared => class::AggregationKind::Shared,
        ast::AggregationKind::Composite => class::AggregationKind::Composite,
    }
}

/// Extracts the simple name from an AST element.
fn ast_element_name(element: &ast::Element) -> SmolStr {
    use legend_pure_parser_ast::element::PackageableElement;
    element.name().clone()
}

/// Extracts the source info from an AST element.
fn ast_element_source(element: &ast::Element) -> &SourceInfo {
    element.source_info()
}

/// Extracts the package path segments from an AST element.
fn ast_element_package_path(element: &ast::Element) -> Vec<SmolStr> {
    use legend_pure_parser_ast::element::PackageableElement;
    match element.package() {
        Some(pkg) => collect_package_segments(pkg),
        None => vec![],
    }
}

/// Collects all segments from a linked-list AST Package into a Vec.
fn collect_package_segments(pkg: &AstPackage) -> Vec<SmolStr> {
    let mut segments = Vec::new();
    let mut current = Some(pkg);
    while let Some(p) = current {
        segments.push(p.name().clone());
        current = p.parent();
    }
    segments.reverse();
    segments
}

/// Builds a fully qualified name from package path + element name.
fn build_fqn(pkg_path: &[SmolStr], name: &SmolStr) -> SmolStr {
    if pkg_path.is_empty() {
        name.clone()
    } else {
        let mut fqn = String::new();
        for (i, seg) in pkg_path.iter().enumerate() {
            if i > 0 {
                fqn.push_str("::");
            }
            fqn.push_str(seg);
        }
        fqn.push_str("::");
        fqn.push_str(name);
        SmolStr::new(&fqn)
    }
}

/// Retrieves the AST element from source files given a declaration.
fn get_ast_element<'a>(
    source_files: &'a [SourceFile],
    decl: &Declaration,
) -> &'a ast::Element {
    &source_files[decl.file_idx].sections[decl.section_idx].elements[decl.element_idx]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_fqn_root() {
        let fqn = build_fqn(&[], &SmolStr::new("Person"));
        assert_eq!(fqn, "Person");
    }

    #[test]
    fn build_fqn_with_package() {
        let fqn = build_fqn(
            &[SmolStr::new("model"), SmolStr::new("domain")],
            &SmolStr::new("Person"),
        );
        assert_eq!(fqn, "model::domain::Person");
    }

    #[test]
    fn compile_empty_input() {
        let result = compile(&[]);
        assert!(result.is_ok());
        let model = result.unwrap();
        // Should have bootstrap chunk only
        assert_eq!(model.chunks.len(), 2); // bootstrap + empty user chunk
    }
}
