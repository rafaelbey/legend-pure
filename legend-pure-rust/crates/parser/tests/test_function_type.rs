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

//! Tests for function type parsing (`{ParamType[mult] -> ReturnType[mult]}`).

mod helpers;

use legend_pure_parser_ast::type_ref::FUNCTION_TYPE_SENTINEL;

// ---------------------------------------------------------------------------
// Function types in type arguments
// ---------------------------------------------------------------------------

#[test]
fn function_type_no_params() {
    // {->Boolean[1]} — zero-parameter function type
    let ast = helpers::parse_ok(
        "native function meta::pure::functions::lang::if<T|m>(test:Boolean[1], valid:Function<{->T[m]}>[1], invalid:Function<{->T[m]}>[1]):T[m];",
    );
    assert_eq!(ast.element_count(), 1);
}

#[test]
fn function_type_single_param() {
    // {T[1] -> Boolean[1]} — single-parameter function type
    let ast = helpers::parse_ok(
        "native function meta::pure::functions::collection::filter<T>(value:T[*], func:Function<{T[1]->Boolean[1]}>[1]):T[*];",
    );
    assert_eq!(ast.element_count(), 1);
}

#[test]
fn function_type_two_params() {
    // {T[1], U[1] -> V[1]} — two-parameter function type
    let ast = helpers::parse_ok(
        "native function pkg::foldWith<T,U,V>(val:T[*], init:U[1], func:Function<{T[1],U[1]->V[1]}>[1]):V[1];",
    );
    assert_eq!(ast.element_count(), 1);
}

#[test]
fn function_type_nested() {
    // Function<{Function<{->Z[y]}>[1]->Z[y]}> — nested function types
    let ast = helpers::parse_ok(
        "function meta::pure::tests::eval<Z|y>(f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1]):Boolean[1] { true }",
    );
    assert_eq!(ast.element_count(), 1);
}

#[test]
fn function_type_sentinel_name() {
    // Verify the internal sentinel encoding
    let ast = helpers::parse_ok(
        "native function pkg::f(func:Function<{String[1]->Integer[1]}>[1]):Integer[1];",
    );
    let elem = &ast.sections[0].elements[0];
    if let legend_pure_parser_ast::element::Element::NativeFunction(nf) = elem {
        let param = &nf.parameters[0];
        // The type should be "Function" with a type argument using the sentinel
        let tr = param.type_ref.as_ref().unwrap();
        assert_eq!(tr.name.as_str(), "Function");
        assert_eq!(tr.type_arguments.len(), 1);
        assert_eq!(tr.type_arguments[0].name.as_str(), FUNCTION_TYPE_SENTINEL);
    } else {
        panic!("expected NativeFunction");
    }
}

// ---------------------------------------------------------------------------
// Error recovery
// ---------------------------------------------------------------------------

#[test]
fn error_recovery_preserves_valid_elements() {
    // First and third functions are valid, second has a parse error in body (Primitive is not valid as expression).
    // Error recovery should preserve the valid functions.
    let source = r"
function pkg::valid(): String[1] { 'hello' }
Primitive pkg::broken extends Integer
function pkg::also_valid(): Integer[1] { 42 }
";
    match legend_pure_parser_parser::parse(source, "test.pure") {
        Ok(ast) => {
            // The Primitive keyword is not recognized as an element, so we get partial
            panic!(
                "Expected partial parse, but got clean parse with {} elements",
                ast.element_count()
            );
        }
        Err(partial) => {
            // Should recover the two valid functions
            assert!(
                partial.source_file.element_count() >= 2,
                "Expected at least 2 recovered elements, got {}",
                partial.source_file.element_count()
            );
            assert!(
                !partial.errors.is_empty(),
                "Expected at least 1 parse error"
            );
        }
    }
}

#[test]
fn error_recovery_native_before_broken() {
    // Native function (valid) followed by unrecognised element keyword,
    // then another native. Recovery must preserve both natives.
    let source = r"
native function pkg::myNative(x:Integer[1]):String[1];
Primitive pkg::broken extends Integer
native function pkg::anotherNative(s:String[1]):Boolean[1];
";
    match legend_pure_parser_parser::parse(source, "test.pure") {
        Ok(ast) => {
            panic!(
                "Expected partial parse, but got clean parse with {} elements",
                ast.element_count()
            );
        }
        Err(partial) => {
            assert!(
                partial.source_file.element_count() >= 2,
                "Expected at least 2 recovered elements (native functions), got {}",
                partial.source_file.element_count()
            );
        }
    }
}
