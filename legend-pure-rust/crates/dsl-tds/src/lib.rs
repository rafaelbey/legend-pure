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

//! # Legend Pure — TDS DSL
//!
//! Self-contained crate hosting the Tabular Data Set inline-island
//! grammar:
//!
//! ```pure
//! #TDS
//!   col1, col2:Type, 'col 3':Type[1]
//!   1, 2, foo
//!   3, 4, bar
//! #
//! ```
//!
//! Tag: `"TDS"`. Body shape is **raw-content** (no `{ … }`): the
//! parser scans tokens after the tag until it sees the closing
//! `Hash` token. Lines are detected by comparing token start-lines
//! (newlines are eaten as whitespace by the lexer).
//!
//! Mirrors Java's
//! `org.finos.legend.pure.m2.inlinedsl.tds.TDSExtension` and the
//! `meta::pure::metamodel::relation::TDS<T>` metaclass:
//!
//! ```pure
//! Class meta::pure::metamodel::relation::TDS<T> { csv: String[1]; }
//! ```
//!
//! The Java implementation uses Deephaven CSV parsing for the data
//! rows; Rust port keeps the rows as raw strings until the
//! [`compiler`] extension lands the typed lowering (commit #14).
//!
//! Crate layout:
//!
//! - [`ast`] — [`ast::TDSExpr`], [`ast::TDSColumn`], [`ast::TDSCell`]
//!   implementing [`legend_pure_parser_ast::island::IslandContent`].
//! - [`parser`] — [`parser::TDSIslandParser`] registers as
//!   [`legend_pure_parser_parser::IslandParser`] with tag `"TDS"`.
//! - [`compose`] — [`compose::TDSIslandComposer`] registers as
//!   [`legend_pure_parser_compose::island::IslandComposer`] with tag
//!   `"TDS"`.
//! - [`protocol`] — [`protocol::TDSIslandProtocol`] registers as
//!   [`legend_pure_parser_protocol::IslandProtocol`] with tag `"TDS"`.
//! - **`CompilerExtension`** (commit #14) — validates types referenced
//!   in column specs against `meta::pure::metamodel::type::Type`
//!   subclasses and wires the embedded metamodel.
//!
//! Core crates carry no TDS knowledge — this crate is the sole owner.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
pub mod compose;
pub mod parser;
pub mod protocol;
