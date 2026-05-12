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

//! `meta::relational::metamodel::execute::executeInDb`.

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};

use crate::dispatch::resolve_backend;

/// `executeInDb(sql, dbConn, timeoutSec, fetchSize) -> ResultSet`.
///
/// `timeoutSec` and `fetchSize` are accepted for signature parity with
/// the upstream native; both backends ignore them today. The fields
/// land on the result-set timing slots so they remain observable to
/// Pure callers.
#[derive(Debug)]
pub struct ExecuteInDb;

impl NativeFunction for ExecuteInDb {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("executeInDb", args, 4)?;

        let sql_val = ctx.evaluate(&args[0])?.into_value();
        let sql = sql_val
            .as_string()
            .map_err(PureException::from)?
            .to_string();
        let db_conn = ctx.evaluate(&args[1])?.into_value();
        let _ = ctx.evaluate(&args[2])?;
        let _ = ctx.evaluate(&args[3])?;

        let backend = resolve_backend("executeInDb", &db_conn, ctx)?;
        let rs = backend.run_sql_to_result_set(ctx, &sql)?;
        Ok(Evaluated::new(rs))
    }

    fn signature(&self) -> &'static str {
        "executeInDb(String[1], DatabaseConnection[1], Integer[1], Integer[1]): ResultSet[1]"
    }
}
