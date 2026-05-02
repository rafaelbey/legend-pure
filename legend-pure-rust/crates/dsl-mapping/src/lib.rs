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

//! # Legend Pure — Mapping DSL
//!
//! Implements the `###Mapping` section grammar plus its compiler
//! extension. Sub-grammar coverage (selected at parse time by the
//! `parserName` token after `:`):
//!
//! - **`Pure`** (model-to-model): `~src`, `~filter`,
//!   `propertyName : transformExpression`. Validators check class /
//!   property resolution, filter return type (`Boolean[1]`),
//!   transform-vs-property type and multiplicity compatibility,
//!   include-graph DAG, super-mapping ID resolution, and
//!   class-mapping ID uniqueness.
//! - **`EnumerationMapping`**: `targetEnumValue : sourceValue`
//!   entries where `sourceValue` is one of `'literal'`, integer, or
//!   `pkg::OtherEnum.VAL`, optionally bracketed `[v1, v2]` for
//!   multi-source. Validators check that the target resolves to an
//!   `Enumeration`, that target value names exist on it, that
//!   referenced source enum values resolve, and that all source
//!   values across one mapping share the same kind.
//! - **`Operation`** (Stage 5, simple parameters form):
//!   `pkg::operations::union(setImplA, setImplB)` — a function-driven
//!   combinator that composes other set implementations. Validators
//!   check that the operation function path resolves to a Function
//!   and that each parameter references a class-mapping ID visible
//!   in this mapping (its own + transitively included). The merge
//!   form (`[ids], { lambda }`) is reserved for a follow-up
//!   sub-stage.
//! - **`AggregationAware`** (Stage 6): `Views: [(modelOperation,
//!   aggregateMapping), ...], ~mainMapping` — composes a
//!   fall-through main set-implementation with one or more
//!   pre-aggregated views, each guarded by a `~modelOperation`
//!   `~canAggregate`/`~groupByFunctions`/`~aggregateValues` block.
//!   Nested `~mainMapping` and per-view `~aggregateMapping` clauses
//!   recurse into `ClassMappingBody`. Validators recursively
//!   validate the nested mappings and check that each
//!   `~mapFn`/`~aggregateFn` returns a `DataType` (primitive type or
//!   enumeration), per Java's `AggregationAwareValidator`.
//! - **`XStore`** (Stage 7): `propName[srcId, tgtId] : crossExpr`
//!   per-association-property cross-store join expressions binding
//!   `$this`/`$that`. The outer class-mapping FQN is reinterpreted
//!   as an `Association` FQN — the only sub-grammar where the
//!   target is not a `Class`. Validators check that the association
//!   resolves, that each property name exists on it, and that the
//!   referenced source/target set-implementation IDs are visible in
//!   the mapping (its own + transitively included).
//!
//! Stage 8 extends `ClassMappingBody` with the `Relation` variant.
//! See `~/.claude/plans/what-is-left-to-iterative-sunrise.md` for
//! the staged roadmap.
//!
//! Module map:
//!
//! - [`ast`] — `MappingDef`, `MappingInclude`, `ClassMapping`,
//!   `ClassMappingBody` enum, body structs for each variant.
//!   `MappingDef` implements
//!   [`legend_pure_parser_ast::dsl::DSLElement`] so it rides on the
//!   core `Element::DSLElement` variant.
//! - [`parser`] — `MappingSectionParser` plugs into
//!   [`legend_pure_parser_parser::SectionParser`] and consumes
//!   `###Mapping` section bodies. Unknown `parserName` tokens raise
//!   `UnsupportedSubParser` pointing at the staged roadmap.
//! - [`compose`] — `compose_mapping` / `compose_mapping_section`
//!   round-trip a `MappingDef` back to canonical Pure source.
//! - [`compiler`] — `MappingExtension` registers `MappingDef`s
//!   during `declare()` and runs the structural + type-check
//!   validators during `validate()`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
pub mod compiler;
pub mod compose;
pub mod parser;
