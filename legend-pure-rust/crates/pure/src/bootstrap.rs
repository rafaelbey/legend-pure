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

//! Bootstrap — Chunk 0 with well-known element IDs for built-in types.
//!
//! Every `PureModel` starts with a bootstrap chunk (`chunk_id` = 0) that
//! contains the primitive types, `Any`, and `Nil`. These have deterministic
//! `ElementId`s that can be used as compile-time constants.
//!
//! # Usage
//!
//! ```
//! use legend_pure_parser_pure::bootstrap;
//!
//! // These are compile-time constants — no model lookup needed:
//! let _ = bootstrap::ANY_ID;
//! let _ = bootstrap::STRING_ID;
//! ```

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::arena::Arena;
use crate::ids::{ElementId, PackageId};
use crate::model::{Element, ElementNode, ModelChunk};
use crate::nodes::class::Class;
use crate::types::PrimitiveType;

// ---------------------------------------------------------------------------
// Well-known ElementIds (deterministic, compile-time constants)
// ---------------------------------------------------------------------------

/// The bootstrap chunk ID.
pub const BOOTSTRAP_CHUNK_ID: u16 = 0;

// -- Type lattice --

/// `Any` — the top type. Everything is a subtype of `Any`.
pub const ANY_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 0,
};
/// `Nil` — the bottom type. Subtype of everything. Type of `[]`.
pub const NIL_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 1,
};

// -- Direct children of Any --

/// `String` — extends `Any`.
pub const STRING_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 2,
};
/// `Boolean` — extends `Any`.
pub const BOOLEAN_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 3,
};
/// `Byte` — extends `Any`.
pub const BYTE_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 4,
};
/// `StrictTime` — extends `Any`.
pub const STRICT_TIME_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 5,
};

// -- Numeric hierarchy: Number → Any --

/// `Number` — abstract numeric supertype. Extends `Any`.
pub const NUMBER_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 6,
};
/// `Integer` — extends `Number`.
pub const INTEGER_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 7,
};
/// `Float` — extends `Number`.
pub const FLOAT_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 8,
};
/// `Decimal` — extends `Number`.
pub const DECIMAL_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 9,
};

// -- Temporal hierarchy: Date → Any --

/// `Date` — abstract temporal supertype. Extends `Any`.
pub const DATE_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 10,
};
/// `StrictDate` — date without time. Extends `Date`.
pub const STRICT_DATE_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 11,
};
/// `DateTime` — date with time. Extends `Date`.
pub const DATE_TIME_ID: ElementId = ElementId {
    chunk_id: 0,
    local_idx: 12,
};

// ---------------------------------------------------------------------------
// Bootstrap types — ordered lists for chunk construction
// ---------------------------------------------------------------------------

/// The total number of bootstrap elements (2 classes + 11 primitives).
const BOOTSTRAP_ELEMENT_COUNT: usize = 13;

/// The well-known primitive types with their inheritance edges.
///
/// **Important:** `Any` and `Nil` are NOT in this list — they are `Class`
/// instances in the M3 metamodel, not `PrimitiveType` instances.
/// They are allocated separately in [`create_bootstrap_chunk`].
///
/// The inheritance hierarchy mirrors the Java M3:
///
/// ```text
/// Any (top, Class)          ← local_idx 0
/// ├── String                ← local_idx 2
/// ├── Boolean               ← local_idx 3
/// ├── Byte                  ← local_idx 4
/// ├── StrictTime            ← local_idx 5
/// ├── Number (abstract)     ← local_idx 6
/// │   ├── Integer           ← local_idx 7
/// │   ├── Float             ← local_idx 8
/// │   └── Decimal           ← local_idx 9
/// └── Date (abstract)       ← local_idx 10
///     ├── StrictDate        ← local_idx 11
///     └── DateTime          ← local_idx 12
///
/// Nil (bottom, Class)       ← local_idx 1
/// ```
const BOOTSTRAP_PRIMITIVES: &[(&str, ElementId, ElementId)] = &[
    // Direct children of Any
    ("String", STRING_ID, ANY_ID),
    ("Boolean", BOOLEAN_ID, ANY_ID),
    ("Byte", BYTE_ID, ANY_ID),
    ("StrictTime", STRICT_TIME_ID, ANY_ID),
    // Numeric hierarchy
    ("Number", NUMBER_ID, ANY_ID),
    ("Integer", INTEGER_ID, NUMBER_ID),
    ("Float", FLOAT_ID, NUMBER_ID),
    ("Decimal", DECIMAL_ID, NUMBER_ID),
    // Temporal hierarchy
    ("Date", DATE_ID, ANY_ID),
    ("StrictDate", STRICT_DATE_ID, DATE_ID),
    ("DateTime", DATE_TIME_ID, DATE_ID),
];

// ---------------------------------------------------------------------------
// Bootstrap chunk construction
// ---------------------------------------------------------------------------

/// Creates the bootstrap `ModelChunk` (`chunk_id` = 0) containing all
/// well-known types: `Any` (top class), `Nil` (bottom class), and the
/// 11 primitive types.
///
/// In the M3 metamodel:
/// - `Any` and `Nil` are instances of `Class` (not `PrimitiveType`)
/// - All other bootstrap types are instances of `PrimitiveType`
///
/// The `root_package` is used as the parent package for all bootstrap elements.
#[must_use]
pub fn create_bootstrap_chunk(root_package: PackageId) -> ModelChunk {
    let mut nodes = Arena::with_capacity(BOOTSTRAP_ELEMENT_COUNT);
    let mut elements = Arena::with_capacity(BOOTSTRAP_ELEMENT_COUNT);

    let synthetic_source = SourceInfo::new("<bootstrap>", 0, 0, 0, 0);

    // Helper to build an ElementNode.
    let mut alloc_node = |name: &str| {
        nodes.alloc(ElementNode {
            name: SmolStr::new(name),
            source_info: synthetic_source.clone(),
            parent_package: root_package,
        })
    };

    // -- Slot 0: Any (Class, top type) --
    let any_node = alloc_node("Any");
    let any_elem = elements.alloc(Element::Class(Class {
        type_parameters: vec![],
        super_types: vec![], // Any has no supertype
        properties: vec![],
        qualified_properties: vec![],
        constraints: vec![],
        stereotypes: vec![],
        tagged_values: vec![],
    }));
    debug_assert_eq!(any_node, ANY_ID.local_idx);
    debug_assert_eq!(any_elem, ANY_ID.local_idx);

    // -- Slot 1: Nil (Class, bottom type) --
    let nil_node = alloc_node("Nil");
    let nil_elem = elements.alloc(Element::Class(Class {
        type_parameters: vec![],
        super_types: vec![], // Nil's subtype-of-all is handled by type checker
        properties: vec![],
        qualified_properties: vec![],
        constraints: vec![],
        stereotypes: vec![],
        tagged_values: vec![],
    }));
    debug_assert_eq!(nil_node, NIL_ID.local_idx);
    debug_assert_eq!(nil_elem, NIL_ID.local_idx);

    // -- Slots 2..12: Primitive types --
    for &(name, expected_id, super_type) in BOOTSTRAP_PRIMITIVES {
        let actual_idx = alloc_node(name);
        let elem_idx = elements.alloc(Element::PrimitiveType(PrimitiveType {
            super_type: Some(super_type),
        }));

        debug_assert_eq!(actual_idx, expected_id.local_idx);
        debug_assert_eq!(elem_idx, expected_id.local_idx);
    }

    ModelChunk {
        chunk_id: BOOTSTRAP_CHUNK_ID,
        nodes,
        elements,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_chunk_has_correct_count() {
        let chunk = create_bootstrap_chunk(PackageId(0));
        assert_eq!(chunk.chunk_id, 0);
        #[allow(clippy::cast_possible_truncation)]
        let expected = BOOTSTRAP_ELEMENT_COUNT as u32;
        assert_eq!(chunk.nodes.len(), expected);
        assert_eq!(chunk.elements.len(), expected);
    }

    #[test]
    fn bootstrap_ids_match_names() {
        let chunk = create_bootstrap_chunk(PackageId(0));
        assert_eq!(chunk.nodes.get(ANY_ID.local_idx).name, "Any");
        assert_eq!(chunk.nodes.get(NIL_ID.local_idx).name, "Nil");
        assert_eq!(chunk.nodes.get(STRING_ID.local_idx).name, "String");
        assert_eq!(chunk.nodes.get(BOOLEAN_ID.local_idx).name, "Boolean");
        assert_eq!(chunk.nodes.get(BYTE_ID.local_idx).name, "Byte");
        assert_eq!(chunk.nodes.get(STRICT_TIME_ID.local_idx).name, "StrictTime");
        assert_eq!(chunk.nodes.get(NUMBER_ID.local_idx).name, "Number");
        assert_eq!(chunk.nodes.get(INTEGER_ID.local_idx).name, "Integer");
        assert_eq!(chunk.nodes.get(FLOAT_ID.local_idx).name, "Float");
        assert_eq!(chunk.nodes.get(DECIMAL_ID.local_idx).name, "Decimal");
        assert_eq!(chunk.nodes.get(DATE_ID.local_idx).name, "Date");
        assert_eq!(chunk.nodes.get(STRICT_DATE_ID.local_idx).name, "StrictDate");
        assert_eq!(chunk.nodes.get(DATE_TIME_ID.local_idx).name, "DateTime");
    }

    #[test]
    fn any_and_nil_are_classes() {
        let chunk = create_bootstrap_chunk(PackageId(0));
        assert!(matches!(
            chunk.elements.get(ANY_ID.local_idx),
            Element::Class(_)
        ));
        assert!(matches!(
            chunk.elements.get(NIL_ID.local_idx),
            Element::Class(_)
        ));
    }

    #[test]
    fn primitives_are_primitive_type() {
        let chunk = create_bootstrap_chunk(PackageId(0));
        for &(_, id, _) in BOOTSTRAP_PRIMITIVES {
            assert!(
                matches!(chunk.elements.get(id.local_idx), Element::PrimitiveType(_)),
                "{} should be PrimitiveType",
                chunk.nodes.get(id.local_idx).name
            );
        }
    }

    #[test]
    fn bootstrap_inheritance_edges() {
        let chunk = create_bootstrap_chunk(PackageId(0));

        // Any is a Class with no supertypes
        match chunk.elements.get(ANY_ID.local_idx) {
            Element::Class(c) => assert!(c.super_types.is_empty()),
            _ => panic!("Any should be a Class"),
        }

        // Nil is a Class with no supertypes (bottom handled by type checker)
        match chunk.elements.get(NIL_ID.local_idx) {
            Element::Class(c) => assert!(c.super_types.is_empty()),
            _ => panic!("Nil should be a Class"),
        }

        // Helper: extract super_type from a PrimitiveType
        let get_super = |id: ElementId| -> ElementId {
            match chunk.elements.get(id.local_idx) {
                Element::PrimitiveType(pt) => {
                    pt.super_type.expect("primitive should have a super_type")
                }
                _ => panic!("expected PrimitiveType"),
            }
        };

        // Direct children of Any
        assert_eq!(get_super(STRING_ID), ANY_ID);
        assert_eq!(get_super(BOOLEAN_ID), ANY_ID);
        assert_eq!(get_super(BYTE_ID), ANY_ID);
        assert_eq!(get_super(STRICT_TIME_ID), ANY_ID);
        assert_eq!(get_super(NUMBER_ID), ANY_ID);
        assert_eq!(get_super(DATE_ID), ANY_ID);

        // Numeric subtypes
        assert_eq!(get_super(INTEGER_ID), NUMBER_ID);
        assert_eq!(get_super(FLOAT_ID), NUMBER_ID);
        assert_eq!(get_super(DECIMAL_ID), NUMBER_ID);

        // Temporal subtypes
        assert_eq!(get_super(STRICT_DATE_ID), DATE_ID);
        assert_eq!(get_super(DATE_TIME_ID), DATE_ID);
    }

    #[test]
    fn bootstrap_source_info_is_synthetic() {
        let chunk = create_bootstrap_chunk(PackageId(0));
        let node = chunk.nodes.get(STRING_ID.local_idx);
        assert_eq!(node.source_info.source.as_str(), "<bootstrap>");
        assert_eq!(node.source_info.start_line, 0);
    }
}
