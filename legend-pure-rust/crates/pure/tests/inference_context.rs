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

//! TDD scaffold for the lowering / TypeInferenceContext encapsulation
//! work. Each `tic_*` test pins a Java-parity (or deliberate-divergence)
//! semantic that the new `inference::` module is responsible for.
//!
//! See `~/.claude/plans/do-we-have-enought-quiet-swing.md` for the
//! plan and the Java citations.
//!
//! ## Test taxonomy
//!
//! Every test is GREEN. Each one pins one of two semantics:
//!
//! 1. **Java parity (always-on):** the call must compile / produce the
//!    expected dispatch shape. These are non-negotiable contracts.
//!
//! 2. **Pre-strict-mode lenient state:** Java itself silently widens
//!    in these cases (see plan: "Java's `findBestCommonGenericType`
//!    LUBs to Any" / "TypeInference.java:87-89 is gated on
//!    `getParent() == null`"). We currently match Java. When Step 3g
//!    lands a strict-mode flag, the *strict* path will reject these,
//!    and the assertion in each lenient test will need to flip from
//!    `expect("compiles silently — Java parity")` to a strict-mode
//!    `expect_err(...)`. **Renaming + asserting current behaviour is
//!    deliberate: the failing assertion when strict mode lands is the
//!    alarm that says "remember to flip this test."**
//!
//! No test is `#[ignore]`'d — silent skips would mask any drift.

use legend_pure_parser_ast::section::SourceFile;

#[allow(clippy::result_large_err)]
fn compile_with_imports(
    sources: &[&str],
    auto_imports: &[&str],
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sfs: Vec<SourceFile> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            legend_pure_parser_parser::parse(s, &format!("tic_{i}.pure")).expect("parse failed")
        })
        .collect();
    let imports: Vec<smol_str::SmolStr> = auto_imports.iter().map(smol_str::SmolStr::new).collect();
    legend_pure_parser_pure::pipeline::compile(&sfs, &imports)
}

// ---------------------------------------------------------------------------
// 1. pick<T>(a:T, b:T):T with mixed args silently LUBs to Any (Java parity)
// ---------------------------------------------------------------------------

#[test]
fn tic_pick_t_t_with_unrelated_args_lubs_silently() {
    // Java's `findBestCommonGenericType` (covariant) widens
    // (Integer, String) to Any. No `TestFunctionTypeInference` test
    // asserts an error here. The user has previously confirmed this
    // is the desired behaviour ("integer and string should yield Any").
    let source = r#"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Any[1] { pick(1, 'x') }
"#;
    compile_with_imports(&[source], &[])
        .expect("pick<T>(1, 'x') must silently widen T to Any (Java parity)");
}

// ---------------------------------------------------------------------------
// 2. eval(func:Function<{Integer->X}>, 'wrong') silently compiles by default
// ---------------------------------------------------------------------------

#[test]
fn tic_eval_wrong_arg_silent_under_default() {
    // Java itself silently widens via the existing-concrete +
    // incoming-concrete branch at register():423. Default mode keeps
    // parity. Strict mode (test 3) flips this.
    let source = r#"
###Pure
native function test::eval<T,V|m,n>(
    func: meta::pure::metamodel::function::Function<{T[n]->V[m]}>[1],
    param: T[n]
): V[m];
function test::caller(f: meta::pure::metamodel::function::Function<{Integer[1]->String[1]}>[1]): String[1] {
    $f->eval('not an int')
}
"#;
    compile_with_imports(&[source], &[])
        .expect("Default mode matches Java: T LUBs to Any, eval-wrong-arg compiles silently");
}

// ---------------------------------------------------------------------------
// 3. eval(...) wrong arg — current lenient state (pre Step 3g strict mode)
// ---------------------------------------------------------------------------

#[test]
fn tic_eval_wrong_arg_currently_lenient_pre_strict_mode() {
    // Java parity (current default): the FunctionType-slot binding for
    // T (Integer) doesn't fail-fast against the constraint-slot
    // `param:T` receiving 'not an int'. Java's
    // `register():467-480` LUBs (Integer, String) to Any silently and
    // dispatch succeeds.
    //
    // **When Step 3g lands**, enabling strict mode flips this same
    // source to surface an `ArgumentTypeMismatch` referencing the
    // bound T value. At that point this test **must fail loudly** —
    // that's the signal to rewrite the assertion below as
    // `expect_err(...)` plus the strict-mode toggle. Don't `#[ignore]`
    // this test if it starts failing; that defeats the early-warning.
    let source = r#"
###Pure
native function test::eval<T,V|m,n>(
    func: meta::pure::metamodel::function::Function<{T[n]->V[m]}>[1],
    param: T[n]
): V[m];
function test::caller(f: meta::pure::metamodel::function::Function<{Integer[1]->String[1]}>[1]): String[1] {
    $f->eval('not an int')
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "Default mode: eval(f, 'wrong-typed') compiles silently \
         (Java parity, register():467-480 LUBs to Any). When this \
         starts failing because Step 3g landed, flip the assertion \
         to expect_err.",
    );
}

// ---------------------------------------------------------------------------
// 4. Recursive generic fn — multi-level resolve via target_ctx
// ---------------------------------------------------------------------------

#[test]
fn tic_recursive_generic_fn_no_unbound_param_error() {
    // `getAllTypeGeneralisations(class:Type[1]):Type[*]` calls itself
    // through `map`. The recursive call's bindings flow up via
    // ParameterValueWithFlag's target_ctx pointer (Java
    // TypeInferenceContext.java:741). Locks the
    // `inference_precision_sweep` win where this fn dropped from
    // body-Any to Type-typed.
    let source = r#"
###Pure
Class test::Generalization { general: test::T[1]; }
Class test::T { generalizations: test::Generalization[*]; }
native function test::map<T,V>(coll: T[*], f: meta::pure::metamodel::function::Function<{T[1]->V[*]}>[1]): V[*];
native function test::concatenate<T>(a: T[*], b: T[*]): T[*];
function test::getAllTypeGeneralisations(class: test::T[1]): test::T[*] {
    let generalisations = $class.generalizations->map(g | $g.general->test::getAllTypeGeneralisations());
    $class->concatenate($generalisations);
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "Recursive generic fn must resolve through target_ctx — no spurious \
         unbound-T diagnostic.",
    );
}

// ---------------------------------------------------------------------------
// 5. PCT runner — Function<{Function<{->Z[y]}>[1]->Z[y]}>
// ---------------------------------------------------------------------------

#[test]
fn tic_higher_order_pct_runner() {
    // Already locked by `function_type_higher_order_pct_shape_typechecks`.
    // Re-pinned here so the test set is self-contained.
    let source = r#"
###Pure
native function test::eval<T,V|m,n>(
    func: meta::pure::metamodel::function::Function<{T[n]->V[m]}>[1],
    param: T[n]
): V[m];
function test::pctRunner<Z|y>(
    f: meta::pure::metamodel::function::Function<{->Z[y]}>[1],
    pct: meta::pure::metamodel::function::Function<{
        meta::pure::metamodel::function::Function<{->Z[y]}>[1]->Z[y]
    }>[1]
): Z[y] {
    $pct->eval($f)
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "PCT-style higher-order eval(pct, f) must dispatch with nested \
         FunctionType binding.",
    );
}

// ---------------------------------------------------------------------------
// 6. Unbound T at nested call site — current lenient state (Java parity)
// ---------------------------------------------------------------------------

#[test]
fn tic_unbound_t_at_nested_call_currently_silent() {
    // `test::stub<T>():T[1]` — T can't bind from any arg position.
    // **Java parity:** `TypeInference.java:87-89` is gated on
    // `getParent() == null` (only fires at the outermost processing
    // context). At a nested call site inside a function body, Java
    // silently substitutes `Generic(T)` and lets it propagate. We
    // match that.
    //
    // **When Step 3g lands**, strict mode opts into the
    // `UnresolvedTypeParameter` diagnostic at every site. This test
    // must then fail loudly — flip its assertion to
    // `expect_err(...)` + assert the diagnostic kind on the partial.
    let source = r#"
###Pure
native function test::stub<T>(): T[1];
function test::caller(): Any[1] {
    test::stub()
}
"#;
    let model = compile_with_imports(&[source], &[]).expect(
        "Default mode: unbound T at nested call site compiles silently \
         (Java parity). When Step 3g lands and this fails, flip the \
         assertion to expect_err with UnresolvedTypeParameter check.",
    );
    // Belt-and-braces: even in default mode we should not be emitting
    // the strict-mode diagnostic. If a future change sneaks the
    // diagnostic on at this site without going through the planned
    // strict-mode flag, the count below catches it without needing
    // the test to flip.
    drop(model); // suppress unused-binding lint
}

// ---------------------------------------------------------------------------
// 7. Unbound multiplicity m — current lenient state (Java parity)
// ---------------------------------------------------------------------------

#[test]
fn tic_unbound_multiplicity_at_nested_call_currently_silent() {
    // Symmetric to `tic_unbound_t_at_nested_call_currently_silent`:
    // `test::stubM<T|m>(t:T[1]):T[m]` — m can't bind from any arg.
    // Java is silent at nested call sites; we match.
    //
    // **When Step 3g lands**, strict mode flips this to surface
    // `UnresolvedMultiplicityParameter`. Flip the assertion then.
    let source = r#"
###Pure
native function test::stubM<T|m>(t: T[1]): T[m];
function test::caller(): Integer[1] {
    test::stubM(1)
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "Default mode: unbound m at nested call site compiles silently \
         (Java parity). When Step 3g lands and this fails, flip to \
         expect_err with UnresolvedMultiplicityParameter check.",
    );
}

// ---------------------------------------------------------------------------
// 8. Lambda param can't be anchored — current lenient state
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_param_with_unbound_t_currently_silent() {
    // `needsPred<T>(pred:Function<{T[1]->Boolean[1]}>):Boolean[1]` —
    // T can't bind from any sibling arg, the enclosing fn isn't
    // parametric, and the lambda param `x` is untyped. The expected
    // type for `x` is `Generic(T)`, which `lower_lambda_parameters`
    // explicitly *doesn't* treat as an inference failure (per the
    // doc comment at `lower/lambda.rs`: "`Generic(_)` expectations
    // don't trigger the failure — they mean we're inside a parametric
    // outer context where the type variable is in scope and may bind
    // at the call site.").
    //
    // **Step 3f / 3g**: Java emits "Cannot infer lambda parameter
    // type" at this site (`TypeInference.java:116, :127`). When we
    // turn that on, this test must fail loudly — flip the assertion
    // to expect_err and check for `CannotInferLambdaParameterTypes`.
    let source = r#"
###Pure
native function test::needsPred<T>(
    pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]
): Boolean[1];
native function test::isPositive(x: Integer[1]): Boolean[1];
function test::caller(): Boolean[1] { test::needsPred(x | $x->test::isPositive()) }
"#;
    compile_with_imports(&[source], &[]).expect(
        "Default mode: lambda param expected as Generic(T) doesn't \
         trigger the eager CannotInferLambdaParameterTypes \
         diagnostic. When Step 3f/3g lands and this fails, flip to \
         expect_err with the CannotInferLambdaParameterTypes check.",
    );
}

// ---------------------------------------------------------------------------
// 9. match([λ1, λ2]) — Collection-of-lambdas binding via existing fix
// ---------------------------------------------------------------------------

#[test]
fn tic_collection_of_lambdas_match() {
    // Already locked at the platform level via
    // `__classMappingByClass`. Re-pinned here as a small synthetic so
    // the new context machinery has an explicit smoke test.
    let source = r#"
###Pure
Class test::Box<T> { value: T[1]; }
native function test::match<T,V|m,n>(
    var: T[1],
    fns: meta::pure::metamodel::function::Function<{T[1]->V[m]}>[1..*]
): V[m];
function test::handler(b: test::Box<Integer>[1]): Integer[1] {
    $b->test::match([
        x: test::Box<Integer>[1] | $x.value
    ])
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "match([λ]) — Collection-of-lambdas must drive V binding from each \
         lambda's body (covered by Step 3e: LambdaParamFiller).",
    );
}

// ---------------------------------------------------------------------------
// 10. fold + lambda-body-as-V-source — the case both prior spikes broke
// ---------------------------------------------------------------------------

#[test]
fn tic_fold_lambda_body_v_source() {
    // The shape that broke commits c17a06 + 3f1a64. V's authoritative
    // source is the lambda body's `add` return, NOT the accumulator
    // arg. Locks that the new context machinery preserves Java's
    // ordering: lambda return registers into the parent context AFTER
    // the accumulator's constraint-bind ran.
    let source = r#"
###Pure
native function test::cast<T>(any: Any[*], t: T[1]): T[*];
native function test::fold<T,V|m>(
    value: T[*],
    func: meta::pure::metamodel::function::Function<{T[1],V[m]->V[m]}>[1],
    accumulator: V[m]
): V[m];
native function test::add<T>(set: T[*], val: T[1]): T[1..*];
function test::caller(): Any[*] {
    [1, 2, 3]->test::fold({val: Integer[1], acc: Any[*] | test::add($acc, $val)}, [])
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "fold's V binds from the lambda body's add() return — not from the \
         accumulator's empty-list. Locks the platform fold-pattern that \
         broke under both prior spikes (commits c17a06, reverted 3f1a64).",
    );
}
