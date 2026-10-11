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
use crate::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args, force_all,
};
use crate::value::Value;

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

/// Pure `take(Any[*], Integer[1]): Any[*]` — first N elements.
#[derive(Debug)]
pub struct Take;

impl NativeFunction for Take {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("take", &values, 2)?;
        let coll = values[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = values[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Evaluated::new(Value::Collection(Box::new(coll.take(n)))))
    }
}

/// Pure `drop(Any[*], Integer[1]): Any[*]` — all elements after first N.
#[derive(Debug)]
pub struct Drop;

impl NativeFunction for Drop {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        let values = force_all(args, ctx)?;
        expect_args("drop", &values, 2)?;
        let coll = values[0].to_collection();
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let n = values[1].as_integer()?.max(0) as usize;
        let n = n.min(coll.len());
        Ok(Evaluated::new(Value::Collection(Box::new(coll.skip(n)))))
    }
}

/// Register the fished collection natives.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("fold_T_MANY__Function_1__V_m__V_m_", Fold);
    registry.register("take_T_MANY__Integer_1__T_MANY_", Take);
    registry.register("drop_T_MANY__Integer_1__T_MANY_", Drop);
}

#[cfg(test)]
mod tests {
    use im_rc::Vector as PVector;

    use super::*;
    use crate::native::{MockCtx, lit_collection, lit_int};

    /// Build a `ValueSpec` for an integer collection — mirrors the old `int_collection`
    /// helper but produces a `ValueSpec` suitable for `MockCtx`.
    fn int_spec(items: &[i64]) -> ValueSpec {
        lit_collection(items.iter().map(|&i| lit_int(i)).collect())
    }

    /// Build the expected `Value::Collection` for assertions.
    fn int_collection(items: &[i64]) -> Value {
        let mut v = PVector::new();
        for &i in items {
            v.push_back(Value::Integer(i));
        }
        Value::Collection(Box::new(v))
    }

    #[test]
    fn take_elements() {
        assert_eq!(
            Take.execute(&[int_spec(&[1, 2, 3, 4, 5]), lit_int(3)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[1, 2, 3])
        );
    }

    #[test]
    fn drop_elements() {
        assert_eq!(
            Drop.execute(&[int_spec(&[1, 2, 3, 4, 5]), lit_int(2)], &mut MockCtx)
                .unwrap()
                .into_value(),
            int_collection(&[3, 4, 5])
        );
    }
}
