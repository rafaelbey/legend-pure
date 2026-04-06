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

//! Compilation error types for the Pure compiler pipeline.

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

/// A compilation error produced during AST → Pure lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct CompilationError {
    /// Human-readable error message.
    pub message: String,
    /// Source location where the error occurred.
    pub source_info: SourceInfo,
    /// Error kind for programmatic handling.
    pub kind: CompilationErrorKind,
}

/// Classification of compilation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilationErrorKind {
    /// An element path could not be resolved.
    UnresolvedElement {
        /// The unresolved path as written.
        path: SmolStr,
    },
    /// A duplicate element name in the same package.
    DuplicateElement {
        /// The duplicate name.
        name: SmolStr,
    },
    /// A cyclic inheritance chain was detected.
    CyclicInheritance {
        /// The element that starts the cycle.
        element_name: SmolStr,
    },
    /// A non-type element was used in a type position.
    NotAType {
        /// The element name.
        name: SmolStr,
    },
    /// Generic catch-all for other errors.
    Other,
}

impl std::fmt::Display for CompilationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source_info, self.message)
    }
}

impl std::error::Error for CompilationError {}
