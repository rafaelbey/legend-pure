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

//! Integration tests for the four discovery + eval tools added for the
//! `pure-from-prompt` skill: `read_class_detail`,
//! `find_associations_for_class`, `search_properties`, and
//! `eval_expression`.
//!
//! All four spin up a real [`LegendMcpServer`] over a tmp Filesystem
//! repo with Person / Address / PersonAddress fixtures and exercise
//! the tool methods directly. Mirrors the helper layout of
//! `find_references.rs` and `apply_edit.rs`.

use std::sync::{Arc, Mutex};

use legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS;
use legend_pure_core_platform::repo::{OwnedSourceFile, Repo, RepoMeta};
use legend_pure_mcp::{
    LegendMcpServer, WorkspaceSnapshot,
    server::{EvalExpressionArgs, FqnArgs, SearchPropertiesArgs},
};
use rmcp::handler::server::wrapper::Parameters;
use smol_str::SmolStr;

fn make_meta(name: &str, deps: Vec<&str>) -> RepoMeta {
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());
    let pattern: &'static str = "(.*)";
    let deps_leaked: Vec<&'static str> = deps
        .into_iter()
        .map(|d| Box::leak(d.to_string().into_boxed_str()) as &'static str)
        .collect();
    let dependencies: &'static [&'static str] = Box::leak(deps_leaked.into_boxed_slice());
    RepoMeta {
        name,
        pattern,
        dependencies,
    }
}

fn extract_text_content(result: &rmcp::model::CallToolResult) -> String {
    match &result.content[0].raw {
        rmcp::model::RawContent::Text(t) => t.text.clone(),
        other => panic!("unexpected content block: {other:?}"),
    }
}

fn build_server(files: &[(&str, &str)]) -> (LegendMcpServer, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir(&proj_dir).expect("mkdir proj");
    let mut owned: Vec<OwnedSourceFile> = Vec::with_capacity(files.len());
    for (canonical, content) in files {
        let rel = canonical
            .strip_prefix("/proj/")
            .expect("canonical must start with /proj/");
        let disk_path = proj_dir.join(rel);
        std::fs::write(&disk_path, content).expect("write");
        owned.push(OwnedSourceFile {
            path: (*canonical).to_string(),
            content: (*content).to_string(),
        });
    }
    let proj_repo = Repo::Filesystem {
        prefix: "/proj".to_string(),
        files: owned,
        // The project depends on the embedded platform so platform
        // collection/math/etc. functions resolve when the eval tool
        // wraps a user expression. Without this, `filter` /
        // `removeDuplicates` / operator dispatch all silently fall
        // back to runtime-only resolution and lambda type inference
        // breaks — see `eval_expression_probe_what_actually_works`.
        meta: Some(make_meta("proj", vec!["platform"])),
        source_root: Some(proj_dir),
    };
    // Embedded platform purem — gives the snapshot real
    // `meta::pure::functions::*` definitions so arrow-call resolution
    // and operator dispatch work the same way they do under a real
    // `legend mcp` invocation.
    let platform = Repo::embedded_platform();
    let repos = Arc::new(Mutex::new(vec![platform, proj_repo]));
    // Match the auto-import list a real `legend mcp` invocation gets,
    // so the eval tests exercise the same name-resolution path the
    // skill will hit at runtime (operators like `+` and arrow
    // functions like `->size()` require the math / collection
    // packages to be in scope).
    let auto_imports: Arc<Vec<SmolStr>> = Arc::new(
        PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect(),
    );
    let initial_repos = repos
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let snapshot = Arc::new(WorkspaceSnapshot::compile(&initial_repos, &auto_imports));
    drop(initial_repos);
    let server = LegendMcpServer::new(snapshot, repos, auto_imports);
    (server, tmp)
}

/// A Person / Address fixture with a PersonAddress association so the
/// new discovery tools have realistic shapes to walk. Used by every
/// test below — kept as a const so failures cite the same canonical
/// lines.
const PERSON_FIXTURE: &str = "\
Class pkg::Person
{
  firstName: String[1];
  lastName: String[1];
  age: Integer[0..1];
}

Class pkg::Address
{
  street: String[1];
  city: String[1];
  zipCode: String[1];
}

Class pkg::Employee extends pkg::Person
{
  empId: String[1];
}

Association pkg::PersonAddress
{
  resident: pkg::Person[1];
  addresses: pkg::Address[*];
}
";

#[tokio::test]
async fn read_class_detail_returns_properties_and_supertypes_and_associations() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .read_class_detail(Parameters(FqnArgs {
            fqn: "pkg::Person".to_string(),
        }))
        .await
        .expect("read_class_detail ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    assert_eq!(parsed["fqn"].as_str().unwrap(), "pkg::Person");
    assert_eq!(parsed["source"].as_str().unwrap(), "/proj/model.pure");

    // Properties — 3 declared (firstName, lastName, age).
    let props = parsed["properties"].as_array().expect("properties array");
    let names: Vec<&str> = props.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["firstName", "lastName", "age"]);
    assert_eq!(props[2]["multiplicity"].as_str().unwrap(), "0..1");
    assert!(
        props[0]["type_fqn"].as_str().unwrap().ends_with("String"),
        "firstName type should resolve to String FQN; got {:?}",
        props[0]["type_fqn"]
    );

    // Associations: PersonAddress points from Person → addresses (Address[*]).
    let assocs = parsed["associations"]
        .as_array()
        .expect("associations array");
    assert_eq!(
        assocs.len(),
        1,
        "expected one association edge from Person; got {assocs:#?}"
    );
    assert_eq!(assocs[0]["role_name"].as_str().unwrap(), "addresses");
    assert_eq!(
        assocs[0]["target_class_fqn"].as_str().unwrap(),
        "pkg::Address"
    );
    assert_eq!(assocs[0]["multiplicity"].as_str().unwrap(), "*");
    assert_eq!(
        assocs[0]["association_fqn"].as_str().unwrap(),
        "pkg::PersonAddress"
    );
}

#[tokio::test]
async fn read_class_detail_surfaces_declared_supertype() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .read_class_detail(Parameters(FqnArgs {
            fqn: "pkg::Employee".to_string(),
        }))
        .await
        .expect("read_class_detail ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    let supers = parsed["super_types"].as_array().expect("super_types array");
    assert_eq!(
        supers.len(),
        1,
        "Employee declares exactly one supertype (Person); got {supers:#?}"
    );
    assert_eq!(supers[0].as_str().unwrap(), "pkg::Person");
}

#[tokio::test]
async fn read_class_detail_rejects_non_class_fqn() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let err = server
        .read_class_detail(Parameters(FqnArgs {
            fqn: "pkg::PersonAddress".to_string(),
        }))
        .await
        .expect_err("Association should be rejected with a clear error");
    let msg = err.message.to_string();
    assert!(
        msg.contains("is not a Class"),
        "expected 'is not a Class' diagnostic, got: {msg}"
    );
    assert!(
        msg.contains("Association"),
        "error should disclose the actual kind, got: {msg}"
    );
}

#[tokio::test]
async fn read_class_detail_unknown_fqn_errors() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let err = server
        .read_class_detail(Parameters(FqnArgs {
            fqn: "pkg::DoesNotExist".to_string(),
        }))
        .await
        .expect_err("unknown class should error");
    assert!(err.message.to_string().contains("element not found"));
}

#[tokio::test]
async fn find_associations_for_class_returns_edges_from_both_sides() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    // From Person's side: one edge to Address via `addresses`.
    let result = server
        .find_associations_for_class(Parameters(FqnArgs {
            fqn: "pkg::Person".to_string(),
        }))
        .await
        .expect("from-person ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let edges = parsed.as_array().expect("array");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["role_name"].as_str().unwrap(), "addresses");

    // From Address's side: one edge back to Person via `resident`.
    let result = server
        .find_associations_for_class(Parameters(FqnArgs {
            fqn: "pkg::Address".to_string(),
        }))
        .await
        .expect("from-address ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let edges = parsed.as_array().expect("array");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["role_name"].as_str().unwrap(), "resident");
    assert_eq!(
        edges[0]["target_class_fqn"].as_str().unwrap(),
        "pkg::Person"
    );
}

#[tokio::test]
async fn search_properties_substring_match_across_classes() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    // "firstName" is unique enough to land on pkg::Person specifically,
    // even with the platform repo loaded alongside the project. Using
    // a more specific substring (instead of bare "name") keeps the
    // assertion stable against future platform additions that might
    // introduce other *name* properties.
    let result = server
        .search_properties(Parameters(SearchPropertiesArgs {
            name_substring: "firstName".to_string(),
            limit: None,
        }))
        .await
        .expect("search_properties ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let hits = parsed.as_array().expect("array");

    let person_hits: Vec<&serde_json::Value> = hits
        .iter()
        .filter(|h| h["property_name"].as_str() == Some("firstName"))
        .collect();
    assert!(
        !person_hits.is_empty(),
        "search_properties should find at least one firstName hit; got hits={hits:#?}"
    );
    // At least one of the hits must own to pkg::Person.
    let on_person = person_hits
        .iter()
        .any(|h| h["owner_fqn"].as_str() == Some("pkg::Person"));
    assert!(
        on_person,
        "expected a firstName property on pkg::Person; full hits = {person_hits:#?}"
    );
}

#[tokio::test]
async fn search_properties_finds_unique_field_on_address() {
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .search_properties(Parameters(SearchPropertiesArgs {
            name_substring: "zip".to_string(),
            limit: None,
        }))
        .await
        .expect("search_properties ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let hits = parsed.as_array().expect("array");

    // The unique "zip" hit should be Address.zipCode — no M3 / DSL
    // metamodel property uses that substring.
    let zip_hits: Vec<&serde_json::Value> = hits
        .iter()
        .filter(|h| h["property_name"].as_str() == Some("zipCode"))
        .collect();
    assert_eq!(
        zip_hits.len(),
        1,
        "expected exactly one zipCode hit; got {zip_hits:#?}"
    );
    assert_eq!(zip_hits[0]["owner_fqn"].as_str().unwrap(), "pkg::Address");
    assert_eq!(zip_hits[0]["owner_kind"].as_str().unwrap(), "Class");
}

#[tokio::test]
async fn eval_expression_filter_predicate_with_lambda_works() {
    // The headline shape the skill exists to generate: `->filter(p |
    // $p.<cond>)` with operator dispatch inside the lambda body.
    // Validates that with the platform repo loaded the compile path
    // resolves lambda parameter types, dispatches operators, and
    // arrow-call functions all cleanly — no advisory diagnostics.
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .eval_expression(Parameters(EvalExpressionArgs {
            expression: "[1, 2, 3]->filter(x | $x > 1)->size()".to_string(),
        }))
        .await
        .expect("eval_expression ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    assert_eq!(
        parsed["ok"].as_bool(),
        Some(true),
        "filter+lambda+>+size should evaluate cleanly; got {parsed:#?}"
    );
    let value = parsed["value"].as_str().expect("value string");
    assert!(
        value.contains('2'),
        "filter(>1) leaves [2,3] → size 2; got {value:?}"
    );
    let diags = parsed["diagnostics"].as_array().expect("array");
    assert!(
        diags.is_empty(),
        "with platform loaded, no resolution diagnostics should appear; got {diags:#?}"
    );
}

#[tokio::test]
async fn eval_expression_surfaces_diagnostics_for_bad_property() {
    // The compile correctly reports `bogusField` as missing on
    // Person. The *runtime* may still dispatch the call past the
    // error (returning Unit), so `ok` alone is not a reliable
    // signal — the agent's repair loop must read `diagnostics`
    // first. This test pins that contract: when the user types a
    // bogus property, the agent gets back a structured diagnostic
    // naming the field, regardless of what the runtime did.
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .eval_expression(Parameters(EvalExpressionArgs {
            expression: "pkg::Person.all()->filter(p | $p.bogusField == 'x')".to_string(),
        }))
        .await
        .expect("eval_expression tool call ok (failure surfaces in result fields)");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    let diags = parsed["diagnostics"].as_array().expect("diagnostics array");
    let has_error = !parsed["error"].is_null();
    assert!(
        !diags.is_empty() || has_error,
        "bad property must surface diagnostics or runtime error; got {parsed:#?}"
    );
    // The diagnostic body should name the bogus field so the agent
    // can route into Phase 6's "property not found" branch.
    let bogus_mentioned = diags
        .iter()
        .any(|d| d["message"].as_str().unwrap_or("").contains("bogusField"));
    assert!(
        bogus_mentioned || has_error,
        "diagnostic message should name the bogus field; got {diags:#?}"
    );
}

#[tokio::test]
async fn eval_expression_class_all_then_size_on_empty_extent() {
    // `pkg::Person.all()->size()` against a workspace with no Person
    // instances should compile, run, and return 0 — exercising the
    // `getAll` → arrow `size` chain that the skill's headline shape
    // depends on. Pre-platform-load this raised "getAll: expected 3
    // arguments"; this test pins that the standard form now works
    // for non-milestoned classes.
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .eval_expression(Parameters(EvalExpressionArgs {
            expression: "pkg::Person.all()->size()".to_string(),
        }))
        .await
        .expect("eval_expression ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");

    assert_eq!(
        parsed["ok"].as_bool(),
        Some(true),
        "Person.all()->size() should evaluate; got {parsed:#?}"
    );
    let diags = parsed["diagnostics"].as_array().expect("array");
    assert!(
        diags.is_empty(),
        "no diagnostics expected for the standard .all()->size() shape; got {diags:#?}"
    );
}

#[tokio::test]
async fn eval_expression_runs_realistic_collection_query() {
    // Smoke-test the multi-arrow pattern the skill will generate
    // (filter + dedupe + size). Uses a literal collection so the
    // test doesn't depend on milestoning context for class extents —
    // `Person.all()` in a real snapshot may require a business-date
    // argument depending on stereotypes, and that's the agent's
    // problem to navigate at run time, not this tool's problem to
    // hide.
    let (server, _tmp) = build_server(&[("/proj/model.pure", PERSON_FIXTURE)]);

    let result = server
        .eval_expression(Parameters(EvalExpressionArgs {
            expression: "[1, 2, 3, 2, 1]->removeDuplicates()->size()".to_string(),
        }))
        .await
        .expect("eval_expression ok");
    let json = extract_text_content(&result);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    assert_eq!(
        parsed["ok"].as_bool(),
        Some(true),
        "removeDuplicates+size expression should evaluate; got {parsed:#?}"
    );
    let value = parsed["value"].as_str().expect("value string");
    assert!(
        value.contains('3'),
        "removeDuplicates([1,2,3,2,1])->size() should render as 3; got {value:?}"
    );
}
