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

//! Shared dispatch helpers for relational-store natives.
//!
//! Every native first decides whether the `DatabaseConnection` argument
//! points at a backend this crate implements ([`require_duckdb`]). DuckDB
//! is the only target today; any other [`DatabaseType`] member returns a
//! `not implemented` exception with the native name and the offending
//! member, so failures explain themselves at the call site instead of
//! producing a generic SQL error.
//!
//! [`DatabaseType`]: legend_pure_runtime::m3_paths::DATABASE_TYPE

use legend_pure_runtime::error::{PureException, PureRuntimeError};
use legend_pure_runtime::native::EvalContextTrait;
use legend_pure_runtime::value::Value;

/// The single backend member name this crate routes to.
///
/// Must match `meta::relational::runtime::DatabaseType.DuckDB` in
/// `platform_store_relational/relationalRuntime.pure`.
pub const SUPPORTED_BACKEND: &str = "DuckDB";

/// Read the `type` property from a `DatabaseConnection` heap object and
/// confirm it is the `DuckDB` enum member.
///
/// Returns `Ok(())` for the supported case; otherwise an error tagged
/// with the native name and the offending enum member. The native should
/// invoke this before performing any DB work — that way unsupported
/// engines fail loudly with a deterministic message rather than emitting
/// a DuckDB-specific SQL error.
///
/// # Errors
/// Returns a [`PureException`] when:
/// * `connection` is not a heap object, OR
/// * its `type` property is not an `EnumValue` of `DatabaseType`, OR
/// * the enum member is anything other than `DuckDB`.
pub fn require_duckdb(
    native_name: &str,
    connection: &Value,
    ctx: &dyn EvalContextTrait,
) -> Result<(), PureException> {
    let Value::Object(obj) = connection else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: expected a DatabaseConnection object, got {:?}",
            connection.type_name()
        ))
        .into());
    };
    let type_values = ctx
        .heap()
        .get_property_values(obj, "type")
        .map_err(PureException::from)?;
    let Some(first) = type_values.front() else {
        return Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseConnection.type is unset; cannot route to a backend"
        ))
        .into());
    };
    match first {
        Value::EnumValue { member, .. } if member.as_str() == SUPPORTED_BACKEND => Ok(()),
        Value::EnumValue { member, .. } => Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseType.{member} is not supported yet \
             (only DuckDB is implemented; H2/Postgres/etc. coming later)"
        ))
        .into()),
        other => Err(PureRuntimeError::EvaluationError(format!(
            "{native_name}: DatabaseConnection.type is not an EnumValue (got {:?})",
            other.type_name()
        ))
        .into()),
    }
}
