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

//! Collection natives: the element accessors (`at`, `first`, `last`, `init`,
//! `tail`, `take`, `drop`, `slice`, `size`, `isEmpty`, `range`, `add`,
//! `concatenate`, `reverse`, `toOne`, `indexOf`), the lambda-driven
//! iterators (`map`, `filter`, `fold`, `exists`, `forAll`, `find`,
//! `removeDuplicates`, `sort`), `zip`, and the `Map` natives (`newMap`,
//! `put`, `putAll`, `get`, `keys`, `values`).
//!
//! Each native is fished from the archive one `impl NativeFunction` at a
//! time, registered under Java's mangled key, when a PCT row asks for it.

use legend_pure_metamodel::types::ValueSpec;

use crate::error::PureException;
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args};

/// Pure `fold(T[*], Function<{T[1],V[m]->V[m]}>[1], V[m]): V[m]`
///
/// Accumulates a collection into a single value using a binary function.
#[derive(Debug)]
pub struct Fold;

impl NativeFunction for Fold {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("fold", args, 3)?;
        let source = ctx.evaluate(&args[0])?.into_value().to_collection();
        let lambda_val = ctx.evaluate(&args[1])?.into_value();
        let mut accumulator = ctx.evaluate(&args[2])?.into_value();

        for item in &source {
            accumulator = ctx.call_function(&lambda_val, &[item.clone(), accumulator])?;
        }

        Ok(Evaluated::new(accumulator))
    }
}

/// Register the fished collection natives.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("fold_T_MANY__Function_1__V_m__V_m_", Fold);
}
