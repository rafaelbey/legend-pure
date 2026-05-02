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

//! # Legend Pure — Mapping DSL (Stage 1 scaffold)
//!
//! Stage 1 ships only:
//!
//! - The `platform_dsl_mapping` repo embedded by `core-platform-pure`
//!   (descriptor in that crate's `Cargo.toml`).
//! - A no-op [`compiler::MappingExtension`] so the `CompilerExtension`
//!   registration site exists for future stages.
//!
//! The metamodel `.pure` files (`mapping.pure`, `functions_*.pure`,
//! `result.pure`) are plain Pure and resolve as part of the regular
//! platform compile. There is no `###Mapping` section parser yet, no
//! processors, no validators, and no composer.
//!
//! Stages 2+ (separate sessions) will add:
//!
//! - `crates/dsl-mapping/src/parser.rs` — `###Mapping` `SectionParser`.
//! - `crates/dsl-mapping/src/compiler.rs` (extended) — port of
//!   `MappingValidator`, `PureInstanceSetImplementationProcessor`/
//!   `Validator`, `EnumerationMappingProcessor`,
//!   `OperationSetImplementationProcessor`,
//!   `AggregationAwareProcessor`, `XStoreProcessor`/`Validator`,
//!   `RelationFunctionInstanceSetImplementationProcessor`/`Validator`.
//! - `crates/dsl-mapping/src/compose.rs` — round-trip composer.
//!
//! See `~/.claude/plans/what-is-left-to-iterative-sunrise.md` for the
//! staged roadmap.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod compiler;
