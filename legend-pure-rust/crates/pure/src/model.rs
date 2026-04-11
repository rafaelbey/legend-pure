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

//! The `PureModel` — the compiled semantic graph.
//!
//! This is the Rust equivalent of Java's `PureModel`. It holds:
//! - A **global package tree** (single arena, cross-chunk namespace)
//! - **Chunked element storage** (one [`ModelChunk`] per source file + Chunk 0 for bootstrap)
//! - **Derived indexes** computed post-freeze for reverse lookups
//!
//! # Lifecycle
//!
//! 1. **Mutable phase**: `PureModelBuilder` populates arenas via compiler passes.
//! 2. **Frozen phase**: After `rebuild_derived_indexes()`, the model is read-only.
//!    All query methods operate on the frozen model. Parallel validation is safe.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::type_ref::Package as AstPackage;
use smol_str::SmolStr;

use crate::arena::Arena;
use crate::ids::{ElementId, PackageId};
use crate::nodes::association::Association;
use crate::nodes::class::Class;
use crate::nodes::enumeration::Enumeration;
use crate::nodes::function::Function;
use crate::nodes::measure::Measure;
use crate::nodes::profile::Profile;
use crate::nodes::unit::Unit;
use crate::types::PrimitiveType;

// ---------------------------------------------------------------------------
// ElementNode — common metadata for all elements
// ---------------------------------------------------------------------------

/// Common metadata shared by all elements in the graph.
///
/// This is the equivalent of Java's `PackageableElement` — it provides
/// the fields needed for generic traversal (package tree, name resolution)
/// without knowing the concrete element type.
#[derive(Debug, Clone)]
pub struct ElementNode {
    /// The element's simple name (e.g., `"Person"`, `"String"`).
    pub name: SmolStr,
    /// Source location in the original `.pure` file.
    pub source_info: SourceInfo,
    /// The package this element belongs to.
    pub parent_package: PackageId,
}

// ---------------------------------------------------------------------------
// Element — the typed payload enum
// ---------------------------------------------------------------------------

/// A compiled Pure element with all references resolved to IDs.
///
/// This mirrors `ast::Element` but at the semantic level: string paths are
/// resolved to [`ElementId`]s, type arguments are lowered, and all names
/// are fully qualified.
///
/// **Not all Elements are Types.** `Profile`, `Association`, and `Function`
/// are packageable elements but cannot appear in type positions. Structural
/// types (`FunctionType`, `RelationType`) live in `TypeExpr`, not here.
///
/// See `DESIGN.md` §5 for the Element vs Type distinction.
#[derive(Debug, Clone)]
pub enum Element {
    /// A class with properties, constraints, and type parameters.
    Class(Class),
    /// An enumeration with named values.
    Enumeration(Enumeration),
    /// A top-level function definition.
    Function(Function),
    /// A profile defining stereotypes and tags.
    Profile(Profile),
    /// An association linking two classes.
    Association(Association),
    /// A measure with canonical and non-canonical units.
    Measure(Measure),
    /// A primitive type (String, Integer, etc.). Bootstrapped in Chunk 0.
    PrimitiveType(PrimitiveType),
    /// A unit within a measure (Kilogram, Meter). Each has its own `ElementId`.
    Unit(Unit),
}

// ---------------------------------------------------------------------------
// Package — node in the global package tree
// ---------------------------------------------------------------------------

/// A node in the global package tree.
///
/// Packages span across chunks — `meta::pure` may contain elements from
/// Chunk 0 (bootstrap), Chunk 1 (user code), and Chunk 2 (plugin code).
#[derive(Debug, Clone)]
pub struct Package {
    /// The package's simple name (e.g., `"pure"` in `meta::pure`).
    pub name: SmolStr,
    /// Parent package, or `None` for the root package.
    pub parent: Option<PackageId>,
    /// Direct child packages.
    pub children_packages: Vec<PackageId>,
    /// Elements in this package (across all chunks).
    pub children_elements: Vec<ElementId>,
}

// ---------------------------------------------------------------------------
// ModelChunk — a batch of elements from one source file
// ---------------------------------------------------------------------------

/// A batch of elements, typically from one parsed source file.
///
/// Chunk 0 is the bootstrap chunk (primitives, `Any`, `Nil`).
/// Each subsequent chunk corresponds to one user source file.
///
/// The `nodes` and `elements` arenas are **parallel** — `nodes[i]` and
/// `elements[i]` describe the same element. `nodes` holds common metadata,
/// `elements` holds the typed payload.
#[derive(Debug, Clone)]
pub struct ModelChunk {
    /// The chunk's unique identifier (0 = bootstrap).
    pub chunk_id: u16,
    /// Common metadata for each element (name, source, package).
    pub nodes: Arena<ElementNode>,
    /// Typed payload for each element (parallel to `nodes`).
    pub elements: Arena<Element>,
}

impl ModelChunk {
    /// Creates a new empty chunk with the given ID.
    #[must_use]
    pub fn new(chunk_id: u16) -> Self {
        Self {
            chunk_id,
            nodes: Arena::new(),
            elements: Arena::new(),
        }
    }

    /// Allocates an element in this chunk, keeping the parallel arenas in sync.
    ///
    /// Returns the `local_idx` that can be used to construct an `ElementId`.
    pub fn alloc_element(&mut self, node: ElementNode, element: Element) -> u32 {
        let node_idx = self.nodes.alloc(node);
        let elem_idx = self.elements.alloc(element);
        debug_assert_eq!(
            node_idx, elem_idx,
            "Parallel arena desync in chunk {}",
            self.chunk_id
        );
        node_idx
    }
}

// ---------------------------------------------------------------------------
// DerivedIndexes — computed post-freeze reverse lookups
// ---------------------------------------------------------------------------

/// Inverted indexes computed post-freeze in a single O(N) scan.
///
/// Never serialized (`#[serde(skip)]`). Rebuilt after deserialization
/// and after chunk merging.
#[derive(Debug, Clone, Default)]
pub struct DerivedIndexes {
    /// Class → its direct subclasses (from `super_types` edges).
    pub specializations: HashMap<ElementId, Vec<ElementId>>,
    /// Class → association-injected properties targeting it.
    ///
    /// Each entry is `(association_id, property_index)` — a lightweight
    /// reference to the property within the Association's `properties` vec.
    pub association_properties: HashMap<ElementId, Vec<(ElementId, usize)>>,
}

// ---------------------------------------------------------------------------
// PureModel — the top-level compiled model
// ---------------------------------------------------------------------------

/// The compiled Pure semantic graph — equivalent to Java's `PureModel`.
///
/// Contains the global package tree, chunked element storage, and derived
/// indexes for efficient querying.
#[derive(Debug)]
pub struct PureModel {
    /// Global package tree (single arena, cross-chunk).
    pub global_packages: Arena<Package>,
    /// The root (unnamed) package.
    pub root_package: PackageId,

    /// Chunked element storage. Chunk 0 = bootstrap.
    pub chunks: Vec<ModelChunk>,

    /// Extension arenas for plugin element types.
    pub extension_arenas: HashMap<TypeId, Box<dyn Any>>,

    /// Derived indexes, computed post-freeze.
    derived: DerivedIndexes,
}

impl PureModel {
    /// Creates a new empty `PureModel` with a root package.
    #[must_use]
    pub fn new() -> Self {
        let mut global_packages = Arena::new();
        let root_idx = global_packages.alloc(Package {
            name: SmolStr::new(""),
            parent: None,
            children_packages: Vec::new(),
            children_elements: Vec::new(),
        });

        Self {
            global_packages,
            root_package: PackageId(root_idx),
            chunks: Vec::new(),
            extension_arenas: HashMap::new(),
            derived: DerivedIndexes::default(),
        }
    }

    /// Returns the element node for the given ID.
    ///
    /// # Panics
    ///
    /// Panics if the chunk or index is out of bounds.
    #[must_use]
    pub fn get_node(&self, id: ElementId) -> &ElementNode {
        let chunk = &self.chunks[id.chunk_id as usize];
        chunk.nodes.get(id.local_idx)
    }

    /// Returns the typed element for the given ID.
    ///
    /// # Panics
    ///
    /// Panics if the chunk or index is out of bounds.
    #[must_use]
    pub fn get_element(&self, id: ElementId) -> &Element {
        let chunk = &self.chunks[id.chunk_id as usize];
        chunk.elements.get(id.local_idx)
    }

    /// Returns the typed element for the given ID, or `None` if out of bounds.
    ///
    /// Use this when the ID might reference an unresolved or invalid element.
    #[must_use]
    pub fn try_get_element(&self, id: ElementId) -> Option<&Element> {
        self.chunks.get(id.chunk_id as usize).and_then(|chunk| {
            if id.local_idx < chunk.elements.len() {
                Some(chunk.elements.get(id.local_idx))
            } else {
                None
            }
        })
    }

    /// Returns the package for the given ID.
    #[must_use]
    pub fn get_package(&self, id: PackageId) -> &Package {
        self.global_packages.get(id.0)
    }

    // -- Derived Index Methods -----------------------------------------------

    /// Rebuilds all derived indexes by scanning every element in every chunk.
    ///
    /// Call this after Pass 2 completes and after deserialization.
    /// This is an O(N) scan where N = total number of elements.
    pub fn rebuild_derived_indexes(&mut self) {
        self.derived = DerivedIndexes::default();

        for chunk in &self.chunks {
            for (local_idx, element) in chunk.elements.iter() {
                let id = ElementId {
                    chunk_id: chunk.chunk_id,
                    local_idx,
                };

                match element {
                    Element::Class(class) => {
                        // Register specialization edges (reverse of super_types)
                        for super_type in &class.super_types {
                            if let crate::types::TypeExpr::Named {
                                element: parent_id, ..
                            } = super_type
                            {
                                self.derived
                                    .specializations
                                    .entry(*parent_id)
                                    .or_default()
                                    .push(id);
                            }
                        }
                    }
                    Element::Association(assoc) => {
                        // Register association-injected properties on target classes
                        for (prop_idx, prop) in assoc.properties.iter().enumerate() {
                            if let crate::types::TypeExpr::Named {
                                element: target_id, ..
                            } = &prop.type_expr
                            {
                                self.derived
                                    .association_properties
                                    .entry(*target_id)
                                    .or_default()
                                    .push((id, prop_idx));
                            }
                        }
                    }
                    Element::Enumeration(_)
                    | Element::Function(_)
                    | Element::Profile(_)
                    | Element::Measure(_)
                    | Element::Unit(_)
                    | Element::PrimitiveType(_) => {}
                }
            }
        }
    }

    /// Returns direct subclasses of the given class (derived index).
    ///
    /// Returns an empty slice if no specializations exist.
    #[must_use]
    pub fn specializations(&self, id: ElementId) -> &[ElementId] {
        self.derived
            .specializations
            .get(&id)
            .map_or(&[], Vec::as_slice)
    }

    /// Returns association-injected properties targeting the given class.
    ///
    /// Each entry is `(association_id, property_index)` into the association's
    /// `properties` vec.
    #[must_use]
    pub fn association_properties(&self, id: ElementId) -> &[(ElementId, usize)] {
        self.derived
            .association_properties
            .get(&id)
            .map_or(&[], Vec::as_slice)
    }

    /// Returns the derived indexes (available only after freeze).
    #[must_use]
    pub fn derived(&self) -> &DerivedIndexes {
        &self.derived
    }

    /// Finds or creates a package by path segments from the root.
    ///
    /// For example, `["meta", "pure"]` returns the `PackageId` for `meta::pure`,
    /// creating intermediate packages as needed.
    pub fn get_or_create_package(&mut self, path: &[SmolStr]) -> PackageId {
        let mut current = self.root_package;
        for segment in path {
            // Check if child already exists
            let existing = self
                .global_packages
                .get(current.0)
                .children_packages
                .iter()
                .find(|&&child_id| self.global_packages.get(child_id.0).name == *segment)
                .copied();

            current = if let Some(id) = existing {
                id
            } else {
                let new_id = PackageId(self.global_packages.alloc(Package {
                    name: segment.clone(),
                    parent: Some(current),
                    children_packages: Vec::new(),
                    children_elements: Vec::new(),
                }));
                self.global_packages
                    .get_mut(current.0)
                    .children_packages
                    .push(new_id);
                new_id
            };
        }
        current
    }

    /// Registers an element in a package's children list.
    pub fn register_element(&mut self, package: PackageId, element: ElementId) {
        self.global_packages
            .get_mut(package.0)
            .children_elements
            .push(element);
    }

    /// Resolves a fully qualified name to an `ElementId` by walking the package tree.
    ///
    /// Returns `None` if the name doesn't resolve.
    #[must_use]
    pub fn resolve_by_path(&self, path: &[SmolStr]) -> Option<ElementId> {
        if path.is_empty() {
            return None;
        }

        // Walk to the parent package
        let (pkg_path, name) = path.split_at(path.len() - 1);
        let mut current = self.root_package;

        for segment in pkg_path {
            let pkg = self.get_package(current);
            current = *pkg
                .children_packages
                .iter()
                .find(|&&child_id| self.global_packages.get(child_id.0).name == *segment)?;
        }

        // Find element by name in the target package
        let pkg = self.get_package(current);
        let target_name = &name[0];
        pkg.children_elements
            .iter()
            .find(|&&eid| self.get_node(eid).name == *target_name)
            .copied()
    }

    /// Resolves an AST `Package` to the corresponding model `PackageId`.
    ///
    /// Walks the recursive `Package` tree directly — no intermediate
    /// `Vec<SmolStr>` allocation needed.
    #[must_use]
    pub fn resolve_package(&self, pkg: &AstPackage) -> Option<PackageId> {
        let mut current = self.root_package;

        // Walk parent chain first (recursive → iterative via segments)
        for segment in pkg.segments() {
            let model_pkg = self.get_package(current);
            current = *model_pkg
                .children_packages
                .iter()
                .find(|&&child_id| self.global_packages.get(child_id.0).name == *segment)?;
        }

        Some(current)
    }

    /// Resolves an element name within an AST `Package`.
    ///
    /// Equivalent to `resolve_by_path(&[...pkg_segments, name])` but walks
    /// the AST `Package` tree directly with zero allocations.
    #[must_use]
    pub fn resolve_in_package(&self, pkg: &AstPackage, name: &SmolStr) -> Option<ElementId> {
        let pkg_id = self.resolve_package(pkg)?;
        let model_pkg = self.get_package(pkg_id);
        model_pkg
            .children_elements
            .iter()
            .find(|&&eid| self.get_node(eid).name == *name)
            .copied()
    }
}

impl Default for PureModel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::class;

    fn test_source() -> SourceInfo {
        SourceInfo::new("test.pure", 1, 1, 1, 10)
    }

    fn empty_class() -> Class {
        class::Class {
            type_parameters: vec![],
            super_types: vec![],
            properties: vec![],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![],
            tagged_values: vec![],
        }
    }

    #[test]
    fn new_model_has_root_package() {
        let model = PureModel::new();
        let root = model.get_package(model.root_package);
        assert_eq!(root.name, "");
        assert!(root.parent.is_none());
        assert!(root.children_packages.is_empty());
        assert!(root.children_elements.is_empty());
    }

    #[test]
    fn model_chunk_parallel_arenas() {
        let mut chunk = ModelChunk::new(1);
        let node_idx = chunk.nodes.alloc(ElementNode {
            name: SmolStr::new("Person"),
            source_info: test_source(),
            parent_package: PackageId(0),
        });
        let elem_idx = chunk.elements.alloc(Element::Class(empty_class()));

        // Parallel: same index in both arenas
        assert_eq!(node_idx, elem_idx);
        assert_eq!(chunk.nodes.get(node_idx).name, "Person");
        assert!(matches!(chunk.elements.get(elem_idx), Element::Class(_)));
    }

    #[test]
    fn model_get_element_and_node() {
        let mut model = PureModel::new();
        let mut chunk = ModelChunk::new(0);
        chunk.nodes.alloc(ElementNode {
            name: SmolStr::new("String"),
            source_info: test_source(),
            parent_package: model.root_package,
        });
        chunk
            .elements
            .alloc(Element::PrimitiveType(PrimitiveType { super_type: None }));
        model.chunks.push(chunk);

        let id = ElementId {
            chunk_id: 0,
            local_idx: 0,
        };
        assert_eq!(model.get_node(id).name, "String");
        assert!(matches!(model.get_element(id), Element::PrimitiveType(_)));
    }

    #[test]
    fn model_default_is_empty() {
        let model = PureModel::default();
        assert!(model.chunks.is_empty());
        assert!(model.derived().specializations.is_empty());
    }

    #[test]
    fn model_with_bootstrap_chunk() {
        let mut model = PureModel::new();
        let bootstrap = crate::bootstrap::create_bootstrap_chunk(model.root_package);
        model.chunks.push(bootstrap);

        // Verify we can look up primitives by well-known IDs
        assert_eq!(model.get_node(crate::bootstrap::STRING_ID).name, "String");
        assert_eq!(model.get_node(crate::bootstrap::ANY_ID).name, "Any");
        assert!(matches!(
            model.get_element(crate::bootstrap::INTEGER_ID),
            Element::PrimitiveType(_)
        ));
    }
}
