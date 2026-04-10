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

//! Source location information for AST nodes.
//!
//! Every AST node carries a [`SourceInfo`] to pinpoint its location in the
//! original source text. This is critical for error messages, IDE integration,
//! and debugging parsing/compilation problems.

use smol_str::SmolStr;

/// Tracks the source location of an AST node within the original source text.
///
/// Line and column numbers are 1-indexed to match conventional editor/IDE behavior
/// and the existing Java `SourceInformation` format.
///
/// # Examples
///
/// ```
/// use legend_pure_parser_ast::SourceInfo;
/// use smol_str::SmolStr;
///
/// let info = SourceInfo {
///     source: SmolStr::new("test.pure"),
///     start_line: 1,
///     start_column: 1,
///     end_line: 1,
///     end_column: 10,
/// };
/// assert_eq!(info.start_line, 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    /// Source identifier (file path or URI).
    pub source: SmolStr,
    /// Starting line (1-indexed).
    pub start_line: u32,
    /// Starting column (1-indexed).
    pub start_column: u32,
    /// Ending line (1-indexed).
    pub end_line: u32,
    /// Ending column (1-indexed).
    pub end_column: u32,
}

impl SourceInfo {
    /// Creates a new `SourceInfo` from a source identifier and position.
    #[must_use]
    pub fn new(
        source: impl Into<SmolStr>,
        start_line: u32,
        start_column: u32,
        end_line: u32,
        end_column: u32,
    ) -> Self {
        Self {
            source: source.into(),
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    /// Merges two source spans into one that covers both.
    ///
    /// Useful when building a parent node whose span should cover all children.
    #[must_use]
    pub fn merge(&self, other: &Self) -> Self {
        let (start_line, start_column) = if self.start_line < other.start_line
            || (self.start_line == other.start_line && self.start_column <= other.start_column)
        {
            (self.start_line, self.start_column)
        } else {
            (other.start_line, other.start_column)
        };

        let (end_line, end_column) = if self.end_line > other.end_line
            || (self.end_line == other.end_line && self.end_column >= other.end_column)
        {
            (self.end_line, self.end_column)
        } else {
            (other.end_line, other.end_column)
        };

        Self {
            source: self.source.clone(),
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }
}

impl std::fmt::Display for SourceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}-{}:{}",
            self.source, self.start_line, self.start_column, self.end_line, self.end_column
        )
    }
}

/// Trait for all AST nodes that carry source location information.
///
/// **Every AST node must implement `Spanned`.** This is a core design principle
/// — no node exists without a known source position. This eliminates the
/// troubleshooting gaps present in the Java parser where some nodes lack
/// `sourceInformation`.
pub trait Spanned {
    /// Returns the source location of this node.
    fn source_info(&self) -> &SourceInfo;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_info_display() {
        let info = SourceInfo::new("test.pure", 1, 5, 3, 10);
        assert_eq!(info.to_string(), "test.pure:1:5-3:10");
    }

    #[test]
    fn test_source_info_merge() {
        let a = SourceInfo::new("test.pure", 1, 5, 2, 10);
        let b = SourceInfo::new("test.pure", 2, 3, 4, 15);
        let merged = a.merge(&b);

        assert_eq!(merged.start_line, 1);
        assert_eq!(merged.start_column, 5);
        assert_eq!(merged.end_line, 4);
        assert_eq!(merged.end_column, 15);
    }

    #[test]
    fn test_source_info_merge_same_line() {
        let a = SourceInfo::new("test.pure", 1, 10, 1, 20);
        let b = SourceInfo::new("test.pure", 1, 1, 1, 5);
        let merged = a.merge(&b);

        assert_eq!(merged.start_line, 1);
        assert_eq!(merged.start_column, 1);
        assert_eq!(merged.end_line, 1);
        assert_eq!(merged.end_column, 20);
    }

    #[derive(crate::Spanned)]
    struct TestNode {
        source_info: SourceInfo,
    }

    #[test]
    fn test_impl_spanned_macro() {
        let node = TestNode {
            source_info: SourceInfo::new("test.pure", 1, 1, 1, 10),
        };
        assert_eq!(node.source_info().start_line, 1);
        assert_eq!(node.source_info().end_column, 10);
    }
}
