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

//! Exercises the `dispatch_bindings_by_kind` helper that powers the
//! `--bindings-file` CLI flag and the annotation-processor JNI shim.

mod common;

use legend_pure_java_codegen::{CodegenError, dispatch_bindings_by_kind};

const MIXED_SOURCE: &str = r#"
Class user_test::Account
{
    name: String[1];
}

Class user_test::Trader
{
    code: String[1];
}

Association user_test::AccountTrader
{
    accounts: user_test::Account[*];
    traders: user_test::Trader[*];
}
"#;

#[test]
fn dispatches_mixed_manifest_into_per_kind_buckets() {
    let model = common::compile_with_platform(Some(MIXED_SOURCE));
    let lines = vec![
        "meta::pure::functions::math::plus_Integer_MANY__Integer_1_".to_owned(),
        "user_test::Account".to_owned(),
        "user_test::Trader".to_owned(),
        "user_test::AccountTrader".to_owned(),
    ];

    let dispatched =
        dispatch_bindings_by_kind(&model, &lines).expect("mixed manifest dispatches cleanly");

    assert_eq!(
        dispatched.functions.len(),
        1,
        "exactly one function FQN dispatched: {dispatched:?}"
    );
    assert_eq!(
        dispatched.functions[0].raw,
        "meta::pure::functions::math::plus_Integer_MANY__Integer_1_"
    );
    assert_eq!(dispatched.classes.len(), 2);
    assert_eq!(dispatched.classes[0].raw, "user_test::Account");
    assert_eq!(dispatched.classes[1].raw, "user_test::Trader");
    assert_eq!(dispatched.associations.len(), 1);
    assert_eq!(dispatched.associations[0].raw, "user_test::AccountTrader");
}

#[test]
fn unresolved_entry_surfaces_typed_error() {
    let model = common::compile_with_platform(None);
    let err = dispatch_bindings_by_kind(&model, &["does::not::Exist".to_owned()])
        .expect_err("unresolved entry must error");
    match err {
        CodegenError::BindingsFileEntryUnresolved { fqn } => {
            assert_eq!(fqn, "does::not::Exist");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn unsupported_kind_surfaces_typed_error() {
    let model = common::compile_with_platform(None);
    // Profiles aren't dispatchable by this helper.
    let err = dispatch_bindings_by_kind(
        &model,
        &["meta::pure::profiles::doc".to_owned()],
    )
    .expect_err("profile entry must error");
    match err {
        CodegenError::BindingsFileEntryWrongKind { fqn, kind } => {
            assert_eq!(fqn, "meta::pure::profiles::doc");
            assert_eq!(kind, "Profile");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn empty_input_returns_empty_buckets() {
    let model = common::compile_with_platform(None);
    let dispatched =
        dispatch_bindings_by_kind(&model, &[]).expect("empty input is fine");
    assert!(dispatched.functions.is_empty());
    assert!(dispatched.classes.is_empty());
    assert!(dispatched.associations.is_empty());
}
