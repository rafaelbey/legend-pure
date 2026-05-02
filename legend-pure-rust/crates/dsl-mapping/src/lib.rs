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

//! # Legend Pure — Mapping DSL (Stage 2)
//!
//! Stage 2 ships parser + AST + composer + a `declare`-only compiler
//! extension that registers parsed `MappingDef`s under their FQN.
//! Users can now write `###Mapping` blocks and have them parse,
//! round-trip, and resolve through `MappingExtension::mappings()` —
//! but **filter and transform lambdas are not lowered or validated
//! yet**. Stage 3 adds the `PureInstanceSetImplementation` processor
//! + validator + `MappingValidator` (DAG check).
//!
//! Module map:
//!
//! - [`ast`] — `MappingDef`, `MappingInclude`, `ClassMapping`,
//!   `ClassMappingBody` enum (Pure variant only), `PureClassMappingBody`,
//!   `PurePropertyMapping`. `MappingDef` implements
//!   [`legend_pure_parser_ast::dsl::DSLElement`] so it rides on the
//!   core `Element::DSLElement` variant.
//! - [`parser`] — `MappingSectionParser` plugs into
//!   [`legend_pure_parser_parser::SectionParser`] and consumes
//!   `###Mapping` section bodies. Sub-grammar dispatch by `parserName`
//!   token; only `Pure` is supported in Stage 2 (others raise
//!   `UnsupportedSubParser`).
//! - [`compose`] — `compose_mapping` round-trips a `MappingDef` back to
//!   canonical Pure source.
//! - [`compiler`] — `MappingExtension` registers `MappingDef`s under
//!   their FQN during `declare()`. No `define_signatures` /
//!   `define_bodies` / `validate` work yet — Stage 3.
//!
//! Stages 4–8 (separate sessions) extend the `ClassMappingBody` enum
//! with `Enumeration`, `Operation`, `AggregationAware`, `XStore`,
//! `Relation` variants and their processors. See
//! `~/.claude/plans/what-is-left-to-iterative-sunrise.md` for the
//! staged roadmap.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
pub mod compiler;
pub mod compose;
pub mod parser;
