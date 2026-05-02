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

//! # Legend Pure — Graph-fetch DSL
//!
//! Self-contained crate hosting the graph-fetch inline-island
//! grammar (`#{ class { field, field { sub-field } } }#`). Tag: `""`.
//!
//! Mirrors the Diagram-DSL crate layout:
//!
//! - [`ast`] — `RootGraphFetchTree`, `PropertyGraphFetchTree`,
//!   `SubTypeGraphFetchTree` — implements
//!   [`legend_pure_parser_ast::island::IslandContent`].
//! - [`parser`] — registers `GraphFetchIslandParser` as an
//!   [`legend_pure_parser_parser::IslandParser`] with tag `""`.
//! - [`compose`] — `IslandComposer` with tag `""` (round-trip back to
//!   Pure source).
//! - [`protocol`] — `IslandProtocol` with tag `""` (AST ↔ Legend
//!   Protocol JSON).
//! - [`compiler`] — `GraphFetchExtension` (`CompilerExtension`)
//!   resolves root classes, validates property names, and recurses
//!   into sub-trees / sub-type trees during the compile pipeline's
//!   `define_bodies` pass.
//!
//! Core crates carry no graph-fetch knowledge — this crate is the
//! sole owner.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
pub mod compiler;
pub mod compose;
pub mod parser;
pub mod protocol;
