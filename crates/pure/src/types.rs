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

//! Shared type-system types for the Pure semantic graph.
//!
//! [`TypeExpr`] is the Rust equivalent of Java's `GenericType`. Every property
//! type, parameter type, and return type in the compiled model is a `TypeExpr`.
//!
//! See `DESIGN.md` §6 for full rationale.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::ids::{ElementId, RelationId};

// ---------------------------------------------------------------------------
// TypeExpr — the Rust equivalent of Java's GenericType
// ---------------------------------------------------------------------------

/// A resolved or partially-resolved type expression in the Pure model.
///
/// This is the equivalent of Java's `GenericType`. In Java you almost never
/// reference a `Type` directly — you wrap it in a `GenericType` that carries
/// type arguments, multiplicity arguments, and type parameter bindings.
///
/// `TypeExpr` plays the same role: it's the currency of the Pure type system.
///
/// # Variants
///
/// | Pure Syntax | Variant |
/// |---|---|
/// | `String`, `Person` | `Named { element, type_arguments: [], .. }` |
/// | `List<String>` | `Named { element: list_id, type_arguments: [Named(string_id)] }` |
/// | `Varchar(255)` | `Named { element: varchar_id, value_arguments: [Integer(255)] }` |
/// | `{String[1] -> Bool[1]}` | `FunctionType { .. }` |
/// | `(a: Integer, b: String)` | `Relation(relation_id)` |
/// | `T`, `U` | `Generic("T")` |
/// | `T + V` | `AlgebraUnion(..)` |
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// A resolved named type, optionally with type and/or value arguments.
    ///
    /// Covers: `String`, `Person`, `List<String>`, `Map<K,V>`, `Varchar(255)`.
    Named {
        /// The resolved element (`Class`, `Enum`, `PrimitiveType`, `Measure`, `Unit`).
        element: ElementId,
        /// Generic type arguments: `<String, Integer>`.
        type_arguments: Vec<TypeExpr>,
        /// Compile-time value arguments: `(255, 'ok')`.
        value_arguments: Vec<ConstValue>,
    },
    /// Anonymous function signature: `{String[1] -> Boolean[1]}`.
    ///
    /// Not an Element — structural type only.
    FunctionType {
        /// Parameter types with multiplicities.
        parameters: Vec<(TypeExpr, Multiplicity)>,
        /// Return type.
        return_type: Box<TypeExpr>,
        /// Return multiplicity.
        return_multiplicity: Multiplicity,
    },
    /// Structural relation type (anonymous column bag), interned for deduplication.
    ///
    /// Not an Element — structural type only.
    Relation(RelationId),
    /// An unresolved type variable: `T`, `U`.
    Generic(SmolStr),
    /// Algebraic union of two relation types: `T + V`.
    AlgebraUnion(Box<TypeExpr>, Box<TypeExpr>),
}

// ---------------------------------------------------------------------------
// ConstValue — compile-time value arguments for parameterized types
// ---------------------------------------------------------------------------

/// A compile-time constant value used in type parameterization.
///
/// For example, `Varchar(255)` uses `ConstValue::Integer(255)`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// Integer value, e.g., `200` in `VARCHAR(200)`.
    Integer(i64),
    /// String value, e.g., `'ok'` in `Res('ok')`.
    String(String),
}

// ---------------------------------------------------------------------------
// Multiplicity — mirrors ast::Multiplicity without Spanned metadata
// ---------------------------------------------------------------------------

/// Multiplicity specification for properties and parameters.
///
/// Mirrors `ast::Multiplicity` but without source location metadata —
/// at the semantic level, multiplicities are structural values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Multiplicity {
    /// Exactly one: `[1]` — `lower=1, upper=1`.
    PureOne,
    /// Zero or one: `[0..1]`.
    ZeroOrOne,
    /// Zero or more: `[*]`.
    ZeroOrMany,
    /// One or more: `[1..*]`.
    OneOrMany,
    /// Arbitrary range: `[lower..upper]`.
    Range {
        /// Lower bound (inclusive).
        lower: u32,
        /// Upper bound (inclusive), `None` = unbounded.
        upper: Option<u32>,
    },
}

// ---------------------------------------------------------------------------
// Parameter
// ---------------------------------------------------------------------------

/// A function or qualified property parameter (resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    /// Parameter name.
    pub name: SmolStr,
    /// Parameter type.
    pub type_expr: TypeExpr,
    /// Parameter multiplicity.
    pub multiplicity: Multiplicity,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Expression (placeholder)
// ---------------------------------------------------------------------------

/// Placeholder for compiled expressions.
///
/// Full expression lowering (`ast::Expression` → resolved expression graph)
/// is a later phase. For now this is an opaque wrapper.
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    /// Source location of the expression.
    pub source_info: SourceInfo,
    // TODO: Phase 4+ — recursive enum mirroring ast::Expression
    // with all names resolved to ElementIds.
}

// ---------------------------------------------------------------------------
// PrimitiveType
// ---------------------------------------------------------------------------

/// A primitive type element (String, Integer, Boolean, etc.).
///
/// **Note:** `Any` and `Nil` are *not* primitive types — they are `Class`
/// instances in the M3 metamodel. Only the 11 concrete types below use this.
///
/// Unlike Classes, primitive types have at most one supertype and form
/// a strict tree (not a DAG). The hierarchy mirrors the Java M3:
///
/// ```text
/// Any (top)
/// ├── String
/// ├── Boolean
/// ├── Byte
/// ├── StrictTime
/// ├── Number (abstract)
/// │   ├── Integer
/// │   ├── Float
/// │   └── Decimal
/// └── Date (abstract)
///     ├── StrictDate
///     └── DateTime
///
/// Nil (bottom — subtype of everything, handled by type checker)
/// ```
///
/// All metadata (name, source, package) lives in the parallel `ElementNode`.
/// Bootstrapped in Chunk 0.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimitiveType {
    /// The single supertype for this primitive.
    ///
    /// All 11 primitives have `Some(parent)` pointing to their parent in
    /// the type tree (e.g., `Integer.super_type = Some(NUMBER_ID)`).
    pub super_type: Option<crate::ids::ElementId>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_expr_named_simple() {
        let ty = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 2 },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        assert!(matches!(ty, TypeExpr::Named { .. }));
    }

    #[test]
    fn type_expr_named_with_type_args() {
        // List<String>
        let string_ty = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 2 },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        let list_ty = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 20 },
            type_arguments: vec![string_ty],
            value_arguments: vec![],
        };
        if let TypeExpr::Named { type_arguments, .. } = &list_ty {
            assert_eq!(type_arguments.len(), 1);
        }
    }

    #[test]
    fn type_expr_named_with_value_args() {
        // Varchar(255)
        let ty = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 12 },
            type_arguments: vec![],
            value_arguments: vec![ConstValue::Integer(255)],
        };
        if let TypeExpr::Named { value_arguments, .. } = &ty {
            assert_eq!(value_arguments.len(), 1);
            assert!(matches!(&value_arguments[0], ConstValue::Integer(255)));
        }
    }

    #[test]
    fn type_expr_function_type() {
        // {String[1] -> Boolean[1]}
        let string = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 2 },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        let boolean = TypeExpr::Named {
            element: ElementId { chunk_id: 0, local_idx: 5 },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        let ft = TypeExpr::FunctionType {
            parameters: vec![(string, Multiplicity::PureOne)],
            return_type: Box::new(boolean),
            return_multiplicity: Multiplicity::PureOne,
        };
        assert!(matches!(ft, TypeExpr::FunctionType { .. }));
    }

    #[test]
    fn type_expr_generic() {
        let ty = TypeExpr::Generic(SmolStr::new("T"));
        assert!(matches!(ty, TypeExpr::Generic(_)));
    }

    #[test]
    fn multiplicity_variants() {
        assert!(matches!(Multiplicity::PureOne, Multiplicity::PureOne));
        assert!(matches!(Multiplicity::ZeroOrOne, Multiplicity::ZeroOrOne));
        assert!(matches!(Multiplicity::ZeroOrMany, Multiplicity::ZeroOrMany));
        assert!(matches!(Multiplicity::OneOrMany, Multiplicity::OneOrMany));
        let r = Multiplicity::Range { lower: 2, upper: Some(5) };
        assert!(matches!(r, Multiplicity::Range { lower: 2, upper: Some(5) }));
    }
}
