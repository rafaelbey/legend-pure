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

//! Parity pins for the Z-propagation fix landed 2026-05-18.
//!
//! Original RED case (provided by user):
//!
//! ```pure
//! function testToListOfVariants<Z|y>(f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1]):Boolean[1] {
//!     let result = $f->eval(|fromJson('[1, 2, 3]')->to(@List<Variant>));
//!     assertEquals(1, $result.values->at(0)->to(@Integer));
//!     ...
//! }
//! ```
//!
//! Symptom (pre-fix): `$result` types as `Generic("Z")[y]`; `.values`
//! falls back to `Named{Any}` via the permissive infer-property path;
//! downstream strict `to(v:Variant[1], …)` rejects `Any` because
//! `is_subtype(Any, Variant) = false`.
//!
//! Fix (two-part):
//! 1. `infer_generic_bindings` pass-2 substitutes `ty_auth` bindings
//!    into `param.type_expr` *before* checking for a FunctionType slot,
//!    so `T[n]` whose `T` was bound from arg-1's structural slot to
//!    `Function<{->Z[y]}>` exposes the inner FunctionType to pass-2's
//!    lambda-body binding logic.
//! 2. `inference::lambda::bind_from_lambda_body` uses
//!    `infer_typeexpr_from_valuespec` (preserves `type_arguments`)
//!    instead of `infer_type_from_valuespec` (returns bare ElementId),
//!    so `Z` binds to `List<Variant>` not bare `List`.
//!
//! Drift histogram dropped 5931 → 4531 (−1400, 24% reduction).

use legend_pure_parser_ast::section::SourceFile;

#[allow(clippy::result_large_err)]
fn compile(
    sources: &[&str],
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sfs: Vec<SourceFile> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            legend_pure_parser_parser::parse(s, &format!("zp_{i}.pure")).expect("parse failed")
        })
        .collect();
    legend_pure_parser_pure::pipeline::compile(&sfs, &[])
}

/// Java-parity: a PCT-style `Function<{Function<{->Z[y]}>[1]->Z[y]}>`
/// receiver, called with a 0-arg lambda whose body returns a
/// parametric `List<Variant>`, must bind `Z := List<Variant>` so
/// `$result.values` resolves through `List<Variant>.values: Variant[*]`,
/// not the `Generic("X")[*]` (or `Any[*]`) fallback path. The strict
/// chain `$result.values->at(1)->to(@VariantInteger)` exercises the
/// full path: `to`'s `v: Variant[1]` slot rejects `Any` outright, so
/// the test fails loudly if Z stayed Generic.
#[test]
fn zp_lambda_body_drives_outer_pct_generic_through_class_typearg() {
    let source = r"
###Pure
Class test::Variant { kind: String[1]; }
Class test::List<X> { values: X[*]; }
Class test::VariantInteger extends test::Variant {}

native function test::eval<T,V|m,n>(
    func: meta::pure::metamodel::function::Function<{T[n]->V[m]}>[1],
    param: T[n]
): V[m];
// Strict `at` and `cast`-shape `to(@T)` mirror the platform shapes
// the original failing chain dispatches against. The `to` slot
// `v: Variant[1]` is the load-bearing strictness — if `.values`
// falls back to `Any`, `is_subtype(Any, Variant) = false` and the
// chain errors. If Z bound correctly, `.values` is `Variant[*]`,
// `.at(1)` returns `Variant[1]`, and `to($v0, @VariantInteger)`
// type-checks cleanly.
native function test::at<T>(coll: T[*], i: Integer[1]): T[1];
native function test::to<T>(v: test::Variant[1], target: T[1]): T[1];

function test::testToListOfVariants<Z|y>(
    f: meta::pure::metamodel::function::Function<{meta::pure::metamodel::function::Function<{->Z[y]}>[1]->Z[y]}>[1]
): Boolean[1] {
    let result = $f->test::eval(|^test::List<test::Variant>(values=[]));
    let v0 = $result.values->test::at(1);
    let i  = $v0->test::to(@test::VariantInteger);
    true
}
";
    compile(&[source]).expect(
        "Z propagation: lambda body's `List<Variant>` return type must \
         bind through the outer PCT generic Z so `$result.values` resolves \
         to `Variant[*]`.",
    );
}
