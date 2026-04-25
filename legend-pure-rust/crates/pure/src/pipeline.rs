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
//! 4. **Type Inference (Pass 2.5)** — Bottom-up type inference for all
//!    function and qualified property bodies.
//! 5. **Freeze** — Call `rebuild_derived_indexes()`.
//! 6. **Validation (Pass 3)** — Read-only pass on the frozen model.

use std::collections::{HashMap, VecDeque};

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::element as ast;
use legend_pure_parser_ast::element::PackageableElement as _;
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_ast::source_info::Spanned;

use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, ElementNode, ModelChunk, PureModel};
use crate::nodes::association::Association;
use crate::nodes::class::{self, Class};
use crate::nodes::enumeration::{EnumValue, Enumeration};
use crate::nodes::function::Function;
use crate::nodes::measure::Measure;
use crate::nodes::profile::Profile;
use crate::nodes::unit::Unit;
use crate::resolve::{self, ResolutionContext};
use crate::types::{Multiplicity, Parameter, PrimitiveType, TypeExpr};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A partial compilation result: the best-effort model plus accumulated errors.
///
/// Returned in the `Err` variant of [`compile()`] when errors occur. Unlike
/// discarding the model on failure, this allows callers to:
/// - **LSP**: show diagnostics alongside partial type info and navigation
/// - **Batch**: report all errors at once, not one at a time
/// - **Autofix**: inspect the partial model to suggest code fixes
#[derive(Debug)]
pub struct PartialPureModel {
    /// The compiled model (may be incomplete due to errors).
    pub model: PureModel,
    /// Compilation errors (guaranteed non-empty).
    pub errors: Vec<CompilationError>,
}

/// Compiles a list of parsed source files into a `PureModel`.
///
/// This is the main entry point for the compiler pipeline.
///
/// `auto_imports` provides the list of packages that are implicitly imported
/// in every section (e.g., `meta::pure::metamodel`, `meta::pure::profiles`).
/// The caller controls which packages are auto-imported.
///
/// # Errors
///
/// - `Ok(PureModel)` — compilation succeeded with zero errors
/// - `Err(PartialPureModel)` — errors occurred, but the model is still
///   available via [`PartialPureModel::model`] for diagnostics / LSP
#[allow(clippy::result_large_err)] // Ok(PureModel) is equally large — intentional API
pub fn compile(
    source_files: &[SourceFile],
    auto_imports: &[SmolStr],
) -> Result<PureModel, PartialPureModel> {
    let mut model = PureModel::new();

    // Chunk 0 — bootstrap primitives
    let bootstrap_chunk = bootstrap::create_bootstrap_chunk(model.root_package);
    model.chunks.push(bootstrap_chunk);

    // Register bootstrap elements in the root package
    for local_idx in 0..model.chunks[0].nodes.len() {
        let eid = ElementId::InstanceId {
            chunk_id: 0,
            local_idx,
        };
        model.register_element(model.root_package, eid);
    }

    // Register M3 metamodel stubs in their canonical packages
    // (e.g., Function → meta::pure::metamodel::function::Function)
    bootstrap::register_m3_packages(&mut model);

    // Resolve M3 supertype strings to actual ElementIds.
    // After M3 parsing, supertypes are stored as TypeExpr::Generic("ClassName").
    // Now that all M3 elements are registered in their packages, we can resolve
    // them to TypeExpr::Named { element } for subtype checking.
    resolve_m3_supertypes(&mut model);

    // Inject `Any.classifierGenericType` and `Any.elementOverride`
    // properties — m3.pure declares them on `Any` but our `m3_parser`
    // intentionally defers parsing the dense slot expression into
    // typed properties. Adding the bare names + resolved types here
    // unblocks the platform `properties()` accumulator (concatenates
    // declared + association + inherited recursively up to Any), which
    // surveyor's `testProperties` asserts as size 7 — without these
    // two slots the count ran short.
    wire_any_reflective_properties(&mut model);

    let mut errors = Vec::new();

    // ---- Pass 1: Declaration ----
    let (declarations, unit_mappings) = pass_declare(source_files, &mut model, &mut errors);

    // ---- Pass 1.5: Topological Sort ----
    let sorted = pass_topo_sort(&declarations, source_files, &model, &mut errors);

    // ---- Pass 2a: Signatures & Non-Function Elements ----
    // Resolve everything EXCEPT function expression bodies.
    // After this pass, all function signatures (params, return types) are
    // available for type-based dispatch during body compilation.
    let (id_to_decl, mut import_scope_cache, mut resolve_caches) = pass_define_signatures(
        &sorted,
        source_files,
        &declarations,
        &unit_mappings,
        auto_imports,
        &mut model,
        &mut errors,
    );

    // ---- Pass 2b: Function Bodies + Class/Assoc/Primitive bodies ----
    // Compile expression bodies using fully-resolved function signatures.
    // Order within Pass 2b doesn't matter — every signature in the model
    // (including all native function return types) is now hydrated, so any
    // body-shape expression sees the real types when resolving overloads.
    pass_define_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        &mut resolve_caches,
        auto_imports,
        &mut model,
        &mut errors,
    );

    // ---- Pass 2b': Class / Association / Primitive bodies ----
    // Constraint expressions, qualified-property bodies, and property
    // default values are body-shape: they call functions whose return
    // types must be known to dispatch operators correctly. They're
    // lowered here, *after* every function/native signature is hydrated,
    // so e.g. `'literal' + $x->toString()` inside a Primitive constraint
    // narrows `+` to the String overload via toString's real `:String[1]`
    // return type instead of the Pass 1 `Any` placeholder.
    pass_define_class_bodies(
        &sorted,
        source_files,
        &id_to_decl,
        &mut import_scope_cache,
        &mut resolve_caches,
        auto_imports,
        &mut model,
        &mut errors,
    );

    // NOTE: Function name mangling happens at declaration time (Pass 1).
    // Elements are registered with their mangled names from the start.

    // ---- Pass 2.5: Type Inference ----
    pass_infer(&mut model, &mut errors);

    // ---- Freeze ----
    model.rebuild_derived_indexes();

    // ---- Pass 3: Validation ----
    errors.extend(crate::validate::validate(&model));

    if errors.is_empty() {
        Ok(model)
    } else {
        Err(PartialPureModel { model, errors })
    }
}

/// Convenience macro for `compile()` with optional auto-imports.
///
/// ```ignore
/// // No auto-imports (equivalent to compile(files, &[]))
/// compile!(files);
///
/// // With auto-imports
/// compile!(files, &auto_imports);
/// ```
#[macro_export]
macro_rules! compile {
    ($source_files:expr) => {
        $crate::pipeline::compile($source_files, &[])
    };
    ($source_files:expr, $auto_imports:expr) => {
        $crate::pipeline::compile($source_files, $auto_imports)
    };
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
    /// Whether this declaration is a function (overloads allowed).
    is_function: bool,
}

/// Tracks unit `ElementId`s allocated for a measure during Pass 1.
#[derive(Debug, Clone)]
struct UnitMapping {
    /// The canonical unit's `ElementId`, if present.
    canonical: Option<ElementId>,
    /// Non-canonical unit `ElementId`s, in order.
    non_canonical: Vec<ElementId>,
}

// ---------------------------------------------------------------------------
// Pass 1: Declaration
// ---------------------------------------------------------------------------

/// Pass 1 — assigns `ElementId`s, allocates element shells, and builds the
/// package tree.
///
/// Returns a map from fully qualified name to declaration(s), and a map from
/// measure `ElementId` to its allocated unit `ElementId`s.
///
/// **Function overloads:** Pure allows multiple function definitions with the
/// same FQN (overloaded by parameter types). Each overload gets its own
/// `ElementId` and `Declaration`, stored in the `Vec`. Non-function elements
/// still produce `DuplicateElement` errors on collision.
/// Resolves M3 supertype strings (`TypeExpr::Generic`) to resolved `TypeExpr::Named`.
///
/// The M3 parser stores supertypes AND property types as
/// `TypeExpr::Generic("ClassName")` because forward references are common
/// in m3.pure. After all M3 elements are registered in their packages,
/// this pass resolves those names to actual `ElementId`s.
///
/// This enables:
/// - `is_subtype()` to walk the M3 type hierarchy correctly
///   (e.g., `Class <: Type <: PackageableElement <: Any`).
/// - Property-access type inference through inherited properties
///   (e.g., `.package` on a `Package` value — `package` is declared on
///   `PackageableElement` with type `Package[0..1]`, which before this
///   pass is stored as `Generic("Package")` and would widen to `Any`).
///
/// Names whose *declared* class type parameter includes the same identifier
/// (e.g. the M3 `Property` class has a type parameter `T`) remain generic —
/// those are real type variables.
/// Wire up `Any`'s two M3 reflective properties — `classifierGenericType:
/// GenericType[0..1]` and `elementOverride: ElementOverride[0..1]` —
/// after `m3_parser` has registered the rest of the metamodel.
///
/// The bootstrap allocates `Any` with `properties: vec![]` so the
/// canonical slot ID stays predictable; m3.pure declares these
/// properties via the dense `^Root.children[...]` syntax that
/// `m3_parser` currently skips. Filling them in here keeps the
/// reflective surface complete without re-parsing the bootstrap
/// region or growing m3_parser to handle the slot syntax.
///
/// Looks up `GenericType` and `ElementOverride` by their canonical
/// FQN and silently no-ops if either resolution fails (e.g. the
/// model was built without the M3 metamodel registered, as in some
/// micro-tests).
fn wire_any_reflective_properties(model: &mut PureModel) {
    use crate::bootstrap::{ANY_ID, BOOTSTRAP_CHUNK_ID};
    use crate::nodes::class::Property;
    use crate::types::{Multiplicity, TypeExpr};
    use legend_pure_parser_ast::SourceInfo;

    let synth = SourceInfo::new("<bootstrap-Any-properties>", 0, 0, 0, 0);
    let generic_type_id = model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("type"),
        SmolStr::new("generics"),
        SmolStr::new("GenericType"),
    ]);
    let element_override_id = model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("type"),
        SmolStr::new("ElementOverride"),
    ]);
    let Some(generic_type_id) = generic_type_id else {
        return;
    };
    let Some(element_override_id) = element_override_id else {
        return;
    };

    let chunk = &mut model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    let Element::Class(any_cls) = chunk.elements.get_mut(ANY_ID.local_idx()) else {
        return;
    };
    // Guard against double-injection if the pipeline runs twice (unit
    // tests, hot-reload scenarios) — bail when the slots are already
    // populated.
    if any_cls
        .properties
        .iter()
        .any(|p| p.name == "classifierGenericType" || p.name == "elementOverride")
    {
        return;
    }
    any_cls.properties.push(Property {
        name: SmolStr::new("classifierGenericType"),
        type_expr: TypeExpr::Named {
            element: generic_type_id,
            type_arguments: vec![],
            value_arguments: vec![],
        },
        multiplicity: Multiplicity::ZeroOrOne,
        source_info: synth.clone(),
        aggregation: None,
        default_value: None,
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
    });
    any_cls.properties.push(Property {
        name: SmolStr::new("elementOverride"),
        type_expr: TypeExpr::Named {
            element: element_override_id,
            type_arguments: vec![],
            value_arguments: vec![],
        },
        multiplicity: Multiplicity::ZeroOrOne,
        source_info: synth,
        aggregation: None,
        default_value: None,
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
    });
}

fn resolve_m3_supertypes(model: &mut PureModel) {
    use crate::bootstrap::BOOTSTRAP_CHUNK_ID;
    use crate::types::TypeExpr;

    // Collect all M3 element IDs from chunk 0
    let chunk = &model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    let m3_count = chunk.elements.len();

    // Build a name → ElementId lookup for M3 classes (same-chunk resolution)
    let mut name_to_id: HashMap<SmolStr, ElementId> = HashMap::new();
    for local_idx in 0..m3_count {
        let eid = ElementId::InstanceId {
            chunk_id: BOOTSTRAP_CHUNK_ID,
            local_idx,
        };
        let name = model.get_node(eid).name.clone();
        name_to_id.insert(name, eid);
    }

    // Resolve Generic → Named on supertypes AND properties/qualified-properties.
    // A Generic("X") remains generic only when "X" is one of the class's own
    // type parameters; otherwise, resolve it against the chunk-0 class set.
    let chunk = &mut model.chunks[BOOTSTRAP_CHUNK_ID as usize];
    for local_idx in 0..m3_count {
        let element = chunk.elements.get_mut(local_idx);
        let Element::Class(c) = element else {
            continue;
        };
        let type_params: std::collections::HashSet<SmolStr> =
            c.type_parameters.iter().cloned().collect();

        let resolve_in_place = |ty: &mut TypeExpr| match ty {
            TypeExpr::Generic(name) if !type_params.contains(name) => {
                if let Some(&resolved_id) = name_to_id.get(name.as_str()) {
                    *ty = TypeExpr::Named {
                        element: resolved_id,
                        type_arguments: vec![],
                        value_arguments: vec![],
                    };
                }
            }
            _ => {}
        };

        for st in c.super_types.iter_mut() {
            resolve_in_place(st);
        }
        for p in c.properties.iter_mut() {
            resolve_in_place(&mut p.type_expr);
        }
        for qp in c.qualified_properties.iter_mut() {
            resolve_in_place(&mut qp.return_type);
            for param in qp.parameters.iter_mut() {
                resolve_in_place(&mut param.type_expr);
            }
        }
    }
}

fn pass_declare(
    source_files: &[SourceFile],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<SmolStr, Vec<Declaration>>,
    HashMap<ElementId, UnitMapping>,
) {
    let mut declarations: HashMap<SmolStr, Vec<Declaration>> = HashMap::new();
    let mut unit_mappings: HashMap<ElementId, UnitMapping> = HashMap::new();
    #[allow(clippy::cast_possible_truncation)] // chunks.len() is bounded by u16 in practice
    let chunk_id = model.chunks.len() as u16;
    let mut chunk = ModelChunk::new(chunk_id);

    for (file_idx, source_file) in source_files.iter().enumerate() {
        for (section_idx, section) in source_file.sections.iter().enumerate() {
            for (element_idx, element) in section.elements.iter().enumerate() {
                let simple_name = ast_element_name(element);
                let source_info = ast_element_source(element);
                let name_source_info = ast_element_name_source(element);

                // Resolve package path
                let pkg_path = ast_element_package_path(element);
                let package_id = if pkg_path.is_empty() {
                    model.root_package
                } else {
                    model.get_or_create_package(&pkg_path)
                };

                // For functions, compute the mangled name at declaration time.
                // This is the element name (like a class name is its element name).
                let is_function_like = matches!(
                    element,
                    ast::Element::Function(_) | ast::Element::NativeFunction(_)
                );

                let element_name = match element {
                    ast::Element::Function(f) => {
                        use legend_pure_parser_ast::element::FunctionSignature;
                        SmolStr::new(f.mangled_name())
                    }
                    ast::Element::NativeFunction(f) => {
                        use legend_pure_parser_ast::element::FunctionSignature;
                        SmolStr::new(f.mangled_name())
                    }
                    _ => simple_name.clone(),
                };

                // Build fully qualified name
                let fqn = build_fqn(&pkg_path, &element_name);

                // Check for duplicates — allow function overloads
                if let Some(existing) = declarations.get(&fqn) {
                    if !is_function_like || !existing.iter().all(|d| d.is_function) {
                        // Non-function duplicate, or mixing function with non-function
                        errors.push(CompilationError {
                            message: format!("Duplicate element: '{fqn}'"),
                            source_info: source_info.clone(),
                            kind: CompilationErrorKind::DuplicateElement { name: fqn.clone() },
                        });
                        continue;
                    }
                    // Function overload — fall through to allocate
                }

                // Allocate shell
                let shell = create_shell(element);
                let local_idx = chunk.alloc_element(
                    ElementNode {
                        name: element_name.clone(),
                        source_info: source_info.clone(),
                        name_source_info: name_source_info.clone(),
                        parent_package: package_id,
                    },
                    shell,
                );

                let id = ElementId::InstanceId {
                    chunk_id,
                    local_idx,
                };
                model.register_element(package_id, id);

                declarations
                    .entry(fqn.clone())
                    .or_default()
                    .push(Declaration {
                        id,
                        file_idx,
                        section_idx,
                        element_idx,
                        is_function: is_function_like,
                    });

                // For Measures: allocate Unit shells now
                if let ast::Element::Measure(measure_def) = element {
                    let mapping = allocate_unit_shells(
                        measure_def,
                        id,
                        &fqn,
                        chunk_id,
                        package_id,
                        &mut chunk,
                        model,
                        &mut declarations,
                        file_idx,
                        section_idx,
                        element_idx,
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
#[allow(clippy::too_many_arguments)]
fn allocate_unit_shells(
    measure_def: &ast::MeasureDef,
    measure_id: ElementId,
    measure_fqn: &str,
    chunk_id: u16,
    package_id: crate::ids::PackageId,
    chunk: &mut ModelChunk,
    model: &mut PureModel,
    declarations: &mut HashMap<SmolStr, Vec<Declaration>>,
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
                name: SmolStr::new(format!("{}~{unit_name}", measure_def.name.value)),
                source_info: unit_def.source_info.clone(),
                name_source_info: unit_def.source_info.clone(),
                parent_package: package_id,
            },
            unit_shell,
        );

        let unit_id = ElementId::InstanceId {
            chunk_id,
            local_idx,
        };
        model.register_element(package_id, unit_id);

        // SAFETY INVARIANT: These AST coordinates point to the parent Measure
        // element, not the unit itself. This is safe because `pass_define`
        // skips `Element::Unit` before looking up the AST. If that skip is
        // ever removed, units would be re-hydrated as duplicate Measures.
        declarations.entry(unit_fqn).or_default().push(Declaration {
            id: unit_id,
            file_idx,
            section_idx,
            element_idx,
            is_function: false,
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

    UnitMapping {
        canonical,
        non_canonical,
    }
}

// ---------------------------------------------------------------------------
// Pass 1.5: Topological Sort (Hard Dependencies)
// ---------------------------------------------------------------------------

/// Pass 1.5 — builds a dependency DAG from supertypes and sorts via Kahn's algorithm.
///
/// Returns an ordered list of element IDs safe for definition.
fn pass_topo_sort(
    declarations: &HashMap<SmolStr, Vec<Declaration>>,
    source_files: &[SourceFile],
    model: &PureModel,
    errors: &mut Vec<CompilationError>,
) -> Vec<ElementId> {
    // Build adjacency list: id → list of hard dependencies (supertypes)
    let mut in_degree: HashMap<ElementId, usize> = HashMap::new();
    let mut dependents: HashMap<ElementId, Vec<ElementId>> = HashMap::new();

    for decls in declarations.values() {
        for decl in decls {
            in_degree.entry(decl.id).or_insert(0);
        }
    }

    for decls in declarations.values() {
        for decl in decls {
            let element = get_ast_element(source_files, decl);
            let hard_deps = extract_hard_dependencies(element, declarations, model);

            for dep_id in hard_deps {
                dependents.entry(dep_id).or_default().push(decl.id);
                *in_degree.entry(decl.id).or_insert(0) += 1;
            }
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<ElementId> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let total_decls: usize = declarations.values().map(|v| v.len()).sum();
    let mut sorted = Vec::with_capacity(total_decls);

    while let Some(id) = queue.pop_front() {
        sorted.push(id);
        if let Some(deps) = dependents.get(&id) {
            for &dep_id in deps {
                let Some(deg) = in_degree.get_mut(&dep_id) else {
                    unreachable!("dep_id was inserted into in_degree during graph construction");
                };
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(dep_id);
                }
            }
        }
    }

    // Check for cycles
    if sorted.len() < total_decls {
        let cyclic: Vec<_> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(&id, _)| id)
            .collect();

        for id in &cyclic {
            let node = model.get_node(*id);
            errors.push(CompilationError {
                message: format!("Cyclic inheritance detected involving '{}'", node.name),
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
    declarations: &HashMap<SmolStr, Vec<Declaration>>,
    _model: &PureModel,
) -> Vec<ElementId> {
    match element {
        ast::Element::Class(class_def) => class_def
            .super_types
            .iter()
            .filter_map(|type_ref| {
                let fqn = SmolStr::new(type_ref.full_path());
                declarations
                    .get(&fqn)
                    .and_then(|ds| ds.first())
                    .map(|d| d.id)
            })
            .collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Pass 2: Definition
// ---------------------------------------------------------------------------

/// Pass 2a — hydrates shells in topological order, resolving everything
/// EXCEPT function expression bodies. Returns the lookup maps and caches
/// for reuse in Pass 2b.
#[allow(clippy::type_complexity)]
fn pass_define_signatures<'a>(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    declarations: &'a HashMap<SmolStr, Vec<Declaration>>,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) -> (
    HashMap<ElementId, &'a Declaration>,
    HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
    HashMap<(usize, usize), HashMap<SmolStr, crate::resolve::ResolveResult>>,
) {
    use crate::resolve::ImportScope;

    // Build reverse lookup: ElementId → Declaration
    let id_to_decl: HashMap<ElementId, &Declaration> = declarations
        .values()
        .flat_map(|ds| ds.iter())
        .map(|d| (d.id, d))
        .collect();

    // Cache per-section import scopes and resolve caches
    let mut import_scope_cache: HashMap<(usize, usize), Vec<ImportScope>> = HashMap::new();
    let mut resolve_caches: HashMap<
        (usize, usize),
        HashMap<SmolStr, crate::resolve::ResolveResult>,
    > = HashMap::new();

    for &id in sorted {
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        // Skip units — they were fully populated during Pass 1 (allocate_unit_shells)
        let chunk = &model.chunks[chunk_id as usize];
        if matches!(chunk.elements.get(local_idx), Element::Unit(_)) {
            continue;
        }

        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Build or retrieve the import scope for this element's section
        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });

        // Implicit self-package: elements can see siblings in the same package
        // without explicit imports (matches Java Pure compiler behavior).
        if let Some(pkg) = ast_element.package() {
            let pkg_str = pkg.to_string();
            if !import_scopes
                .iter()
                .any(|s| s.package.to_string() == pkg_str)
            {
                import_scopes.push(ImportScope::from_path_str(&pkg_str));
            }
        }

        // Get or create the per-section resolve cache
        let resolve_cache = resolve_caches.entry(scope_key).or_default();

        // Extract type parameters from the AST element (Class<T,V>, function<T|m>)
        let type_params = ast_type_parameters(ast_element);

        let mut ctx = ResolutionContext {
            model,
            import_scopes,
            resolve_cache,
            type_parameters: &type_params,
            variable_types: HashMap::new(),
        };

        // Hydrate everything EXCEPT function bodies (bodies resolved in Pass 2b)
        let hydrated = hydrate_element_signature(ast_element, id, unit_mappings, &mut ctx, errors);

        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        *chunk.elements.get_mut(local_idx) = hydrated;
    }

    (id_to_decl, import_scope_cache, resolve_caches)
}

/// Pass 2b — compile function expression bodies.
///
/// At this point all function signatures (params, return types) are fully
/// resolved, enabling type-based dispatch in `resolve_function_call`.
///
/// Takes `&mut` references to the per-section caches so the follow-up
/// `pass_define_class_bodies` can reuse the populated import scopes and
/// resolve memos.
fn pass_define_bodies(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    id_to_decl: &HashMap<ElementId, &Declaration>,
    import_scope_cache: &mut HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
    resolve_caches: &mut HashMap<(usize, usize), HashMap<SmolStr, crate::resolve::ResolveResult>>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) {
    for &id in sorted {
        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Only process functions with bodies
        let body_exprs = match ast_element {
            ast::Element::Function(f) if !f.body.is_empty() => &f.body,
            _ => continue,
        };

        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });

        // Implicit self-package (same as Pass 2a)
        if let Some(pkg) = ast_element.package() {
            let pkg_str = pkg.to_string();
            if !import_scopes
                .iter()
                .any(|s| s.package.to_string() == pkg_str)
            {
                import_scopes.push(crate::resolve::ImportScope::from_path_str(&pkg_str));
            }
        }
        let resolve_cache = resolve_caches.entry(scope_key).or_default();

        let type_params = ast_type_parameters(ast_element);

        // Seed variable scope with the function's own resolved parameters
        let mut variable_types = HashMap::new();
        if let Element::Function(f) = model.get_element(id) {
            for param in &f.parameters {
                variable_types.insert(
                    param.name.clone(),
                    (param.type_expr.clone(), param.multiplicity.clone()),
                );
            }
        }

        let mut ctx = ResolutionContext {
            model,
            import_scopes,
            resolve_cache,
            type_parameters: &type_params,
            variable_types,
        };

        let body = crate::lower::lower_expression_body(body_exprs, &mut ctx, errors);

        // Patch the body into the already-resolved function
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        if let Element::Function(func) = chunk.elements.get_mut(local_idx) {
            func.body = body;
        }
    }
}

/// Pass 2b' — compile body-shape expressions on Class / Association /
/// Primitive elements: constraint functions and messages, qualified-property
/// bodies, and property default values.
///
/// These were left as placeholders by Pass 2a (`vec![]` for QP bodies and
/// constraint lists; `None` for default values) because their lowering
/// drives `resolve_function_call`, which needs every native function's
/// real return type to narrow operator overloads correctly. Lowering them
/// here — after both Pass 2a (signatures) and Pass 2b (function bodies) —
/// guarantees the model is fully populated before any class-side body
/// expression resolves a call.
///
/// Skips elements whose AST has no body-shape items, so the cost is
/// proportional to the number of constraints / QP bodies / default values
/// in the program, not the total element count.
fn pass_define_class_bodies(
    sorted: &[ElementId],
    source_files: &[SourceFile],
    id_to_decl: &HashMap<ElementId, &Declaration>,
    import_scope_cache: &mut HashMap<(usize, usize), Vec<crate::resolve::ImportScope>>,
    resolve_caches: &mut HashMap<(usize, usize), HashMap<SmolStr, crate::resolve::ResolveResult>>,
    auto_imports: &[SmolStr],
    model: &mut PureModel,
    errors: &mut Vec<CompilationError>,
) {
    use crate::resolve::ImportScope;

    for &id in sorted {
        let Some(decl) = id_to_decl.get(&id) else {
            continue;
        };
        let ast_element = get_ast_element(source_files, decl);

        // Only Classes / Associations / Primitives carry deferred bodies.
        if !matches!(
            ast_element,
            ast::Element::Class(_) | ast::Element::Association(_) | ast::Element::Primitive(_)
        ) {
            continue;
        }

        let scope_key = (decl.file_idx, decl.section_idx);
        let import_scopes = import_scope_cache.entry(scope_key).or_insert_with(|| {
            build_import_scope(source_files, decl.file_idx, decl.section_idx, auto_imports)
        });
        if let Some(pkg) = ast_element.package() {
            let pkg_str = pkg.to_string();
            if !import_scopes
                .iter()
                .any(|s| s.package.to_string() == pkg_str)
            {
                import_scopes.push(ImportScope::from_path_str(&pkg_str));
            }
        }
        let resolve_cache = resolve_caches.entry(scope_key).or_default();

        let type_params = ast_type_parameters(ast_element);

        // Per-element variable scope: seeded with type-variable parameters
        // from parametric Classes / Primitives so `$x` inside a constraint
        // resolves against the class's declared `(x:Integer[1])`.
        let mut variable_types = HashMap::new();
        if let Element::Class(c) = model.get_element(id) {
            for tvp in &c.type_variable_parameters {
                variable_types.insert(
                    tvp.name.clone(),
                    (tvp.type_expr.clone(), tvp.multiplicity.clone()),
                );
            }
        } else if let Element::PrimitiveType(p) = model.get_element(id) {
            for tvp in &p.type_variable_parameters {
                variable_types.insert(
                    tvp.name.clone(),
                    (tvp.type_expr.clone(), tvp.multiplicity.clone()),
                );
            }
        }

        // Lower body-shape items into owned locals. This temporarily
        // borrows `model` mutably via `ctx`; we drop ctx before patching.
        let (new_constraints, new_qp_bodies, new_default_values) = {
            let mut ctx = ResolutionContext {
                model,
                import_scopes,
                resolve_cache,
                type_parameters: &type_params,
                variable_types,
            };
            match ast_element {
                ast::Element::Class(c) => (
                    lower_constraints(&c.constraints, &mut ctx, errors),
                    lower_qualified_property_bodies(&c.qualified_properties, &mut ctx, errors),
                    lower_property_default_values(&c.properties, &mut ctx, errors),
                ),
                ast::Element::Association(a) => (
                    Vec::new(),
                    lower_qualified_property_bodies(&a.qualified_properties, &mut ctx, errors),
                    lower_property_default_values(&a.properties, &mut ctx, errors),
                ),
                ast::Element::Primitive(p) => (
                    lower_constraints(&p.constraints, &mut ctx, errors),
                    Vec::new(),
                    Vec::new(),
                ),
                _ => unreachable!(),
            }
        };

        // Patch the lowered bodies into the model element.
        let ElementId::InstanceId {
            chunk_id,
            local_idx,
        } = id
        else {
            continue;
        };
        let chunk = &mut model.chunks[chunk_id as usize];
        match chunk.elements.get_mut(local_idx) {
            Element::Class(c) => {
                c.constraints = new_constraints;
                patch_qp_bodies(&mut c.qualified_properties, new_qp_bodies);
                patch_property_default_values(&mut c.properties, new_default_values);
            }
            Element::Association(a) => {
                patch_qp_bodies(&mut a.qualified_properties, new_qp_bodies);
                patch_property_default_values(&mut a.properties, new_default_values);
            }
            Element::PrimitiveType(p) => {
                p.constraints = new_constraints;
            }
            _ => {}
        }
    }
}

/// Patches lowered QP bodies into existing QualifiedProperty entries.
/// AST QP count may exceed model QP count if signature lowering filtered
/// some out (return_type didn't resolve), so we zip and drop any extras.
fn patch_qp_bodies(
    model_qps: &mut [class::QualifiedProperty],
    new_bodies: Vec<Vec<crate::types::ValueSpec>>,
) {
    for (qp, body) in model_qps.iter_mut().zip(new_bodies.into_iter()) {
        qp.body = body;
    }
}

/// Patches lowered default values into existing Property entries. Same
/// alignment caveat as `patch_qp_bodies`.
fn patch_property_default_values(
    model_props: &mut [class::Property],
    new_defaults: Vec<Option<crate::types::ValueSpec>>,
) {
    for (p, dv) in model_props.iter_mut().zip(new_defaults.into_iter()) {
        p.default_value = dv;
    }
}

/// Builds the import scope for a specific section.
///
/// Explicit imports use the AST `Package` directly (the parser already
/// built the recursive tree). Auto-imports arrive as path strings and
/// get parsed into `Package` once at startup via [`ImportScope::from_path_str`].
fn build_import_scope(
    source_files: &[SourceFile],
    file_idx: usize,
    section_idx: usize,
    auto_imports: &[SmolStr],
) -> Vec<crate::resolve::ImportScope> {
    use crate::resolve::ImportScope;

    let section = &source_files[file_idx].sections[section_idx];

    // Wrap the AST Package directly — zero conversion needed
    let mut scope: Vec<ImportScope> = section
        .imports
        .iter()
        .map(|import| ImportScope::from_package(import.path.clone()))
        .collect();

    // Append auto-imports (deduplicating by Display string)
    for auto in auto_imports {
        let auto_str = auto.as_str();
        if !scope.iter().any(|s| s.package.to_string() == auto_str) {
            scope.push(ImportScope::from_path_str(auto_str));
        }
    }

    scope
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates an empty shell for an AST element.
fn create_shell(element: &ast::Element) -> Element {
    match element {
        ast::Element::Class(_) => Element::Class(Class {
            type_parameters: vec![],
            type_variable_parameters: vec![],
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
        ast::Element::Function(f) => {
            let placeholder_params: Vec<Parameter> = f
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: p.name.clone(),
                    type_expr: TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: vec![],
                        value_arguments: vec![],
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: p.source_info.clone(),
                })
                .collect();
            Element::Function(Function {
                function_name: f.name.value.clone(),
                is_native: false,
                parameters: placeholder_params,
                return_type: TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    value_arguments: vec![],
                },
                return_multiplicity: Multiplicity::PureOne,
                body: vec![],
                stereotypes: vec![],
                tagged_values: vec![],
            })
        }
        ast::Element::NativeFunction(f) => {
            let placeholder_params: Vec<Parameter> = f
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: p.name.clone(),
                    type_expr: TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: vec![],
                        value_arguments: vec![],
                    },
                    multiplicity: Multiplicity::PureOne,
                    source_info: p.source_info.clone(),
                })
                .collect();
            Element::Function(Function {
                function_name: f.name.value.clone(),
                is_native: true,
                parameters: placeholder_params,
                return_type: TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    value_arguments: vec![],
                },
                return_multiplicity: Multiplicity::PureOne,
                body: vec![],
                stereotypes: vec![],
                tagged_values: vec![],
            })
        }
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
        ast::Element::Primitive(_) => Element::PrimitiveType(PrimitiveType {
            super_type: None,
            super_type_value_arguments: Vec::new(),
            type_variable_parameters: Vec::new(),
            constraints: Vec::new(),
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
fn hydrate_element_signature(
    element: &ast::Element,
    element_id: ElementId,
    unit_mappings: &HashMap<ElementId, UnitMapping>,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Element {
    match element {
        ast::Element::Class(class_def) => {
            // Super types
            let super_types: Vec<TypeExpr> = class_def
                .super_types
                .iter()
                .filter_map(|type_ref| resolve::resolve_type_ref(type_ref, ctx, errors))
                .collect();

            // Properties — signatures only. Default-value bodies are lowered
            // in Pass 2b (`pass_define_class_bodies`) once all function
            // signatures are hydrated, so type-based dispatch in any
            // operator/function call inside a default value sees real
            // return types instead of Pass 1 placeholders.
            let properties = lower_property_signatures(&class_def.properties, ctx, errors);

            // Qualified properties — signatures only; bodies deferred to
            // Pass 2b for the same reason.
            let qualified_properties =
                lower_qualified_property_signatures(&class_def.qualified_properties, ctx, errors);

            // Constraints — fully deferred to Pass 2b. Constraint expressions
            // are body-shape: they call functions that may not yet have
            // hydrated signatures during Pass 2a topo-ordered hydration.
            let constraints = Vec::new();

            // Annotations
            let stereotypes = resolve::resolve_stereotypes(&class_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&class_def.tagged_values, ctx, errors);

            let type_variable_parameters =
                lower_type_variable_parameters(&class_def.type_variable_parameters, ctx, errors);
            Element::Class(Class {
                type_parameters: class_def.type_parameters.clone(),
                type_variable_parameters,
                super_types,
                properties,
                qualified_properties,
                constraints,
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Enumeration(enum_def) => {
            let stereotypes = resolve::resolve_stereotypes(&enum_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&enum_def.tagged_values, ctx, errors);

            Element::Enumeration(Enumeration {
                values: enum_def
                    .values
                    .iter()
                    .map(|v| {
                        let v_stereos = resolve::resolve_stereotypes(&v.stereotypes, ctx, errors);
                        let v_tvs = resolve::resolve_tagged_values(&v.tagged_values, ctx, errors);
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
        ast::Element::Profile(prof_def) => Element::Profile(Profile {
            stereotypes: prof_def
                .stereotype_names
                .iter()
                .map(|s| s.value.clone())
                .collect(),
            tags: prof_def.tag_names.iter().map(|t| t.value.clone()).collect(),
        }),
        ast::Element::Function(func_def) => {
            let parameters = lower_parameters(&func_def.parameters, ctx, errors);
            let return_type = resolve::resolve_type_spec(&func_def.return_type, ctx, errors)
                .unwrap_or(TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    value_arguments: vec![],
                });
            let return_multiplicity = resolve::lower_multiplicity(&func_def.return_multiplicity);
            let stereotypes = resolve::resolve_stereotypes(&func_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&func_def.tagged_values, ctx, errors);

            Element::Function(Function {
                function_name: func_def.name.value.clone(),
                is_native: false,
                parameters,
                return_type,
                return_multiplicity,
                body: vec![], // Bodies resolved in Pass 2b
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Association(assoc_def) => {
            // Properties / QPs — signatures only; default values & QP bodies
            // deferred to Pass 2b (see Class arm above for rationale).
            let properties = lower_property_signatures(&assoc_def.properties, ctx, errors);
            let qualified_properties =
                lower_qualified_property_signatures(&assoc_def.qualified_properties, ctx, errors);
            let stereotypes = resolve::resolve_stereotypes(&assoc_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&assoc_def.tagged_values, ctx, errors);

            Element::Association(Association {
                properties,
                qualified_properties,
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::NativeFunction(func_def) => {
            let parameters = lower_parameters(&func_def.parameters, ctx, errors);
            let return_type = resolve::resolve_type_spec(&func_def.return_type, ctx, errors)
                .unwrap_or(TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    value_arguments: vec![],
                });
            let return_multiplicity = resolve::lower_multiplicity(&func_def.return_multiplicity);
            let stereotypes = resolve::resolve_stereotypes(&func_def.stereotypes, ctx, errors);
            let tagged_values =
                resolve::resolve_tagged_values(&func_def.tagged_values, ctx, errors);

            Element::Function(Function {
                function_name: func_def.name.value.clone(),
                is_native: true,
                parameters,
                return_type,
                return_multiplicity,
                body: vec![],
                stereotypes,
                tagged_values,
            })
        }
        ast::Element::Measure(_measure_def) => {
            // Unit ElementIds were allocated in Pass 1; look them up
            let mapping = unit_mappings.get(&element_id);
            Element::Measure(Measure {
                canonical_unit: mapping.and_then(|m| m.canonical),
                non_canonical_units: mapping.map(|m| m.non_canonical.clone()).unwrap_or_default(),
            })
        }
        ast::Element::Primitive(prim_def) => {
            let super_named = resolve::resolve_type_ref(&prim_def.super_type, ctx, errors);
            let (super_type, super_type_value_arguments) = match super_named {
                Some(TypeExpr::Named {
                    element,
                    value_arguments,
                    ..
                }) => (Some(element), value_arguments),
                _ => (None, Vec::new()),
            };
            let type_variable_parameters =
                lower_type_variable_parameters(&prim_def.type_variable_parameters, ctx, errors);
            // Constraints — fully deferred to Pass 2b (see Class arm).
            Element::PrimitiveType(PrimitiveType {
                super_type,
                super_type_value_arguments,
                type_variable_parameters,
                constraints: Vec::new(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Property Lowering Helpers
// ---------------------------------------------------------------------------

/// Lowers AST properties to Pure properties — **signatures only**.
///
/// Default-value expressions are body-shape and need every native function
/// signature to be hydrated before they can resolve operator overloads
/// correctly, so they are deferred to Pass 2b
/// (`pass_define_class_bodies`). The returned `Property.default_value` is
/// always `None` from this function — Pass 2b patches the value in-place.
fn lower_property_signatures(
    props: &[ast::Property],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::Property> {
    props
        .iter()
        .filter_map(|p| {
            let type_expr = resolve::resolve_type_spec(&p.type_ref, ctx, errors)?;
            let multiplicity = resolve::lower_multiplicity(&p.multiplicity);
            let aggregation = p.aggregation.map(lower_aggregation_kind);
            let stereotypes = resolve::resolve_stereotypes(&p.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(&p.tagged_values, ctx, errors);

            Some(class::Property {
                name: p.name.clone(),
                source_info: p.source_info.clone(),
                type_expr,
                multiplicity,
                aggregation,
                default_value: None, // patched in Pass 2b
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers AST qualified properties to Pure qualified properties —
/// **signatures only**. Bodies are deferred to Pass 2b for the same
/// reason as [`lower_property_signatures`]. Returned `body` is always
/// `vec![]`; Pass 2b patches in the lowered body.
fn lower_qualified_property_signatures(
    qprops: &[ast::QualifiedProperty],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::QualifiedProperty> {
    qprops
        .iter()
        .filter_map(|qp| {
            let return_type = resolve::resolve_type_spec(&qp.return_type, ctx, errors)?;
            let return_multiplicity = resolve::lower_multiplicity(&qp.return_multiplicity);
            let parameters = lower_parameters(&qp.parameters, ctx, errors);
            let stereotypes = resolve::resolve_stereotypes(&qp.stereotypes, ctx, errors);
            let tagged_values = resolve::resolve_tagged_values(&qp.tagged_values, ctx, errors);

            Some(class::QualifiedProperty {
                name: qp.name.clone(),
                source_info: qp.source_info.clone(),
                parameters,
                return_type,
                return_multiplicity,
                body: vec![], // patched in Pass 2b
                stereotypes,
                tagged_values,
            })
        })
        .collect()
}

/// Lowers QP bodies for all qualified properties of the given AST list.
/// Returns one `Vec<Expression>` per QP, in the same order; `vec![]` if
/// the QP was filtered out of the signature pass (return_type couldn't
/// resolve) so the index doesn't get out of sync with the model.
fn lower_qualified_property_bodies(
    qprops: &[ast::QualifiedProperty],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<Vec<crate::types::ValueSpec>> {
    qprops
        .iter()
        .map(|qp| crate::lower::lower_expression_body(&qp.body, ctx, errors))
        .collect()
}

/// Lowers default-value expressions for the given AST property list.
/// Returns one `Option<Expression>` per AST property, in order — `None`
/// when the property had no default or when lowering produced no value.
fn lower_property_default_values(
    props: &[ast::Property],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<Option<crate::types::ValueSpec>> {
    props
        .iter()
        .map(|p| {
            p.default_value
                .as_ref()
                .and_then(|dv| crate::lower::lower_expression(dv, ctx, errors))
        })
        .collect()
}

/// Lowers AST parameters to Pure parameters.
fn lower_parameters(
    params: &[legend_pure_parser_ast::annotation::Parameter],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params
        .iter()
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

/// Lowers AST `TypeVariableParameter`s to compiled `Parameter`s.
/// Used by Class and Primitive to carry parametric-value declarations
/// (e.g. the `x:Integer[1]` in `Primitive P(x:Integer[1])`).
fn lower_type_variable_parameters(
    params: &[legend_pure_parser_ast::type_ref::TypeVariableParameter],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::Parameter> {
    params
        .iter()
        .filter_map(|p| {
            let type_expr = resolve::resolve_type_ref(&p.type_ref, ctx, errors)?;
            Some(crate::types::Parameter {
                name: p.name.clone(),
                type_expr,
                multiplicity: resolve::lower_multiplicity(&p.multiplicity),
                source_info: p.source_info.clone(),
            })
        })
        .collect()
}

/// Lowers AST constraints to Pure constraints.
fn lower_constraints(
    constraints: &[ast::Constraint],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<class::Constraint> {
    constraints
        .iter()
        .filter_map(|c| {
            let function = crate::lower::lower_expression(&c.function_definition, ctx, errors)?;
            let message = c
                .message
                .as_ref()
                .and_then(|m| crate::lower::lower_expression(m, ctx, errors));
            Some(class::Constraint {
                name: c.name.clone(),
                source_info: c.source_info.clone(),
                function,
                enforcement_level: c.enforcement_level.clone(),
                external_id: c.external_id.clone(),
                message,
            })
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

/// Extracts the source span of the element's **name identifier** (the
/// `XTestClass` in `Class meta::pure::…::XTestClass { … }`), falling
/// back to the full declaration span for element kinds that don't
/// carry a separate name span.
///
/// Matches Java Pure's `SourceInformation.line`/`column` (distinct from
/// `startLine`/`startColumn`).
fn ast_element_name_source(element: &ast::Element) -> &SourceInfo {
    use legend_pure_parser_ast::Spanned;
    match element {
        ast::Element::Class(c) => c.name.source_info(),
        ast::Element::Function(f) => f.name.source_info(),
        ast::Element::NativeFunction(f) => f.name.source_info(),
        ast::Element::Association(a) => a.name.source_info(),
        ast::Element::Enumeration(e) => e.name.source_info(),
        ast::Element::Profile(p) => p.name.source_info(),
        ast::Element::Measure(m) => m.name.source_info(),
        ast::Element::Primitive(p) => p.name.source_info(),
    }
}

/// Extracts the package path segments from an AST element.
fn ast_element_package_path(element: &ast::Element) -> Vec<SmolStr> {
    use legend_pure_parser_ast::element::PackageableElement;
    match element.package() {
        Some(pkg) => pkg.segments().into_iter().cloned().collect(),
        None => vec![],
    }
}

/// Extracts type parameter names from an AST element.
///
/// Classes and functions can declare type parameters (e.g., `Class<T, V>`,
/// `function<Z|m>`). These names must be treated as generic type variables
/// during resolution, not as packageable element references.
fn ast_type_parameters(element: &ast::Element) -> Vec<SmolStr> {
    match element {
        ast::Element::Class(c) => c.type_parameters.clone(),
        ast::Element::Function(f) => f.type_parameters.clone(),
        ast::Element::NativeFunction(f) => f.type_parameters.clone(),
        _ => vec![],
    }
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
fn get_ast_element<'a>(source_files: &'a [SourceFile], decl: &Declaration) -> &'a ast::Element {
    &source_files[decl.file_idx].sections[decl.section_idx].elements[decl.element_idx]
}

// ---------------------------------------------------------------------------
// Pass 2.5 — Type Inference
// ---------------------------------------------------------------------------

/// Runs bottom-up type inference over all function and qualified property bodies.
///
/// For each function, infers types for every expression in the body and sets
/// `type_info` on each expression node in place.
fn pass_infer(model: &mut PureModel, errors: &mut Vec<CompilationError>) {
    use crate::infer;

    // Collect (chunk_idx, element_idx, params, body) pairs, avoiding borrow conflicts.
    // We clone the minimal data needed, then write back after inference.
    struct InferTarget {
        chunk_idx: usize,
        local_idx: u32,
        /// `true` = Function body, `false` = `QualifiedProperty` body (by qp index)
        kind: TargetKind,
        params: Vec<crate::types::Parameter>,
        body: Vec<crate::types::ValueSpec>,
    }

    enum TargetKind {
        FunctionBody,
        QualifiedProperty(usize),
    }

    let mut targets: Vec<InferTarget> = Vec::new();

    // Only infer in the current compilation chunk (the last one).
    // Bootstrap chunk (0) has no function/QP bodies.
    let chunk_idx = model.chunks.len() - 1;
    let chunk = &model.chunks[chunk_idx];

    for (local_idx, element) in chunk.elements.iter() {
        match element {
            Element::Function(f) if !f.body.is_empty() => {
                targets.push(InferTarget {
                    chunk_idx,
                    local_idx,
                    kind: TargetKind::FunctionBody,
                    params: f.parameters.clone(),
                    body: f.body.clone(),
                });
            }
            Element::Class(c) => {
                for (qp_idx, qp) in c.qualified_properties.iter().enumerate() {
                    if !qp.body.is_empty() {
                        targets.push(InferTarget {
                            chunk_idx,
                            local_idx,
                            kind: TargetKind::QualifiedProperty(qp_idx),
                            params: qp.parameters.clone(),
                            body: qp.body.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Run inference on cloned bodies, then write back.
    for target in &mut targets {
        infer::infer_function_body(model, &target.params, &mut target.body, errors);
    }

    // Write back the typed bodies.
    for target in targets {
        let element = model.chunks[target.chunk_idx]
            .elements
            .get_mut(target.local_idx);
        match target.kind {
            TargetKind::FunctionBody => {
                if let Element::Function(f) = element {
                    f.body = target.body;
                }
            }
            TargetKind::QualifiedProperty(qp_idx) => {
                if let Element::Class(c) = element {
                    c.qualified_properties[qp_idx].body = target.body;
                }
            }
        }
    }
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
        let result = compile!(&[]);
        assert!(result.is_ok());
        let model = result.unwrap();
        // Should have bootstrap chunk only
        assert_eq!(model.chunks.len(), 2); // bootstrap + empty user chunk
    }
}
