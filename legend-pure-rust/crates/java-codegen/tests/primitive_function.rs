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

mod common;

use legend_pure_java_codegen::{FqnInput, Options, generate};

#[test]
fn primitive_plus_emits_typed_wrapper() {
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new(
        "meta::pure::functions::math::plus_Integer_MANY__Integer_1_",
    )];
    let files = generate(&model, &fns, &[], &[], &opts).expect("codegen succeeds");

    let facade = files
        .iter()
        .find(|f| f.relative_path.ends_with("PureFunctions.java"))
        .expect("PureFunctions.java emitted");

    let src = &facade.contents;
    assert!(
        src.contains("package com.example.gen;"),
        "wrong package: {src}"
    );
    assert!(
        src.contains("public final class PureFunctions"),
        "missing class header: {src}"
    );
    assert!(
        src.contains("public static Long meta_pure_functions_math_plus_Integer_MANY__Integer_1_("),
        "method signature missing or wrong: {src}"
    );
    assert!(src.contains("Iterable<Long> "), "param type wrong: {src}");
    assert!(
        src.contains(", PureRustEvaluator eval) {"),
        "evaluator must be the last parameter: {src}"
    );
    assert!(
        src.contains(
            "eval.evaluate(\"meta::pure::functions::math::plus_Integer_MANY__Integer_1_\""
        ),
        "mangled FQN must be passed verbatim: {src}"
    );
}

#[test]
fn unresolved_fqn_returns_error() {
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new("does::not::exist__Any_1_")];
    let err = generate(&model, &fns, &[], &[], &opts).expect_err("expected error");
    assert!(
        err.to_string().contains("does::not::exist__Any_1_"),
        "error must mention the offending FQN: {err}"
    );
}
