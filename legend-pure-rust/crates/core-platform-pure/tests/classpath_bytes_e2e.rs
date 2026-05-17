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

//! End-to-end: drive `compile_classpath_bytes` with realistic TOML
//! input and verify the produced `PureModel` is fully usable + the
//! `[extension.<…>]` table flows back to the caller intact.
//!
//! Unit tests in `src/classpath.rs` stop at parse + repo-build; this
//! file exercises the full byte-array → `repo::load` → `PureModel`
//! pipeline that the JNI `nativeInitContextWithClasspath` entry
//! point drives in production.

use legend_pure_core_platform::classpath::compile_classpath_bytes;
use smol_str::SmolStr;

#[test]
fn bytes_mode_compiles_embedded_platform_with_extension_configs() {
    // No `[[repo]]` entries — `merge_with_embedded` injects the
    // trimmed embedded platform automatically. The `[extension.<…>]`
    // tables exercise the path that JNI uses to seed evaluator
    // configs from a Java-provided classpath.
    let toml = br#"
[extension.relational.h2]
jar_path = "/opt/h2/h2-2.1.214.jar"
version = "2.1.214"
pg_port = 5435

[extension.lake.snowflake]
account = "test-acct"
"#;

    let (model, configs) =
        compile_classpath_bytes(toml).expect("bytes-mode compile should succeed");

    // Model is a real, usable PureModel: well-known elements resolve.
    let string_id = model
        .resolve_by_path(&[SmolStr::new("String")])
        .expect("`String` primitive must resolve in the embedded platform");
    assert_eq!(model.get_node(string_id).name.as_str(), "String");

    let class_id = model
        .resolve_by_path(&[
            SmolStr::new("meta"),
            SmolStr::new("pure"),
            SmolStr::new("metamodel"),
            SmolStr::new("type"),
            SmolStr::new("Class"),
        ])
        .expect("`meta::pure::metamodel::type::Class` must resolve");
    assert_eq!(model.get_node(class_id).name.as_str(), "Class");

    // Extension configs flow back, preserving the nested
    // `domain.engine` shape and the opaque `toml::Value` payload.
    let relational = configs.get("relational").expect("relational domain");
    let h2 = relational.get("h2").expect("h2 engine");
    let h2_table = h2.as_table().expect("h2 entry is a TOML table");
    assert_eq!(
        h2_table.get("version").and_then(|v| v.as_str()),
        Some("2.1.214")
    );
    assert_eq!(
        h2_table.get("pg_port").and_then(|v| v.as_integer()),
        Some(5435)
    );
    // Sibling domain coexists.
    let lake = configs.get("lake").expect("lake domain");
    assert!(lake.contains_key("snowflake"));
}

#[test]
fn bytes_mode_root_override_resolves_relative_repo_paths() {
    // Pattern A from the recipe doc §11.1: TOML sets its own
    // `root = "/abs/path"`, and `[[repo]] path =` entries are
    // relative to that root. Use a tempdir as the absolute root +
    // a header-only purem blob inside it; the bytes-mode parser
    // joins them and finds the file.
    let tmp = tempfile::tempdir().expect("tempdir");
    let purem_path = tmp.path().join("ext.purem");
    let mut blob = Vec::new();
    legend_pure_parser_pure::purem::header::write_header(&mut blob, 0);
    std::fs::write(&purem_path, &blob).expect("write blob");

    let toml = format!(
        r#"
root = "{}"

[[repo]]
name = "ext"
kind = "purem"
path = "ext.purem"
"#,
        tmp.path().display()
    );

    // The empty-payload blob makes `repo::load` produce CompileFailed
    // — the point of this test is that parse + repo-build cleared the
    // path-resolution step, NOT that an empty blob compiles to a
    // useful model. `CompileFailed` here proves the bytes-mode root
    // override flowed all the way to `repo::Repo::from_purem_file`.
    match compile_classpath_bytes(toml.as_bytes()) {
        Ok(_) | Err(legend_pure_core_platform::classpath::ClasspathError::CompileFailed { .. }) => {
        }
        other => panic!(
            "expected Ok or CompileFailed (proof that parse + repo build cleared); got {other:?}"
        ),
    }
}
