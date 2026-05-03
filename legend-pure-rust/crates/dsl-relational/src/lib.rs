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

#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Relational store DSL for Legend Pure (`###Relational` section grammar).
//!
//! Stage 1 ships the top-level shell of the relational grammar:
//!
//! ```text
//! ###Relational
//! Database pkg::myDb
//! (
//!   include other::db
//!
//!   Schema sales
//!   (
//!     Table tradeTable (id INT PRIMARY KEY, ts TIMESTAMP, qty FLOAT(10,2))
//!     View activeTrades ( ~distinct quantity : tradeTable.qty )
//!   )
//!
//!   Join tradeProduct(tradeTable.prodId = {target}.id)
//!   Filter active(tradeTable.status = 'A')
//!   MultiGrainFilter byMonth(tradeTable.month = {target}.month)
//! )
//! ```
//!
//! What's parsed at Stage 1:
//! - `Database <fqn> ( … )` envelope (with optional include* prefix)
//! - `Schema <name> ( <table | view>* )`
//! - `Table <name> ( <column-defs> )` — columns with optional size,
//!   PRIMARY KEY / NOT NULL flags, stereotypes/tagged-values
//! - `View <name> ( ... )` — accepts the same body shape as Java
//!   syntactically; structural validators land in a later stage
//! - `Join <name> ( <op> )`, `Filter <name> ( <op> )`,
//!   `MultiGrainFilter <name> ( <op> )` — the `<op>` body is captured
//!   verbatim (token-stream slice) by Stage 1 so existing fixtures
//!   round-trip; structural `op_operation` parsing arrives in Stage 2.
//! - `include <fqn>` directives at the top of the database body.
//!
//! What's *not* in Stage 1 (deferred):
//! - Structural `op_operation` AST (binary/unary/variadic, DynaFunction,
//!   TableAliasColumn, …) — Stage 2.
//! - Milestoning specs on Tables — Stage 3.
//! - Validators (include DAG, table/column/PK existence, join refs) —
//!   Stage 4.
//! - `Relational` class-mapping body for `###Mapping` — Stages 5–7.
//! - Association-mapping integration — Stage 7.
//!
//! See `~/.claude/plans/lets-plan-for-implementing-lucky-scott.md` for
//! the full staging.

pub mod ast;
pub mod compiler;
pub mod compose;
pub mod parser;
