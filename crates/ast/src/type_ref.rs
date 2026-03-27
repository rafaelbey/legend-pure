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

//! Type references, identifiers, multiplicities, and package paths.
//!
//! This module defines the core type system primitives shared across all AST nodes.

use smol_str::SmolStr;

use crate::source_info::{SourceInfo, Spanned};

// ---------------------------------------------------------------------------
// Identifier
// ---------------------------------------------------------------------------

/// Interned identifier — cheap to clone, compare, and hash.
///
/// Most Pure identifiers (class names, property names, keywords) fit within
/// `SmolStr`'s 24-byte inline buffer, avoiding heap allocation entirely.
pub type Identifier = SmolStr;

// ---------------------------------------------------------------------------
// Package
// ---------------------------------------------------------------------------

/// A package in the Package hierarchy, with an optional parent.
///
/// This models the recursive nature of packages: `meta::pure::profiles` is
/// represented as `Package("profiles", parent=Package("pure", parent=Package("meta")))`.
///
/// # Design Rationale
///
/// Unlike a flat `Vec<Identifier>`, this recursive structure:
/// - Naturally models parent-child relationships
/// - Makes parent traversal trivial (`package.parent()`)
/// - Enables sharing common parent prefixes via `Arc` (future optimization)
///
/// # Examples
///
/// ```
/// use legend_pure_parser_ast::type_ref::Package;
/// use legend_pure_parser_ast::SourceInfo;
/// use smol_str::SmolStr;
///
/// let src = SourceInfo::new("test.pure", 1, 1, 1, 20);
/// let meta = Package::new(SmolStr::new("meta"), None, src.clone());
/// let pure = Package::new(SmolStr::new("pure"), Some(Box::new(meta)), src.clone());
/// let profiles = Package::new(SmolStr::new("profiles"), Some(Box::new(pure)), src);
///
/// assert_eq!(profiles.to_string(), "meta::pure::profiles");
/// assert_eq!(profiles.parent().unwrap().name(), "pure");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The name of this package segment.
    name: Identifier,
    /// The parent package, or `None` for root-level packages.
    parent: Option<Box<Package>>,
    /// Source location of this package reference.
    source_info: SourceInfo,
}

impl Package {
    /// Creates a new package with an optional parent.
    #[must_use]
    pub fn new(name: Identifier, parent: Option<Box<Package>>, source_info: SourceInfo) -> Self {
        Self {
            name,
            parent,
            source_info,
        }
    }

    /// Creates a root package (no parent).
    #[must_use]
    pub fn root(name: Identifier, source_info: SourceInfo) -> Self {
        Self {
            name,
            parent: None,
            source_info,
        }
    }

    /// Creates a child package under `self`.
    #[must_use]
    pub fn child(self, name: Identifier, source_info: SourceInfo) -> Self {
        Self {
            name,
            parent: Some(Box::new(self)),
            source_info,
        }
    }

    /// Returns the name of this package segment.
    #[must_use]
    pub fn name(&self) -> &Identifier {
        &self.name
    }

    /// Returns the parent package, if any.
    #[must_use]
    pub fn parent(&self) -> Option<&Package> {
        self.parent.as_deref()
    }

    /// Returns the depth of this package (0 for root).
    #[must_use]
    pub fn depth(&self) -> usize {
        match &self.parent {
            None => 0,
            Some(p) => 1 + p.depth(),
        }
    }

    /// Collects all segments from root to self as a vector of identifiers.
    #[must_use]
    pub fn segments(&self) -> Vec<&Identifier> {
        let mut parts = Vec::with_capacity(self.depth() + 1);
        self.collect_segments(&mut parts);
        parts
    }

    fn collect_segments<'a>(&'a self, parts: &mut Vec<&'a Identifier>) {
        if let Some(parent) = &self.parent {
            parent.collect_segments(parts);
        }
        parts.push(&self.name);
    }
}

impl std::fmt::Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(parent) = &self.parent {
            write!(f, "{parent}::")?;
        }
        write!(f, "{}", self.name)
    }
}

impl Spanned for Package {
    fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }
}

// ---------------------------------------------------------------------------
// Multiplicity
// ---------------------------------------------------------------------------

/// Multiplicity specification for properties and parameters.
///
/// Uses an enum with common well-known multiplicities for ergonomics and pattern
/// matching, plus a `Range` variant for arbitrary bounds.
///
/// The internal representation is opaque — construction is via factory functions,
/// and access to bounds is via the [`HasMultiplicity`] trait.
///
/// # Examples
///
/// ```
/// use legend_pure_parser_ast::type_ref::{Multiplicity, HasMultiplicity};
///
/// let one = Multiplicity::one();
/// assert_eq!(one.lower(), 0);
/// assert_eq!(one.upper(), Some(1));
///
/// // Wait - [1] means exactly one, not zero-to-one
/// let exactly_one = Multiplicity::pure_one();
/// assert_eq!(exactly_one.lower(), 1);
/// assert_eq!(exactly_one.upper(), Some(1));
///
/// let many = Multiplicity::zero_or_many();
/// assert_eq!(many.lower(), 0);
/// assert!(many.upper().is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Multiplicity {
    /// Exactly one: `[1]` — `lower=1, upper=1`.
    PureOne,
    /// Zero or one: `[0..1]` — `lower=0, upper=1`.
    ZeroOrOne,
    /// Zero or more: `[*]` — `lower=0, upper=None`.
    ZeroOrMany,
    /// One or more: `[1..*]` — `lower=1, upper=None`.
    OneOrMany,
    /// Arbitrary range: `[lower..upper]`.
    Range {
        /// Lower bound (inclusive).
        lower: u32,
        /// Upper bound (inclusive), `None` = unbounded.
        upper: Option<u32>,
    },
}

/// Trait for accessing multiplicity bounds.
///
/// This abstracts over the internal representation of [`Multiplicity`],
/// allowing pattern-matching on common cases while still supporting arbitrary ranges.
pub trait HasMultiplicity {
    /// Returns the lower bound (inclusive).
    fn lower(&self) -> u32;
    /// Returns the upper bound (inclusive), or `None` for unbounded (`*`).
    fn upper(&self) -> Option<u32>;
    /// Returns `true` if this multiplicity allows zero elements.
    fn is_optional(&self) -> bool {
        self.lower() == 0
    }
    /// Returns `true` if this multiplicity is unbounded.
    fn is_many(&self) -> bool {
        self.upper().is_none()
    }
    /// Returns `true` if this is exactly `[1]`.
    fn is_required_single(&self) -> bool {
        self.lower() == 1 && self.upper() == Some(1)
    }
}

impl Multiplicity {
    /// Creates `[1]` — exactly one.
    #[must_use]
    pub fn pure_one() -> Self {
        Self::PureOne
    }

    /// Creates `[0..1]` — zero or one (optional).
    #[must_use]
    pub fn one() -> Self {
        Self::ZeroOrOne
    }

    /// Creates `[*]` — zero or more.
    #[must_use]
    pub fn zero_or_many() -> Self {
        Self::ZeroOrMany
    }

    /// Creates `[1..*]` — one or more.
    #[must_use]
    pub fn one_or_many() -> Self {
        Self::OneOrMany
    }

    /// Creates an arbitrary multiplicity `[lower..upper]`.
    ///
    /// Returns a well-known variant if the range matches one.
    #[must_use]
    pub fn range(lower: u32, upper: Option<u32>) -> Self {
        match (lower, upper) {
            (1, Some(1)) => Self::PureOne,
            (0, Some(1)) => Self::ZeroOrOne,
            (0, None) => Self::ZeroOrMany,
            (1, None) => Self::OneOrMany,
            _ => Self::Range { lower, upper },
        }
    }
}

impl HasMultiplicity for Multiplicity {
    fn lower(&self) -> u32 {
        match self {
            Self::PureOne | Self::OneOrMany => 1,
            Self::ZeroOrOne | Self::ZeroOrMany => 0,
            Self::Range { lower, .. } => *lower,
        }
    }

    fn upper(&self) -> Option<u32> {
        match self {
            Self::PureOne | Self::ZeroOrOne => Some(1),
            Self::ZeroOrMany | Self::OneOrMany => None,
            Self::Range { upper, .. } => *upper,
        }
    }
}

impl std::fmt::Display for Multiplicity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PureOne => write!(f, "[1]"),
            Self::ZeroOrOne => write!(f, "[0..1]"),
            Self::ZeroOrMany => write!(f, "[*]"),
            Self::OneOrMany => write!(f, "[1..*]"),
            Self::Range {
                lower,
                upper: Some(u),
            } => {
                if lower == u {
                    write!(f, "[{lower}]")
                } else {
                    write!(f, "[{lower}..{u}]")
                }
            }
            Self::Range {
                lower,
                upper: None,
            } => write!(f, "[{lower}..*]"),
        }
    }
}

// ---------------------------------------------------------------------------
// TypeReference
// ---------------------------------------------------------------------------

/// A reference to a type, including optional type arguments and type variable values.
///
/// Every `TypeReference` is [`Spanned`] to enable precise error reporting.
///
/// # Examples
///
/// - `String` → path only
/// - `Result<String>` → path + type arguments
/// - `VARCHAR(200)` → path + type variable values
/// - `Res<String>(1, 'a')` → path + type arguments + type variable values
/// - `Relation<(a:Integer, b:String)>` → path + type arguments (column specs)
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct TypeReference {
    /// The type path, e.g., `Result`, `meta::pure::String`.
    pub path: Package,
    /// Generic type arguments: `<String, Integer>`.
    pub type_arguments: Vec<TypeReference>,
    /// Type variable values: `(200, 'ok')`.
    pub type_variable_values: Vec<TypeVariableValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A value in a type variable position, e.g., `VARCHAR(200)` or `Res(1, 'ok')`.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeVariableValue {
    /// Integer value, e.g., `200` in `VARCHAR(200)`.
    Integer(i64, SourceInfo),
    /// String value, e.g., `'ok'` in `Res('ok')`.
    String(String, SourceInfo),
}

impl Spanned for TypeVariableValue {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Integer(_, s) | Self::String(_, s) => s,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_src() -> SourceInfo {
        SourceInfo::new("test.pure", 1, 1, 1, 10)
    }

    // -- Package tests --

    #[test]
    fn test_root_package() {
        let pkg = Package::root(SmolStr::new("meta"), test_src());
        assert_eq!(pkg.to_string(), "meta");
        assert!(pkg.parent().is_none());
        assert_eq!(pkg.depth(), 0);
    }

    #[test]
    fn test_nested_package() {
        let pkg = Package::root(SmolStr::new("meta"), test_src())
            .child(SmolStr::new("pure"), test_src())
            .child(SmolStr::new("profiles"), test_src());

        assert_eq!(pkg.to_string(), "meta::pure::profiles");
        assert_eq!(pkg.depth(), 2);
        assert_eq!(pkg.parent().unwrap().name(), "pure");
        assert_eq!(pkg.parent().unwrap().parent().unwrap().name(), "meta");
    }

    #[test]
    fn test_package_segments() {
        let pkg = Package::root(SmolStr::new("model"), test_src())
            .child(SmolStr::new("domain"), test_src())
            .child(SmolStr::new("Person"), test_src());

        let segments: Vec<&str> = pkg.segments().iter().map(|s| s.as_str()).collect();
        assert_eq!(segments, vec!["model", "domain", "Person"]);
    }

    #[test]
    fn test_package_is_spanned() {
        let pkg = Package::root(SmolStr::new("meta"), SourceInfo::new("file.pure", 3, 5, 3, 9));
        assert_eq!(pkg.source_info().start_line, 3);
        assert_eq!(pkg.source_info().start_column, 5);
    }

    // -- Multiplicity tests --

    #[test]
    fn test_multiplicity_pure_one() {
        let m = Multiplicity::pure_one();
        assert_eq!(m.lower(), 1);
        assert_eq!(m.upper(), Some(1));
        assert!(m.is_required_single());
        assert!(!m.is_optional());
        assert!(!m.is_many());
        assert_eq!(m.to_string(), "[1]");
    }

    #[test]
    fn test_multiplicity_zero_or_one() {
        let m = Multiplicity::one();
        assert_eq!(m.lower(), 0);
        assert_eq!(m.upper(), Some(1));
        assert!(m.is_optional());
        assert!(!m.is_many());
        assert_eq!(m.to_string(), "[0..1]");
    }

    #[test]
    fn test_multiplicity_zero_or_many() {
        let m = Multiplicity::zero_or_many();
        assert_eq!(m.lower(), 0);
        assert!(m.upper().is_none());
        assert!(m.is_optional());
        assert!(m.is_many());
        assert_eq!(m.to_string(), "[*]");
    }

    #[test]
    fn test_multiplicity_one_or_many() {
        let m = Multiplicity::one_or_many();
        assert_eq!(m.lower(), 1);
        assert!(m.upper().is_none());
        assert!(!m.is_optional());
        assert!(m.is_many());
        assert_eq!(m.to_string(), "[1..*]");
    }

    #[test]
    fn test_multiplicity_range_normalizes() {
        // range(1, Some(1)) should normalize to PureOne
        let m = Multiplicity::range(1, Some(1));
        assert!(matches!(m, Multiplicity::PureOne));

        let m = Multiplicity::range(0, None);
        assert!(matches!(m, Multiplicity::ZeroOrMany));
    }

    #[test]
    fn test_multiplicity_custom_range() {
        let m = Multiplicity::range(2, Some(5));
        assert_eq!(m.lower(), 2);
        assert_eq!(m.upper(), Some(5));
        assert!(!m.is_optional());
        assert!(!m.is_many());
        assert_eq!(m.to_string(), "[2..5]");
    }

    #[test]
    fn test_multiplicity_same_bound_display() {
        let m = Multiplicity::range(3, Some(3));
        assert_eq!(m.to_string(), "[3]");
    }

    // -- TypeReference tests --

    #[test]
    fn test_type_reference_simple() {
        let type_ref = TypeReference {
            path: Package::root(SmolStr::new("String"), test_src()),
            type_arguments: vec![],
            type_variable_values: vec![],
            source_info: test_src(),
        };
        assert_eq!(type_ref.path.to_string(), "String");
        assert!(type_ref.type_arguments.is_empty());
    }

    #[test]
    fn test_type_reference_is_spanned() {
        let type_ref = TypeReference {
            path: Package::root(SmolStr::new("String"), test_src()),
            type_arguments: vec![],
            type_variable_values: vec![],
            source_info: SourceInfo::new("file.pure", 5, 3, 5, 9),
        };
        assert_eq!(type_ref.source_info().start_line, 5);
    }

    #[test]
    fn test_type_variable_value_is_spanned() {
        let val = TypeVariableValue::Integer(200, SourceInfo::new("f.pure", 1, 10, 1, 13));
        assert_eq!(val.source_info().start_column, 10);

        let val = TypeVariableValue::String("ok".to_string(), SourceInfo::new("f.pure", 2, 5, 2, 9));
        assert_eq!(val.source_info().start_line, 2);
    }
}
