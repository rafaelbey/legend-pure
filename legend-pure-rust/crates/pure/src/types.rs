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
// DateValue — parsed date/time literals
// ---------------------------------------------------------------------------

/// A parsed date/time value for use in compiled expressions.
///
/// Parsed from the raw strings in `ast::StrictDateLiteral`, `ast::DateTimeLiteral`,
/// and `ast::StrictTimeLiteral`. Avoids carrying raw strings through the
/// semantic graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateValue {
    /// Strict date: `%2024-01-15`.
    StrictDate {
        /// Year.
        year: i32,
        /// Month (1–12).
        month: u8,
        /// Day (1–31).
        day: u8,
    },
    /// Date-time: `%2024-01-15T10:30:00`.
    DateTime {
        /// Year.
        year: i32,
        /// Month (1–12).
        month: u8,
        /// Day (1–31).
        day: u8,
        /// Hour (0–23).
        hour: u8,
        /// Minute (0–59).
        minute: u8,
        /// Second (0–59).
        second: u8,
        /// Sub-second nanoseconds.
        subsecond_nanos: u32,
    },
    /// Strict time: `%10:30:00`.
    StrictTime {
        /// Hour (0–23).
        hour: u8,
        /// Minute (0–59).
        minute: u8,
        /// Second (0–59).
        second: u8,
        /// Sub-second nanoseconds.
        subsecond_nanos: u32,
    },
}

// ---------------------------------------------------------------------------
// ValueSpec — compiled expression (replaces placeholder)
// ---------------------------------------------------------------------------

/// A compiled value specification — the semantic expression type.
///
/// All names are resolved. Operators are desugared to function calls.
/// `let` is `FunctionCall("letFunction", ...)`, `new` is
/// `FunctionCall("new", ...)`. `Group(...)` is eliminated (unwrapped).
///
/// This is the Rust equivalent of Java's `ValueSpecification` hierarchy
/// (M3 metamodel: `InstanceValue`, `SimpleFunctionExpression`,
/// `VariableExpression`).
#[derive(Debug, Clone, PartialEq)]
pub enum ValueSpec {
    // -- Literals ----------------------------------------------------------
    /// Integer literal: `42`.
    IntegerLiteral(i64, SourceInfo),
    /// Float literal: `3.14`.
    FloatLiteral(f64, SourceInfo),
    /// Decimal literal: `3.14D`.
    DecimalLiteral(rust_decimal::Decimal, SourceInfo),
    /// String literal: `'hello'`.
    StringLiteral(SmolStr, SourceInfo),
    /// Boolean literal: `true`, `false`.
    BooleanLiteral(bool, SourceInfo),
    /// Date/time literal: `%2024-01-15`, `%2024-01-15T10:30:00`, `%10:30:00`.
    DateLiteral(DateValue, SourceInfo),

    // -- Variable reference ------------------------------------------------
    /// Variable reference: `$name`.
    ///
    /// Just a name. Type/multiplicity information lives on [`Parameter`]
    /// in function/lambda definitions — *not* on the variable reference.
    Variable {
        /// Variable name (without the `$` prefix).
        name: SmolStr,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Function call (covers operators, let, new, arrow) -----------------
    /// Any function call, including desugared operators, `let`, `new`, arrow.
    ///
    /// Operators desugar to: `plus`, `minus`, `times`, `divide`, `equal`,
    /// `lessThan`, `and`, `or`, `not`, etc.
    /// `let` desugars to `letFunction`. `new` desugars to `new`.
    /// Arrow `x->filter(p)` becomes `FunctionCall("filter", [x, p])`.
    FunctionCall {
        /// Resolved function element (user-defined functions).
        function: Option<ElementId>,
        /// Function name (for built-ins, operators, unresolved).
        function_name: SmolStr,
        /// Arguments.
        arguments: Vec<ValueSpec>,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Property access ---------------------------------------------------
    /// Simple property access: `$x.name`.
    PropertyAccess {
        /// The target expression.
        target: Box<ValueSpec>,
        /// Property name.
        property: SmolStr,
        /// Source location.
        source_info: SourceInfo,
    },
    /// Qualified property access: `$x.derived('arg')`.
    QualifiedPropertyAccess {
        /// The target expression.
        target: Box<ValueSpec>,
        /// Property name.
        property: SmolStr,
        /// Arguments.
        arguments: Vec<ValueSpec>,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Enum value --------------------------------------------------------
    /// Enum value reference: `MyEnum.VALUE` (after semantic disambiguation).
    EnumValue {
        /// The resolved Enumeration element.
        enum_element: ElementId,
        /// The enum value name.
        value: SmolStr,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Lambda ------------------------------------------------------------
    /// Lambda expression: `{x: String[1] | $x + 'hello'}`.
    Lambda {
        /// Lambda parameters.
        parameters: Vec<Parameter>,
        /// Body expressions.
        body: Vec<ValueSpec>,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Collection --------------------------------------------------------
    /// Collection literal: `[1, 2, 3]`.
    Collection {
        /// Elements.
        elements: Vec<ValueSpec>,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Type reference ----------------------------------------------------
    /// Type reference expression: `@MyType`.
    TypeReference {
        /// The resolved type.
        type_expr: TypeExpr,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Element reference (bare) ------------------------------------------
    /// Bare element reference: `String`, `my::Enum`, `MyClass`.
    PackageableElementRef {
        /// The resolved element.
        element: ElementId,
        /// Source location.
        source_info: SourceInfo,
    },

    // -- Column (TDS — placeholder) ----------------------------------------
    /// Column expression (TDS — full lowering deferred).
    Column {
        /// Source location.
        source_info: SourceInfo,
    },
}

/// Backward-compatible alias: existing code uses `Expression` throughout
/// (`Function.body`, `Constraint.function`, `QualifiedProperty.body`, etc.).
pub type Expression = ValueSpec;

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
            element: ElementId {
                chunk_id: 0,
                local_idx: 2,
            },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        assert!(matches!(ty, TypeExpr::Named { .. }));
    }

    #[test]
    fn type_expr_named_with_type_args() {
        // List<String>
        let string_ty = TypeExpr::Named {
            element: ElementId {
                chunk_id: 0,
                local_idx: 2,
            },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        let list_ty = TypeExpr::Named {
            element: ElementId {
                chunk_id: 0,
                local_idx: 20,
            },
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
            element: ElementId {
                chunk_id: 0,
                local_idx: 12,
            },
            type_arguments: vec![],
            value_arguments: vec![ConstValue::Integer(255)],
        };
        if let TypeExpr::Named {
            value_arguments, ..
        } = &ty
        {
            assert_eq!(value_arguments.len(), 1);
            assert!(matches!(&value_arguments[0], ConstValue::Integer(255)));
        }
    }

    #[test]
    fn type_expr_function_type() {
        // {String[1] -> Boolean[1]}
        let string = TypeExpr::Named {
            element: ElementId {
                chunk_id: 0,
                local_idx: 2,
            },
            type_arguments: vec![],
            value_arguments: vec![],
        };
        let boolean = TypeExpr::Named {
            element: ElementId {
                chunk_id: 0,
                local_idx: 5,
            },
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
        let r = Multiplicity::Range {
            lower: 2,
            upper: Some(5),
        };
        assert!(matches!(
            r,
            Multiplicity::Range {
                lower: 2,
                upper: Some(5)
            }
        ));
    }
}
