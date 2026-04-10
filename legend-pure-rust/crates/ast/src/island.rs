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

//! Island grammar expressions — `#tag{ content }#`.
//!
//! Island grammars are embedded mini-languages within Pure expressions.
//! Each island is identified by a tag between `#` and `{`:
//!
//! | Syntax      | Tag   | Content Type |
//! |-------------|-------|-------------|
//! | `#{ }#`     | `""`  | Graph fetch tree |
//! | `#>{ }#`    | `">"` | Path expression (future) |
//! | `#sql{ }#`  | `"sql"`| Embedded SQL (future) |
//!
//! ## Plugin Architecture
//!
//! The island system is trait-based to support extensibility:
//!
//! - **[`IslandContent`]** (this module): trait for AST storage. Each island
//!   grammar implements this to provide its content type. The AST stores
//!   `Box<dyn IslandContent>` so new islands can be added from external crates.
//! - **`IslandParser`** (parser crate): trait for parsing `#tag{ content }#`.
//! - **`IslandComposer`** (compose crate): trait for rendering content back to text.
//!
//! Graph fetch is the first built-in island grammar. Future islands (path,
//! SQL, etc.) can implement the same traits from separate crates.

use std::any::Any;
use std::fmt;

use crate::annotation::PackageableElementPtr;
use crate::expression::Expression;
use crate::source_info::SourceInfo;
use crate::type_ref::Identifier;
use smol_str::SmolStr;

// ---------------------------------------------------------------------------
// IslandContent Trait
// ---------------------------------------------------------------------------

/// Trait for island grammar content stored in the AST.
///
/// Every island grammar (graph fetch, path, SQL, etc.) implements this
/// trait to provide its content type. The AST stores `Box<dyn IslandContent>`
/// so new islands can be added from external crates without modifying the
/// AST `Expression` enum.
///
/// # Implementing a New Island Grammar
///
/// ```rust,ignore
/// #[derive(Debug, Clone, PartialEq)]
/// pub struct MyIslandContent { /* ... */ }
///
/// impl IslandContent for MyIslandContent {
///     fn tag(&self) -> &str { "myTag" }
///     fn as_any(&self) -> &dyn Any { self }
///     fn clone_box(&self) -> Box<dyn IslandContent> { Box::new(self.clone()) }
///     fn eq_content(&self, other: &dyn IslandContent) -> bool {
///         other.as_any().downcast_ref::<Self>().map_or(false, |o| self == o)
///     }
/// }
/// ```
pub trait IslandContent: fmt::Debug + Send + Sync {
    /// The island tag: `""` for `#{}#`, `">"` for `#>{}#`, etc.
    fn tag(&self) -> &str;

    /// Downcast to the concrete type for type-specific handling.
    fn as_any(&self) -> &dyn Any;

    /// Clone into a new boxed trait object.
    fn clone_box(&self) -> Box<dyn IslandContent>;

    /// Equality comparison with another island content.
    ///
    /// Implementations should downcast `other` to `Self` and compare.
    fn eq_content(&self, other: &dyn IslandContent) -> bool;
}

// ---------------------------------------------------------------------------
// IslandExpression — AST envelope
// ---------------------------------------------------------------------------

/// An island grammar expression: `#tag{ content }#`.
///
/// The `content` field holds a type-erased `Box<dyn IslandContent>`.
/// Consumers access the concrete type via:
///
/// ```rust,ignore
/// if let Some(tree) = island.content.as_any().downcast_ref::<RootGraphFetchTree>() {
///     // handle graph fetch
/// }
/// ```
pub struct IslandExpression {
    /// The parsed content of the island grammar (type-erased).
    pub content: Box<dyn IslandContent>,
    /// Source location of the entire `#tag{ ... }#` expression.
    pub source_info: SourceInfo,
}

impl IslandExpression {
    /// Returns the island tag (delegated to the content trait).
    #[must_use]
    pub fn tag(&self) -> &str {
        self.content.tag()
    }
}

impl Clone for IslandExpression {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone_box(),
            source_info: self.source_info.clone(),
        }
    }
}

impl PartialEq for IslandExpression {
    fn eq(&self, other: &Self) -> bool {
        self.content.eq_content(other.content.as_ref())
    }
}

impl fmt::Debug for IslandExpression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IslandExpression")
            .field("tag", &self.content.tag())
            .field("content", &self.content)
            .field("source_info", &self.source_info)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Graph Fetch Tree — built-in island grammar
// ---------------------------------------------------------------------------

/// Root of a graph fetch tree: `#{Type{field1, field2}}#`.
///
/// Corresponds to Java's `RootGraphFetchTree` protocol type.
///
/// # Example
///
/// ```text
/// #{
///     my::Person {
///         firstName,
///         lastName,
///         address {
///             city
///         }
///     }
/// }#
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RootGraphFetchTree {
    /// The root class reference (e.g., `my::Person`).
    pub class: PackageableElementPtr,
    /// Property sub-trees: regular field selections.
    pub sub_trees: Vec<PropertyGraphFetchTree>,
    /// Subtype sub-trees: `->subType(@Type){...}` casts.
    pub sub_type_trees: Vec<SubTypeGraphFetchTree>,
    /// Source location.
    pub source_info: SourceInfo,
}

impl IslandContent for RootGraphFetchTree {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        ""
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn IslandContent> {
        Box::new(self.clone())
    }

    fn eq_content(&self, other: &dyn IslandContent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

/// A property field within a graph fetch tree.
///
/// Represents: `property`, `property(args)`, `'alias':property`, or
/// `property{subFields}`, and combinations thereof.
///
/// Corresponds to Java's `PropertyGraphFetchTree` protocol type.
///
/// # Examples
///
/// ```text
/// firstName                          // simple
/// 'aliasName' : firstName            // with alias
/// employeesByName(['Peter'])         // with parameters (qualified property)
/// address { city, street }           // with sub-tree
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyGraphFetchTree {
    /// The property name.
    pub property: Identifier,
    /// Qualified property parameters (e.g., `['Peter']`).
    pub parameters: Vec<Expression>,
    /// Optional alias: `'aliasName' : property`.
    pub alias: Option<SmolStr>,
    /// Optional subtype cast on the property: `->subType(@Type)`.
    pub sub_type: Option<PackageableElementPtr>,
    /// Property sub-trees (nested field selections).
    pub sub_trees: Vec<PropertyGraphFetchTree>,
    /// Subtype sub-trees within this property.
    pub sub_type_trees: Vec<SubTypeGraphFetchTree>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A subtype cast within a graph fetch tree: `->subType(@Type){fields}`.
///
/// Corresponds to Java's `SubTypeGraphFetchTree` protocol type.
///
/// # Example
///
/// ```text
/// ->subType(@my::FirmSubType) {
///     SubTypeName
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SubTypeGraphFetchTree {
    /// The subtype class reference.
    pub sub_type_class: PackageableElementPtr,
    /// Property sub-trees within the subtype.
    pub sub_trees: Vec<PropertyGraphFetchTree>,
    /// Nested subtype sub-trees.
    pub sub_type_trees: Vec<SubTypeGraphFetchTree>,
    /// Source location.
    pub source_info: SourceInfo,
}
