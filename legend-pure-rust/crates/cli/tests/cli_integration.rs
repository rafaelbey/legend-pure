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

//! Integration tests for the legend CLI.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_version_command() {
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("legend-cli"));
}

#[test]
fn test_init_command() {
    let temp_dir = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");

    cmd.current_dir(temp_dir.path())
        .arg("init")
        .arg("my_project")
        .assert()
        .success()
        .stderr(predicate::str::contains("project 'my_project'..."));

    let project_dir = temp_dir.path().join("my_project");
    assert!(project_dir.exists());
    assert!(project_dir.join("src").exists());
}

#[test]
fn test_parse_valid_file() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("model.pure");
    std::fs::write(&file_path, "Class model::Person { name: String[1]; }").unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("parse")
        .arg(&file_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("model::Person"));
}

#[test]
fn test_parse_file_not_found() {
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("parse")
        .arg("non_existent_file.pure")
        .assert()
        .failure()
        .stderr(predicate::str::contains("error"));
}

#[test]
fn test_check_valid_directory() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("file1.pure"), "Class A {}").unwrap();
    std::fs::write(temp_dir.path().join("file2.pure"), "Class B {}").unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("check")
        .arg(temp_dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("file(s) are valid"));
}

#[test]
fn test_check_syntax_error() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("broken.pure"),
        "Class A { missing_bracket",
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("check")
        .arg(temp_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("had parse errors"));
}

#[test]
fn test_compile_semantic_error() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("broken.pure"),
        "Class A extends MissingSuperClass {}",
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("compile")
        .arg(temp_dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("compilation error(s)"));
}

#[test]
fn test_emit_valid_json() {
    let temp_dir = TempDir::new().unwrap();
    let json_file = temp_dir.path().join("model.json");

    // minimal JSON mimicking protocol PureModelContextData
    let json_content = r#"{
        "serializer": { "name": "pure", "version": "v1" },
        "elements": [
            {
                "_type": "class",
                "package": "model",
                "name": "Person",
                "properties": []
            }
        ]
    }"#;
    std::fs::write(&json_file, json_content).unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("emit")
        .arg(&json_file)
        .assert()
        .success()
        .stdout(predicate::str::contains("Class model::Person"));
}

#[test]
fn test_emit_invalid_json() {
    let temp_dir = TempDir::new().unwrap();
    let json_file = temp_dir.path().join("broken.json");
    std::fs::write(&json_file, "{ broken json }").unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("emit")
        .arg(&json_file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("JSON serialization error"));
}

#[test]
fn test_completions() {
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("completions")
        .arg("bash")
        .assert()
        .success()
        .stdout(predicate::str::contains("_legend()"));
}

// Stubs coverage
#[test]
fn test_test_command_runs_surveyor() {
    // `legend test --package <known-clean-package>` wires up the
    // Pure-native surveyor end-to-end and renders a TestReport. Scoped
    // to a package whose tests are 100% green so the command exits 0.
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("test")
        .arg("--package")
        .arg("meta::pure::functions::math::tests")
        .assert()
        .success()
        .stderr(predicate::str::contains("Test Suite completed"));
}

// The `plan`, `package`, and `publish` stub commands were removed
// from the CLI on 2026-05-16 — keeping them in the help output was
// advertising functionality that didn't exist. Design context lives
// in `docs/deferred/cli_{plan,package,publish}.md` for when the real
// implementations land.
#[test]
fn removed_stub_commands_are_not_recognized() {
    for subcommand in ["plan", "package", "publish"] {
        let mut cmd = Command::cargo_bin("legend").unwrap();
        cmd.env("NO_COLOR", "1");
        cmd.arg(subcommand)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}
