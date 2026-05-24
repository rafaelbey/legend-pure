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

//! `meta::pure::functions::io::logActivities(Activity[*]):Nil[0]`.
//!
//! Logging hook invoked by relational executions to surface query
//! activity (timings, SQL strings, connection metadata). Pure
//! declaration lives in `platform_store_relational/runtimeLogging.pure`;
//! the runtime is expected to consume the `Activity[*]` collection and
//! return `Nil[0]` (no values).
//!
//! Java parity: `LogActivities` in the upstream interpreted runtime
//! delegates to a configurable logger. The Rust port emits a single
//! `tracing` event per call at `INFO` level so embedders that wire
//! a tracing subscriber see them; producers that don't care can ignore
//! the events at no cost.

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::native::{EvalContextTrait, Evaluated, NativeFunction, expect_args};
use legend_pure_runtime::value::Value;

/// `logActivities(Activity[*]): Nil[0]` — consume the activity
/// collection, emit a `tracing` event, return the empty collection.
#[derive(Debug)]
pub struct LogActivities;

impl NativeFunction for LogActivities {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("logActivities", args, 1)?;
        // Force-evaluate the activities arg so any errors in the
        // upstream expression surface here (a passing lambda that
        // panics wouldn't otherwise propagate).
        let activities = ctx.evaluate(&args[0])?.into_value();
        let count = match &activities {
            Value::Collection(v) => v.len(),
            Value::Object(_) => 1,
            _ => 0,
        };
        tracing::info!(activity_count = count, "logActivities");
        // `Nil[0]` is the empty collection.
        Ok(Evaluated::new(Value::from_vec(Vec::new())))
    }
}
