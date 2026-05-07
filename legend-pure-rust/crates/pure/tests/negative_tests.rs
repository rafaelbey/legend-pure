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

//! **Negative-test contract.**
//!
//! Every test in this file pins a Pure source that *must not compile*
//! under a well-built compiler. The tests are organised by category;
//! each one documents:
//!
//! 1. The diagnostic shape it must produce (`error.kind`).
//! 2. *Why* that diagnostic is the right one (Java parity citation
//!    where applicable).
//! 3. The "loud-failure" recipe if the assertion ever flips: read the
//!    doc comment, decide whether the new behaviour is intentional,
//!    and if so update the assertion or move it into a strict-mode
//!    wrapper.
//!
//! This is the **safety net** for the inference / lowering work
//! described in `~/.claude/plans/do-we-have-enought-quiet-swing.md`.
//! Step 3d-cont's binding-algorithm work breaks fold-style chains
//! when it goes wrong — these tests catch the "we silently accept
//! something that should error" failure mode that platform compile
//! sometimes misses (a class with subtly-wrong-typed bodies still
//! produces a clean PureModel even when downstream PCT runs would
//! fail).
//!
//! See also:
//! - `integration_tests.rs` — has 28 scattered negative cases mixed
//!   in with positive tests; this file consolidates the
//!   inference-relevant subset and adds new categories.
//! - `inference_context.rs` — pins the lenient/strict pair for each
//!   strict-mode divergence.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::CompilationErrorKind;

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
            legend_pure_parser_parser::parse(s, &format!("neg_{i}.pure")).expect("parse failed")
        })
        .collect();
    legend_pure_parser_pure::pipeline::compile(&sfs, &[])
}

/// Convenience: assert that compilation produced any error matching
/// the kind-predicate. The message included in the assertion is the
/// list of error messages, so failures are debuggable.
fn expect_diagnostic_kind(
    sources: &[&str],
    description: &str,
    pred: impl Fn(&CompilationErrorKind) -> bool,
) {
    let result = compile(sources);
    let partial = match result {
        Ok(_) => panic!(
            "{description}: expected compilation to fail with a specific \
             diagnostic, but compile succeeded."
        ),
        Err(p) => p,
    };
    assert!(
        partial.errors.iter().any(|e| pred(&e.kind)),
        "{description}: expected diagnostic kind not found. Got: {:?}",
        partial
            .errors
            .iter()
            .map(|e| (&e.kind, &e.message))
            .collect::<Vec<_>>()
    );
}

/// Stricter version: asserts compile *failed* (returned Err).
#[allow(dead_code)]
fn expect_compile_failure(sources: &[&str], description: &str) {
    let result = compile(sources);
    assert!(
        result.is_err(),
        "{description}: expected compilation to fail; got success"
    );
}

// ===========================================================================
// CATEGORY 1: Reference / resolution errors
// ===========================================================================
//
// These errors fire at resolve time (Pass 2a / hydration). They are
// always-on Java parity — every Pure compiler must emit them,
// regardless of strict mode.

#[test]
fn neg_unknown_class_reference() {
    let source = r#"
###Pure
function test::caller(): Any[1] { ^test::DoesNotExist() }
"#;
    expect_diagnostic_kind(
        &[source],
        "unknown class in ^DoesNotExist() must surface UnresolvedElement",
        |k| matches!(k, CompilationErrorKind::UnresolvedElement { .. }),
    );
}

#[test]
fn neg_unknown_property_on_user_class() {
    let source = r#"
###Pure
Class test::Box { value: Integer[1]; }
function test::caller(b: test::Box[1]): Integer[1] { $b.nonExistent }
"#;
    expect_diagnostic_kind(
        &[source],
        "$b.nonExistent on a user class must surface UnknownProperty",
        |k| {
            matches!(k, CompilationErrorKind::UnknownProperty { property_name, .. }
            if property_name.as_str() == "nonExistent")
        },
    );
}

#[test]
fn neg_qualified_property_arity_mismatch() {
    let source = r#"
###Pure
Class test::Box {
    value: Integer[1];
    derived(n: Integer[1]) { $this.value->test::plus($n) }: Integer[1];
}
native function test::plus(a: Integer[1], b: Integer[1]): Integer[1];
function test::caller(b: test::Box[1]): Integer[1] {
    $b.derived(1, 2, 3)
}
"#;
    expect_diagnostic_kind(
        &[source],
        "QP called with too many args must surface QualifiedPropertyArityMismatch",
        |k| {
            matches!(
                k,
                CompilationErrorKind::QualifiedPropertyArityMismatch { .. }
            )
        },
    );
}

// ===========================================================================
// CATEGORY 2: Structural model errors
// ===========================================================================
//
// Hierarchical / structural integrity violations — caught at hydration
// or resolver-eager validation. Always-on.

#[test]
fn neg_cyclic_inheritance() {
    let source = r#"
###Pure
Class test::A extends test::B {}
Class test::B extends test::A {}
"#;
    expect_diagnostic_kind(
        &[source],
        "A extends B and B extends A must surface CyclicInheritance",
        |k| matches!(k, CompilationErrorKind::CyclicInheritance { .. }),
    );
}

#[test]
fn neg_duplicate_class_in_same_chunk() {
    let source = r#"
###Pure
Class test::Dup { x: Integer[1]; }
Class test::Dup { y: Integer[1]; }
"#;
    expect_diagnostic_kind(
        &[source],
        "two classes with the same FQN must surface DuplicateElement",
        |k| matches!(k, CompilationErrorKind::DuplicateElement { .. }),
    );
}

#[test]
fn neg_duplicate_property_in_class() {
    let source = r#"
###Pure
Class test::WithDup {
    value: Integer[1];
    value: String[1];
}
"#;
    expect_diagnostic_kind(
        &[source],
        "two properties named `value` in the same class must surface DuplicateProperty",
        |k| matches!(k, CompilationErrorKind::DuplicateProperty { .. }),
    );
}

// ===========================================================================
// CATEGORY 3: Annotation / DSL syntax errors
// ===========================================================================

#[test]
fn neg_stereotype_reference_to_unknown_profile() {
    // The stereotype refers to a profile that doesn't exist —
    // resolver-eager catch.
    let source = r#"
###Pure
Class <<test::DoesNotExist.someStereo>> test::WithStereo {
    value: Integer[1];
}
"#;
    expect_diagnostic_kind(
        &[source],
        "stereotype on missing profile must surface InvalidAnnotation",
        |k| {
            matches!(k, CompilationErrorKind::InvalidAnnotation { .. })
                || matches!(k, CompilationErrorKind::UnresolvedElement { .. })
        },
    );
}

// ===========================================================================
// CATEGORY 4: Lambda / inference errors (default mode)
// ===========================================================================
//
// These fire even without strict mode, because the eager lambda
// inference-failure diagnostic catches the case where neither the
// source nor any caller-side expectation supplies a type.

#[test]
fn neg_lambda_no_annotation_no_caller_expectation() {
    // The lambda has no parameter annotation AND there's no
    // `Function<{T->X}>`-shaped slot to flow an expectation from
    // (the function returns the lambda directly).
    //
    // Eager `CannotInferLambdaParameterTypes` fires per
    // `lower/lambda.rs:lower_lambda_parameters`.
    let source = r#"
###Pure
function test::makesLambda(): meta::pure::metamodel::function::Function<{Any[1]->Any[1]}>[1] {
    {x | $x}
}
"#;
    expect_diagnostic_kind(
        &[source],
        "lambda with untyped param + no caller expectation must surface \
         CannotInferLambdaParameterTypes",
        |k| {
            matches!(
                k,
                CompilationErrorKind::CannotInferLambdaParameterTypes { .. }
            )
        },
    );
}

// ===========================================================================
// CATEGORY 5: Strict-mode-only divergences
// ===========================================================================
//
// These pass under default mode (Java parity) and fail under strict.
// Captured in detail in `inference_context.rs`'s `tic_*_strict_*`
// pins; cross-referenced here for consolidated visibility.

#[test]
fn neg_eval_wrong_arg_errors() {
    // Deliberate divergence over Java semantics: `eval(intFunc,
    // 'wrong')` errors. T binds Integer authoritatively from the
    // structural FunctionType slot; the constraint slot
    // `param:T` substituted to `param:Integer` rejects the String
    // arg. Java would silently widen via
    // `findBestCommonGenericType`, accepting the call — a known
    // parity hole this rejects.
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
    let result = compile(&[source]);
    assert!(
        result.is_err(),
        "eval(f, 'wrong-typed') must error. T binds Integer \
         authoritatively from the FunctionType slot; the param:T \
         slot substituted to param:Integer rejects the String arg."
    );
}

// ===========================================================================
// CATEGORY 6: Acceptance contract
// ===========================================================================
//
// Things that are sometimes tempting to flag but MUST be permitted —
// they're not bugs; they're Pure semantics. If a "fix" inadvertently
// rejects them, these tests catch the regression.

#[test]
fn pos_pick_t_t_with_unrelated_args_must_compile() {
    // `pick<T>(1, 'x')` — two unrelated T-binding contributors LUB
    // to Any. Java parity. Reject this and the whole platform corpus
    // breaks.
    let source = r#"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Any[1] { test::pick(1, 'x') }
"#;
    compile(&[source]).expect(
        "pick<T>(1, 'x') must compile silently — Java's covariant LUB \
         widens T to Any. Rejecting this shape regresses fold-style \
         chains and `match([λ1, λ2])` in the platform corpus.",
    );
}

#[test]
fn pos_lambda_inside_parametric_outer_with_generic_t_must_compile() {
    // The enclosing fn IS parametric (`<T>`), so `Generic(T)` flowing
    // into the lambda from the callee is fine — T is in scope. The
    // strict-mode diagnostic at `lower_lambda_parameters` checks
    // `ctx.type_parameters` to keep this case silent.
    let source = r#"
###Pure
native function test::needsPred<T>(
    pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]
): Boolean[1];
native function test::isType(x: Any[1]): Boolean[1];
function test::caller<T>(): Boolean[1] {
    test::needsPred(x | $x->test::isType())
}
"#;

    compile(&[source]).expect(
        "Strict mode: lambda param with `Generic(T)` expected MUST \
             compile when T is in the enclosing fn's `type_parameters` \
             — that's the parametric-scope-anchor case.",
    );
}

#[test]
fn pos_recursive_generic_no_unbound_param_under_strict() {
    // The `getAllTypeGeneralisations` shape — the recursive call's
    // bindings flow through, no unbound-T diagnostic fires under
    // strict mode either, because T is in `type_params_in_scope` of
    // the enclosing fn.
    let source = r#"
###Pure
Class test::Generalization { general: test::T[1]; }
Class test::T { generalizations: test::Generalization[*]; }
native function test::map<T,V>(coll: T[*], f: meta::pure::metamodel::function::Function<{T[1]->V[*]}>[1]): V[*];
native function test::concatenate<T>(a: T[*], b: T[*]): T[*];
function test::getAllTypeGeneralisations(class: test::T[1]): test::T[*] {
    let generalisations = $class.generalizations->test::map(g | $g.general->test::getAllTypeGeneralisations());
    $class->test::concatenate($generalisations);
}
"#;

    compile(&[source]).expect(
        "Strict mode: recursive generic fn must NOT surface a \
             spurious unbound-T diagnostic — T is in the enclosing fn's \
             `type_params_in_scope`.",
    );
}

// ===========================================================================
// CATEGORY 7: Wrong arity / wrong receiver
// ===========================================================================

#[test]
fn neg_function_call_with_wrong_arity_falls_through() {
    // `wantsTwo(x:Integer[1], y:Integer[1]):Integer[1]` called with
    // one arg — name+arity dispatch finds nothing; the resolver
    // doesn't pick anything else up either (no overload).
    let source = r#"
###Pure
native function test::wantsTwo(x: Integer[1], y: Integer[1]): Integer[1];
function test::caller(): Integer[1] { test::wantsTwo(1) }
"#;
    let result = compile(&[source]);
    assert!(
        result.is_err(),
        "Calling a 2-arg function with 1 arg must fail dispatch. \
         Got: {result:?}"
    );
}
