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
//! State at landing time (Step 1 of the plan):
//! - GREEN now (locking current behaviour): tests 1, 2, 5, 9, 10.
//! - RED, will green as Steps 3f / 3g land: tests 3, 6, 7, 8, 4.

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
            legend_pure_parser_parser::parse(s, &format!("tic_{i}.pure"))
                .expect("parse failed")
        })
        .collect();
    let imports: Vec<smol_str::SmolStr> =
        auto_imports.iter().map(smol_str::SmolStr::new).collect();
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
    compile_with_imports(&[source], &[]).expect(
        "Default mode matches Java: T LUBs to Any, eval-wrong-arg compiles silently",
    );
}

// ---------------------------------------------------------------------------
// 3. eval(...) wrong arg ERRORS under strict mode (deliberate divergence)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "Pending Step 3g (strict-mode flag for arg-type validation against \
            bound T). Requires `inference::TypeInferenceContext` + the \
            two-branch dispatch + `make_concrete`-driven per-arg check."]
fn tic_eval_wrong_arg_strict_errors() {
    // Once Step 3g lands, enabling strict mode (mechanism TBD —
    // probably an env var or compile flag) makes this same source
    // surface an `ArgumentTypeMismatch` referencing the bound T value.
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
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "Strict mode: T binds Integer authoritatively from FunctionType slot; \
         the constraint slot `param:T` should reject String.",
    );
    assert!(
        !partial.errors.is_empty(),
        "expected strict-mode arg-type mismatch error"
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
// 6. Top-level T can't bind from any arg — Java parity hard error
// ---------------------------------------------------------------------------

#[test]
#[ignore = "STRICT-MODE DIVERGENCE (not Java parity). Java's \
            `TypeInference.java:87-89` is gated on `getParent() == null` — \
            it fires only at the outermost processing context, never at \
            nested call sites inside a function body. To make this test \
            green we'd flip the check on as a strict-mode opt-in (Step 3g). \
            Java itself silently substitutes `Generic(T)` and lets it \
            propagate, matching our current default behaviour."]
fn tic_unbound_top_level_t_errors() {
    // The function body uses `someFn`, whose signature has T but no
    // arg position can supply T at this call site (everything is
    // hard-coded, so dispatch can't even guess T's bottom). Java
    // emits "The type parameter T was not resolved".
    let source = r#"
###Pure
native function test::stub<T>(): T[1];
function test::caller(): Any[1] {
    test::stub()
}
"#;
    let partial = compile_with_imports(&[source], &[])
        .expect_err("unbound T must surface UnresolvedTypeParameter");
    assert!(
        partial
            .errors
            .iter()
            .any(|e| e.message.contains("type parameter") && e.message.contains("not resolved")),
        "expected 'type parameter not resolved' diagnostic, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 7. Top-level m can't bind from any arg — Java parity hard error
// ---------------------------------------------------------------------------

#[test]
#[ignore = "STRICT-MODE DIVERGENCE (not Java parity). Symmetric to \
            `tic_unbound_top_level_t_errors`: Java is silent at nested \
            call sites; strict mode would flip it on. See `Step 3g` of \
            the plan."]
fn tic_unbound_multiplicity_errors() {
    let source = r#"
###Pure
native function test::stubM<T|m>(t: T[1]): T[m];
function test::caller(): Integer[1] {
    test::stubM(1)
}
"#;
    let partial = compile_with_imports(&[source], &[])
        .expect_err("unbound m must surface UnresolvedMultiplicityParameter");
    assert!(
        partial.errors.iter().any(|e| {
            e.message.contains("multiplicity parameter") && e.message.contains("not resolved")
        }),
        "expected 'multiplicity parameter not resolved' diagnostic, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 8. Lambda can't infer parameter type — Java parity hard error
// ---------------------------------------------------------------------------

#[test]
#[ignore = "Pending Step 3f (CannotInferLambdaParameterType diagnostic, \
            Java site `TypeInference.java:116, :127`). We currently \
            silently leave the lambda param as Unresolved when there's \
            no enclosing parametric scope to anchor it."]
fn tic_lambda_unable_to_infer_errors() {
    // `needsPred<T>(pred:Function<{T[1]->Boolean[1]}>):Boolean[1]` —
    // T can't bind from any sibling arg, the enclosing fn isn't
    // parametric, and the lambda param `x` is untyped. Java errors;
    // we currently silently produce Unresolved.
    let source = r#"
###Pure
native function test::needsPred<T>(
    pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]
): Boolean[1];
native function test::isPositive(x: Integer[1]): Boolean[1];
function test::caller(): Boolean[1] { test::needsPred(x | $x->test::isPositive()) }
"#;
    let partial = compile_with_imports(&[source], &[])
        .expect_err("lambda param without anchor must surface CannotInferLambdaParameterType");
    assert!(
        partial.errors.iter().any(|e| matches!(
            e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::CannotInferLambdaParameterTypes { .. }
        )),
        "expected CannotInferLambdaParameterTypes; got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
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
