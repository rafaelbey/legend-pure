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

//! Exercises the `--classes` and `--associations` extra-seed paths:
//!   * a class that no requested function references still gets emitted;
//!   * an association seed pulls in both endpoint classes and the
//!     association-injected properties show up on each side.

mod common;

use legend_pure_java_codegen::{CodegenError, FqnInput, Options, generate};

const CLASS_ONLY_SOURCE: &str = r"
Class user_test::Standalone
{
    label: String[1];
    count: Integer[0..1];
}
";

#[test]
fn extra_class_seed_emits_interface_with_no_function_request() {
    let model = common::compile_with_platform(Some(CLASS_ONLY_SOURCE));
    let opts = Options::new("com.example.gen");
    let files = generate(
        &model,
        &[],
        &[FqnInput::new("user_test::Standalone")],
        &[],
        &opts,
    )
    .expect("codegen succeeds with no functions but a class seed");

    let standalone = files
        .iter()
        .find(|f| f.relative_path.ends_with("Standalone.java"))
        .expect("Standalone.java emitted");
    let src = &standalone.contents;
    assert!(
        src.contains("public interface Standalone extends org.finos.legend.pure.rust.proxy.Any"),
        "Standalone must extend the hand-written Any: {src}"
    );
    assert!(src.contains("String label();"), "label() missing: {src}");
    assert!(
        src.contains(
            "default java.util.Optional<Long> count() { return java.util.Optional.empty(); }"
        ),
        "count() must default to Optional.empty(): {src}"
    );

    let facade = files
        .iter()
        .find(|f| f.relative_path.ends_with("PureFunctions.java"))
        .expect("PureFunctions.java emitted");
    assert!(
        facade.contents.contains("Standalone.$REGISTERED"),
        "facade must touch the explicit class so it self-registers: {}",
        facade.contents
    );
}

const ASSOCIATION_SOURCE: &str = r"
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
";

#[test]
fn association_seed_pulls_in_both_endpoint_classes() {
    let model = common::compile_with_platform(Some(ASSOCIATION_SOURCE));
    let opts = Options::new("com.example.gen");
    let files = generate(
        &model,
        &[],
        &[],
        &[FqnInput::new("user_test::AccountTrader")],
        &opts,
    )
    .expect("codegen succeeds with an association seed");

    let account = files
        .iter()
        .find(|f| f.relative_path.ends_with("Account.java"))
        .expect("Account.java emitted via association seed");
    let trader = files
        .iter()
        .find(|f| f.relative_path.ends_with("Trader.java"))
        .expect("Trader.java emitted via association seed");

    // Association-injected `traders` should surface on Account, and
    // `accounts` should surface on Trader. The closure walker already
    // pulls those in once the participating class is in the seed set.
    assert!(
        account.contents.contains(
            "default Iterable<com.example.gen.user_test.Trader> traders() { \
             return java.util.Collections.emptyList(); }"
        ),
        "Account.traders() association property missing or wrong default: {}",
        account.contents
    );
    assert!(
        trader.contents.contains(
            "default Iterable<com.example.gen.user_test.Account> accounts() { \
             return java.util.Collections.emptyList(); }"
        ),
        "Trader.accounts() association property missing or wrong default: {}",
        trader.contents
    );
}

#[test]
fn unresolved_class_seed_returns_typed_error() {
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let err = generate(
        &model,
        &[],
        &[FqnInput::new("does::not::ExistClass")],
        &[],
        &opts,
    )
    .expect_err("unresolved class must error");
    assert!(
        matches!(err, CodegenError::UnresolvedClass { ref fqn } if fqn == "does::not::ExistClass"),
        "wrong error: {err:?}"
    );
}

#[test]
fn class_seed_pointing_at_a_function_returns_not_a_class() {
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let err = generate(
        &model,
        &[],
        &[FqnInput::new(
            "meta::pure::functions::math::plus_Integer_MANY__Integer_1_",
        )],
        &[],
        &opts,
    )
    .expect_err("a function FQN passed via --classes must error");
    match err {
        CodegenError::NotAClass { fqn, kind } => {
            assert!(fqn.contains("plus_Integer_MANY"), "wrong fqn: {fqn}");
            assert_eq!(kind, "Function");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn unresolved_association_seed_returns_typed_error() {
    let model = common::compile_with_platform(None);
    let opts = Options::new("com.example.gen");
    let err = generate(
        &model,
        &[],
        &[],
        &[FqnInput::new("does::not::ExistAssoc")],
        &opts,
    )
    .expect_err("unresolved association must error");
    assert!(
        matches!(
            err,
            CodegenError::UnresolvedAssociation { ref fqn } if fqn == "does::not::ExistAssoc"
        ),
        "wrong error: {err:?}"
    );
}
