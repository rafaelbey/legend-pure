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
// 3a. eval(...) wrong arg under strict mode — Step 3g divergence
// ---------------------------------------------------------------------------

#[test]
fn tic_eval_wrong_arg_strict_mode_errors() {
    // **Deliberate divergence over Java semantics** (per
    // `parity_semantics.md`): when strict-inference is enabled, the
    // call-arg validator substitutes the param type with the call's
    // bindings before the compatibility check. T binds Integer
    // authoritatively from the FunctionType slot, so the constraint
    // slot `param:T` becomes `param:Integer`, and 'not an int' (a
    // String) is rejected.
    //
    // This test exercises the toggle through
    // `legend_pure_parser_pure::strict_mode::with_strict_mode` —
    // thread-local override that restores on closure exit even on
    // panic, so other tests stay unaffected.
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
    let result =
        legend_pure_parser_pure::strict_mode::with_strict_mode(true, || compile_with_imports(&[source], &[]));
    let partial = result.expect_err(
        "Strict mode: T binds Integer authoritatively from FunctionType slot; \
         the constraint slot `param:T` substituted to `param:Integer` should \
         reject String.",
    );
    assert!(
        !partial.errors.is_empty(),
        "expected strict-mode arg-type mismatch error, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        partial.errors.iter().any(|e| e.message.contains("argument") || e.message.contains("Argument")),
        "expected an argument-mismatch diagnostic, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
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
// 6a. Unbound T at nested call site — strict-mode divergence (Step 3f)
// ---------------------------------------------------------------------------

#[test]
fn tic_unbound_t_at_nested_call_strict_mode_errors() {
    // Strict mode flips the lenient default below: every call site
    // reports surviving `Generic(name)` that isn't in the enclosing
    // function's signature. Strictly more diagnostics than Java
    // emits (Java is gated on outermost context only); we err on the
    // side of loudness because that's what porters/migrations need.
    let source = r#"
###Pure
native function test::stub<T>(): T[1];
function test::caller(): Any[1] {
    test::stub()
}
"#;
    let result = legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[])
    });
    let partial = result.expect_err(
        "Strict mode: unresolved T at the call to `stub` must surface \
         UnresolvedTypeParameter.",
    );
    assert!(
        partial.errors.iter().any(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedTypeParameter { parameter, .. }
                if parameter.as_str() == "T"
        )),
        "expected UnresolvedTypeParameter {{ parameter: \"T\", .. }}; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
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
// 7a. Unbound multiplicity m — strict-mode divergence (Step 3f)
// ---------------------------------------------------------------------------

#[test]
fn tic_unbound_multiplicity_at_nested_call_strict_mode_errors() {
    let source = r#"
###Pure
native function test::stubM<T|m>(t: T[1]): T[m];
function test::caller(): Integer[1] {
    test::stubM(1)
}
"#;
    let result = legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[])
    });
    let partial = result.expect_err(
        "Strict mode: unresolved m at the call to `stubM` must surface \
         UnresolvedMultiplicityParameter.",
    );
    assert!(
        partial.errors.iter().any(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedMultiplicityParameter { parameter, .. }
                if parameter.as_str() == "m"
        )),
        "expected UnresolvedMultiplicityParameter {{ parameter: \"m\", .. }}; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
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
// 8b. Lambda param expected type is `List<T>` (T not in scope) — strict
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_param_with_nested_unbound_t_strict_mode_errors() {
    // `needsListPred<T>(p:Function<{List<T>[1]->Boolean[1]}>):Boolean[1]`.
    // The only arg is the lambda — T can't bind from any sibling arg.
    // The lambda's expected param type stays `List<T>` with `Generic(T)`
    // NESTED inside `Named<List>{[T]}`. The shallow Generic-only check
    // misses this; the deep walk via `unresolved_type_params` catches it.
    let source = r#"
###Pure
Class test::List<T> { values: T[*]; }
native function test::needsListPred<T>(
    pred: meta::pure::metamodel::function::Function<{test::List<T>[1]->Boolean[1]}>[1]
): Boolean[1];
function test::caller(): Boolean[1] { test::needsListPred(xs | true) }
"#;
    let result = legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[])
    });
    let partial = result.expect_err(
        "Strict mode: lambda param `xs` whose expected type is \
         `List<T>` with T not in scope must surface \
         CannotInferLambdaParameterTypes — even though `Generic(T)` \
         is nested rather than at the top.",
    );
    assert!(
        partial.errors.iter().any(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::CannotInferLambdaParameterTypes { names }
                if names.iter().any(|n| n.as_str() == "xs")
        )),
        "expected CannotInferLambdaParameterTypes containing 'xs'; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 8c. Nested Generic(T) inside FunctionType expected — strict
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_param_with_function_type_unbound_t_strict_mode_errors() {
    // The expected type is `Function<{T->Boolean}>` — `Generic(T)`
    // is nested inside the FunctionType's parameters. Same deep-walk
    // requirement as 8b but through a different structural node.
    let source = r#"
###Pure
native function test::needsHigher<T>(
    f: meta::pure::metamodel::function::Function<{
        meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]->Boolean[1]
    }>[1]
): Boolean[1];
function test::caller(): Boolean[1] {
    test::needsHigher(inner | true)
}
"#;
    let result = legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[])
    });
    let partial = result.expect_err(
        "Strict mode: lambda param `inner` whose expected type is \
         `Function<{T->Boolean}>` with T not in scope must surface \
         CannotInferLambdaParameterTypes.",
    );
    assert!(
        partial.errors.iter().any(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::CannotInferLambdaParameterTypes { names }
                if names.iter().any(|n| n.as_str() == "inner")
        )),
        "expected CannotInferLambdaParameterTypes containing 'inner'; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 8a. Lambda param can't be anchored — strict-mode divergence (Step 3f)
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_param_with_unbound_t_strict_mode_errors() {
    // Strict mode treats `Generic(T)`-expected-but-not-in-scope as
    // a lambda-inference failure. `needsPred<T>(...)` is non-parametric
    // outside, T can't bind from any sibling arg, so the lambda
    // param `x`'s expected type stays `Generic(T)` with T not in
    // `ctx.type_parameters`.
    let source = r#"
###Pure
native function test::needsPred<T>(
    pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]
): Boolean[1];
native function test::isPositive(x: Integer[1]): Boolean[1];
function test::caller(): Boolean[1] { test::needsPred(x | $x->test::isPositive()) }
"#;
    let result = legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[])
    });
    let partial = result.expect_err(
        "Strict mode: lambda param `x` whose expected type is Generic(T) \
         with T not in the enclosing fn's type-params must surface \
         CannotInferLambdaParameterTypes.",
    );
    assert!(
        partial.errors.iter().any(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::CannotInferLambdaParameterTypes { names }
                if names.iter().any(|n| n.as_str() == "x")
        )),
        "expected CannotInferLambdaParameterTypes containing 'x'; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
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

#[test]
fn tic_platform_canreactivate_lambda_evaluate_property_toOne_chain() {
    // Lock the canReactivateDynamically.pure:21 platform pattern at
    // the LOADED-PLATFORM level (rather than synthetic) since this
    // exact chain depends on real m3 LambdaFunction →
    // FunctionDefinition supertype walk + the `.expressionSequence`
    // property declared on FunctionDefinition. The bare-FunctionType
    // → Named<LambdaFunction>{[FunctionType]} bridge in
    // `infer_property_access` makes property access on a 0-arg
    // lambda literal resolve through LambdaFunction's supertype
    // chain. Without it, the chain fell off and downstream
    // `toOne` had no T to bind.
    let model = legend_pure_core_platform::platform::load_platform();
    assert!(
        model.is_ok(),
        "Default-mode platform compile must stay clean — variance + \
         lambda bridge fixes already removed all strict-mode-only \
         errors; if THIS assertion breaks, a clean-mode regression \
         landed."
    );
}

#[test]
fn tic_let_bound_collection_of_lambdas_match() {
    // Mirror of `match.pure:80-83` platform pattern:
    //   let lambdas = [λ1, λ2, ...];
    //   var->match($lambdas)
    // A let-bound collection of lambdas with mixed param types. The
    // FunctionType slot must survive Collection LUB so downstream
    // `match($lambdas)` can bind T/V from the FunctionType return
    // slot. Without preserving type_arguments through the LUB step,
    // $lambdas's variable_type collapses to bare
    // `Named<LambdaFunction>{[]}` and the strict-mode return-check
    // emits "type parameter T was not resolved at call to 'match'"
    // (28 platform errors at match.pure trace to this case).
    let source = r#"
###Pure
native function test::match<T,V|m,n>(
    var: T[1],
    fns: meta::pure::metamodel::function::Function<{T[1]->V[m]}>[1..*]
): V[m];
function test::caller(s: String[1]): Integer[1] {
    let lambdas = [
        x: Integer[1] | 1,
        x: Integer[1] | 2,
        x: Integer[1] | 3
    ];
    1->test::match($lambdas)
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "let-bound [λ1, λ2, ...] preserves FunctionType slot through \
             Collection LUB; match($lambdas) binds T/V from it under strict.",
        );
    });
}

// ---------------------------------------------------------------------------
// 15. function-ref-to-eval binds T/V/m through lifted FunctionType
// ---------------------------------------------------------------------------

#[test]
fn tic_function_ref_eval_two_args_binds_through_lift() {
    let source = r#"
###Pure
native function test::myrem(a: Integer[1], b: Integer[1]): Integer[1];
native function test::myEval2<T,U,V|m,n,p>(
    func: meta::pure::metamodel::function::Function<{T[n],U[p]->V[m]}>[1],
    a: T[n],
    b: U[p]
): V[m];
function test::caller(): Integer[1] {
    test::myrem_Integer_1__Integer_1__Integer_1_->test::myEval2(12, 5)
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Function-ref + 2-arg eval: T/U/V/m/n/p all bind from \
             the lifted FunctionType.",
        );
    });
}

// ---------------------------------------------------------------------------
// 17. Lambda-wrapped generic call: `{|toOneMany('a')}` inside a chain
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_body_toOneMany_binds_T() {
    // Mirror of `{|toOneMany('a')}.expressionSequence->at(0)`-style
    // platform pattern. A 0-arg lambda whose body is a generic call.
    // The inner generic must bind even when the lambda is a value
    // passed elsewhere.
    let source = r#"
###Pure
native function test::myToOneMany<T>(values: T[*]): T[1..*];
function test::caller(): meta::pure::metamodel::function::Function<{->String[1..*]}>[1] {
    {| test::myToOneMany('a') }
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Inner generic call inside a 0-arg lambda body must bind \
             T from the call's argument.",
        );
    });
}

// ---------------------------------------------------------------------------
// 16. M3 metamodel chain: ($h.gt->toOne().typeArguments->at(0)).rawType
// ---------------------------------------------------------------------------

#[test]
fn tic_m3_metamodel_chain_binds_through_property_steps() {
    // Synthetic version of the platform's `functionType.pure:20` chain:
    //   $f.classifierGenericType
    //     ->toOne()
    //     .typeArguments
    //     ->at(0)
    //     .rawType
    //     ->toOne()
    //
    // Each link's type-info must flow into the next call's binding
    // pass. Synthetic shape uses `G { rawType, typeArguments }` (a
    // GenericType-shaped class) and `Holder { gt: G[0..1] }`. Every
    // generic call (`myToOne`, `myAt`) must bind its T from the
    // chain.
    let source = r#"
###Pure
Class test::G { rawType: test::T[0..1]; typeArguments: test::G[*]; }
Class test::T {}
Class test::Holder { gt: test::G[0..1]; }
native function test::myToOne<X>(coll: X[*]): X[1];
native function test::myAt<X>(coll: X[*], i: Integer[1]): X[1];
function test::caller(h: test::Holder[1]): test::T[1] {
    $h.gt->test::myToOne().typeArguments->test::myAt(0).rawType->test::myToOne()
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "M3 chain: each step's type binds through; X resolves \
             at every toOne/at call.",
        );
    });
}

// ---------------------------------------------------------------------------
// 14. let-bound copy carries source's type to property/method chains
// ---------------------------------------------------------------------------

#[test]
fn tic_let_copy_carries_type_for_method_chain() {
    // `let p2 = ^$p(...); $p2.address->toOne()` — p2's type should
    // be Person (same as $p), so .address resolves and toOne's T
    // binds Address. Without this, var_types[p2] stays empty,
    // .address fails type-resolution, and downstream toOne reports
    // "T was not resolved" under strict mode (~22 platform errors
    // in copy.pure pre-fix).
    let source = r#"
###Pure
Class test::Address { name: String[1]; }
Class test::Person { name: String[1]; address: test::Address[0..1]; }
native function test::toOne<T>(coll: T[*]): T[1];
function test::caller(p: test::Person[1]): test::Address[1] {
    let p2 = ^$p(name='David');
    $p2.address->test::toOne()
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "let-bound copy must carry source's type to var_types so \
             the downstream property + toOne chain binds correctly.",
        );
    });
}

// ---------------------------------------------------------------------------
// 13. Lambda body extracts type-arg from a generic property
// ---------------------------------------------------------------------------

#[test]
fn tic_lambda_body_property_of_pair_eval() {
    // Mirror of `if.pure:26` shape: filter a collection of `Pair<F1, F2>`,
    // accessing `.first` (which is F1) inside the predicate. The
    // first lambda body's `$f.first->eval()` requires `eval<V|m>(func:Function<{->V[m]}>):V[m]`
    // to bind V from the property's substituted type.
    let source = r#"
###Pure
Class test::Pair<U,V> { first: U[1]; second: V[1]; }
native function test::pair<U,V>(first: U[1], second: V[1]): test::Pair<U,V>[1];
native function test::find<T>(coll: T[*], pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]): T[0..1];
native function test::eval<V|m>(func: meta::pure::metamodel::function::Function<{->V[m]}>[1]): V[m];
function test::caller<T|m>(
    condList: test::Pair<meta::pure::metamodel::function::Function<{->Boolean[1]}>, meta::pure::metamodel::function::Function<{->T[m]}>>[*]
): test::Pair<meta::pure::metamodel::function::Function<{->Boolean[1]}>, meta::pure::metamodel::function::Function<{->T[m]}>>[0..1] {
    $condList->test::find(f | $f.first->test::eval())
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        let result = compile_with_imports(&[source], &[]);
        if let Err(p) = &result {
            for e in &p.errors {
                eprintln!("  ERR: {}", e.message);
            }
        }
        result.expect(
            "Lambda body `$p.first->eval()`: $p.first has type \
             Function<{->Boolean[1]}>, eval should bind V:=Boolean.",
        );
    });
}

// ---------------------------------------------------------------------------
// 12. Collection LUB preserves type-arguments
// ---------------------------------------------------------------------------

#[test]
fn tic_collection_lub_preserves_type_args() {
    // Mirrors the platform's `newMap([pair(1,'a'), pair(2,'b')])`
    // pattern. The Collection LUB must preserve `Pair<Integer,String>`
    // so newMap's `<U,V>` bind from the arg.
    let source = r#"
###Pure
Class test::Pair<U,V> {}
native function test::pair<U,V>(first: U[1], second: V[1]): test::Pair<U,V>[1];
native function test::newMap<U,V>(pairs: test::Pair<U,V>[*]): test::Pair<U,V>[1];
function test::caller(): test::Pair<Integer,String>[1] {
    test::newMap([test::pair(1, 'a'), test::pair(2, 'b')])
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Collection LUB must preserve Pair<Integer,String> through \
             newMap's <U,V> bind. Strict-mode unresolved-T check should \
             stay silent.",
        );
    });
}

// ---------------------------------------------------------------------------
// 11b. Subtype-view-walking through TWO levels (Property → AbstractProperty → Function)
// ---------------------------------------------------------------------------

#[test]
fn tic_subtype_view_walks_two_levels() {
    // Mirror the platform's Property → AbstractProperty → Function
    // chain. `MyProp<L> extends MyAbs<L>` and `MyAbs<F> extends MyFunc<F>`.
    // myEval<T>(p:MyFunc<T>):T against `MyProp<Integer>` must walk two
    // hops to extract T:=Integer.
    let source = r#"
###Pure
Class test::MyFunc<F> {}
Class test::MyAbs<F> extends test::MyFunc<F> {}
Class test::MyProp<L> extends test::MyAbs<L> {}
native function test::myEval<T>(p: test::MyFunc<T>[1]): T[1];
function test::caller(p: test::MyProp<Integer>[1]): Integer[1] {
    test::myEval($p)
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Two-level subtype walk: MyProp → MyAbs → MyFunc, T binds \
             through both hops.",
        );
    });
}

// ---------------------------------------------------------------------------
// 11c. Subtype-view with FunctionType inside (Property-shaped)
// ---------------------------------------------------------------------------

#[test]
fn tic_subtype_view_with_function_type_arg() {
    // Closer to the real Property pattern: outer container has a
    // FunctionType inside its supertype's type-arguments.
    // `MyProp<L>` extends `MyFunc<{L[1]->L[1]}>` (FunctionType inside).
    // `eval<T>(f:MyFunc<{T[1]->T[1]}>):T` against `MyProp<Integer>`
    // must extract T:=Integer through the FunctionType slot.
    let source = r#"
###Pure
Class test::MyFunc<F> {}
Class test::MyProp<L> extends test::MyFunc<test::Box<L>> {}
Class test::Box<X> {}
native function test::myEval<T>(p: test::MyFunc<test::Box<T>>[1]): T[1];
function test::caller(p: test::MyProp<Integer>[1]): Integer[1] {
    test::myEval($p)
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Subtype walk with nested generic class: T must bind \
             Integer through Box<L>.",
        );
    });
}

// ---------------------------------------------------------------------------
// 11. Subtype-view binding — Function param binds against Property arg
// ---------------------------------------------------------------------------

#[test]
fn tic_subtype_view_binds_function_against_property_subtype() {
    // The platform pattern: `getProperty('a')->toOne()->eval($r)` where
    // the eval overload's `func: Function<{T[n]->V[m]}>` parameter
    // receives a `Property<...>`-typed arg. Property is a subtype of
    // Function, so binding requires walking arg's supertype chain to
    // find the Function-shaped ancestor with concrete substituted
    // type/mult args. Synthetic version: `MyProp<L,R|m>` extends
    // `MyFunc<{L[1]->R[m]}>`; calling
    // `myEval(p:MyFunc<{T[n]->V[k]}>):V[k]` against a `MyProp<Int,Str|*>`
    // should bind T:=Int, V:=Str, k:=*, and the return type becomes
    // Str[*]. Without the supertype-view, T/V/k stay Variable/Generic
    // and strict mode reports unresolved generics.
    let source = r#"
###Pure
Class test::MyFunc<F> {}
Class test::MyProp<L> extends test::MyFunc<L> {}
native function test::myEval<T>(p: test::MyFunc<T>[1]): T[1];
function test::caller(p: test::MyProp<Integer>[1]): Integer[1] {
    test::myEval($p)
}
"#;
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        compile_with_imports(&[source], &[]).expect(
            "Subtype-view binding: MyFunc<T> param against MyProp<L> \
             arg must walk MyProp's supertype chain to extract \
             T:=Integer. Without subtype_view, T stays Generic and \
             strict mode reports unresolved.",
        );
    });
}

// ---------------------------------------------------------------------------
// 10b. Two-branch dispatch — converged-args path (all converge, LUB)
// ---------------------------------------------------------------------------

#[test]
fn tic_two_branch_all_converge_lubs_to_any() {
    // Every arg converges in pass 1 → constraint (merge=true) path.
    // Java behaviour: `pick<T>(1, 'x')` LUBs T to Any — that's
    // already locked by `tic_pick_t_t_with_unrelated_args_lubs_silently`.
    // Re-pin here as the explicit "all-converged → constraint mode"
    // assertion so a future change that flips the dispatch
    // direction breaks loudly.
    let source = r#"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Any[1] { test::pick(1, 'x') }
"#;
    compile_with_imports(&[source], &[]).expect(
        "Two-branch dispatch (all-converged → Constraint mode): \
         pick<T>(1, 'x') LUBs T to Any silently. If this fails, the \
         dispatch picked Authoritative mode for an all-converged \
         call — Java parity is broken.",
    );
}

// ---------------------------------------------------------------------------
// 10c. Two-branch dispatch — authoritative path (lambda blocks pass 1)
// ---------------------------------------------------------------------------

#[test]
fn tic_two_branch_lambda_unconverged_uses_authoritative() {
    // The lambda arg doesn't converge in pass 1
    // (`infer_typeexpr_from_valuespec` returns None for `Lambda`).
    // Java's `potentiallyUpdate…` path engages → converged args bind
    // authoritatively. The non-lambda concrete arg's T-binding
    // can't be widened by subsequent concrete bindings.
    //
    // For `evalWithSeed<T>(t:T[1], f:Function<{T[1]->T[1]}>):T[1]`
    // called as `evalWithSeed(7, x | $x)`, T binds Integer
    // authoritatively from the seed; the lambda's later contribution
    // (when its body is processed in pass 2) doesn't widen T to Any.
    let source = r#"
###Pure
native function test::evalWithSeed<T>(
    seed: T[1],
    f: meta::pure::metamodel::function::Function<{T[1]->T[1]}>[1]
): T[1];
function test::caller(): Integer[1] {
    test::evalWithSeed(7, x | $x)
}
"#;
    compile_with_imports(&[source], &[]).expect(
        "Two-branch dispatch (lambda unconverged → Authoritative \
         mode): T binds Integer from the seed; the lambda's body \
         binding doesn't conflict.",
    );
}

// ---------------------------------------------------------------------------
// 10d. Authoritative protects fold-style accumulator
// ---------------------------------------------------------------------------

#[test]
fn tic_two_branch_fold_accumulator_preserved_under_authoritative() {
    // The classic fold pattern. Pass 1: arg 0 ([1,2,3]) converges
    // T:=Integer; arg 1 (lambda) unconverged; arg 2 (the accumulator
    // [], type Nil[0..0]) converges. Authoritative mode means T
    // stays Integer (insert), V stays Nil (insert). Pass 2 binds
    // the lambda body's return — that path uses constraint LUB
    // against existing V via bind_from_lambda_body, which the
    // platform's add() chain depends on.
    //
    // Locks the case both prior spikes broke. If this fails after a
    // structural change, the lambda-body-as-V-source pathway is
    // mis-classified.
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
        "Two-branch dispatch must NOT regress fold-style chains. \
         T:=Integer authoritatively from arg 0; V:=Nil from arg 2; \
         lambda body LUBs V → Any (constraint mode in pass 2). \
         Both prior spikes (c17a06, reverted 3f1a64) broke this; \
         locked here.",
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
