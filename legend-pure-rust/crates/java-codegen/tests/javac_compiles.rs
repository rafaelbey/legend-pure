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

//! Compiles the codegen output through `javac --release 11` together
//! with the hand-written runtime support classes, asserting the
//! generated Java is syntactically valid against the same JDK target the
//! Maven `legend-pure-runtime-rust-evaluator` module uses.
//!
//! Skipped when `javac` is not on `PATH`.

mod common;

use std::path::PathBuf;
use std::process::Command;

use legend_pure_java_codegen::{FqnInput, Options, generate};

const SOURCE: &str = r#"
Class user_test::Address
{
    street: String[1];
}

Class user_test::Person
{
    firstName: String[1];
    age: Integer[0..1];
    addresses: user_test::Address[*];
}

function user_test::makePerson(firstName: String[1]): user_test::Person[1]
{
    ^user_test::Person(firstName=$firstName, addresses=[]);
}
"#;

fn javac_available() -> bool {
    Command::new("javac")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(PathBuf::from)
        .expect("workspace root walks up from CARGO_MANIFEST_DIR")
}

#[test]
fn generated_java_compiles_with_runtime_support() {
    if !javac_available() {
        eprintln!("javac not available, skipping");
        return;
    }

    let model = common::compile_with_platform(Some(SOURCE));
    let opts = Options::new("com.example.gen");
    let fns = vec![
        FqnInput::new("user_test::makePerson_String_1__Person_1_"),
        FqnInput::new("meta::pure::functions::math::plus_Integer_MANY__Integer_1_"),
    ];
    let files = generate(&model, &fns, &[], &[], &opts).expect("codegen succeeds");

    let tmp = tempdir_for_test("legend-codegen-javac");
    let out_root = tmp.join("gen");
    let class_root = tmp.join("classes");
    std::fs::create_dir_all(&class_root).expect("create classes dir");

    let mut java_files: Vec<PathBuf> = Vec::with_capacity(files.len() + 4);
    for file in &files {
        let dest = out_root.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(&dest, &file.contents).expect("write file");
        java_files.push(dest);
    }

    // Plus the four hand-written runtime sources.
    let runtime_root = workspace_root()
        .parent()
        .expect("legend-pure-rust → legend-pure")
        .join("legend-pure-runtime/legend-pure-runtime-rust-evaluator/src/main/java");
    for rel in [
        "org/finos/legend/pure/rust/PureRustEvaluator.java",
        "org/finos/legend/pure/rust/PureRustEvaluationException.java",
        "org/finos/legend/pure/rust/PureRustException.java",
        "org/finos/legend/pure/rust/PureRustInstance.java",
        "org/finos/legend/pure/rust/PureRustResult.java",
        "org/finos/legend/pure/rust/proxy/PureRegistered.java",
        "org/finos/legend/pure/rust/proxy/Any.java",
        "org/finos/legend/pure/rust/proxy/PureProxyFactory.java",
        "org/finos/legend/pure/rust/proxy/PureInvocationHandler.java",
        "org/finos/legend/pure/rust/proxy/PureLambda.java",
    ] {
        java_files.push(runtime_root.join(rel));
    }

    // Eclipse Collections is referenced by PureRustEvaluator. If the
    // local Maven cache has it, include it on the classpath; otherwise
    // skip — the generated proxy/codegen code itself is the unit under
    // test, and the runtime support is incidentally compiled.
    let mut cp = String::new();
    if let Some(home) = std::env::var_os("HOME") {
        let m2 = PathBuf::from(home).join(".m2/repository/org/eclipse/collections");
        let api = m2.join("eclipse-collections-api/10.2.0/eclipse-collections-api-10.2.0.jar");
        let core = m2.join("eclipse-collections/10.2.0/eclipse-collections-10.2.0.jar");
        if api.exists() && core.exists() {
            cp.push_str(api.to_str().unwrap_or(""));
            cp.push(':');
            cp.push_str(core.to_str().unwrap_or(""));
        }
    }
    if cp.is_empty() {
        eprintln!("skipping: eclipse-collections jars not found in ~/.m2");
        return;
    }

    let output = Command::new("javac")
        .arg("--release")
        .arg("11")
        .arg("-d")
        .arg(&class_root)
        .arg("-cp")
        .arg(&cp)
        .args(&java_files)
        .output()
        .expect("invoke javac");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("javac failed:\n{stderr}");
    }
}

fn tempdir_for_test(prefix: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}
