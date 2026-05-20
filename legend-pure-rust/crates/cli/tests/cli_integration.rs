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
fn test_parse_compile_flag_emits_original_milestoned_properties() {
    // `legend parse --compile` should run the compile pass and emit
    // `originalMilestonedProperties` for a milestoned class. The bare
    // `legend parse` (no `--compile`) is AST-only and emits an empty
    // array for that field — milestoning synthesis only happens at
    // compile time.
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("model.pure");
    // The bare `legend parse --compile` doesn't load the platform —
    // we declare the milestoning metamodel inline so the synthesis
    // pass has the profile + carrier classes to resolve.
    let source = "\
Profile meta::pure::profiles::temporal
{
    stereotypes: [bitemporal, businesstemporal, processingtemporal];
}

Profile meta::pure::profiles::milestoning
{
    stereotypes: [generatedmilestoningproperty, generatedmilestoningdateproperty];
}

Class meta::pure::milestoning::DateMilestoning {}
Class meta::pure::milestoning::BusinessDateMilestoning extends meta::pure::milestoning::DateMilestoning
{
   from : Date[1];
   thru : Date[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    address: demo::Address[1];
}
";
    std::fs::write(&file_path, source).unwrap();

    // Without --compile: originalMilestonedProperties is empty.
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    let assert_no_compile = cmd
        .arg("parse")
        .arg(&file_path)
        .assert()
        .success();
    let out_no_compile =
        String::from_utf8_lossy(&assert_no_compile.get_output().stdout).to_string();
    // Customer is the milestoned-target referrer; its
    // originalMilestonedProperties is `[]` without --compile.
    assert!(out_no_compile.contains("demo::Customer"));

    // With --compile: the output reflects the compiled, post-synthesis
    // view: originalMilestonedProperties is populated AND the
    // synthesised `addressAllVersions` edge-point + `address(td)` /
    // `addressAllVersionsInRange` qualified-property signatures appear
    // in `properties` / `qualifiedProperties`.
    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    let assert_compile = cmd
        .arg("parse")
        .arg("--compile")
        .arg(&file_path)
        .assert()
        .success();
    let out_compile = String::from_utf8_lossy(&assert_compile.get_output().stdout).to_string();

    assert!(out_compile.contains("\"originalMilestonedProperties\""));
    // The patched output should mention the moved-aside `address` property
    // by name (under originalMilestonedProperties).
    assert!(
        out_compile.contains("\"name\": \"address\"")
            || out_compile.contains("\"name\":\"address\""),
        "--compile output should mention the moved-aside `address` property; got:\n{out_compile}"
    );
    // Post-synthesis: the synthesised edge-point property
    // `addressAllVersions` and the synthesised qualified property
    // `addressAllVersionsInRange` should both surface in the output.
    assert!(
        out_compile.contains("addressAllVersions"),
        "--compile should emit the synthesised `addressAllVersions` edge-point; got:\n{out_compile}"
    );
    assert!(
        out_compile.contains("addressAllVersionsInRange"),
        "--compile should emit the synthesised range QP; got:\n{out_compile}"
    );
}

#[test]
fn test_parse_emits_multiplicity_literal_wire_form() {
    // Multiplicity literals in expression position (`@[1]`, `@[*]`,
    // `@[m]`) must emit:
    //   - Well-known multiplicities → `packageableElementPtr`
    //     pointing at `meta::pure::metamodel::multiplicity::{PureOne,
    //     ZeroOne, ZeroMany, OneMany}`.
    //   - Parameter variables → `classInstance("multiplicity",
    //     { multiplicityParameter: <name> })`.
    // No `@[*]`-named `Var` placeholder remains anywhere in the
    // output.
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("mults.pure");
    let source = "\
function demo::pickOne(): meta::pure::metamodel::multiplicity::Multiplicity[1]
{
    @[1]
}

function demo::pickMany(): meta::pure::metamodel::multiplicity::Multiplicity[1]
{
    @[*]
}
";
    std::fs::write(&file_path, source).unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    let assert = cmd.arg("parse").arg(&file_path).assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();

    // The legacy `@[…]` placeholder Var should never appear.
    assert!(
        !out.contains("\"name\": \"@[")
            && !out.contains("\"name\":\"@["),
        "`@[…]` Var placeholder must not appear in output; got:\n{out}"
    );
    // Well-known multiplicities surface as packageableElementPtr.
    assert!(
        out.contains("meta::pure::metamodel::multiplicity::PureOne"),
        "expected PureOne packageableElementPtr in output; got:\n{out}"
    );
    assert!(
        out.contains("meta::pure::metamodel::multiplicity::ZeroMany"),
        "expected ZeroMany packageableElementPtr in output; got:\n{out}"
    );
}

#[test]
fn test_parse_compile_does_not_emit_unsupported_colspec_or_relation() {
    // ColSpec / ColSpecArray / RelationLiteral literals appearing
    // anywhere in the source must NOT surface as `@unsupported:<kind>`
    // placeholders in the JSON output. Function bodies today flow
    // through AST→Protocol (which already emits `classInstance("colSpec",
    // ...)` for these), so this assertion primarily acts as a
    // regression guard — the Pure→Protocol converter in
    // `parse_compiled.rs` must already render the same shape, ready
    // for when the broader path takes over function-body emission.
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("relations.pure");
    let source = "\
function demo::pickColSpec(): meta::pure::metamodel::relation::ColSpec<Any>[1]
{
    ~name
}

function demo::pickColSpecArray(): meta::pure::metamodel::relation::ColSpecArray<Any>[1]
{
    ~[a, b]
}
";
    std::fs::write(&file_path, source).unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    let assert = cmd
        .arg("parse")
        .arg("--compile")
        .arg(&file_path)
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();

    assert!(
        !out.contains("@unsupported:RelationLiteral")
            && !out.contains("@unsupported:ColSpecLiteral")
            && !out.contains("@unsupported:ColSpecArrayLiteral"),
        "RelationLiteral / ColSpec* must not fall back to the @unsupported placeholder; got:\n{out}"
    );
    // The AST path already emits `classInstance("colSpec", ...)` for
    // these — confirm that the shape is preserved through `--compile`
    // (i.e. nothing in the patch step disturbs it).
    assert!(
        out.contains("classInstance"),
        "expected at least one classInstance in output; got:\n{out}"
    );
}

#[test]
fn test_parse_compile_does_not_fall_back_to_path_literal_placeholder() {
    // A function body containing a navigation path literal `#/Customer/name#`
    // must under `--compile` continue to emit the protocol
    // `classInstance("path", ...)` shape — never the
    // `@unsupported:PathLiteral` placeholder. Function bodies today
    // flow through the AST→Protocol path, so this test primarily acts
    // as a regression guard: if the Pure→Protocol path ever takes over
    // function-body emission (a planned follow-up), the PathLiteral
    // converter in `parse_compiled.rs` must already render the same
    // shape. The lowered-IR variant is structurally identical to the
    // AST variant for path literals (start type + steps + optional
    // alias), and the converter mirrors the AST converter (see
    // `crates/protocol/src/v1/convert.rs::convert_navigation_path`).
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("paths.pure");
    let source = "\
Class demo::Customer
{
    name: String[1];
}

function demo::pickName(): meta::pure::metamodel::path::Path<demo::Customer, String|1>[1]
{
    #/demo::Customer/name#
}
";
    std::fs::write(&file_path, source).unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    let assert = cmd
        .arg("parse")
        .arg("--compile")
        .arg(&file_path)
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();

    assert!(
        out.contains("\"_type\": \"classInstance\"")
            || out.contains("\"_type\":\"classInstance\""),
        "expected path literal to render as classInstance; got:\n{out}"
    );
    assert!(
        out.contains("\"propertyPathElement\""),
        "expected propertyPathElement step in path output; got:\n{out}"
    );
    assert!(
        !out.contains("@unsupported:PathLiteral"),
        "PathLiteral should NOT fall back to the @unsupported placeholder; got:\n{out}"
    );
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

#[test]
fn test_test_command_with_explicit_classpath_loads_repos() {
    // `legend test --classpath <toml> --live` should:
    //   1. emit the "--live / --watch / --platform-dir ignored" warning
    //      because --classpath wins;
    //   2. compile the embedded platform via `repo::load(resolved.repos, …)`
    //      (the classpath branch — empty toml falls through to
    //      `Repo::default_embedded()`, which is the trimmed platform blob);
    //   3. drive the surveyor to completion against a known-clean package
    //      that lives in the trimmed embedded blob.
    //
    // The package `meta::pure::functions::math` exists in the trimmed
    // platform blob; its test cases live in the separate `.tests`
    // artifact (not in `default_embedded()` — by design, downstream
    // classpaths shouldn't auto-include our test artifacts), so the
    // surveyor reports 0 tests and exits 0.
    let temp_dir = TempDir::new().unwrap();
    let toml_path = temp_dir.path().join("legend-pure-classpath.toml");
    std::fs::write(&toml_path, "root = \".\"\n").unwrap();

    let mut cmd = Command::cargo_bin("legend").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd.arg("test")
        .arg("--classpath")
        .arg(&toml_path)
        .arg("--live")
        .arg("--package")
        .arg("meta::pure::functions::math")
        .assert()
        .success()
        .stderr(
            predicate::str::contains("`--live` / `--watch` / `--platform-dir` ignored")
                .and(predicate::str::contains("Test Suite completed")),
        );
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
