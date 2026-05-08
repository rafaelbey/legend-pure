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

//! When `Options::external_bindings` is populated, types that match
//! its keys must (a) render to the imported Java FQN at every
//! reference site and (b) not be emitted as Java sources by this
//! codegen run. This is the cross-module composition story: a
//! downstream module imports M3 bindings and emits only its DSL
//! classes that reference them.

mod common;

use std::collections::HashMap;

use legend_pure_java_codegen::{FqnInput, Options, generate};

const TWO_MODULE_SOURCE: &str = r#"
Class user_test::Account
{
    name: String[1];
}

Class user_test::Mapping extends user_test::Account
{
    label: String[1];
    parent: user_test::Account[0..1];
    siblings: user_test::Account[*];
}
"#;

/// Real-world cross-module composition: an upstream module emits
/// `Account` under `org.upstream.gen`. A downstream module declares
/// `@import: pure-bindings/upstream.jpure` and emits a Mapping
/// subclass referencing Account by its imported Java FQN — never
/// re-emitting Account itself. This test simulates that two-phase
/// codegen entirely on the Rust side: a downstream `generate(...)`
/// call with `external_bindings` populated must (a) skip Account.java
/// and (b) point Mapping's supertype + property references at
/// `org.upstream.gen.Account`.
#[test]
fn external_class_supertype_is_skipped_and_referenced_externally() {
    let model = common::compile_with_platform(Some(TWO_MODULE_SOURCE));
    let mut external = HashMap::new();
    external.insert(
        "user_test::Account".to_owned(),
        "org.upstream.gen.Account".to_owned(),
    );

    let mut opts = Options::new("org.downstream.gen");
    opts.external_bindings = external;

    let files = generate(
        &model,
        &[],
        &[FqnInput::new("user_test::Mapping")],
        &[],
        &opts,
    )
    .expect("generate must succeed when supertype is external");

    // Account.java is NOT emitted — it lives in the upstream module.
    assert!(
        !files
            .iter()
            .any(|f| f.relative_path.ends_with("Account.java")),
        "Account is external — must NOT be emitted: {:?}",
        files
            .iter()
            .map(|f| f.relative_path.display().to_string())
            .collect::<Vec<_>>()
    );

    // Mapping.java references Account via the imported Java FQN —
    // both as a supertype and at every property reference site.
    let mapping = files
        .iter()
        .find(|f| f.relative_path.ends_with("Mapping.java"))
        .expect("Mapping.java emitted");
    let src = &mapping.contents;
    assert!(
        src.contains("extends org.upstream.gen.Account"),
        "Mapping must extend the imported Account: {src}"
    );
    assert!(
        src.contains("org.upstream.gen.Account parent()") || src.contains("Optional<org.upstream.gen.Account> parent()"),
        "parent property must reference imported Account: {src}"
    );
    assert!(
        src.contains("Iterable<org.upstream.gen.Account> siblings()"),
        "siblings property must reference imported Account: {src}"
    );
}
