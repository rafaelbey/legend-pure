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

//! End-to-end integration tests for `runtime::display`.
//!
//! These tests build a full platform model + custom Pure source,
//! wire a capturing `EvalHooks` into `Evaluator::with_hooks`, evaluate
//! a function that has locals at a known line, and assert on the
//! `DisplayTree` the renderer produces.
//!
//! These exercise the path the DAP server takes at every breakpoint:
//! `eval()` → `before_eval` returns true → evaluator drives
//! `display::render_locals` → `pause_with_snapshot` receives the
//! finished tree.

use std::sync::OnceLock;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::context::VariableContext;
use legend_pure_runtime::display::DisplayTree;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::hooks::EvalHooks;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// `EvalHooks` impl that requests a snapshot at the first
/// expression whose start_line is `target_line` AND whose source ends
/// in `target_suffix`, stores the resulting `DisplayTree`, and refuses
/// any further snapshot requests so we don't drown in noise.
struct CaptureAtLine {
    target_line: u32,
    target_suffix: String,
    captured: Option<DisplayTree>,
}

impl CaptureAtLine {
    fn new(line: u32, suffix: &str) -> Self {
        Self {
            target_line: line,
            target_suffix: suffix.to_string(),
            captured: None,
        }
    }
}

impl EvalHooks for CaptureAtLine {
    fn before_eval(&mut self, source: &SourceInfo, _context: &VariableContext) -> bool {
        if self.captured.is_some() {
            return false;
        }
        source.start_line == self.target_line && source.source.ends_with(&self.target_suffix)
    }

    fn pause_with_snapshot(&mut self, _source: &SourceInfo, tree: DisplayTree) {
        if self.captured.is_none() {
            self.captured = Some(tree);
        }
    }

    fn after_eval(&mut self, _source: &SourceInfo, _result: &Value) {}
    fn enter_function(&mut self, _name: &str, _source: &SourceInfo) {}
    fn leave_function(&mut self, _name: &str) {}
}

fn init_test_tracing() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

struct PlatformFixture {
    repos: Vec<legend_pure_core_platform::repo::Repo>,
    auto_imports: Vec<SmolStr>,
}

fn platform_model() -> &'static PlatformFixture {
    static FIXTURE: OnceLock<PlatformFixture> = OnceLock::new();
    init_test_tracing();
    FIXTURE.get_or_init(|| {
        let repos = legend_pure_core_platform::repo::Repo::default_with_build_snapshots();
        let auto_imports = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
            .iter()
            .map(|&s| SmolStr::new(s))
            .collect();
        PlatformFixture {
            repos,
            auto_imports,
        }
    })
}

fn synthetic_user_repo(user_source: &str) -> legend_pure_core_platform::repo::Repo {
    use legend_pure_core_platform::repo::{OwnedSourceFile, RepoMeta};
    static USER_DEPS: &[&str] = &[
        "platform",
        "platform_precise_primitives",
        "platform_dsl_store",
        "platform_dsl_mapping",
        "platform_dsl_diagram",
        "platform_dsl_graph",
        "platform_dsl_tds",
        "platform_dsl_path",
        "platform_store_relational",
    ];
    let meta = RepoMeta {
        name: "user_test",
        pattern: ".*",
        dependencies: USER_DEPS,
    };
    legend_pure_core_platform::repo::Repo::Filesystem {
        prefix: "/user_test".into(),
        files: vec![OwnedSourceFile {
            path: "/user_test/test_source.pure".into(),
            content: user_source.into(),
        }],
        meta: Some(meta),
        source_root: None,
    }
}

fn compile_with_platform(user_source: &str) -> PureModel {
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    let result = legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports);
    match result {
        Ok(model) => model,
        Err(partial) => {
            eprintln!(
                "compile_with_platform: {} errors during repo::load",
                partial.errors.len()
            );
            for e in partial.errors.iter().take(5) {
                eprintln!("  - {e}");
            }
            partial.model
        }
    }
}

/// Compile + evaluate the given Pure source, capturing the local
/// snapshot at the given source line.
fn capture_snapshot(source: &str, target_line: u32, fqn: &str) -> DisplayTree {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let hooks = CaptureAtLine::new(target_line, "/user_test/test_source.pure");
    let mut eval = Evaluator::with_hooks(&model, &registry, hooks);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));
    let _ = eval
        .call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("Evaluation error: {e}"));
    let captured = eval.into_hooks();
    captured
        .captured
        .unwrap_or_else(|| panic!("No snapshot captured at line {target_line}"))
}

// =========================================================================
// Tests
// =========================================================================

#[test]
fn captures_primitive_local_with_pure_syntax_value() {
    // Line 2 binds `$x = 42`; the third expression on line 4 is the
    // candidate pause line — pick its line for capture.
    let source = "\
function test::f(): Integer[1] {
    let x = 42;
    let y = 'hello';
    $x + 1
}
";
    let tree = capture_snapshot(source, 4, "f__Integer_1_");
    let root = tree.nodes.get(&tree.root).expect("root node list present");
    let names: Vec<&str> = root.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"x"), "locals should include x: {names:?}");
    assert!(names.contains(&"y"), "locals should include y: {names:?}");
    let x = root.iter().find(|n| n.name == "x").unwrap();
    assert_eq!(x.value, "42");
    assert_eq!(x.r#type.as_deref(), Some("Integer[1]"));
    let y = root.iter().find(|n| n.name == "y").unwrap();
    assert_eq!(y.value, "'hello'");
    assert_eq!(y.r#type.as_deref(), Some("String[1]"));
}

#[test]
fn captures_object_with_navigable_children() {
    // Build a tiny class with two scalar properties and instantiate
    // it. The renderer must expose `propA` + `propB` as navigable
    // children of the object node.
    let source = "\
Class test::Pt
{
    propA : Integer[1];
    propB : String[1];
}

function test::f(): test::Pt[1] {
    let p = ^test::Pt(propA = 7, propB = 'lo');
    $p
}
";
    let tree = capture_snapshot(source, 9, "f__Pt_1_");
    let root = tree.nodes.get(&tree.root).expect("root list");
    let p = root.iter().find(|n| n.name == "p").expect("found p");
    assert!(
        p.value.starts_with("^test::Pt("),
        "object inline should be ^test::Pt(...); got {:?}",
        p.value
    );
    assert_eq!(p.r#type.as_deref(), Some("test::Pt[1]"));
    assert!(p.child_ref > 0, "object should have a child ref");
    // Pure semantics: `properties(genericType($p))` returns declared
    // props + inherited Any props (`classifierGenericType`,
    // `elementOverride`). Assert the user-declared two are present
    // rather than the exact count.
    let kids = tree.nodes.get(&p.child_ref).expect("kids list");
    let kid_names: Vec<&str> = kids.iter().map(|n| n.name.as_str()).collect();
    assert!(kid_names.contains(&"propA"), "propA missing: {kid_names:?}");
    assert!(kid_names.contains(&"propB"), "propB missing: {kid_names:?}");
    let prop_a = kids.iter().find(|n| n.name == "propA").unwrap();
    assert_eq!(prop_a.value, "7");
    assert_eq!(prop_a.r#type.as_deref(), Some("Integer[1]"));
    let prop_b = kids.iter().find(|n| n.name == "propB").unwrap();
    assert_eq!(prop_b.value, "'lo'");
}

#[test]
fn inherited_properties_appear_in_child_children() {
    let source = "\
Class test::Base
{
    base : Integer[1];
}

Class test::Sub extends test::Base
{
    own : String[1];
}

function test::f(): test::Sub[1] {
    let s = ^test::Sub(base = 1, own = 'x');
    $s
}
";
    let tree = capture_snapshot(source, 13, "f__Sub_1_");
    let root = tree.nodes.get(&tree.root).expect("root");
    let s = root.iter().find(|n| n.name == "s").expect("found s");
    let kids = tree.nodes.get(&s.child_ref).expect("kids");
    let kid_names: Vec<&str> = kids.iter().map(|n| n.name.as_str()).collect();
    assert!(
        kid_names.contains(&"own"),
        "own property must be present: {kid_names:?}"
    );
    assert!(
        kid_names.contains(&"base"),
        "inherited base property must be present: {kid_names:?}"
    );
}

#[test]
fn no_internal_rust_tokens_leak_into_any_node_value() {
    let source = "\
Class test::Box
{
    n : Integer[1];
}

function test::f(): test::Box[1] {
    let xs = [1, 2, 3];
    let b = ^test::Box(n = 99);
    $b
}
";
    let tree = capture_snapshot(source, 9, "f__Box_1_");
    let banned = [
        "RefCell",
        "Rc<",
        "Dynamic",
        "RuntimeObject",
        "InstanceId",
        "PureDate",
        "Arc<",
        "SmolStr",
    ];
    let mut seen_any_value = false;
    for nodes in tree.nodes.values() {
        for node in nodes {
            seen_any_value = true;
            for tok in banned {
                assert!(
                    !node.value.contains(tok),
                    "banned token {tok:?} leaked into node {:?}: value={:?}",
                    node.name,
                    node.value
                );
            }
        }
    }
    assert!(seen_any_value, "snapshot should contain at least one node");
}

#[test]
fn rendering_does_not_re_enter_pause() {
    // The renderer calls toRepresentation for each leaf. Each such
    // call enters Evaluator::eval, which dispatches before_eval. The
    // rendering_depth guard must skip that hook — otherwise the
    // renderer would request another snapshot at the same line and
    // recurse forever. This test passing without stack overflow is
    // the actual assertion; we also verify exactly one snapshot
    // was captured.
    let source = "\
function test::f(): Integer[1] {
    let x = 42;
    $x + 1
}
";
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let hooks = CaptureAtLine::new(3, "/user_test/test_source.pure");
    let mut eval = Evaluator::with_hooks(&model, &registry, hooks);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), "f__Integer_1_".into()])
        .unwrap();
    let _ = eval.call_user_function_by_id(fn_id).unwrap();
    let captured = eval.into_hooks();
    assert!(captured.captured.is_some(), "must capture exactly one tree");
}
