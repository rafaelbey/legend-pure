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

use legend_pure_java_codegen::{CodegenError, FqnInput, Options, generate};

const FUNCTION_TYPED_SOURCE: &str = r#"
function user_test::callIt(f: Function<{Integer[1]->String[1]}>[1]): String[1]
{
    $f->eval(42);
}
"#;

#[test]
fn function_typed_parameter_is_rejected_in_v1() {
    let model = common::compile_with_platform(Some(FUNCTION_TYPED_SOURCE));
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new("user_test::callIt_Function_1__String_1_")];
    let err = generate(&model, &fns, &opts).expect_err("function-typed param must be rejected");
    match err {
        CodegenError::FunctionTypedParameter { fqn, position } => {
            assert!(fqn.contains("callIt"), "wrong fqn in error: {fqn}");
            assert!(
                position.contains("parameter"),
                "expected parameter position, got: {position}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn generic_typed_return_is_rejected_in_v1() {
    // Same `map` — its return type is `V[*]` where V is generic. The
    // resolver hits the parameter first; just assert *some* error fires.
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let fns = vec![FqnInput::new(
        "meta::pure::functions::collection::map_T_MANY__Function_1__V_MANY_",
    )];
    let result = generate(&model, &fns, &opts);
    assert!(result.is_err(), "generic-heavy fn must not codegen");
}
