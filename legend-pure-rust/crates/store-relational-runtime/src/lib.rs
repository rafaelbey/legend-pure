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

//! DuckDB-backed native function bodies for the `meta::relational` store.
//!
//! This crate is a [`legend_pure_runtime::native::RuntimeExtension`] that
//! contributes implementations for the 10 platform-declared relational
//! natives:
//!
//! - `executeInDb`
//! - `loadCsvToDbTable`, `loadValuesToDbTable` (×2 overloads)
//! - `fetchDbTablesMetaData` / `fetchDbColumnsMetaData` /
//!   `fetchDbSchemasMetaData` / `fetchDbPrimaryKeysMetaData` /
//!   `fetchDbImportedKeysMetaData`
//! - `createTempTable` (×2 overloads), `dropTempTable`
//!
//! # Connection lifecycle
//!
//! Each [`legend_pure_runtime::eval::Evaluator`] owns a single in-memory
//! DuckDB connection through [`DuckDBState`], lazily initialised on first
//! native call and dropped with the evaluator. Two evaluators get fully
//! independent databases — no process-wide singletons, supports concurrent
//! evaluations and per-test isolation out of the box.
//!
//! See [`legend_pure_runtime::extensions::ExtensionStateStore`] for the
//! storage hook this crate plugs into.
//!
//! # Engine routing
//!
//! Natives read `databaseConnection.type` (a
//! `meta::relational::runtime::DatabaseType` enum value) and route only
//! `DuckDB` to a real implementation. H2, Postgres, Snowflake and friends
//! return a `not implemented` error; bringing them online is future work.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// PureException carries a call stack; mirrors the lib-level allow in
// `legend-pure-runtime`.
#![allow(clippy::result_large_err)]

pub mod config;
pub mod connection;
pub mod dispatch;
pub mod extension;
pub mod natives;
pub mod resultset;

pub use config::{H2Config, H2ConfigError};
pub use connection::DuckDBState;
pub use extension::RelationalStoreExtension;
