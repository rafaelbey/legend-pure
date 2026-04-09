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

//! # Legend Pure Parser — Pure Semantic Layer
//!
//! This crate implements the **Pure Semantic Layer** — the compiled `PureModel`
//! graph for the Legend Pure compiler. It is the Rust equivalent of Java's
//! `PureModel` and `ExecutionSupport`.
//!
//! ## Architecture
//!
//! ```text
//! Source → Parser → AST → Pure (this crate) → Execution / Validation
//!                          ↑
//!                    Arena/Index graph
//! ```
//!
//! The crate consumes `ast::SourceFile` and produces a [`PureModel`](model::PureModel)
//! — a fully resolved, navigable semantic graph.
//!
//! ## Key Modules
//!
//! - [`ids`] — Typed index wrappers (`ElementId`, `PackageId`, `RelationId`)
//! - [`arena`] — Generic `Arena<T>` container
//! - [`model`] — `PureModel`, `ModelChunk`, `ElementNode`, `Element`, `Package`
//!
//! ## Design Decisions
//!
//! See `DESIGN.md` in this crate's root for the full set of architectural
//! decisions (Arena/Index pattern, Element vs Type distinction, derived indexes,
//! `SourceInfo` retention, etc.).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod annotations;
pub mod arena;
pub mod bootstrap;
pub mod error;
pub mod ids;
pub(crate) mod lower;
pub mod model;
pub mod nodes;
pub mod pipeline;
pub(crate) mod resolve;
pub mod types;
pub(crate) mod validate;
