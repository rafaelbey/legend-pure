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

//! End-to-end evaluation tests: compile Pure → evaluate → assert Value.
//!
//! These tests exercise the full pipeline: parse platform + user Pure →
//! compile → evaluate. The platform Pure files provide the standard library
//! (plus, equal, if, map, etc.), so the compiler resolves operator calls
//! to their platform Function `ElementId`s, and FQN mangling produces the
//! correct native registry keys.
//!
//! # Test progression
//!
//! 1. Literal evaluation (int, float, string, bool)
//! 2. Arithmetic and comparison operators
//! 3. Variables (`let`)
//! 4. Control flow (`if`)
//! 5. Collections and lambdas (`map`, `filter`, `fold`)
//! 6. User-defined functions
//! 7. Assertions (prerequisite for PCT)

use std::sync::OnceLock;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use smol_str::SmolStr;

/// Initialise `tracing-subscriber` once per test process so the
/// `tracing::debug!` / `tracing::warn!` events emitted by the
/// compiler (`resolve_function_call`, `narrow_candidates_by_type`,
/// `reactivate_value`, etc.) surface in test output when the user
/// sets `RUST_LOG`. Default filter is `error` (silent); override
/// with e.g.
///     `RUST_LOG=legend_pure_parser_pure::resolve=debug` cargo test ...
/// to trace overload resolution for a specific failing test.
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

/// Cached platform repos + auto-imports.
///
/// Phase 3b: only `platform` is embedded as `.purem`; DSLs are loaded
/// from `LEGEND_PURE_BUILD_SNAPSHOTS_DIR` (set by the build script).
/// We cache the resolved `Vec<Repo>` once; each test clones it,
/// appends a synthetic user-source repo, and runs `repo::load` for a
/// fresh model.
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

struct PlatformFixture {
    repos: Vec<legend_pure_core_platform::repo::Repo>,
    auto_imports: Vec<SmolStr>,
}

/// Build a synthetic `Repo::Filesystem` repo carrying the user source
/// pretended to live at `/user_test/<test>.pure`. Declared dependencies
/// cover every embedded + artifact repo so cross-repo references in
/// the user source resolve cleanly.
fn synthetic_user_repo(user_source: &str) -> legend_pure_core_platform::repo::Repo {
    use legend_pure_core_platform::repo::{OwnedSourceFile, RepoMeta};
    // Leak a static deps slice covering every standard repo.
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

/// Compile platform + user Pure source and evaluate a function by its
/// mangled FQN within the `test` package.
///
/// The `fqn` parameter is the mangled function name as produced by Pass 2.1,
/// e.g. `"f__Integer_1_"` for `function test::f(): Integer[1]`.
fn eval_pure(source: &str, fqn: &str) -> Value {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);

    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));

    eval.call_user_function_by_id(fn_id)
        .unwrap_or_else(|e| panic!("Evaluation error: {e}"))
}

/// Like `eval_pure` but expects an evaluation error.
fn eval_pure_err(source: &str, fqn: &str) -> String {
    let model = compile_with_platform(source);
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);

    let fn_id = model
        .resolve_by_fqn(&["test".into(), fqn.into()])
        .unwrap_or_else(|| panic!("Function test::{fqn} not found in model"));

    eval.call_user_function_by_id(fn_id)
        .expect_err("Expected evaluation to fail")
        .to_string()
}

/// Compile user test source together with the platform model.
fn compile_with_platform(user_source: &str) -> PureModel {
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    let result = legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports);
    match result {
        Ok(model) => model,
        Err(partial) => {
            // Debug: dump errors so test failures are diagnosable.
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

// ===========================================================================
// 0. Compile-time property validation against the loaded platform
// ===========================================================================

/// Re-runs platform compile WITHOUT swallowing errors, so the test can
/// assert on the error set. Mirrors `compile_with_platform` but returns
/// the `PartialPureModel` directly when compilation produces errors.
#[allow(clippy::result_large_err)] // PartialPureModel is intentionally rich for diagnostics
fn try_compile_with_platform(
    user_source: &str,
) -> Result<PureModel, legend_pure_parser_pure::pipeline::PartialPureModel> {
    let fixture = platform_model();
    let mut repos: Vec<_> = fixture.repos.clone();
    repos.push(synthetic_user_repo(user_source));
    legend_pure_core_platform::repo::load(&repos, &fixture.auto_imports)
}

#[test]
fn compile_pair_first_type_is_unknown_property() {
    // Locks the user-reported bug: against the platform's `Pair<U,V>` (in
    // anonymousCollections.pure, chunk 1+), accessing the non-existent
    // property `firstType` must produce exactly one `UnknownProperty`
    // error. The valid sibling `pair(1,3).first` still compiles cleanly
    // (separate test below).
    use legend_pure_parser_pure::error::CompilationErrorKind;
    let source = "function test::f(): Any[*] { pair(1, 3).firstType }";
    let partial = try_compile_with_platform(source)
        .expect_err("missing property `firstType` must produce a compile error");
    let unknown_on_pair: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| match &e.kind {
            CompilationErrorKind::UnknownProperty {
                type_name,
                property_name,
            } => type_name.as_str() == "Pair" && property_name.as_str() == "firstType",
            _ => false,
        })
        .collect();
    assert_eq!(
        unknown_on_pair.len(),
        1,
        "expected exactly one `Pair.firstType` UnknownProperty, got: {:?}",
        partial
            .errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::UnknownProperty { .. }))
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_pair_first_compiles_clean() {
    // Positive regression for the user-reported bug: the valid sibling
    // `pair(1,3).first` must still compile (no UnknownProperty fires).
    // Scopes the assertion to the user's source ("<test>") — platform
    // residuals (e.g. dispatcher narrowing on parameterized supertype
    // overloads in `relationalRuntime.pure`'s `elementToPath` calls) are
    // tracked separately in BACKLOG and don't represent a regression of
    // user-code correctness.
    let source = "function test::f(): Integer[1] { pair(1, 3).first }";
    let user_errors: Vec<_> = match try_compile_with_platform(source) {
        Ok(_) => Vec::new(),
        Err(p) => p
            .errors
            .into_iter()
            .filter(|e| e.source_info.source == "<test>")
            .collect(),
    };
    assert!(
        user_errors.is_empty(),
        "valid `Pair.first` must still compile; user errors: {user_errors:?}",
    );
}

#[test]
fn deactivate_property_access_produces_simple_function_expression() {
    // Java-parity check on the metamodel shape `deactivate` produces
    // for a simple property access. After Option A unification:
    //   parse → ExprKind::PropertyCall(FunctionCallData { ... })
    //   deactivate → SimpleFunctionExpression with `_propertyName`
    //                populated and `_func` carrying a synthesized
    //                Property heap wrapper.
    //
    // Pre-fix this would have wrapped the evaluated value as an
    // InstanceValue (the catch-all `_ =>` arm), so reflection like
    // `$f->cast(@SimpleFunctionExpression).propertyName.values->toOne()`
    // wouldn't have found the slot.
    let source = r"
        Class test::User { lastName: String[1]; }
        function test::f(u: test::User[1]): String[1] {
            let f = $u.lastName->deactivate();
            $f->cast(@SimpleFunctionExpression).propertyName.values
              ->toOne()->cast(@String)->toOne()
        }
    ";
    let result = eval_pure(source, "f_User_1__String_1_");
    assert_eq!(result, Value::String("lastName".into()));
}

#[test]
fn deactivate_property_access_on_collection_rewrites_to_map() {
    // Java-parity check on the automap rewrite. When the receiver
    // multiplicity is non-strictly-toOne (`[*]`, `[0..1]`, `[1..*]`),
    // inference rewrites `$x.aliases` in place to
    // `map($x.aliases_collection, λ{v_automap | $v_automap.something})`.
    // Here we test the simpler shape: `$u.aliases` where aliases:[*]
    // doesn't itself rewrite (it's the FIRST property access, with a
    // toOne `$u`), so we exercise the chained case `$u.aliases.length`
    // implicitly through the platform's ConcreteFunctionDefinition.all
    // pattern, but here we just check that compiling a simple [*]
    // accessor produces the expected metamodel shape.
    //
    // This test checks: a property access whose RECEIVER is multi-valued
    // gets rewritten to `map`. We construct that scenario by using
    // `pair(1,3).first` (returns Integer[1]) — that's still toOne, no
    // rewrite. The actual chained shape `$col.field` requires a
    // platform helper — defer that as a follow-up. For now, lock in
    // that the deactivate path produces SimpleFunctionExpression with
    // `_functionName = 'map'` when the rewrite fires.
    //
    // To avoid building a full platform-dependent multi-valued source
    // here, this test currently uses `let xs = [1, 2]; $xs.foo` —
    // BUT primitive integers don't have a `.foo` property. So we use
    // a property of a class on a multi-valued receiver via a chain
    // through the platform's `ConcreteFunctionDefinition.all`.
    let source = r"
        Class test::User { firstName: String[1]; }
        function test::f(): String[1] {
            let users = [^test::User(firstName='a'), ^test::User(firstName='b')];
            // $users is User[*]; $users.firstName triggers automap rewrite.
            let f = $users.firstName->deactivate();
            $f->cast(@SimpleFunctionExpression).functionName->toOne()
        }
    ";
    let result = eval_pure(source, "f__String_1_");
    assert_eq!(
        result,
        Value::String("map".into()),
        "multi-valued property access must rewrite to a `map` SFE"
    );
}

#[test]
fn compile_typo_on_generic_type_is_unknown_property() {
    // User-reported gap: `pair(1,2)->genericType().rawTyp` (typo of
    // `.rawType`) on a GenericType receiver compiled clean instead of
    // erroring. The previous skip predicate suppressed validation for
    // ALL chunk-0 classes — too aggressive for non-parametric M3
    // classes like `GenericType`, `Property`, `FunctionType`, which
    // are real types with declared properties.
    //
    // Refined predicate: skip only `Any` and parametric M3 metatype
    // carriers (`Class<T>`, `Enumeration<T>`, `Function<...>`).
    // `GenericType` is non-parametric — validation now fires.
    use legend_pure_parser_pure::error::CompilationErrorKind;
    let source = r"
        function test::f(): Any[*] {
            pair(1, 2)->genericType().rawTyp
        }
    ";
    let partial = try_compile_with_platform(source)
        .expect_err(".rawTyp on a GenericType receiver must produce a compile error");
    let unknown: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| match &e.kind {
            CompilationErrorKind::UnknownProperty {
                type_name,
                property_name,
            } => type_name.as_str() == "GenericType" && property_name.as_str() == "rawTyp",
            _ => false,
        })
        .collect();
    assert_eq!(
        unknown.len(),
        1,
        "expected exactly one `GenericType.rawTyp` UnknownProperty, got: {:?}",
        partial
            .errors
            .iter()
            .filter(|e| matches!(e.kind, CompilationErrorKind::UnknownProperty { .. }))
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_known_property_on_generic_type_compiles_clean() {
    // Positive regression: the valid `.rawType` access still compiles.
    // Scopes the assertion to the user's source ("<test>") — platform
    // residuals don't represent a user-code regression. See
    // `compile_pair_first_compiles_clean` above for the same pattern.
    let source = r"
        function test::f(): Any[*] {
            pair(1, 2)->genericType().rawType
        }
    ";
    let user_errors: Vec<_> = match try_compile_with_platform(source) {
        Ok(_) => Vec::new(),
        Err(p) => p
            .errors
            .into_iter()
            .filter(|e| e.source_info.source == "<test>")
            .collect(),
    };
    assert!(
        user_errors.is_empty(),
        "`.rawType` on GenericType must compile; user errors: {user_errors:?}",
    );
}

#[test]
fn pair_generic_type_args_match_call_site_substitution_first() {
    // User-reported gap: `pair(1, '')->genericType().typeArguments` was
    // empty because the body's `^Pair<U,V>(...)` saw Generic
    // placeholders. Pass 2.5 inference substitutes U → Integer, V →
    // String into the OUTER call's `expr.type_info` (mirroring
    // Java's `_genericType` slot). The runtime now reads that
    // type_info after the body returns and back-fills
    // `__typeArguments` onto the heap object.
    let source = r"
        function test::f(): String[1] {
            pair(1, 'hello')->genericType().typeArguments->at(0).rawType->toOne()->id()
        }
    ";
    let result = eval_pure(source, "f__String_1_");
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn pair_generic_type_args_match_call_site_substitution_second() {
    let source = r"
        function test::f(): String[1] {
            pair(1, 'hello')->genericType().typeArguments->at(1).rawType->toOne()->id()
        }
    ";
    let result = eval_pure(source, "f__String_1_");
    assert_eq!(result, Value::String("String".into()));
}

#[test]
fn passthrough_generic_function_preserves_existing_type_args() {
    // `myId<T>(x:T[1]):T[1] { $x }` returning a Pair created by
    // `pair(1,2)`: the back-fill's empty-slot guard must skip writing
    // because pair's outer-call back-fill already populated the slot
    // with [Integer, Integer]. Without the guard, the outer myId call's
    // back-fill would attempt to overwrite (with the same data — so
    // observed result is the same). The guard avoids the wasted write
    // and preserves correctness for any future case where T is bound
    // to a wider type than the inner construction.
    let source = r"
        function <<test.Test>> test::myId<T>(x: T[1]): T[1] { $x }
        function test::f(): String[1] {
            test::myId(pair(1, 2))->genericType().typeArguments->at(0).rawType->toOne()->id()
        }
    ";
    let result = eval_pure(source, "f__String_1_");
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn deactivate_property_access_func_carries_property_wrapper() {
    // The synthesized Property wrapper on `_func` exposes
    // `name` (the property name) and `_owner` (the receiver class).
    // This locks in the parity with Java's `_func: Property` slot.
    let source = r"
        Class test::User { lastName: String[1]; }
        function test::f(u: test::User[1]): String[1] {
            let f = $u.lastName->deactivate();
            $f->cast(@SimpleFunctionExpression).func->toOne()
              ->cast(@meta::pure::metamodel::function::property::Property<Nil,Any|*>).name->toOne()
        }
    ";
    let result = eval_pure(source, "f_User_1__String_1_");
    assert_eq!(result, Value::String("lastName".into()));
}

// ===========================================================================
// 1. Literals
// ===========================================================================

#[test]
fn eval_integer_literal() {
    let result = eval_pure("function test::f(): Integer[1] { 42 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn evaluator_new_default_uses_standard_registry() {
    // Locks Phase 4c: `Evaluator::new_default(model)` builds an
    // evaluator backed by the process-wide default
    // `NativeRegistry::standard()` without the caller threading a
    // registry through. A trivial call must dispatch the `plus` native
    // to confirm the registry is wired.
    use legend_pure_runtime::eval::Evaluator;
    let model = compile_with_platform("function test::add(): Integer[1] { 1 + 2 }");
    let mut evaluator = Evaluator::new_default(&model);
    let result = evaluator
        .call("test::add__Integer_1_", &[])
        .expect("call must dispatch via default registry");
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn store_dsl_metamodel_resolves_in_platform() {
    // Locks the Store DSL embedding: the platform_dsl_store repo
    // ships two metamodel files (`grammar/store.pure`,
    // `grammar/runtime.pure`) defining `meta::pure::store::Store`,
    // `meta::pure::store::set::*`, and `meta::core::runtime::*`.
    // After the build script picks them up, every named class must
    // resolve to a Class element in the compiled model.
    //
    // Until the descriptor-driven loader lands, these files are
    // hand-embedded via `crates/core-platform-pure/build.rs`. This
    // test catches a regression where the embedding is silently
    // dropped, or where one of the M2 classes fails to compile
    // against the M3 base model.
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    for fqn_segments in [
        ["meta", "pure", "store", "Store"].as_slice(),
        ["meta", "pure", "store", "set", "SetBasedStore"].as_slice(),
        ["meta", "pure", "store", "set", "Namespace"].as_slice(),
        ["meta", "pure", "store", "set", "SetRelation"].as_slice(),
        ["meta", "pure", "store", "set", "SetColumn"].as_slice(),
        ["meta", "core", "runtime", "Runtime"].as_slice(),
        ["meta", "core", "runtime", "ConnectionStore"].as_slice(),
        ["meta", "core", "runtime", "Connection"].as_slice(),
        ["meta", "pure", "runtime", "ExecutionContext"].as_slice(),
    ] {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Store DSL element missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Class(_)),
            "Store DSL element is not a Class: {fqn_segments:?}",
        );
    }
}

#[test]
fn path_dsl_metamodel_resolves_in_platform() {
    // Locks the Path DSL embedding (Stage 2): the platform_dsl_path
    // repo ships a single `path.pure` file with three classes:
    //   - meta::pure::metamodel::path::Path<-U,V|m> extends Function<{U[1]→V[m]}>
    //   - meta::pure::metamodel::path::PathElement
    //   - meta::pure::metamodel::path::PropertyPathElement extends PathElement
    //   - meta::pure::metamodel::path::CastPathElement extends PathElement
    //
    // Stage 3 (compiler lowering) and Stage 4 (runtime evaluate)
    // both depend on these resolving as Class elements in the
    // compiled model — without this they have nothing to construct
    // against. This test catches a regression where the embedding
    // is silently dropped or where one of the classes fails to
    // compile against the M3 base model (e.g. variance prefix
    // not parsed, contravariant `<-U>` rejected).
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    for fqn_segments in [
        ["meta", "pure", "metamodel", "path", "Path"].as_slice(),
        ["meta", "pure", "metamodel", "path", "PathElement"].as_slice(),
        ["meta", "pure", "metamodel", "path", "PropertyPathElement"].as_slice(),
        ["meta", "pure", "metamodel", "path", "CastPathElement"].as_slice(),
    ] {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Path DSL element missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Class(_)),
            "Path DSL element is not a Class: {fqn_segments:?}",
        );
    }
}

#[test]
fn diagram_dsl_metamodel_resolves_in_platform() {
    // Locks the Diagram DSL embedding: 150 LOC of metamodel in
    // `platform_dsl_diagram/diagram.pure` defining the visual-graph
    // shapes (Diagram, TypeView, AssociationView, …) plus four
    // Associations that wire Diagram to its child views.
    //
    // Like Store, Diagram has no top-level grammar of its own at the
    // metamodel layer — it's plain Pure. The `###Diagram` user
    // syntax (for user-authored diagrams) is a separate parser concern.
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    let classes = [
        ["meta", "pure", "diagram", "DiagramNode"].as_slice(),
        ["meta", "pure", "diagram", "Visibility"].as_slice(),
        ["meta", "pure", "diagram", "AttributeVisibility"].as_slice(),
        ["meta", "pure", "diagram", "TypeVisibility"].as_slice(),
        ["meta", "pure", "diagram", "AssociationVisibility"].as_slice(),
        ["meta", "pure", "diagram", "Rendering"].as_slice(),
        ["meta", "pure", "diagram", "Point"].as_slice(),
        ["meta", "pure", "diagram", "Geometry"].as_slice(),
        ["meta", "pure", "diagram", "RectangleGeometry"].as_slice(),
        ["meta", "pure", "diagram", "LineGeometry"].as_slice(),
        ["meta", "pure", "diagram", "AbstractPathView"].as_slice(),
        ["meta", "pure", "diagram", "PropertyView"].as_slice(),
        ["meta", "pure", "diagram", "AssociationPropertyView"].as_slice(),
        ["meta", "pure", "diagram", "AssociationView"].as_slice(),
        ["meta", "pure", "diagram", "GeneralizationView"].as_slice(),
        ["meta", "pure", "diagram", "TypeView"].as_slice(),
        ["meta", "pure", "diagram", "Diagram"].as_slice(),
    ];
    for fqn_segments in classes {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Diagram DSL class missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Class(_)),
            "Diagram DSL element is not a Class: {fqn_segments:?}",
        );
    }
    let assocs = [
        ["meta", "pure", "diagram", "DiagramTypeViews"].as_slice(),
        ["meta", "pure", "diagram", "DiagramAssociationViews"].as_slice(),
        ["meta", "pure", "diagram", "DiagramPropertyViews"].as_slice(),
        ["meta", "pure", "diagram", "DiagramGeneralizationViews"].as_slice(),
    ];
    for fqn_segments in assocs {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Diagram DSL association missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Association(_)),
            "Diagram DSL element is not an Association: {fqn_segments:?}",
        );
    }
    let id = model
        .resolve_by_path(&[
            "meta".into(),
            "pure".into(),
            "diagram".into(),
            "LineStyle".into(),
        ])
        .expect("LineStyle enum missing");
    assert!(
        matches!(model.get_element(id), Element::Enumeration(_)),
        "LineStyle is not an Enumeration"
    );
}

#[test]
fn relational_metamodel_resolves_in_platform() {
    // Locks the platform_store_relational embedding: `relational.pure`
    // (~80 metamodel classes) and `relationalMapping.pure` (~10 mapping
    // classes) must resolve cleanly against the M3 + Mapping + Store
    // base models. After the build script picks them up, every named
    // class must resolve to a Class (or Enum) element in the compiled
    // model.
    //
    // This catches regressions where the embed is silently dropped, a
    // dependency repo is missing from the descriptor, or one of the M2
    // classes fails to compile against its declared supertypes.
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    let classes: &[&[&str]] = &[
        // metamodel core
        &["meta", "relational", "metamodel", "Database"],
        &["meta", "relational", "metamodel", "Schema"],
        &["meta", "relational", "metamodel", "Filter"],
        &["meta", "relational", "metamodel", "MultiGrainFilter"],
        &["meta", "relational", "metamodel", "Column"],
        &["meta", "relational", "metamodel", "TableAlias"],
        &["meta", "relational", "metamodel", "TableAliasColumn"],
        &["meta", "relational", "metamodel", "Alias"],
        &["meta", "relational", "metamodel", "ColumnName"],
        &["meta", "relational", "metamodel", "Literal"],
        &["meta", "relational", "metamodel", "LiteralList"],
        &["meta", "relational", "metamodel", "OrderBy"],
        &["meta", "relational", "metamodel", "Window"],
        &["meta", "relational", "metamodel", "WindowColumn"],
        &["meta", "relational", "metamodel", "Frame"],
        &["meta", "relational", "metamodel", "SQLQuery"],
        &["meta", "relational", "metamodel", "SQLNull"],
        // relations
        &["meta", "relational", "metamodel", "relation", "Relation"],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "NamedRelation",
        ],
        &["meta", "relational", "metamodel", "relation", "Table"],
        &["meta", "relational", "metamodel", "relation", "View"],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "TabularFunction",
        ],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "SelectSQLQuery",
        ],
        // joins
        &["meta", "relational", "metamodel", "join", "Join"],
        &["meta", "relational", "metamodel", "join", "AsOfJoin"],
        &["meta", "relational", "metamodel", "join", "JoinTreeNode"],
        &[
            "meta",
            "relational",
            "metamodel",
            "join",
            "RootJoinTreeNode",
        ],
        // milestoning
        &["meta", "relational", "metamodel", "relation", "Milestoning"],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "TemporalMilestoning",
        ],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "BusinessMilestoning",
        ],
        &[
            "meta",
            "relational",
            "metamodel",
            "relation",
            "ProcessingMilestoning",
        ],
        // operations
        &["meta", "relational", "metamodel", "operation", "Operation"],
        &[
            "meta",
            "relational",
            "metamodel",
            "operation",
            "BinaryOperation",
        ],
        &[
            "meta",
            "relational",
            "metamodel",
            "operation",
            "UnaryOperation",
        ],
        &[
            "meta",
            "relational",
            "metamodel",
            "operation",
            "VariableArityOperation",
        ],
        &["meta", "relational", "metamodel", "DynaFunction"],
        // datatypes
        &["meta", "relational", "metamodel", "datatype", "DataType"],
        &[
            "meta",
            "relational",
            "metamodel",
            "datatype",
            "CoreDataType",
        ],
        &["meta", "relational", "metamodel", "datatype", "Integer"],
        &["meta", "relational", "metamodel", "datatype", "Varchar"],
        &["meta", "relational", "metamodel", "datatype", "Decimal"],
        &["meta", "relational", "metamodel", "datatype", "Date"],
        &["meta", "relational", "metamodel", "datatype", "Timestamp"],
        // mapping
        &[
            "meta",
            "relational",
            "mapping",
            "RelationalInstanceSetImplementation",
        ],
        &[
            "meta",
            "relational",
            "mapping",
            "RootRelationalInstanceSetImplementation",
        ],
        &[
            "meta",
            "relational",
            "mapping",
            "EmbeddedRelationalInstanceSetImplementation",
        ],
        &[
            "meta",
            "relational",
            "mapping",
            "InlineEmbeddedRelationalInstanceSetImplementation",
        ],
        &[
            "meta",
            "relational",
            "mapping",
            "OtherwiseEmbeddedRelationalInstanceSetImplementation",
        ],
        &["meta", "relational", "mapping", "RelationalPropertyMapping"],
        &[
            "meta",
            "relational",
            "mapping",
            "RelationalAssociationImplementation",
        ],
        &["meta", "relational", "mapping", "FilterMapping"],
        &["meta", "relational", "mapping", "GroupByMapping"],
        // runtime
        &[
            "meta",
            "external",
            "store",
            "relational",
            "runtime",
            "DatabaseConnection",
        ],
        &[
            "meta",
            "external",
            "store",
            "relational",
            "runtime",
            "TestDatabaseConnection",
        ],
        &["meta", "relational", "runtime", "DataSource"],
        &["meta", "relational", "runtime", "PostProcessor"],
    ];
    for fqn_segments in classes {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Relational class missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Class(_)),
            "Relational element is not a Class: {fqn_segments:?}",
        );
    }
    // enums
    for fqn_segments in [
        ["meta", "relational", "metamodel", "join", "JoinType"].as_slice(),
        ["meta", "relational", "metamodel", "SortDirection"].as_slice(),
        ["meta", "relational", "metamodel", "FrameType"].as_slice(),
        ["meta", "relational", "metamodel", "FrameValueDirection"].as_slice(),
        ["meta", "relational", "runtime", "DatabaseType"].as_slice(),
    ] {
        let segments: Vec<smol_str::SmolStr> = fqn_segments
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("Relational enum missing: {fqn_segments:?}"));
        assert!(
            matches!(model.get_element(id), Element::Enumeration(_)),
            "Relational element is not an Enumeration: {fqn_segments:?}",
        );
    }
}

#[test]
fn eval_negative_integer() {
    let result = eval_pure("function test::f(): Integer[1] { -7 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(-7));
}

#[test]
fn eval_string_literal() {
    let result = eval_pure(
        "function test::f(): String[1] { 'hello world' }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn eval_boolean_true() {
    let result = eval_pure("function test::f(): Boolean[1] { true }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_boolean_false() {
    let result = eval_pure("function test::f(): Boolean[1] { false }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_float_literal() {
    let result = eval_pure("function test::f(): Float[1] { 2.5 }", "f__Float_1_");
    assert_eq!(result, Value::Float(2.5));
}

// ===========================================================================
// 2. Arithmetic operators
// ===========================================================================

#[test]
fn eval_addition() {
    let result = eval_pure("function test::f(): Integer[1] { 1 + 2 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn eval_subtraction() {
    let result = eval_pure("function test::f(): Integer[1] { 10 - 3 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(7));
}

#[test]
fn eval_multiplication() {
    let result = eval_pure("function test::f(): Integer[1] { 6 * 7 }", "f__Integer_1_");
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_complex_arithmetic() {
    let result = eval_pure(
        "function test::f(): Integer[1] { 2 + 3 * 4 }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(14));
}

// ===========================================================================
// 3. Comparison operators
// ===========================================================================

#[test]
fn eval_equal_true() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 == 1 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_equal_false() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 == 2 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_less_than() {
    let result = eval_pure("function test::f(): Boolean[1] { 1 < 2 }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(true));
}

// ===========================================================================
// 4. Boolean operators
// ===========================================================================

#[test]
fn eval_and() {
    let result = eval_pure(
        "function test::f(): Boolean[1] { true && false }",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_or() {
    let result = eval_pure(
        "function test::f(): Boolean[1] { false || true }",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_not() {
    let result = eval_pure("function test::f(): Boolean[1] { !true }", "f__Boolean_1_");
    assert_eq!(result, Value::Boolean(false));
}

// ===========================================================================
// 5. String operations
// ===========================================================================

#[test]
fn eval_string_concat() {
    let result = eval_pure(
        "function test::f(): String[1] { 'hello' + ' ' + 'world' }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("hello world".into()));
}

// ===========================================================================
// 6. Variables (let)
// ===========================================================================

#[test]
fn eval_let_binding() {
    let result = eval_pure(
        "function test::f(): Integer[1] { let x = 42; $x; }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_let_with_arithmetic() {
    let result = eval_pure(
        "function test::f(): Integer[1] { let x = 10; let y = 20; $x + $y; }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(30));
}

// ===========================================================================
// 7. Control flow (if)
// ===========================================================================

#[test]
fn eval_if_true_branch() {
    let result = eval_pure(
        "function test::f(): Integer[1] { if(true, |1, |2) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn eval_if_false_branch() {
    let result = eval_pure(
        "function test::f(): Integer[1] { if(false, |1, |2) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn eval_if_with_expression() {
    let result = eval_pure(
        "function test::f(): String[1] { if(1 == 1, |'yes', |'no') }",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("yes".into()));
}

// ===========================================================================
// 8. Collections
// ===========================================================================

#[test]
fn eval_collection_literal() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3] }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(1));
            assert_eq!(v[1], Value::Integer(2));
            assert_eq!(v[2], Value::Integer(3));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

// ===========================================================================
// 9. User-defined function calls
// ===========================================================================

#[test]
fn eval_call_user_function() {
    let result = eval_pure(
        r"
        function test::double(x: Integer[1]): Integer[1] { $x * 2 }
        function test::f(): Integer[1] { test::double(21) }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_recursive_function() {
    let result = eval_pure(
        r"
        function test::factorial(n: Integer[1]): Integer[1] {
            if($n == 0, |1, |$n * test::factorial($n - 1))
        }
        function test::f(): Integer[1] { test::factorial(5) }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(120));
}

// ===========================================================================
// 10. Lambda + higher-order (map, filter, fold)
// ===========================================================================

#[test]
fn eval_map_lambda() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3]->map(x | $x * 2) }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(2));
            assert_eq!(v[1], Value::Integer(4));
            assert_eq!(v[2], Value::Integer(6));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_filter_lambda() {
    let result = eval_pure(
        "function test::f(): Integer[*] { [1, 2, 3, 4, 5]->filter(x | $x > 3) }",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 2);
            assert_eq!(v[0], Value::Integer(4));
            assert_eq!(v[1], Value::Integer(5));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_fold_sum() {
    let result = eval_pure(
        "function test::f(): Integer[1] { [1, 2, 3, 4, 5]->fold({x, acc | $acc + $x}, 0) }",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(15));
}

// ===========================================================================
// 11. Combined: approaching Pure test complexity
// ===========================================================================

#[test]
fn eval_let_with_if_and_arithmetic() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            let x = 10;
            let y = 20;
            if($x + $y == 30, |'correct', |'wrong');
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("correct".into()));
}

// ===========================================================================
// 12. Advanced Variable Shadowing & Closures (TDD)
// ===========================================================================

#[test]

fn eval_lambda_let_shadows_outer_let() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 10;
            [1, 2, 3]->map(y | 
                let x = 20;
                $x + $y
            );
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(21));
            assert_eq!(v[1], Value::Integer(22));
            assert_eq!(v[2], Value::Integer(23));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]

fn eval_variable_shadowing_lambda() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 10;
            [1, 2, 3]->map(x | $x * 2);
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(2));
            assert_eq!(v[1], Value::Integer(4));
            assert_eq!(v[2], Value::Integer(6));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_closure_captures_outer_scope() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let multiplier = 10;
            let base = 5;
            [1, 2, 3]->map(x | ($x * $multiplier) + $base);
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], Value::Integer(15));
            assert_eq!(v[1], Value::Integer(25));
            assert_eq!(v[2], Value::Integer(35));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

#[test]
fn eval_closure_captures_and_binds_inner_let() {
    let result = eval_pure(
        r"
        function test::f(): Integer[*] {
            let x = 100;
            [1, 2]->map(y |
                let inner = $x;
                $inner + $y
            );
        }
        ",
        "f__Integer_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 2);
            assert_eq!(v[0], Value::Integer(101));
            assert_eq!(v[1], Value::Integer(102));
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

// ===========================================================================
// 8. Meta-model natives: pathToElement / elementToPath / element properties
// ===========================================================================

#[test]
fn eval_path_to_element_resolves_package() {
    // Resolve a package from its qualified path and then round-trip via
    // elementToPath — the 1-arg Pure wrapper forwards to the 3-arg native.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure".into()));
}

#[test]
fn eval_path_to_element_resolves_nested_package() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure::metamodel::type', '::')->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure::metamodel::type".into()));
}

#[test]
fn eval_element_name_package() {
    // $pkg.name returns the simple package name (scalar in the M3 metamodel).
    let result = eval_pure(
        r"
        function test::f(): String[*] {
            pathToElement('meta::pure', '::').name;
        }
        ",
        "f__String_MANY_",
    );
    // Check that we got either a single-value or a collection containing 'pure'.
    match result {
        Value::String(s) => assert_eq!(s.as_str(), "pure"),
        Value::Collection(v) => {
            assert_eq!(v.len(), 1, "expected 1 element, got {}", v.len());
            assert_eq!(v[0], Value::String("pure".into()));
        }
        other => panic!("Expected String or Collection, got {other:?}"),
    }
}

#[test]
fn eval_package_children_includes_sub_packages() {
    // meta::pure has many sub-packages (metamodel, functions, test, ...).
    // Just assert the collection is non-empty.
    let result = eval_pure(
        r"
        function test::f(): Integer[1] {
            pathToElement('meta::pure', '::').children->size();
        }
        ",
        "f__Integer_1_",
    );
    match result {
        Value::Integer(n) => assert!(n > 0, "expected non-empty children, got {n}"),
        other => panic!("Expected Integer, got {other:?}"),
    }
}

#[test]
fn eval_package_children_excludes_units() {
    // M3: Units live under their parent Measure, not the package.
    // `RomanLength~Pes` is registered in `meta::pure::functions::meta::tests::model`
    // for type-position resolution, but reflective `package.children`
    // must skip it (Java semantics). Navigate Units via
    // `RomanLength.canonicalUnit` / `.nonCanonicalUnits` instead.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            let pkg = pathToElement('meta::pure::functions::meta::tests::model', '::');
            $pkg.children->forAll(c | !$c.name->contains('~'));
        }
        ",
        "f__Boolean_1_",
    );
    match result {
        Value::Boolean(true) => {}
        other => panic!("expected true (no Unit in children), got {other:?}"),
    }
}

#[test]
fn eval_element_to_path_with_custom_separator() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            elementToPath(pathToElement('meta::pure', '::'), '.', false);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta.pure".into()));
}

#[test]
fn eval_element_to_path_include_root() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            elementToPath(pathToElement('meta::pure', '::'), '::', true);
        }
        ",
        "f__String_1_",
    );
    // includeRoot=true prepends the literal "Root" segment (matches the
    // Java Pure runtime's rendering of the unnamed root package).
    assert_eq!(result, Value::String("Root::meta::pure".into()));
}

#[test]
fn eval_surveyor_entry_point_runs_to_completion() {
    // End-to-end proof that the Pure-native surveyor can walk the compiled
    // platform: discovery → match/instanceOf dispatch → ^Class(...) object
    // construction → flatten → aggregate → `^TestReport(...)` return.
    //
    // The collection package has `<<test.Test>>`-stereotyped functions that
    // rely on `assertEquals`/`joinStrings`/etc. (not all implemented yet), so
    // many will bucket as ERROR. The regression lock is structural: the
    // surveyor must return a `TestReport` with all five counter fields
    // populated to non-negative integers that sum to the total.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::collection".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let report_id = match report {
        Value::Object(id) => id,
        other => panic!("surveyor returned non-object: {other:?}"),
    };

    let heap = evaluator.heap();
    assert_eq!(
        heap.classifier(&report_id).unwrap(),
        "meta::pure::test::surveyor::TestReport"
    );

    // Each counter is a single Integer >= 0.
    let read_counter = |name: &str| -> i64 {
        let values = heap.get_property_values(&report_id, name).unwrap();
        let collected: Vec<_> = values.iter().cloned().collect();
        assert_eq!(
            collected.len(),
            1,
            "expected one Integer for {name}, got {collected:?}"
        );
        match collected[0] {
            Value::Integer(n) => n,
            ref other => panic!("expected Integer for {name}, got {other:?}"),
        }
    };
    let pass = read_counter("passCount");
    let fail = read_counter("failCount");
    let error = read_counter("errorCount");
    let skip = read_counter("skipCount");
    for (name, v) in [
        ("passCount", pass),
        ("failCount", fail),
        ("errorCount", error),
        ("skipCount", skip),
    ] {
        assert!(v >= 0, "{name} should be non-negative, got {v}");
    }

    // `results` length must equal pass + fail + error + skip — every test
    // outcome is classified into exactly one bucket.
    let results = heap.get_property_values(&report_id, "results").unwrap();
    let total = i64::try_from(results.len()).expect("results length fits in i64");
    assert_eq!(
        total,
        pass + fail + error + skip,
        "results length ({total}) must equal pass+fail+error+skip"
    );
}

#[test]
fn eval_cast_routes_through_prefix_fallback_despite_name_mangle_drift() {
    // The `cast` native is registered as `cast_Any_m__V_1__V_m_` but the
    // compiler mangles Pure-level `cast` call sites differently (e.g., T vs V
    // type-variable naming). This must still route via the simple-name
    // prefix fallback. Regressing here would silently break `cast` in any
    // platform function that calls it.
    let result = eval_pure(
        r"
        function test::f(): Integer[1] {
            42->cast(@Integer);
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn eval_instance_of_with_type_reference() {
    // @X evaluates to Value::Element after the TypeReference change —
    // instanceOf must accept it as its type argument.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            42->instanceOf(@Integer);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_match_dispatches_to_first_matching_lambda() {
    // A simple match that distinguishes Integer from String. We feed
    // an integer literal via a trivial wrapping — the lambda's declared
    // parameter type drives dispatch.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->match([
                i: Integer[1] | 'int',
                s: String[1] | 'str'
            ]);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("int".into()));
}

// ===========================================================================
// 9. Meta / Reflection (id, type, genericType, rawType, enumName,
//    enumValues, toRepresentation, subTypeOf)
// ===========================================================================

#[test]
fn eval_type_of_integer_resolves_to_integer_primitive() {
    // type(42) must return the bootstrap `Integer` primitive. elementToPath
    // gives us a stable string comparison despite the primitive's generated
    // ElementId drifting between runs.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn eval_type_of_string_resolves_to_string_primitive() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            'hi'->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("String".into()));
}

#[test]
fn eval_type_of_element_is_itself() {
    // type(<element>) returns the element — round-trip through elementToPath
    // proves the same path is preserved.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->type()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("meta::pure".into()));
}

#[test]
fn eval_generic_type_wraps_rawtype() {
    // genericType(42).rawType->elementToPath() must round-trip back to
    // Integer. This exercises rawType's read of the heap-stored property.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->genericType()->rawType()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Integer".into()));
}

#[test]
fn eval_id_of_integer_returns_display() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->id();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("42".into()));
}

#[test]
fn eval_id_of_element_returns_simple_name() {
    // `id()` on a model-element reference returns the **simple name**, not
    // the qualified path — matches Java Pure and the `testId`/`testPrimitives`
    // PCT tests (`CC_Person->id() == 'CC_Person'`, not the full
    // `meta::pure::...::CC_Person` path). `elementToPath` remains the native
    // for qualified-path rendering.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            pathToElement('meta::pure', '::')->id();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("pure".into()));
}

#[test]
fn eval_to_representation_quotes_strings() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            'hello'->toRepresentation();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("'hello'".into()));
}

#[test]
fn eval_to_representation_integer_is_unquoted() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            42->toRepresentation();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("42".into()));
}

#[test]
fn eval_sub_type_of_integer_number_is_true() {
    // Integer extends Number — walking the primitive super_type chain must
    // find it.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @Number);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_sub_type_of_is_reflexive() {
    // Reflexivity: every type is a subtype of itself.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @Integer);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_sub_type_of_unrelated_is_false() {
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            subTypeOf(@Integer, @String);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn eval_enum_name_of_user_enum() {
    // Define a tiny enumeration, then ask for its simple name via the
    // metamodel reference.
    let result = eval_pure(
        r"
        Enum test::Color { RED, GREEN, BLUE }

        function test::f(): String[1] {
            enumName(@test::Color);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Color".into()));
}

#[test]
fn eval_enum_values_expands_to_each_member() {
    // enumValues returns one `Value::EnumValue { enum_id, member }` entry
    // per declared value in declaration order. The result's enum_id field
    // points at the same Color Enumeration element across every member.
    let result = eval_pure(
        r"
        Enum test::Color { RED, GREEN, BLUE }

        function test::f(): Color[*] {
            enumValues(@test::Color);
        }
        ",
        "f__Color_MANY_",
    );
    match result {
        Value::Collection(v) => {
            assert_eq!(v.len(), 3);
            let members: Vec<(SmolStr, SmolStr)> = v
                .iter()
                .filter_map(|val| match val {
                    Value::EnumValue { enum_id: _, member } => {
                        Some((SmolStr::new("Color"), member.clone()))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(members.len(), 3, "all entries must be EnumValue, got {v:?}");
            assert_eq!(members[0].1, "RED");
            assert_eq!(members[1].1, "GREEN");
            assert_eq!(members[2].1, "BLUE");
            if let (
                Value::EnumValue { enum_id: e0, .. },
                Value::EnumValue { enum_id: e1, .. },
                Value::EnumValue { enum_id: e2, .. },
            ) = (&v[0], &v[1], &v[2])
            {
                assert_eq!(e0, e1);
                assert_eq!(e1, e2);
            }
        }
        other => panic!("Expected Collection, got {other:?}"),
    }
}

// ===========================================================================
// 10. Coordinator regression — contains_ prefix disambiguation + find lambda
// ===========================================================================

#[test]
fn eval_contains_collection_variant_dispatches_correctly() {
    // Collection `contains(T[*], Any[1]):Boolean[1]` mangles to
    // `contains_T_MANY__Any_1__Boolean_1_`; string
    // `contains(String[1], String[1]):Boolean[1]` mangles to
    // `contains_String_1__String_1__Boolean_1_`. Both keys share the
    // `contains_` prefix — exact-FQN dispatch must win so the collection
    // form gets its own native.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            [1, 2, 3]->contains(2);
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_contains_string_variant_still_routes_to_string_impl() {
    // Sanity: the string form continues to work after the collection form
    // was added.
    let result = eval_pure(
        r"
        function test::f(): Boolean[1] {
            'hello world'->contains('world');
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_find_returns_first_matching_element() {
    // Closes out the `find_returns_first_match` placeholder that was
    // ignored at unit-test level (lambda-dependent — needs real evaluator).
    let result = eval_pure(
        r"
        function test::f(): Integer[0..1] {
            [1, 2, 3, 4]->find(x | $x > 2);
        }
        ",
        "f__Integer_$0_1$_",
    );
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn eval_find_returns_unit_when_no_match() {
    let result = eval_pure(
        r"
        function test::f(): Integer[0..1] {
            [1, 2, 3]->find(x | $x > 10);
        }
        ",
        "f__Integer_$0_1$_",
    );
    assert_eq!(result, Value::Unit);
}

#[test]
fn eval_element_to_path_ephemeral_element() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Root')
                )
            )->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_element_include_root_true() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Root')
                )
            )->elementToPath(true);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Root::pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_element_non_root_outermost() {
    // Outermost package is named "Other" (not "Root"). With include_root=true
    // the outermost name is still used as the prefix.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement(
                name='MyElement',
                package=^Package(
                    name='pkg',
                    package=^Package(name='Other')
                )
            )->elementToPath(true);
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("Other::pkg::MyElement".into()));
}

#[test]
fn eval_element_to_path_ephemeral_nameless() {
    // `^PackageableElement()` with no `name` renders as the empty string.
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            ^PackageableElement()->elementToPath();
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("".into()));
}

#[test]
fn eval_partial_date_literals_round_trip_to_representation() {
    // A year-only and a year-month StrictDate literal must preserve
    // precision through lowering and `toRepresentation`. Before the
    // partial-precision fix, the parser accepted `%2014` / `%2014-01`
    // but `parse_strict_date` in lower.rs required 3 segments and
    // silently returned `None`, dropping the expression during
    // lowering and corrupting argument counts for the surrounding call.
    assert_eq!(
        eval_pure(
            r"function test::f(): String[1] { %2014->toRepresentation() }",
            "f__String_1_",
        ),
        Value::String("%2014".into()),
    );
    assert_eq!(
        eval_pure(
            r"function test::g(): String[1] { %2014-01->toRepresentation() }",
            "g__String_1_",
        ),
        Value::String("%2014-01".into()),
    );
}

#[test]
fn eval_multiple_assert_eq_mixed_date_precision() {
    // Regression: multiple assertEq calls with mixed date precisions —
    // previously crashed with "Variable 'actual' not found" because
    // the year-month date literal failed to lower and `filter_map`
    // silently dropped the second argument of `assertEq`.
    assert_eq!(
        eval_pure(
            r"
            function test::f(): Boolean[1] {
                assertEq('%2014-01-01', %2014-01-01->toRepresentation());
                assertEq('%2014-01', %2014-01->toRepresentation());
                assertEq('%2014', %2014->toRepresentation());
            }
            ",
            "f__Boolean_1_",
        ),
        Value::Boolean(true),
    );
}

#[test]
fn eval_string_plus_inside_fold_lambda() {
    // Mirrors the platform plus.pure:66 pattern:
    // `$people->fold({p1, p2 | $p2.lastName + ' ' + $p1.lastName}, init)`
    // — `+` between two narrowed-by-fold lambda params and a string
    // literal. Today this fails to compile against the platform with
    // "Ambiguous function call 'plus': found 5 overloads with 1 args
    // (narrowed from 5 candidates)" because the lambda-param narrowing
    // doesn't reach `+` operands inside `fold`.
    let result = eval_pure(
        r"
        function test::f(): String[1]
        {
            ['a', 'b', 'c']->fold({p, s | $s + '/' + $p}, 'init')
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("init/a/b/c".into()));
}

#[test]
fn eval_class_let_then_lambda_param_typed_via_pkg_ref_typeinfo() {
    // Locks the platform-AmbiguousImport fix from the same series as
    // `eval_lambda_param_inference_narrows_overloads` but on the
    // type-info-flow side. Pre-fix, `^test::P(...)` lowering produced
    // a `new(@P, ...)` ValueSpec whose `function` was None and whose
    // `type_info` carried `P[1]` — but `infer_let_type` didn't read
    // `type_info`, so `var_types["people"]` was never populated,
    // causing `$people->map(p | …)` to leave `p` typed as `Any` and
    // killing every `+` overload narrowing inside the lambda body.
    // Post-fix:
    //   1. `lower_packageable_element_ref` pre-sets `type_info` to
    //      `Class<P>[1]` via the new `build_packageable_element_ref`
    //      helper (used by all three PackageableElementRef call
    //      sites).
    //   2. `infer_typeexpr_from_valuespec` and `infer_let_type` both
    //      honour `vs.type_info` first, mirroring the canonical
    //      `reference_type_info_capture` pattern.
    // Together this lets `$people->map(p | $p.lastName + 'X')` flow
    // `Person` from the let through to the lambda body's `+` operand
    // narrowing.
    let result = eval_pure(
        r"
        Class test::P { lastName: String[1]; }

        function test::f(): String[1]
        {
            let people = [^test::P(lastName='Doe')];
            $people->map(p | $p.lastName + 'X')->joinStrings(',')
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("DoeX".into()));
}

#[test]
fn eval_string_plus_string_variable() {
    // Mirrors what stress generators emit: `function f(name: String[1]) { 'hello ' + $name }`.
    use legend_pure_runtime::eval::Evaluator;
    let model = compile_with_platform(
        r"
        function test::greet(name: String[1]): String[1]
        {
            'hello ' + $name
        }
        ",
    );
    let registry = NativeRegistry::standard();
    let mut eval = Evaluator::new(&model, &registry);
    let result = eval
        .call("test::greet", &[Value::String("world".into())])
        .expect("call should succeed");
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
fn eval_lambda_param_inference_narrows_overloads() {
    // Locks the bug fixed by the type-narrowing step in
    // `lower_args_with_lambda_inference`. Pure's platform `map` declares
    // three arity-2 overloads; without narrowing, the orchestrator gives
    // up and types `s` as `Any[1]`, which causes `+` to dispatch to a
    // non-string overload and the lambda to silently return nothing.
    // After the fix, narrowing by `['a','b']` (the concrete first arg)
    // collapses the overload set to one and feeds `String[1]` into the
    // lambda body's type info.
    let result = eval_pure(
        r"
        function test::f(): String[1]
        {
            ['a','b']->map(s| $s + 'X')->joinStrings(',')
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("aX,bX".into()));
}

#[test]
fn compile_let_bound_untyped_lambda_emits_inference_error() {
    // Locks the diagnostic added for "let-bound generic lambdas without
    // caller-side type inference". Pre-fix: silently typed `x`, `y` as
    // `Any[1]`, which made the body's `$x + $y` ambiguous across all five
    // `plus` overloads, surfacing as the unhelpful
    //   "Ambiguous function call 'plus': found 5 overloads with 1 args …"
    // Post-fix: `lower_lambda_parameters` flags the missing inference
    // eagerly with `CannotInferLambdaParameterTypes`, and
    // `resolve_function_call` suppresses the cascading ambiguity diagnostic
    // for calls whose args read from those flagged params.
    use legend_pure_parser_pure::error::CompilationErrorKind;
    let source = r"
        function test::f(): Integer[1]
        {
            let f = {x, y | $x + $y};
            $f->eval(1, 2)
        }
        ";
    let partial = try_compile_with_platform(source)
        .expect_err("untyped let-bound lambda must produce a compile error");

    let inference_failures: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                CompilationErrorKind::CannotInferLambdaParameterTypes { .. }
            )
        })
        .collect();
    assert_eq!(
        inference_failures.len(),
        1,
        "expected exactly one CannotInferLambdaParameterTypes, got errors: {:?}",
        partial
            .errors
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
    );
    let CompilationErrorKind::CannotInferLambdaParameterTypes { names } =
        &inference_failures[0].kind
    else {
        unreachable!("filter above guarantees this variant");
    };
    assert_eq!(
        names.iter().map(SmolStr::as_str).collect::<Vec<_>>(),
        vec!["x", "y"]
    );
    let message = inference_failures[0].to_string();
    assert!(
        message.contains("Cannot infer types for lambda parameters 'x', 'y'"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("annotate explicitly"),
        "message should suggest annotation: {message}"
    );

    // Cascade suppression: no AmbiguousImport / "Ambiguous function call"
    // line should reach the user. The new diagnostic stands alone.
    // Scoped to the user's source ("<test>") — platform residuals (e.g.
    // dispatcher narrowing on parameterized supertype overloads in
    // `relationalRuntime.pure`) don't represent a leak of the user-source
    // cascade and are tracked separately in BACKLOG.
    let cascading: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| {
            matches!(e.kind, CompilationErrorKind::AmbiguousImport { .. })
                && e.source_info.source == "<test>"
        })
        .collect();
    assert!(
        cascading.is_empty(),
        "ambiguity cascade should be suppressed, got: {:?}",
        cascading
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
    );
}

#[test]
fn eval_let_bound_typed_lambda_still_works() {
    // Regression guard: annotating the let-bound lambda's parameters
    // restores normal compilation and evaluation. Locks that the new
    // inference-failure diagnostic only fires when the user is missing
    // both annotations and caller-side expectations.
    let result = eval_pure(
        r"
        function test::f(): Integer[1]
        {
            let f = {x: Integer[1], y: Integer[1] | $x + $y};
            $f->eval(1, 2)
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn eval_relation_at_chain_returns_relation_type() {
    // Locks in the `@(x:String)->genericType().rawType->cast(@RelationType<Any>)`
    // chain that `meta::pure::functions::meta::tests::addColumns` needs as
    // its source argument. Returns the column's element name reached via
    // `.columns->at(0).classifierGenericType.typeArguments[1].rawType.name`.
    let result = eval_pure(
        r"
        function test::f(): String[1]
        {
            let rt = @(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne();
            let col = $rt.columns->at(0);
            $col.classifierGenericType->toOne().typeArguments->at(1).rawType.name->toOne()
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("String".into()));
}

#[test]
fn eval_relation_add_columns_against_at_chain_source() {
    // End-to-end: `@(x:String)->genericType().rawType->cast(@RelationType<Any>)
    // ->toOne()->addColumns(~[ab:String[1], z:Integer])` materialises a
    // RelationType whose merged columns are reachable through the
    // `.classifierGenericType.multiplicityArguments[0].lowerBound.value`
    // chain (P0's MultiplicityValue shape fix).
    let result = eval_pure(
        r"
        function test::f(): String[1]
        {
            let rt = addColumns(
                @(x:String)->genericType().rawType->cast(@RelationType<Any>)->toOne(),
                ~[ab:String[1], z:Integer]);
            let col = $rt.columns->at(2);
            let mult = $col.classifierGenericType.multiplicityArguments->at(0);
            $col.name->toOne()
                + ':'
                + $col.classifierGenericType->toOne().typeArguments->at(1).rawType.name->toOne()
                + '['
                + $mult.lowerBound.value->toOne()->toString()
                + '..'
                + $mult.upperBound.value->toOne()->toString()
                + ']'
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String("z:Integer[0..1]".into()));
}

#[test]
fn eval_relation_add_columns_after_evaluate_and_deactivate() {
    // Drives the platform-defined
    // `meta::pure::functions::relation::tests::testAddColumnsAfterEvaluateAndDeactivate`
    // in isolation. Source is `^RelationType<Any>()->evaluateAndDeactivate()`,
    // so this test exercises the addColumns native end-to-end without
    // depending on the `@(x:String)->genericType().rawType->cast(...)` chain
    // that the sister test `testAddColumns` requires.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let result = evaluator
        .call(
            "meta::pure::functions::relation::tests::testAddColumnsAfterEvaluateAndDeactivate",
            &[],
        )
        .expect("test should evaluate without error");

    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn eval_surveyor_element_to_path_all_tests_pass() {
    // After the compiler's inherited-property-type-resolution fix, every
    // `<<test.Test>>` function under `meta::pure::functions::meta::tests::elementToPath`
    // should PASS. If this regresses, the surveyor has a real problem —
    // diagnose before relaxing the assertion.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::meta::tests::elementToPath".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let Value::Object(ref report_id) = report else {
        panic!("surveyor returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    let read = |name: &str| -> i64 {
        let values = heap.get_property_values(report_id, name).unwrap();
        match values.iter().next() {
            Some(Value::Integer(n)) => *n,
            _ => -1,
        }
    };
    let pass = read("passCount");
    let fail = read("failCount");
    let error = read("errorCount");
    let skip = read("skipCount");
    assert_eq!(
        error, 0,
        "expected 0 ERROR'd tests, got {error} (pass={pass}, fail={fail}, skip={skip})"
    );
    assert!(pass >= 7, "expected >=7 PASS, got {pass}");
}

#[test]
fn eval_surveyor_on_element_to_path_tests_has_nonzero_runs() {
    // Canary: after the Track 1–5 native rollout, surveyor should be able to
    // bucket real platform `<<test.Test>>` functions as PASS/FAIL/ERROR
    // (not all-ERROR). We don't assert specific counts — the assertion chain
    // still reaches into many natives we haven't implemented — but at least
    // *one* test in this package must flip out of ERROR.
    use legend_pure_runtime::eval::Evaluator;

    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[
                Value::String("meta::pure::functions::meta::tests::elementToPath".into()),
                Value::String("".into()),
            ],
        )
        .expect("surveyor should return a TestReport");

    let Value::Object(ref report_id) = report else {
        panic!("surveyor returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    let read = |name: &str| -> i64 {
        let values = heap.get_property_values(report_id, name).unwrap();
        match values.iter().next() {
            Some(Value::Integer(n)) => *n,
            _ => -1,
        }
    };
    let pass = read("passCount");
    let fail = read("failCount");
    let error = read("errorCount");
    let skip = read("skipCount");
    let total = pass + fail + error + skip;
    assert!(total > 0, "expected at least one test, got 0");
    assert!(
        pass + fail + skip > 0,
        "expected at least one non-error outcome after native rollout (pass={pass}, fail={fail}, error={error}, skip={skip})"
    );
}

// ===========================================================================
// PCT (Pure Compatibility Tests) — adapter-driven test execution
// ===========================================================================
//
// PCT tests differ from `<<test.Test>>` tests in two ways:
// 1. They are annotated `<<PCT.test>>` and live alongside the function they
//    exercise (e.g. `boolean/operation/not.pure` defines both `not` and the
//    8 `<<PCT.test>>` functions for it).
// 2. They take an *adapter* as their sole parameter — a function-of-function
//    that the runtime injects so the same test body can run against multiple
//    execution back-ends. The in-memory adapter
//    `meta::pure::test::pct::testAdapterForInMemoryExecution<X|o>` is
//    just `$f->eval()`.
//
// The Pure-side surveyor (`meta::pure::test::surveyor::runPCTTests`) walks
// a package, filters `<<PCT.test>>` functions, and invokes each via
// `executePCTTest($t, $adapter, $exclusions)`.
//
// These canaries de-risk that pipeline end-to-end *before* the
// `loadPCTManifest` native lands — they pass the adapter and an empty
// exclusion map directly so the test exercises only discovery → adapter
// dispatch → classify, not JSON manifest parsing.

/// Build the in-memory adapter `Value::Element` and an empty exclusions
/// `Value::Map` for direct `runPCTTests` invocation.
fn pct_canary_args(model: &PureModel) -> (Value, Value) {
    use legend_pure_runtime::value::MapState;
    use std::cell::RefCell;
    use std::rc::Rc;

    // The adapter's mangled FQN as it appears in the shipped manifest
    // `pct_essential_native.json`. Resolves against the platform model.
    let adapter_path: [SmolStr; 5] = [
        "meta".into(),
        "pure".into(),
        "test".into(),
        "pct".into(),
        "testAdapterForInMemoryExecution_Function_1__X_o_".into(),
    ];
    let adapter_id = model
        .resolve_by_path(&adapter_path)
        .expect("in-memory adapter must resolve in the platform model");
    let adapter = Value::Element(adapter_id);

    let exclusions = Value::Map(Rc::new(RefCell::new(MapState::default())));
    (adapter, exclusions)
}

/// PCT manifest for the Rust port — colocated JSON file mirroring
/// the Java `pct_*_native.json` schema (`adapter` + `exclusions`)
/// from `legend-pure-core/.../platform/pure/{essential,grammar/...}/
/// pct_*_native.json`.
///
/// `exclusions` is a `Map<test_fqn → expected_message>` where each
/// entry pins a Rust-port-specific failure: tests whose expected
/// output exceeds a representational limit of our runtime, not a
/// fixable behaviour gap. [`apply_exclusion`] flips matching
/// FAIL/ERROR results to PASS; if a test ever stops failing, the
/// helper flips PASS back to FAIL with "PCT exclusion needs rebase"
/// so stale entries can't go quietly.
///
/// **Exclusion categories (review quarterly):**
///
/// `testAdjust*BigNumber` (5 tests) — assert the result of adding
/// extreme spans (`9_600_000_000` months, `12_345_678_912` hours, …)
/// to dates and expect years like `800002016` or `-1406373`. Our
/// `PureDate` carries the year as `i16` (the `jiff::civil::DateTime`
/// field), so any year outside `[-32768, 32767]` overflows. Java
/// Pure carries year as `int`, giving it roughly
/// `[-2_147_483_648, 2_147_483_647]`. The Rust port makes a
/// smaller-but-correct trade — reject extreme years instead of
/// silently truncating. The non-`BigNumber` siblings in the same
/// packages cover the same arithmetic for in-range inputs.
///
/// `testLarge{Times,Minus,Plus}` (3 tests) — assert i64-overflowing
/// arithmetic with literals like `9223372036854775898` (`i64::MAX` +
/// 91) and expected results like `18446744073709551614` (2^64 - 2)
/// that don't fit in i64. The platform marks these
/// `{test.excludePlatform = 'Java compiled'}` because Java's Long
/// arithmetic wraps the same way ours does — this is a parity
/// statement, not a bug. Our parser silently parses out-of-i64
/// literals as 0; both expected and actual produce wrong but
/// stable values.
///
/// To remove an exclusion: fix the underlying representational
/// limit (widen year to i32, promote to Decimal on i64 overflow,
/// …). The next canary run will report "PCT exclusion needs
/// rebase" — drop the entry from the JSON.
/// Build a Rust-port-specific exclusions Map for PCT runs.
///
/// Delegates to [`legend_pure_runtime::pct::rust_native_pct_args`] —
/// the canonical loader of `crates/runtime/resources/pct_grammar_rust_native.json`,
/// also consumed by `legend test --pct` so the CLI and tests stay in
/// lockstep on which platform tests are intentionally skipped.
fn pct_canary_args_with_rust_exclusions(model: &PureModel) -> (Value, Value) {
    legend_pure_runtime::pct::rust_native_pct_args(model)
        .expect("pct_grammar_rust_native.json: adapter must resolve in the platform model")
}

/// Read a non-negative integer counter from a heap-allocated `TestReport`.
fn read_report_counter(
    evaluator: &Evaluator,
    report_id: &legend_pure_runtime::heap::ObjectHandle,
    name: &str,
) -> i64 {
    let values = evaluator
        .heap()
        .get_property_values(report_id, name)
        .unwrap_or_else(|e| panic!("get {name}: {e}"));
    match values.iter().next() {
        Some(Value::Integer(n)) => *n,
        _ => -1,
    }
}

#[test]
fn eval_pct_canary_boolean_not_runs() {
    // Phase 1 de-risk: PCT tests have generic signatures
    // (`testNotTrue<Z|y>(f:Function<{Function<{->Z[y]}>[1]->Z[y]}>[1])`).
    // The adapter is bound to `$f`, then `$f->eval(|true->not())` must
    // dispatch correctly with `Z=Boolean, y=1` inferred at the call site.
    //
    // We don't assert PASS counts — the goal is that *at least one* test
    // in the package flips out of ERROR, proving discovery + adapter
    // dispatch + classify all work end-to-end. Specific counts are
    // measured by `eval_pct_broad_canary` (Phase 4).
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let pkg = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[
                Value::String("meta::pure::functions::boolean::tests::conjunctions::not".into()),
                Value::String("::".into()),
            ],
        )
        .expect("pathToElement must resolve the not-tests package");

    let (adapter, exclusions) = pct_canary_args(&model);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTests",
            &[pkg, Value::String("".into()), adapter, exclusions],
        )
        .expect("runPCTTests must return a TestReport");

    let Value::Object(ref report_id) = report else {
        panic!("runPCTTests returned non-object: {report:?}");
    };

    let pass = read_report_counter(&evaluator, report_id, "passCount");
    let fail = read_report_counter(&evaluator, report_id, "failCount");
    let error = read_report_counter(&evaluator, report_id, "errorCount");
    let skip = read_report_counter(&evaluator, report_id, "skipCount");
    let total = pass + fail + error + skip;

    assert!(
        total > 0,
        "expected at least one PCT test discovered, got 0"
    );
    assert!(
        pass + fail + skip > 0,
        "expected at least one non-ERROR PCT outcome (pass={pass}, fail={fail}, error={error}, skip={skip}); \
         all-ERROR usually means generic adapter dispatch is broken — start there"
    );
}

#[test]
fn find_pct_adapter_resolves_in_memory() {
    // Phase 6 contract: `find_pct_adapter(model, "In-Memory")` discovers the
    // shipped `meta::pure::test::pct::testAdapterForInMemoryExecution`
    // function by walking the model for `<<PCT.adapter>>`-stereotyped
    // functions whose `PCT.adapterName` tag matches.
    use legend_pure_runtime::native::testing::find_pct_adapter;

    let model = compile_with_platform("");
    let id = find_pct_adapter(&model, "In-Memory").expect("In-Memory adapter must resolve");
    let node = model.get_node(id);
    assert_eq!(
        node.name.as_str(),
        "testAdapterForInMemoryExecution_Function_1__X_o_",
    );
}

#[test]
fn find_pct_adapter_unknown_name_returns_none() {
    use legend_pure_runtime::native::testing::find_pct_adapter;
    let model = compile_with_platform("");
    assert!(find_pct_adapter(&model, "DefinitelyNotARealAdapterName").is_none());
}

#[test]
fn eval_pct_load_manifest_essential_resolves() {
    // Phase 2 contract: `loadPCTManifest('pct_essential_native.json')` must
    // resolve the embedded platform manifest, parse it, and build a
    // PCTManifest heap object carrying the resolved adapter Function and an
    // empty exclusions Map. Invoke via a thin Pure wrapper because
    // `Evaluator::call` routes through `call_user_function`, which evaluates
    // the (empty) body of `native function` declarations and silently
    // returns `Unit`. A Pure-level call site lets Pass 2.1's name mangling
    // reach the registered native via the eval_function_call dispatch chain.
    let model = compile_with_platform(
        r"
        import meta::pure::test::pct::*;

        function test::loadEssential(): PCTManifest[1] {
            loadPCTManifest('pct_essential_native.json');
        }
        ",
    );
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);
    let fn_id = model
        .resolve_by_fqn(&["test".into(), "loadEssential__PCTManifest_1_".into()])
        .expect("test::loadEssential resolves");
    let report = evaluator
        .call_user_function_by_id(fn_id)
        .expect("loadEssential must succeed");
    let Value::Object(ref manifest_id) = report else {
        panic!("loadPCTManifest wrapper returned non-object: {report:?}");
    };

    let heap = evaluator.heap();
    assert_eq!(
        heap.classifier(manifest_id).unwrap(),
        "meta::pure::test::pct::PCTManifest"
    );

    let adapter = heap
        .get_property_values(manifest_id, "adapter")
        .expect("adapter slot")
        .iter()
        .next()
        .cloned()
        .expect("adapter populated");
    let Value::Element(adapter_id) = adapter else {
        panic!("adapter slot not Element: {adapter:?}");
    };
    let adapter_node = model.get_node(adapter_id);
    assert_eq!(
        adapter_node.name.as_str(),
        "testAdapterForInMemoryExecution_Function_1__X_o_",
        "adapter resolves to in-memory adapter"
    );

    let exclusions = heap
        .get_property_values(manifest_id, "exclusions")
        .expect("exclusions slot")
        .iter()
        .next()
        .cloned()
        .expect("exclusions populated");
    let Value::Map(state) = exclusions else {
        panic!("exclusions slot not Map: {exclusions:?}");
    };
    assert!(
        state.borrow().entries.is_empty(),
        "shipped pct_essential_native.json has empty exclusions"
    );
}

#[test]
fn eval_pct_run_from_path_essential_manifest() {
    // End-to-end: the developer-facing entry point. Mirrors what
    // `legend test --pct` will invoke.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTestsFromPath",
            &[
                Value::String("meta::pure::functions::boolean::tests::conjunctions::not".into()),
                Value::String("".into()),
                Value::String("pct_essential_native.json".into()),
            ],
        )
        .expect("runPCTTestsFromPath must succeed");

    let Value::Object(ref report_id) = report else {
        panic!("runPCTTestsFromPath returned non-object: {report:?}");
    };
    let pass = read_report_counter(&evaluator, report_id, "passCount");
    let total = pass
        + read_report_counter(&evaluator, report_id, "failCount")
        + read_report_counter(&evaluator, report_id, "errorCount")
        + read_report_counter(&evaluator, report_id, "skipCount");
    assert!(total > 0, "expected at least one PCT test discovered");
    assert!(pass > 0, "expected at least one PASS via the manifest path");
}

#[test]
fn eval_year_native_directly() {
    // year(%2015) directly — no arrow, no eval/lambda — should return 2015.
    let r = eval_pure(
        "function test::f(): Integer[1] { %2015->year(); }",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_minute_precision_date_isolated() {
    // The line that fails inside testYear: minute-precision datetime
    // followed by ->year(). Standalone test isolates the issue from
    // the multi-statement body to make sure it's not state pollution.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17:09->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_three_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_four_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_hour_precision_no_let() {
    // T17 hour-precision but no `let adapter` binding — call the adapter
    // by FQN inline.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            testAdapterForInMemoryExecution_Function_1__X_o_->eval(|%2015-04-15T17->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_hour_precision_direct() {
    // Just `year(T17 date)` — no eval/lambda/adapter at all.
    let r = eval_pure(
        "function test::f(): Integer[1] { %2015-04-15T17->year(); }",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_hour_precision_no_assert() {
    // Hour-only T17 precision, but no assertEquals wrapper. If this
    // passes, the issue is the assertEquals + T17 combo. If it fails,
    // the issue is the T17 literal itself in a let-bound context.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

#[test]
fn eval_year_minute_precision_with_assert() {
    // Same minute-precision date but wrapped in assertEquals. Tests if
    // it's the assertEquals wrapper or the hour-only precision.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_hour_precision_with_let_only() {
    // Just the let + hour-precision year call. No subsequent statements.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_hour_precision_first() {
    // Reorder to put hour-precision FIRST. If the issue is sequence-
    // dependent compiler inference, this would isolate it.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_five_precisions() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_pair_two_precisions() {
    // Try just two consecutive assertEquals — see if state pollution
    // appears already at this size.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_datediff_weeks_sat_to_sun_eq_1() {
    let r = eval_pure(
        r"
        function test::f(): Integer[1] {
            %2015-07-04->dateDiff(%2015-07-05, DurationUnit.WEEKS);
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(1));
}

#[test]
fn eval_year_full_test_body() {
    // Inline the full testYear body (sans the PCT.test annotation
    // round-trip) to trace whether the multi-precision date inputs
    // matter.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(2015, $adapter->eval(|%2015->year()));
            assertEquals(2015, $adapter->eval(|%2015-04->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09:21->year()));
            assertEquals(2015, $adapter->eval(|%2015-04-15T17:09:21.398->year()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_year_via_pct_adapter_lambda() {
    // Exact PCT shape: lambda-of-lambda where inner uses ->year() on a date.
    // testYear pattern: assertEquals(2015, $f->eval(|%2015->year()));
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015->year());
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(r, Value::Integer(2015));
}

// ===========================================================================
// Date native — verbatim ports of Java <<PCT.test>> functions and matching
// Rust-only error tests. Migrated from the empty MockCtx stubs in
// crates/runtime/src/native/datetime.rs (those required full evaluator
// support to exercise date literals + arrow chains).
//
// Source-of-truth Java tests live in
// legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure/essential/date/
// — copy the assertion lines verbatim into the function bodies below so a
// future divergence is caught here, not in the broad PCT canary.
// ===========================================================================

// ----- monthNumber (mirrors testMonthNumber in extract/monthNumber.pure) -----

#[test]
fn eval_test_month_number() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(4, $adapter->eval(|%2015-04->monthNumber()));
            assertEquals(4, $adapter->eval(|%2015-04-15->monthNumber()));
            assertEquals(4, $adapter->eval(|%2015-04-15T17->monthNumber()));
            assertEquals(4, $adapter->eval(|%2015-04-15T17:09->monthNumber()));
            assertEquals(4, $adapter->eval(|%2015-04-15T17:09:21->monthNumber()));
            assertEquals(4, $adapter->eval(|%2015-04-15T17:09:21.398->monthNumber()));

            assertEquals(1, $adapter->eval(|%2015-01->monthNumber()));
            assertEquals(2, $adapter->eval(|%2015-02->monthNumber()));
            assertEquals(3, $adapter->eval(|%2015-03->monthNumber()));
            assertEquals(5, $adapter->eval(|%2015-05->monthNumber()));
            assertEquals(6, $adapter->eval(|%2015-06->monthNumber()));
            assertEquals(7, $adapter->eval(|%2015-07->monthNumber()));
            assertEquals(8, $adapter->eval(|%2015-08->monthNumber()));
            assertEquals(9, $adapter->eval(|%2015-09->monthNumber()));
            assertEquals(10, $adapter->eval(|%2015-10->monthNumber()));
            assertEquals(11, $adapter->eval(|%2015-11->monthNumber()));
            assertEquals(12, $adapter->eval(|%2015-12->monthNumber()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_month_number_year_only_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015->monthNumber());
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("month"),
        "expected month-component error, got: {msg}"
    );
}

// ----- dayOfMonth (mirrors testDayOfMonth + testDayOfMonthError) -----

#[test]
fn eval_test_day_of_month() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(15, $adapter->eval(|%2015-04-15->dayOfMonth()));
            assertEquals(15, $adapter->eval(|%2015-04-15T17->dayOfMonth()));
            assertEquals(15, $adapter->eval(|%2015-04-15T17:09->dayOfMonth()));
            assertEquals(15, $adapter->eval(|%2015-04-15T17:09:21->dayOfMonth()));
            assertEquals(15, $adapter->eval(|%2015-04-15T17:09:21.398->dayOfMonth()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_day_of_month_no_day_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2017->dayOfMonth());
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("Cannot get day of month"),
        "expected day-component error, got: {msg}"
    );
}

// ----- hour (mirrors testHour + testHourError) -----

#[test]
fn eval_test_hour() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(17, $adapter->eval(|%2015-04-15T17->hour()));
            assertEquals(17, $adapter->eval(|%2015-04-15T17:09->hour()));
            assertEquals(17, $adapter->eval(|%2015-04-15T17:09:21->hour()));
            assertEquals(17, $adapter->eval(|%2015-04-15T17:09:21.398->hour()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_hour_no_time_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2017->hour());
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("Cannot get hour"),
        "expected hour-component error, got: {msg}"
    );
}

// ----- minute (mirrors testMinute + testMinuteError) -----

#[test]
fn eval_test_minute() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(9, $adapter->eval(|%2015-04-15T17:09->minute()));
            assertEquals(9, $adapter->eval(|%2015-04-15T17:09:21->minute()));
            assertEquals(9, $adapter->eval(|%2015-04-15T17:09:21.398->minute()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_minute_only_hour_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17->minute());
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("Cannot get minute"),
        "expected minute-component error, got: {msg}"
    );
}

// ----- second (mirrors testSecond + testSecondError) -----

#[test]
fn eval_test_second() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(21, $adapter->eval(|%2015-04-15T17:09:21->second()));
            assertEquals(21, $adapter->eval(|%2015-04-15T17:09:21.398->second()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_second_only_minute_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015-04-15T17:09->second());
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("Cannot get second"),
        "expected second-component error, got: {msg}"
    );
}

// ----- datePart (mirrors testDatePart + …Trivial + …YearMonthOnly + …YearOnly) -----

#[test]
fn eval_test_date_part() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%1973-11-05, $adapter->eval(|%1973-11-05T13:01:25->datePart()));
            assertEquals(%2015-08-29, $adapter->eval(|%2015-08-29T22:22:22.9914234->datePart()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_part_trivial() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%1973-11-05, $adapter->eval(|%1973-11-05->datePart()));
            assertEquals(%2015-08-29, $adapter->eval(|%2015-08-29->datePart()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_part_year_month_only() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%1973-11, $adapter->eval(|%1973-11->datePart()));
            assertEquals(%2015-08, $adapter->eval(|%2015-08->datePart()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_part_year_only() {
    // Mirrors testDatePartYearOnly. Note: stub `date_part_rejects_year_only`
    // had a misleading name — Java semantics (and the Rust impl at
    // datetime.rs:378) explicitly pass year-only dates through unchanged.
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%1973, $adapter->eval(|%1973->datePart()));
            assertEquals(%2015, $adapter->eval(|%2015->datePart()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

// ----- dateDiff (mirrors testDateDiff{Years,Months,Weeks,Days,Hours,Minutes,Seconds} in operation/dateDiff.pure) -----

#[test]
fn eval_test_date_diff_years() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(1, $adapter->eval(|%2015->dateDiff(%2016, DurationUnit.YEARS)));
            assertEquals(-1, $adapter->eval(|%2016->dateDiff(%2015, DurationUnit.YEARS)));
            assertEquals(20, $adapter->eval(|%2000->dateDiff(%2020, DurationUnit.YEARS)));
            assertEquals(-20, $adapter->eval(|%2020->dateDiff(%2000, DurationUnit.YEARS)));
            assertEquals(0, $adapter->eval(|%2015->dateDiff(%2015, DurationUnit.YEARS)));
            assertEquals(0, $adapter->eval(|%2015-01-01T00:00:00->dateDiff(%2015-12-31T23:59:59, DurationUnit.YEARS)));
            assertEquals(1, $adapter->eval(|%2015-12-31T23:59:59->dateDiff(%2016-01-01T00:00:01, DurationUnit.YEARS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_months() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2016-02-01->dateDiff(%2016-02-01, DurationUnit.MONTHS)));
            assertEquals(0, $adapter->eval(|%2016-02-01->dateDiff(%2016-02-29, DurationUnit.MONTHS)));
            assertEquals(1, $adapter->eval(|%2016-02-01->dateDiff(%2016-03-01, DurationUnit.MONTHS)));
            assertEquals(-1, $adapter->eval(|%2016-03-01->dateDiff(%2016-02-01, DurationUnit.MONTHS)));
            assertEquals(12, $adapter->eval(|%2015-01-29->dateDiff(%2016-01-29, DurationUnit.MONTHS)));
            assertEquals(14, $adapter->eval(|%2015-01-29->dateDiff(%2016-03-29, DurationUnit.MONTHS)));
            assertEquals(-14, $adapter->eval(|%2016-03-29->dateDiff(%2015-01-29, DurationUnit.MONTHS)));
            assertEquals(0, $adapter->eval(|%2014-12-01T00:00:00->dateDiff(%2014-12-01T23:59:59, DurationUnit.MONTHS)));
            assertEquals(11, $adapter->eval(|%2016-01-01->dateDiff(%2016-12-31, DurationUnit.MONTHS)));
            assertEquals(11, $adapter->eval(|%2016-01-31->dateDiff(%2016-12-31, DurationUnit.MONTHS)));
            assertEquals(12, $adapter->eval(|%2016-01-01->dateDiff(%2017-01-01, DurationUnit.MONTHS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_weeks() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2015-07-05->dateDiff(%2015-07-05, DurationUnit.WEEKS)));
            assertEquals(0, $adapter->eval(|%2015-07-03->dateDiff(%2015-07-04, DurationUnit.WEEKS)));
            assertEquals(1, $adapter->eval(|%2015-07-04->dateDiff(%2015-07-05, DurationUnit.WEEKS)));
            assertEquals(0, $adapter->eval(|%2015-07-05->dateDiff(%2015-07-04, DurationUnit.WEEKS)));
            assertEquals(1, $adapter->eval(|%2015-07-05->dateDiff(%2015-07-12, DurationUnit.WEEKS)));
            assertEquals(-1, $adapter->eval(|%2015-07-12->dateDiff(%2015-07-05, DurationUnit.WEEKS)));
            assertEquals(0, $adapter->eval(|%2015-07-12->dateDiff(%2015-07-06, DurationUnit.WEEKS)));
            assertEquals(4, $adapter->eval(|%2015-07-05->dateDiff(%2015-08-02, DurationUnit.WEEKS)));
            assertEquals(-4, $adapter->eval(|%2015-08-02->dateDiff(%2015-07-05, DurationUnit.WEEKS)));
            assertEquals(-3, $adapter->eval(|%2015-08-02->dateDiff(%2015-07-06, DurationUnit.WEEKS)));
            assertEquals(1, $adapter->eval(|%2014-12-28->dateDiff(%2015-01-04, DurationUnit.WEEKS)));
            assertEquals(52, $adapter->eval(|%2015-01-01->dateDiff(%2016-01-01, DurationUnit.WEEKS)));
            assertEquals(52, $adapter->eval(|%2016-01-01->dateDiff(%2016-12-31, DurationUnit.WEEKS)));
            assertEquals(53, $adapter->eval(|%2016-01-01->dateDiff(%2017-01-01, DurationUnit.WEEKS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_days() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2015-07-07->dateDiff(%2015-07-07, DurationUnit.DAYS)));
            assertEquals(1, $adapter->eval(|%2015-07-07->dateDiff(%2015-07-08, DurationUnit.DAYS)));
            assertEquals(-1, $adapter->eval(|%2015-07-08->dateDiff(%2015-07-07, DurationUnit.DAYS)));
            assertEquals(365, $adapter->eval(|%2015-01-1->dateDiff(%2016-01-01, DurationUnit.DAYS)));
            assertEquals(366, $adapter->eval(|%2016-01-1->dateDiff(%2017-01-01, DurationUnit.DAYS)));
            assertEquals(394, $adapter->eval(|%2014-01-31->dateDiff(%2015-03-01, DurationUnit.DAYS)));
            assertEquals(395, $adapter->eval(|%2016-01-31->dateDiff(%2017-03-01, DurationUnit.DAYS)));
            assertEquals(-395, $adapter->eval(|%2017-03-01->dateDiff(%2016-01-31, DurationUnit.DAYS)));
            assertEquals(7, $adapter->eval(|%2014-12-28->dateDiff(%2015-01-04, DurationUnit.DAYS)));
            assertEquals(-7, $adapter->eval(|%2015-01-04->dateDiff(%2014-12-28, DurationUnit.DAYS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_hours() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:00:00, DurationUnit.HOURS)));
            assertEquals(1, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T14:00:00, DurationUnit.HOURS)));
            assertEquals(-1, $adapter->eval(|%2015-07-07T14:00:00->dateDiff(%2015-07-07T13:00:00, DurationUnit.HOURS)));
            assertEquals(2, $adapter->eval(|%2015-07-07T23:00:00->dateDiff(%2015-07-08T01:00:00, DurationUnit.HOURS)));
            assertEquals(2, $adapter->eval(|%2015-07-07T23:00:00->dateDiff(%2015-07-08T01:59:59, DurationUnit.HOURS)));
            assertEquals(24, $adapter->eval(|%2014-12-31T23:00:00->dateDiff(%2015-01-01T23:00:00, DurationUnit.HOURS)));
            assertEquals(0, $adapter->eval(|%2014-12-01T00:00:00->dateDiff(%2014-12-01T00:59:59, DurationUnit.HOURS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_minutes() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:00:00, DurationUnit.MINUTES)));
            assertEquals(1, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:01:00, DurationUnit.MINUTES)));
            assertEquals(-1, $adapter->eval(|%2015-07-07T13:01:00->dateDiff(%2015-07-07T13:00:00, DurationUnit.MINUTES)));
            assertEquals(1, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:01:01, DurationUnit.MINUTES)));
            assertEquals(61, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T14:01:00, DurationUnit.MINUTES)));
            assertEquals(120, $adapter->eval(|%2015-07-07T23:00:00->dateDiff(%2015-07-08T01:00:00, DurationUnit.MINUTES)));
            assertEquals(0, $adapter->eval(|%2014-12-01T00:00:00->dateDiff(%2014-12-01T00:00:59, DurationUnit.MINUTES)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_date_diff_seconds() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(0, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:00:00, DurationUnit.SECONDS)));
            assertEquals(1, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:00:01, DurationUnit.SECONDS)));
            assertEquals(-1, $adapter->eval(|%2015-07-07T13:00:01->dateDiff(%2015-07-07T13:00:00, DurationUnit.SECONDS)));
            assertEquals(60, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:01:00, DurationUnit.SECONDS)));
            assertEquals(61, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T13:01:01, DurationUnit.SECONDS)));
            assertEquals(3661, $adapter->eval(|%2015-07-07T13:00:00->dateDiff(%2015-07-07T14:01:01, DurationUnit.SECONDS)));
            assertEquals(7200, $adapter->eval(|%2015-07-07T23:00:00->dateDiff(%2015-07-08T01:00:00, DurationUnit.SECONDS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_date_diff_time_unit_needs_time_precision() {
    // Time-based units (HOURS/MINUTES/SECONDS/...) require both inputs to have
    // datetime precision; year-only inputs should error out of jiff's until().
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Integer[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015->dateDiff(%2016, DurationUnit.HOURS));
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        msg.contains("dateDiff") || msg.contains("Cannot get hour"),
        "expected time-precision error from dateDiff(YEAR, YEAR, HOURS), got: {msg}"
    );
}

// ----- adjust (mirrors testAdjustBy{Days,Weeks,Years,Hours} in operation/adjust.pure) -----

#[test]
fn eval_test_adjust_by_days() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%2015-04-20, $adapter->eval(|%2015-04-16->adjust(4, DurationUnit.DAYS)));
            assertEquals(%2015-04-12, $adapter->eval(|%2015-04-16->adjust(-4, DurationUnit.DAYS)));

            assertEquals(%2015-05-02, $adapter->eval(|%2015-04-16->adjust(16, DurationUnit.DAYS)));
            assertEquals(%2015-06-01, $adapter->eval(|%2015-05-16->adjust(16, DurationUnit.DAYS)));

            assertEquals(%2015-03-31, $adapter->eval(|%2015-04-16->adjust(-16, DurationUnit.DAYS)));
            assertEquals(%2015-03-30, $adapter->eval(|%2015-04-16->adjust(-17, DurationUnit.DAYS)));

            assertEquals(%2015-03-30, $adapter->eval(|%2014-03-30->adjust(365, DurationUnit.DAYS)));
            assertEquals(%2013-03-30, $adapter->eval(|%2014-03-30->adjust(-365, DurationUnit.DAYS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_adjust_by_weeks() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%2015-04-23, $adapter->eval(|%2015-04-16->adjust(1, DurationUnit.WEEKS)));
            assertEquals(%2015-04-09, $adapter->eval(|%2015-04-16->adjust(-1, DurationUnit.WEEKS)));

            assertEquals(%2015-04-30, $adapter->eval(|%2015-04-16->adjust(2, DurationUnit.WEEKS)));
            assertEquals(%2015-04-02, $adapter->eval(|%2015-04-16->adjust(-2, DurationUnit.WEEKS)));

            assertEquals(%2015-05-07, $adapter->eval(|%2015-04-16->adjust(3, DurationUnit.WEEKS)));
            assertEquals(%2015-06-06, $adapter->eval(|%2015-05-16->adjust(3, DurationUnit.WEEKS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_adjust_by_years() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%2016, $adapter->eval(|%2015->adjust(1, DurationUnit.YEARS)));
            assertEquals(%2027, $adapter->eval(|%2015->adjust(12, DurationUnit.YEARS)));
            assertEquals(%2011, $adapter->eval(|%2015->adjust(-4, DurationUnit.YEARS)));

            assertEquals(%2016-02-28, $adapter->eval(|%2015-02-28->adjust(1, DurationUnit.YEARS)));
            assertEquals(%2013-02-28, $adapter->eval(|%2012-02-29->adjust(1, DurationUnit.YEARS)));
            assertEquals(%2016-02-29, $adapter->eval(|%2012-02-29->adjust(4, DurationUnit.YEARS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_adjust_by_hours() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assertEquals(%2015-04-15T13:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(1, DurationUnit.HOURS)));
            assertEquals(%2015-04-15T11:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(-1, DurationUnit.HOURS)));

            assertEquals(%2015-04-16T12:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(24, DurationUnit.HOURS)));
            assertEquals(%2015-04-14T12:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(-24, DurationUnit.HOURS)));

            assertEquals(%2015-04-17T00:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(36, DurationUnit.HOURS)));
            assertEquals(%2015-04-14T00:12:11, $adapter->eval(|%2015-04-15T12:12:11->adjust(-36, DurationUnit.HOURS)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_adjust_hours_on_year_only_errors() {
    let msg = eval_pure_err(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Date[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            $adapter->eval(|%2015->adjust(1, DurationUnit.HOURS));
        }
        ",
        "f__Date_1_",
    );
    assert!(
        !msg.is_empty(),
        "expected error adjusting hours on year-only date, got empty"
    );
}

// ----- has* (mirrors test{HasMonth,HasDay,HasHour,HasMinute,HasSecond,HasSubsecond,HasSubsecondWithAtLeastPrecision}) -----

#[test]
fn eval_test_has_month() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasMonth()));
            assert($adapter->eval(|%2015-04-15T17:09:21->hasMonth()));
            assert($adapter->eval(|%2015-04-15T17:09->hasMonth()));
            assert($adapter->eval(|%2015-04-15T17->hasMonth()));
            assert($adapter->eval(|%2015-04-15->hasMonth()));
            assert($adapter->eval(|%2015-04->hasMonth()));
            assertFalse($adapter->eval(|%2015->hasMonth()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_day() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasDay()));
            assert($adapter->eval(|%2015-04-15T17:09:21->hasDay()));
            assert($adapter->eval(|%2015-04-15T17:09->hasDay()));
            assert($adapter->eval(|%2015-04-15T17->hasDay()));
            assert($adapter->eval(|%2015-04-15->hasDay()));
            assertFalse($adapter->eval(|%2015-04->hasDay()));
            assertFalse($adapter->eval(|%2015->hasDay()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_hour() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasHour()));
            assert($adapter->eval(|%2015-04-15T17:09:21->hasHour()));
            assert($adapter->eval(|%2015-04-15T17:09->hasHour()));
            assert($adapter->eval(|%2015-04-15T17->hasHour()));
            assertFalse($adapter->eval(|%2015-04-15->hasHour()));
            assertFalse($adapter->eval(|%2015-04->hasHour()));
            assertFalse($adapter->eval(|%2015->hasHour()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_minute() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasMinute()));
            assert($adapter->eval(|%2015-04-15T17:09:21->hasMinute()));
            assert($adapter->eval(|%2015-04-15T17:09->hasMinute()));
            assertFalse($adapter->eval(|%2015-04-15T17->hasMinute()));
            assertFalse($adapter->eval(|%2015-04-15->hasMinute()));
            assertFalse($adapter->eval(|%2015-04->hasMinute()));
            assertFalse($adapter->eval(|%2015->hasMinute()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_second() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasSecond()));
            assert($adapter->eval(|%2015-04-15T17:09:21->hasSecond()));
            assertFalse($adapter->eval(|%2015-04-15T17:09->hasSecond()));
            assertFalse($adapter->eval(|%2015-04-15T17->hasSecond()));
            assertFalse($adapter->eval(|%2015-04-15->hasSecond()));
            assertFalse($adapter->eval(|%2015-04->hasSecond()));
            assertFalse($adapter->eval(|%2015->hasSecond()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_subsecond() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecond()));
            assertFalse($adapter->eval(|%2015-04-15T17:09:21->hasSubsecond()));
            assertFalse($adapter->eval(|%2015-04-15T17:09->hasSubsecond()));
            assertFalse($adapter->eval(|%2015-04-15T17->hasSubsecond()));
            assertFalse($adapter->eval(|%2015-04-15->hasSubsecond()));
            assertFalse($adapter->eval(|%2015-04->hasSubsecond()));
            assertFalse($adapter->eval(|%2015->hasSubsecond()));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

#[test]
fn eval_test_has_subsecond_with_at_least_precision() {
    let r = eval_pure(
        r"
        import meta::pure::test::pct::*;
        function test::f(): Boolean[1] {
            let adapter = testAdapterForInMemoryExecution_Function_1__X_o_;
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecondWithAtLeastPrecision(1)));
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecondWithAtLeastPrecision(2)));
            assert($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecondWithAtLeastPrecision(3)));
            assertFalse($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecondWithAtLeastPrecision(4)));
            assertFalse($adapter->eval(|%2015-04-15T17:09:21.398->hasSubsecondWithAtLeastPrecision(5)));
            assertFalse($adapter->eval(|%2015-04-15T17:09:21->hasSubsecondWithAtLeastPrecision(1)));
            assertFalse($adapter->eval(|%2015-04-15T17:09->hasSubsecondWithAtLeastPrecision(1)));
            assertFalse($adapter->eval(|%2015-04-15T17->hasSubsecondWithAtLeastPrecision(1)));
            assertFalse($adapter->eval(|%2015-04-15->hasSubsecondWithAtLeastPrecision(1)));
            assertFalse($adapter->eval(|%2015-04->hasSubsecondWithAtLeastPrecision(1)));
            assertFalse($adapter->eval(|%2015->hasSubsecondWithAtLeastPrecision(1)));
        }
        ",
        "f__Boolean_1_",
    );
    assert_eq!(r, Value::Boolean(true));
}

// ===========================================================================
// Root-level surveyor regression locks — run on every `cargo test` (no #[ignore]).
//
// These three tests emulate the Java coverage matrix as a strict
// regression gate: every discovered test must PASS. There is no
// "minimum baseline" tolerance — fail / error counts must be zero.
// Each invokes one surveyor entry point from the root package (`::`) so
// newly-added test files in any subpackage are picked up automatically,
// with no hardcoded package list to maintain.
//
// On failure the test prints every non-PASS result with its FQN and
// failure message — that is the entire diagnostic surface, intentionally.
// To investigate locally:
//   cargo test -p legend-pure-runtime --test eval_tests \
//     <test_name> -- --nocapture
//
// Cost: ~10–30s combined because each runs every test in scope on every
// build. That's the price of a regression-proof gate.
// ===========================================================================

/// Pretty-print every non-PASS `TestResult` row from a `TestReport` heap
/// object as `[STATUS] fqn — message-first-line`. Used by the three
/// strict-assertion surveyor locks below to surface failure detail
/// directly inside the panic message.
fn dump_non_pass_results(
    evaluator: &Evaluator,
    report_id: &legend_pure_runtime::heap::ObjectHandle,
) -> String {
    use std::fmt::Write as _;
    let results = evaluator
        .heap()
        .get_property_values(report_id, "results")
        .unwrap_or_else(|_| im_rc::Vector::new());
    let mut out = String::new();
    for v in &results {
        let Value::Object(rid) = v else { continue };
        let status = evaluator
            .heap()
            .get_property_values(rid, "status")
            .ok()
            .and_then(|v| v.iter().next().cloned());
        let bucket = match &status {
            Some(Value::EnumValue { member, .. }) if member.as_str() == "PASS" => continue,
            Some(Value::EnumValue { member, .. }) => member.to_string(),
            _ => "?".into(),
        };
        let fqn = evaluator
            .heap()
            .get_property_values(rid, "fqn")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let msg = evaluator
            .heap()
            .get_property_values(rid, "message")
            .ok()
            .and_then(|v| v.iter().next().cloned())
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let summary = msg
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim_matches('"')
            .trim();
        let _ = writeln!(out, "  [{bucket}] {fqn}\n      {summary}");
    }
    out
}

#[test]
fn eval_surveyor_root_strict_pass() {
    // Strict gate on `runTestsFromPath('Root', '')` — no <<test.Test>>
    // discovered from the root package may FAIL or ERROR. Pass and skip
    // counts are not asserted: pass count drifts naturally as new tests
    // land, and skips are intentional (manifest exclusions or
    // representational gaps). If this turns red, fix the underlying tests.
    //
    // The relational-store extension and its DSL populators are wired
    // here because the platform now ships `<<test.Test>>` functions
    // under `meta::relational::tests::*` that exercise `executeInDb`
    // and friends; without the extension these would error at native
    // dispatch.
    let model = compile_with_platform("");
    let relational_ext = legend_pure_store_relational_runtime::RelationalStoreExtension;
    let registry = NativeRegistry::with_extensions(&[&relational_ext]);
    let mut evaluator = Evaluator::new(&model, &registry);
    let mapping_pop = legend_pure_dsl_mapping_runtime::MappingDSLPopulator;
    let db_pop = legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator;
    let cm_pop = legend_pure_dsl_relational_runtime::RelationalClassMappingDSLPopulator;
    legend_pure_runtime::dsl::run_populators(
        &model,
        evaluator.heap_mut(),
        &[&mapping_pop, &db_pop, &cm_pop],
    );

    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runTestsFromPath",
            &[Value::String("Root".into()), Value::String("".into())],
        )
        .expect("runTestsFromPath('Root', '') must succeed");
    let Value::Object(ref report_id) = report else {
        panic!("runTestsFromPath returned non-object: {report:?}");
    };
    let fail = read_report_counter(&evaluator, report_id, "failCount");
    let error = read_report_counter(&evaluator, report_id, "errorCount");

    if fail != 0 || error != 0 {
        let detail = dump_non_pass_results(&evaluator, report_id);
        panic!("<<test.Test>> root surveyor: fail={fail} error={error}\n{detail}");
    }
}

#[test]
fn eval_pct_essential_strict_pass() {
    // Strict gate on `runPCTTests(::, '/platform/pure/essential/', …)` —
    // every <<PCT.test>> sourced from /platform/pure/essential/ must PASS
    // (after applying the bundled Rust-port exclusions in
    // crates/runtime/resources/pct_grammar_rust_native.json — those are
    // representational limits like the i16 year clamp, not behavioural
    // gaps). No baseline tolerance.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let root = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[Value::String("Root".into()), Value::String("::".into())],
        )
        .expect("pathToElement('Root') must resolve to ::");
    let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTests",
            &[
                root,
                Value::String("/platform/pure/essential/".into()),
                adapter,
                exclusions,
            ],
        )
        .expect("runPCTTests on /platform/pure/essential/ must succeed");
    let Value::Object(ref report_id) = report else {
        panic!("runPCTTests returned non-object: {report:?}");
    };
    let fail = read_report_counter(&evaluator, report_id, "failCount");
    let error = read_report_counter(&evaluator, report_id, "errorCount");

    if fail != 0 || error != 0 {
        let detail = dump_non_pass_results(&evaluator, report_id);
        panic!("PCT essential surveyor: fail={fail} error={error}\n{detail}");
    }
}

#[test]
fn eval_pct_grammar_functions_strict_pass() {
    // Strict gate on `runPCTTests(::, '/platform/pure/grammar/functions/', …)` —
    // grammar-layer counterpart to the essential lock. No baseline tolerance.
    let model = compile_with_platform("");
    let registry = NativeRegistry::standard();
    let mut evaluator = Evaluator::new(&model, &registry);

    let root = evaluator
        .call(
            "meta::pure::functions::meta::pathToElement",
            &[Value::String("Root".into()), Value::String("::".into())],
        )
        .expect("pathToElement('Root') must resolve to ::");
    let (adapter, exclusions) = pct_canary_args_with_rust_exclusions(&model);
    let report = evaluator
        .call(
            "meta::pure::test::surveyor::runPCTTests",
            &[
                root,
                Value::String("/platform/pure/grammar/functions/".into()),
                adapter,
                exclusions,
            ],
        )
        .expect("runPCTTests on /platform/pure/grammar/functions/ must succeed");
    let Value::Object(ref report_id) = report else {
        panic!("runPCTTests returned non-object: {report:?}");
    };
    let fail = read_report_counter(&evaluator, report_id, "failCount");
    let error = read_report_counter(&evaluator, report_id, "errorCount");

    if fail != 0 || error != 0 {
        let detail = dump_non_pass_results(&evaluator, report_id);
        panic!("PCT grammar/functions surveyor: fail={fail} error={error}\n{detail}");
    }
}

// ---------------------------------------------------------------------------
// getAll reachability walk (Step 6 — frame-scoped instance discovery)
// ---------------------------------------------------------------------------

/// `Class.all()` invoked from a function whose `let` bindings are still
/// alive must surface those instances. Pinned because the post-Rc heap
/// has no global registry — instance enumeration relies on walking the
/// variable context.
#[test]
fn get_all_finds_user_instances_reachable_from_let_bindings() {
    let model = compile_with_platform(
        "Class my::pkg::Trade\n\
         {\n\
             ticker: String[1];\n\
         }\n\
         function my::pkg::countTrades(): Integer[1]\n\
         {\n\
             let a = ^my::pkg::Trade(ticker='AAPL');\n\
             let b = ^my::pkg::Trade(ticker='MSFT');\n\
             let c = ^my::pkg::Trade(ticker='GOOG');\n\
             my::pkg::Trade.all()->size()\n\
         }",
    );
    let mut evaluator = legend_pure_runtime::eval::Evaluator::new_default(&model);
    let result = evaluator
        .call("my::pkg::countTrades", &[])
        .expect("countTrades should evaluate");
    assert_eq!(
        result,
        Value::Integer(3),
        "getAll should report all 3 in-scope Trade instances"
    );
}

/// Conversely, when no Trade instance exists in any reachable root,
/// `Trade.all()` must return empty — RAII has freed them.
#[test]
fn get_all_returns_empty_when_no_user_instances_reachable() {
    let model = compile_with_platform(
        "Class my::pkg::Widget\n\
         {\n\
             id: Integer[1];\n\
         }\n\
         function my::pkg::countWidgets(): Integer[1]\n\
         {\n\
             my::pkg::Widget.all()->size()\n\
         }",
    );
    let mut evaluator = legend_pure_runtime::eval::Evaluator::new_default(&model);
    let result = evaluator
        .call("my::pkg::countWidgets", &[])
        .expect("countWidgets should evaluate");
    assert_eq!(
        result,
        Value::Integer(0),
        "getAll should report 0 instances when none are reachable"
    );
}

/// Metamodel `.all()` must keep working — e.g. `Class.all()` returns
/// every compiled class via the metamodel arena.
#[test]
fn get_all_class_metamodel_finds_user_classes() {
    let model = compile_with_platform(
        "Class my::pkg::Foo {}\n\
         Class my::pkg::Bar {}\n\
         function my::pkg::namesOfClasses(): String[*]\n\
         {\n\
             meta::pure::metamodel::type::Class.all()->map(c | $c.name)\n\
         }",
    );
    let mut evaluator = legend_pure_runtime::eval::Evaluator::new_default(&model);
    let result = evaluator
        .call("my::pkg::namesOfClasses", &[])
        .expect("namesOfClasses should evaluate");
    let names: Vec<String> = match result {
        Value::Collection(items) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        Value::String(s) => vec![s.to_string()],
        Value::Unit => Vec::new(),
        other => panic!("expected Collection of String, got {other:?}"),
    };
    assert!(
        names.iter().any(|n| n == "Foo"),
        "Class.all() should include user-defined Foo; got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Bar"),
        "Class.all() should include user-defined Bar; got {names:?}"
    );
}

// ===========================================================================
// platform_precise_primitives — embed + cast-time constraint enforcement
// ===========================================================================

/// Locks the `platform_precise_primitives` repo embedding: every type
/// declared in `precisePrimitives.pure` (sized integer variants,
/// `Varchar(x)`, `Numeric(p, s)`, `Timestamp`, `Float4`, `Double`)
/// must resolve to an `Element::PrimitiveType` after platform compile.
#[test]
fn precise_primitives_metamodel_resolves_in_platform() {
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    let primitives = [
        "TinyInt",
        "UTinyInt",
        "SmallInt",
        "USmallInt",
        "Int",
        "UInt",
        "BigInt",
        "UBigInt",
        "Varchar",
        "Timestamp",
        "Float4",
        "Double",
        "Numeric",
    ];
    for name in primitives {
        let segments: Vec<smol_str::SmolStr> = ["meta", "pure", "precisePrimitives", name]
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("precise primitive missing: {name}"));
        assert!(
            matches!(model.get_element(id), Element::PrimitiveType(_)),
            "{name} should be a PrimitiveType element",
        );
    }
}

/// Parametric precise primitives (`Varchar(x:Integer[1])`,
/// `Numeric(precision:Integer[1], scale:Integer[1])`) must carry their
/// type-variable parameters through the compile pipeline so that
/// cast-time constraint evaluation can bind them.
#[test]
fn precise_primitives_parametric_carry_type_variable_params() {
    use legend_pure_parser_pure::model::Element;
    let model = compile_with_platform("");
    for (name, expected_param_count) in [
        ("Varchar", 1usize),
        ("Numeric", 2usize),
        ("TinyInt", 0usize),
    ] {
        let segments: Vec<smol_str::SmolStr> = ["meta", "pure", "precisePrimitives", name]
            .iter()
            .map(|s| smol_str::SmolStr::new(*s))
            .collect();
        let id = model
            .resolve_by_path(&segments)
            .expect("primitive resolves");
        match model.get_element(id) {
            Element::PrimitiveType(p) => {
                assert_eq!(
                    p.type_variable_parameters.len(),
                    expected_param_count,
                    "{name} should declare {expected_param_count} type-variable parameter(s)",
                );
            }
            other => panic!("{name} is not PrimitiveType: {other:?}"),
        }
    }
}

/// Cast-time happy path: an in-range integer survives `cast(@TinyInt)`
/// and the same value comes back. Locks that constraint evaluation
/// reaches the precise-primitive constraint block, calls the platform
/// `pow` native, and produces a true predicate.
#[test]
fn precise_primitives_cast_tinyint_in_range() {
    let result = eval_pure(
        r"
        function test::f(): Integer[1] {
            42->cast(@meta::pure::precisePrimitives::TinyInt);
        }
        ",
        "f__Integer_1_",
    );
    assert_eq!(result, Value::Integer(42));
}

/// Cast-time negative path: an out-of-range integer must raise a
/// constraint-violation error from the `[$this >= -pow(2,7) && ...]`
/// block. Bound is `[-128, 127]`; 200 is well outside.
#[test]
fn precise_primitives_cast_tinyint_out_of_range_violates_constraint() {
    let err = eval_pure_err(
        r"
        function test::f(): Integer[1] {
            200->cast(@meta::pure::precisePrimitives::TinyInt);
        }
        ",
        "f__Integer_1_",
    );
    assert!(
        err.to_lowercase().contains("constraint") || err.to_lowercase().contains("violated"),
        "expected a constraint-violation error, got: {err}"
    );
}

/// Parametric `Varchar(x)` happy path: a string within the supplied
/// length bound passes the constraint `$this->length() <= $x`.
#[test]
fn precise_primitives_cast_varchar_within_length() {
    let result = eval_pure(
        r"
        function test::f(): String[1] {
            'abc'->cast(@meta::pure::precisePrimitives::Varchar(5));
        }
        ",
        "f__String_1_",
    );
    assert_eq!(result, Value::String(SmolStr::new("abc")));
}

/// Parametric `Varchar(x)` negative path: a string longer than the
/// supplied bound must raise a constraint-violation error. Locks both
/// the parametric type-variable binding (`@Varchar(3)` → `x = 3`) and
/// the constraint evaluation against the call-site bound.
#[test]
fn precise_primitives_cast_varchar_exceeds_length_violates_constraint() {
    let err = eval_pure_err(
        r"
        function test::f(): String[1] {
            'abcdef'->cast(@meta::pure::precisePrimitives::Varchar(3));
        }
        ",
        "f__String_1_",
    );
    assert!(
        err.to_lowercase().contains("constraint") || err.to_lowercase().contains("violated"),
        "expected a constraint-violation error, got: {err}"
    );
}

/// Parametric `Numeric(precision, scale)` happy path: a Decimal that
/// fits within the precision/scale bound passes. Notably, this primitive's
/// constraint body delegates to a *user-defined* helper function
/// (`meta::pure::precisePrimitives::validate`) which itself calls
/// `floor`, `toString`, `length`, and `fractionDigits` — so this test
/// also locks that constraint-body lowering can call user functions
/// declared in the same repo.
#[test]
fn precise_primitives_cast_numeric_within_precision() {
    let result = eval_pure(
        r"
        function test::f(): Decimal[1] {
            123.45d->cast(@meta::pure::precisePrimitives::Numeric(5, 2));
        }
        ",
        "f__Decimal_1_",
    );
    match result {
        Value::Decimal(_) => {}
        other => panic!("expected Decimal value, got {other:?}"),
    }
}

/// Parametric `Numeric(precision, scale)` negative path: a Decimal
/// whose precision exceeds the bound must raise a constraint-violation
/// error. `123.45` has 5 significant digits; `Numeric(3, 1)` permits at
/// most 3 — so the cast must fail.
#[test]
fn precise_primitives_cast_numeric_exceeds_precision_violates_constraint() {
    let err = eval_pure_err(
        r"
        function test::f(): Decimal[1] {
            123.45d->cast(@meta::pure::precisePrimitives::Numeric(3, 1));
        }
        ",
        "f__Decimal_1_",
    );
    assert!(
        err.to_lowercase().contains("constraint") || err.to_lowercase().contains("violated"),
        "expected a constraint-violation error, got: {err}"
    );
}

// toMultiplicity (T-20260512-05) behaviour is fully specified by the
// 11 `<<test.Test>>` functions in
// `platform/pure/essential/lang/cast/toMultiplicity.pure`. The
// stack-agnostic `eval_surveyor_root_strict_pass` test above already
// gates `fail == 0 && error == 0` across every `<<test.Test>>` in the
// model, which includes those. A dedicated Rust seam here would only
// duplicate that signal.
