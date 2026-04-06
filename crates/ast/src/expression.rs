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

//! Expression and value specification types.
//!
//! ## Design Decisions
//!
//! - **`Literal` enum**: All literal types grouped under a single enum so callers
//!   can match on "is this a literal?" without enumerating every type.
//! - **`MemberAccess` enum**: Simple (`$x.name`) vs qualified (`$x.derived('arg')`)
//!   access are separate variants for type-safe handling.
//! - **Unified dot-access**: Both property access and enum value access parse
//!   identically (`expr.identifier`); disambiguation is semantic.
//! - **No `Cast`/`InstanceOf`/`If` nodes**: These are ordinary function/arrow calls.
//! - **No island types**: `GraphFetchTree`, `Path` deferred to island plugins.
//! - **Bitwise operators**: Supported from day one.

use crate::annotation::{PackageableElementPtr, Parameter};
use crate::source_info::{SourceInfo, Spanned};
use crate::type_ref::{Identifier, Multiplicity, TypeReference};

// ---------------------------------------------------------------------------
// Expression enum
// ---------------------------------------------------------------------------

/// An expression in the Pure grammar.
///
/// This is a recursive type — expressions contain sub-expressions.
/// All variants carry [`SourceInfo`] for precise error reporting.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    // -- Literals --
    /// Any literal value (integer, float, string, boolean, date, etc.).
    Literal(Literal),

    // -- Variables --
    /// Variable reference: `$name`.
    Variable(Variable),

    // -- Operators --
    /// Arithmetic: `+`, `-`, `*`, `/`.
    Arithmetic(ArithmeticExpr),
    /// Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`.
    Comparison(ComparisonExpr),
    /// Logical: `&&`, `||`.
    Logical(LogicalExpr),
    /// Bitwise: `&`, `|`, `^`, `<<`, `>>`.
    Bitwise(BitwiseExpr),
    /// Unary not: `!expr`.
    Not(NotExpr),
    /// Unary minus: `-expr`.
    UnaryMinus(UnaryMinusExpr),
    /// Bitwise complement: `~expr`.
    BitwiseNot(BitwiseNotExpr),

    // -- Function & member access --
    /// Function application: `func(args)` or `pkg::func(args)`.
    FunctionApplication(FunctionApplication),
    /// Arrow (collection) function: `x->filter(...)`.
    ArrowFunction(ArrowFunction),
    /// Member access (dot): `$x.name` or `$x.derived('arg')`.
    MemberAccess(MemberAccess),
    /// Bare packageable element reference: `String`, `my::Enum`, `MyClass`.
    ///
    /// Distinguished from `FunctionApplication` by having no argument list.
    /// Java grammar: `instanceReference: qualifiedName` (without `allOrFunction`).
    PackageableElementRef(PackageableElementRef),

    // -- Type reference --
    /// Type reference expression: `@MyType`.
    TypeReferenceExpr(TypeReferenceExpr),

    // -- Complex expressions --
    /// Lambda: `{x: String[1] | $x + 'hello'}` or `x | $x + 1`.
    Lambda(Lambda),
    /// Let binding: `let x = expr; ...`.
    Let(LetExpr),
    /// Collection literal: `[1, 2, 3]`.
    Collection(CollectionExpr),
    /// New instance: `^MyClass(name='John')`.
    NewInstance(NewInstanceExpr),

    // -- Column specification (TDS) --
    /// Column expression covering all column syntax variants.
    Column(ColumnExpression),

    // -- Grouping --
    /// Explicit parenthesized grouping: `(expr)`.
    ///
    /// Preserves source parentheses for faithful roundtripping. Semantically
    /// transparent — the inner expression is the value.
    Group(Box<Expression>),
}

impl Spanned for Expression {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Literal(e) => e.source_info(),
            Self::Variable(e) => &e.source_info,
            Self::Arithmetic(e) => &e.source_info,
            Self::Comparison(e) => &e.source_info,
            Self::Logical(e) => &e.source_info,
            Self::Bitwise(e) => &e.source_info,
            Self::Not(e) => &e.source_info,
            Self::UnaryMinus(e) => &e.source_info,
            Self::BitwiseNot(e) => &e.source_info,
            Self::FunctionApplication(e) => &e.source_info,
            Self::ArrowFunction(e) => &e.source_info,
            Self::MemberAccess(e) => e.source_info(),
            Self::PackageableElementRef(e) => &e.source_info,
            Self::TypeReferenceExpr(e) => &e.source_info,
            Self::Lambda(e) => &e.source_info,
            Self::Let(e) => &e.source_info,
            Self::Collection(e) => &e.source_info,
            Self::NewInstance(e) => &e.source_info,
            Self::Column(e) => e.source_info(),
            Self::Group(e) => e.source_info(),
        }
    }
}

// ---------------------------------------------------------------------------
// Literal enum
// ---------------------------------------------------------------------------

/// All literal types grouped together.
///
/// Enables pattern matching on "is this any literal?" without enumerating
/// every literal kind in calling code.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Integer literal: `42`, `-1`.
    Integer(IntegerLiteral),
    /// Float literal: `3.14`, `-0.5`.
    Float(FloatLiteral),
    /// Decimal literal: `3.14D`.
    Decimal(DecimalLiteral),
    /// String literal: `'hello'`.
    String(StringLiteral),
    /// Boolean literal: `true`, `false`.
    Boolean(BooleanLiteral),
    /// Strict date literal: `%2024-01-15`.
    StrictDate(StrictDateLiteral),
    /// Date-time literal: `%2024-01-15T10:30:00`.
    DateTime(DateTimeLiteral),
    /// Strict time literal: `%10:30:00`.
    StrictTime(StrictTimeLiteral),
}

impl Spanned for Literal {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Integer(e) => &e.source_info,
            Self::Float(e) => &e.source_info,
            Self::Decimal(e) => &e.source_info,
            Self::String(e) => &e.source_info,
            Self::Boolean(e) => &e.source_info,
            Self::StrictDate(e) => &e.source_info,
            Self::DateTime(e) => &e.source_info,
            Self::StrictTime(e) => &e.source_info,
        }
    }
}

// ---------------------------------------------------------------------------
// Literal value types
// ---------------------------------------------------------------------------

/// Integer literal: `42`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct IntegerLiteral {
    /// The integer value.
    pub value: i64,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Float literal: `3.14`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct FloatLiteral {
    /// The float value.
    pub value: f64,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Decimal literal: `3.14D`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct DecimalLiteral {
    /// The raw decimal string (preserves precision beyond f64).
    pub value: String,
    /// Source location.
    pub source_info: SourceInfo,
}

/// String literal: `'hello'`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct StringLiteral {
    /// The string value (unescaped).
    pub value: String,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Boolean literal: `true` or `false`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct BooleanLiteral {
    /// The boolean value.
    pub value: bool,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Strict date literal: `%2024-01-15`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct StrictDateLiteral {
    /// The raw date string.
    pub value: String,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Date-time literal: `%2024-01-15T10:30:00`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct DateTimeLiteral {
    /// The raw datetime string.
    pub value: String,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Strict time literal: `%10:30:00`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct StrictTimeLiteral {
    /// The raw time string.
    pub value: String,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Variable
// ---------------------------------------------------------------------------

/// A variable reference: `$name`.
#[derive(Debug, Clone, PartialEq, Eq, crate::Spanned)]
pub struct Variable {
    /// Variable name (without the `$` prefix).
    pub name: Identifier,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// Arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithmeticOp {
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Times,
    /// `/`
    Divide,
}

/// An arithmetic expression: `left op right`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ArithmeticExpr {
    /// Left operand.
    pub left: Box<Expression>,
    /// The operator.
    pub op: ArithmeticOp,
    /// Right operand.
    pub right: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `<`
    LessThan,
    /// `<=`
    LessThanOrEqual,
    /// `>`
    GreaterThan,
    /// `>=`
    GreaterThanOrEqual,
}

/// A comparison expression: `left op right`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ComparisonExpr {
    /// Left operand.
    pub left: Box<Expression>,
    /// The operator.
    pub op: ComparisonOp,
    /// Right operand.
    pub right: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Logical operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    /// `&&`
    And,
    /// `||`
    Or,
}

/// A logical expression: `left op right`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct LogicalExpr {
    /// Left operand.
    pub left: Box<Expression>,
    /// The operator.
    pub op: LogicalOp,
    /// Right operand.
    pub right: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Bitwise operator.
///
/// Uses F#-style triple operators to avoid ambiguity with existing Pure
/// syntax: `|` (lambda pipe), `^` (new instance), `<<`/`>>` (stereotypes),
/// and `&&`/`||` (logical AND/OR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitwiseOp {
    /// `&&&`
    And,
    /// `|||`
    Or,
    /// `^^^`
    Xor,
    /// `<<<`
    ShiftLeft,
    /// `>>>`
    ShiftRight,
}

/// A bitwise expression: `left op right`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct BitwiseExpr {
    /// Left operand.
    pub left: Box<Expression>,
    /// The operator.
    pub op: BitwiseOp,
    /// Right operand.
    pub right: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Unary not: `!expr`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct NotExpr {
    /// The operand.
    pub operand: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Unary minus: `-expr`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct UnaryMinusExpr {
    /// The operand.
    pub operand: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Bitwise complement: `~~~expr`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct BitwiseNotExpr {
    /// The operand.
    pub operand: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Function & member access
// ---------------------------------------------------------------------------

/// A function application: `func(args)` or `pkg::func(args)`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct FunctionApplication {
    /// The function being called (a packageable element reference).
    pub function: PackageableElementPtr,
    /// Arguments.
    pub arguments: Vec<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Arrow (collection) function: `expr->func(args)`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ArrowFunction {
    /// The left-hand side expression.
    pub target: Box<Expression>,
    /// The function being called (may be fully qualified, e.g., `meta::pure::functions::math::max`).
    pub function: PackageableElementPtr,
    /// Arguments (not including the implicit first argument).
    pub arguments: Vec<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A bare reference to a packageable element: `String`, `my::Enum`, `MyClass`.
///
/// This represents a name without an argument list — distinct from
/// `FunctionApplication` which always has parens.
/// Corresponds to Java grammar rule `instanceReference: qualifiedName`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct PackageableElementRef {
    /// The element being referenced.
    pub element: PackageableElementPtr,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Member access (dot) covering simple and qualified variants.
///
/// Both property access and enum value access parse identically
/// (`expr.identifier`); disambiguation is semantic.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberAccess {
    /// Simple member access: `$x.name`, `MyEnum.VALUE`.
    /// No arguments — just `target.member`.
    Simple(SimpleMemberAccess),
    /// Qualified member access: `$x.derived('arg', 42)`.
    /// Has arguments — `target.member(args)`.
    Qualified(QualifiedMemberAccess),
}

impl Spanned for MemberAccess {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Simple(m) => &m.source_info,
            Self::Qualified(m) => &m.source_info,
        }
    }
}

/// Simple member access: `expr.member` (no arguments).
///
/// Covers property access (`$x.name`) and enum value reference (`MyEnum.VALUE`).
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct SimpleMemberAccess {
    /// The target expression (left of the dot).
    pub target: Box<Expression>,
    /// The member name (right of the dot).
    pub member: Identifier,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Qualified member access: `expr.member(args)` (with arguments).
///
/// Covers qualified property calls like `$x.derivedProp('arg')`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct QualifiedMemberAccess {
    /// The target expression (left of the dot).
    pub target: Box<Expression>,
    /// The member name (right of the dot).
    pub member: Identifier,
    /// Arguments.
    pub arguments: Vec<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Type reference expression
// ---------------------------------------------------------------------------

/// A type reference expression: `@MyType`.
///
/// Used as an argument to `cast` and `instanceOf` arrow functions:
/// `$x->cast(@MyType)`, `$x->instanceOf(@MyType)`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct TypeReferenceExpr {
    /// The referenced type.
    pub type_ref: TypeReference,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Complex expressions
// ---------------------------------------------------------------------------

/// A lambda expression: `{params | body}` or `params | body`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct Lambda {
    /// Lambda parameters.
    pub parameters: Vec<Parameter>,
    /// Body expressions.
    pub body: Vec<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A let binding: `let x = expr`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct LetExpr {
    /// Variable name being bound.
    pub name: Identifier,
    /// The expression being assigned.
    pub value: Box<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A collection literal: `[1, 2, 3]`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct CollectionExpr {
    /// Elements of the collection.
    pub elements: Vec<Expression>,
    /// Multiplicity (inferred or explicit).
    pub multiplicity: Option<Multiplicity>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A new instance expression: `^MyClass(prop1='val', prop2=42)`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct NewInstanceExpr {
    /// The class being instantiated (a packageable element reference).
    pub class: PackageableElementPtr,
    /// Property value assignments.
    pub assignments: Vec<KeyValuePair>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A key-value pair in a new instance: `propName = expr`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct KeyValuePair {
    /// The property name.
    pub key: Identifier,
    /// The value expression.
    pub value: Expression,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Column specification
// ---------------------------------------------------------------------------

/// Column expression covering all TDS column syntax variants.
///
/// Pure has several column forms:
/// - `~colName` — simple column reference
/// - `~colName: x | $x.prop` — column with inline lambda
/// - `~[colName: Type]` — typed column
/// - `~colName: func` — column with function reference
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnExpression {
    /// Simple column by name: `~colName`.
    Name(ColumnName),
    /// Column with inline lambda: `~colName: x | $x.prop`.
    WithLambda(ColumnWithLambda),
    /// Typed column: `~[colName: Type]`.
    Typed(ColumnTyped),
    /// Column with function reference: `~colName: funcRef`.
    WithFunction(ColumnWithFunction),
}

impl Spanned for ColumnExpression {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Name(c) => &c.source_info,
            Self::WithLambda(c) => &c.source_info,
            Self::Typed(c) => &c.source_info,
            Self::WithFunction(c) => &c.source_info,
        }
    }
}

/// Simple column reference: `~colName`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ColumnName {
    /// Column name.
    pub name: Identifier,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Column with inline lambda: `~colName: x | $x.prop`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ColumnWithLambda {
    /// Column name.
    pub name: Identifier,
    /// The lambda expression.
    pub lambda: Box<Lambda>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Typed column: `~[colName: Type]`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ColumnTyped {
    /// Column name.
    pub name: Identifier,
    /// Column type.
    pub type_ref: TypeReference,
    /// Source location.
    pub source_info: SourceInfo,
}

/// Column with function reference: `~colName: funcRef`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct ColumnWithFunction {
    /// Column name.
    pub name: Identifier,
    /// Function reference (a packageable element reference).
    pub function: PackageableElementPtr,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Visitor
// ---------------------------------------------------------------------------

/// Visitor pattern for walking expression trees.
///
/// Default implementations are no-ops — override only the variants you care about.
#[allow(unused_variables)]
pub trait ExpressionVisitor {
    /// Visit any expression — dispatches to specific `visit_*` methods.
    fn visit(&mut self, expr: &Expression) {
        match expr {
            Expression::Literal(e) => self.visit_literal(e),
            Expression::Variable(e) => self.visit_variable(e),
            Expression::Arithmetic(e) => self.visit_arithmetic(e),
            Expression::Comparison(e) => self.visit_comparison(e),
            Expression::Logical(e) => self.visit_logical(e),
            Expression::Bitwise(e) => self.visit_bitwise(e),
            Expression::Not(e) => self.visit_not(e),
            Expression::UnaryMinus(e) => self.visit_unary_minus(e),
            Expression::BitwiseNot(e) => self.visit_bitwise_not(e),
            Expression::FunctionApplication(e) => self.visit_function_application(e),
            Expression::ArrowFunction(e) => self.visit_arrow_function(e),
            Expression::MemberAccess(e) => self.visit_member_access(e),
            Expression::TypeReferenceExpr(e) => self.visit_type_reference(e),
            Expression::Lambda(e) => self.visit_lambda(e),
            Expression::Let(e) => self.visit_let(e),
            Expression::Collection(e) => self.visit_collection(e),
            Expression::NewInstance(e) => self.visit_new_instance(e),
            Expression::Column(e) => self.visit_column(e),
            Expression::PackageableElementRef(e) => self.visit_element_ref(e),
            Expression::Group(e) => self.visit(e),
        }
    }

    /// Visit any literal.
    fn visit_literal(&mut self, expr: &Literal) {}
    /// Visit a variable reference.
    fn visit_variable(&mut self, expr: &Variable) {}
    /// Visit an arithmetic expression.
    fn visit_arithmetic(&mut self, expr: &ArithmeticExpr) {}
    /// Visit a comparison expression.
    fn visit_comparison(&mut self, expr: &ComparisonExpr) {}
    /// Visit a logical expression.
    fn visit_logical(&mut self, expr: &LogicalExpr) {}
    /// Visit a bitwise expression.
    fn visit_bitwise(&mut self, expr: &BitwiseExpr) {}
    /// Visit a not expression.
    fn visit_not(&mut self, expr: &NotExpr) {}
    /// Visit a unary minus expression.
    fn visit_unary_minus(&mut self, expr: &UnaryMinusExpr) {}
    /// Visit a bitwise complement expression.
    fn visit_bitwise_not(&mut self, expr: &BitwiseNotExpr) {}
    /// Visit a function application.
    fn visit_function_application(&mut self, expr: &FunctionApplication) {}
    /// Visit an arrow function.
    fn visit_arrow_function(&mut self, expr: &ArrowFunction) {}
    /// Visit a member access (simple or qualified).
    fn visit_member_access(&mut self, expr: &MemberAccess) {}
    /// Visit a type reference expression.
    fn visit_type_reference(&mut self, expr: &TypeReferenceExpr) {}
    /// Visit a lambda.
    fn visit_lambda(&mut self, expr: &Lambda) {}
    /// Visit a let expression.
    fn visit_let(&mut self, expr: &LetExpr) {}
    /// Visit a collection expression.
    fn visit_collection(&mut self, expr: &CollectionExpr) {}
    /// Visit a new instance expression.
    fn visit_new_instance(&mut self, expr: &NewInstanceExpr) {}
    /// Visit a column expression.
    fn visit_column(&mut self, expr: &ColumnExpression) {}
    /// Visit a bare packageable element reference.
    fn visit_element_ref(&mut self, expr: &PackageableElementRef) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_ref::Package;
    use smol_str::SmolStr;

    use crate::test_utils::src;

    fn elem_ptr(name: &str) -> PackageableElementPtr {
        PackageableElementPtr {
            package: None,
            name: SmolStr::new(name),
            source_info: src(),
        }
    }

    #[test]
    fn test_literal_integer() {
        let expr = Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 42,
            source_info: SourceInfo::new("test.pure", 1, 5, 1, 7),
        }));
        assert_eq!(expr.source_info().start_column, 5);
    }

    #[test]
    fn test_literal_enum_matching() {
        let lit = Literal::String(StringLiteral {
            value: "hello".to_string(),
            source_info: src(),
        });
        // Can match "is literal?" without caring about type
        assert!(matches!(lit, Literal::String(_)));
        // Can also match at the Expression level
        let expr = Expression::Literal(lit);
        assert!(matches!(expr, Expression::Literal(_)));
    }

    #[test]
    fn test_arithmetic() {
        let left = Box::new(Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 1,
            source_info: src(),
        })));
        let right = Box::new(Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 2,
            source_info: src(),
        })));
        let expr = Expression::Arithmetic(ArithmeticExpr {
            left,
            op: ArithmeticOp::Plus,
            right,
            source_info: src(),
        });
        assert_eq!(expr.source_info().start_line, 1);
    }

    #[test]
    fn test_bitwise_operations() {
        let left = Box::new(Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 0xFF,
            source_info: src(),
        })));
        let right = Box::new(Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 0x0F,
            source_info: src(),
        })));
        let expr = Expression::Bitwise(BitwiseExpr {
            left,
            op: BitwiseOp::And,
            right,
            source_info: src(),
        });
        if let Expression::Bitwise(b) = &expr {
            assert_eq!(b.op, BitwiseOp::And);
        }
    }

    #[test]
    fn test_simple_member_access() {
        // $x.name
        let expr = Expression::MemberAccess(MemberAccess::Simple(SimpleMemberAccess {
            target: Box::new(Expression::Variable(Variable {
                name: SmolStr::new("x"),
                source_info: src(),
            })),
            member: SmolStr::new("name"),
            source_info: src(),
        }));
        if let Expression::MemberAccess(MemberAccess::Simple(ma)) = &expr {
            assert_eq!(ma.member, "name");
        }
    }

    #[test]
    fn test_qualified_member_access() {
        // $x.derivedProp('arg')
        let expr = Expression::MemberAccess(MemberAccess::Qualified(QualifiedMemberAccess {
            target: Box::new(Expression::Variable(Variable {
                name: SmolStr::new("x"),
                source_info: src(),
            })),
            member: SmolStr::new("derivedProp"),
            arguments: vec![Expression::Literal(Literal::String(StringLiteral {
                value: "arg".to_string(),
                source_info: src(),
            }))],
            source_info: src(),
        }));
        if let Expression::MemberAccess(MemberAccess::Qualified(ma)) = &expr {
            assert_eq!(ma.member, "derivedProp");
            assert_eq!(ma.arguments.len(), 1);
        }
    }

    #[test]
    fn test_function_application_uses_element_ptr() {
        let expr = Expression::FunctionApplication(FunctionApplication {
            function: PackageableElementPtr {
                package: Some(Package::root(SmolStr::new("pkg"), src())),
                name: SmolStr::new("myFunc"),
                source_info: src(),
            },
            arguments: vec![],
            source_info: src(),
        });
        if let Expression::FunctionApplication(fa) = &expr {
            assert_eq!(fa.function.name, "myFunc");
            assert_eq!(fa.function.package.as_ref().unwrap().name(), "pkg");
        }
    }

    #[test]
    fn test_new_instance_uses_element_ptr() {
        let expr = Expression::NewInstance(NewInstanceExpr {
            class: elem_ptr("MyClass"),
            assignments: vec![],
            source_info: src(),
        });
        if let Expression::NewInstance(ni) = &expr {
            assert_eq!(ni.class.name, "MyClass");
        }
    }

    #[test]
    fn test_type_reference_expr() {
        let expr = Expression::TypeReferenceExpr(TypeReferenceExpr {
            type_ref: TypeReference {
                package: None,
                name: SmolStr::new("MyType"),
                type_arguments: vec![],
                type_variable_values: vec![],
                source_info: src(),
            },
            source_info: src(),
        });
        if let Expression::TypeReferenceExpr(tr) = &expr {
            assert_eq!(tr.type_ref.full_path(), "MyType");
        }
    }

    #[test]
    fn test_lambda() {
        let lambda = Expression::Lambda(Lambda {
            parameters: vec![Parameter {
                name: SmolStr::new("x"),
                type_ref: Some(TypeReference {
                    package: None,
                    name: SmolStr::new("String"),
                    type_arguments: vec![],
                    type_variable_values: vec![],
                    source_info: src(),
                }),
                multiplicity: Some(Multiplicity::one()),
                source_info: src(),
            }],
            body: vec![Expression::Variable(Variable {
                name: SmolStr::new("x"),
                source_info: src(),
            })],
            source_info: src(),
        });
        if let Expression::Lambda(l) = &lambda {
            assert_eq!(l.parameters.len(), 1);
        }
    }

    #[test]
    fn test_column_expression_variants() {
        // ~colName
        let col = Expression::Column(ColumnExpression::Name(ColumnName {
            name: SmolStr::new("firstName"),
            source_info: src(),
        }));
        assert!(matches!(col, Expression::Column(ColumnExpression::Name(_))));

        // ~[colName: Type]
        let col_typed = Expression::Column(ColumnExpression::Typed(ColumnTyped {
            name: SmolStr::new("age"),
            type_ref: TypeReference {
                package: None,
                name: SmolStr::new("Integer"),
                type_arguments: vec![],
                type_variable_values: vec![],
                source_info: src(),
            },
            source_info: src(),
        }));
        assert!(matches!(
            col_typed,
            Expression::Column(ColumnExpression::Typed(_))
        ));
    }

    #[test]
    fn test_visitor_with_literal() {
        struct LitCounter {
            count: u32,
        }

        impl ExpressionVisitor for LitCounter {
            fn visit_literal(&mut self, _: &Literal) {
                self.count += 1;
            }
        }

        let expr = Expression::Literal(Literal::Integer(IntegerLiteral {
            value: 42,
            source_info: src(),
        }));

        let mut counter = LitCounter { count: 0 };
        counter.visit(&expr);
        assert_eq!(counter.count, 1);
    }
}
