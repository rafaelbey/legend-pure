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

//! Test support native functions: `assert`, `executeTest`, `executePCTTest`.
//!
//! These support the PCT (Pure Compatibility Tests) and user-authored
//! Pure test suites via the `meta::pure::test::surveyor` orchestrator.

use std::time::Instant;

use legend_pure_parser_pure::types::ValueSpec;
use smol_str::SmolStr;

use crate::error::{PureException, PureExceptionKind, PureRuntimeError};
use crate::native::{EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, expect_args};
use crate::value::{FunctionValue, Value};

// ---------------------------------------------------------------------------
// assert
// ---------------------------------------------------------------------------

/// Pure `assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]`
///
/// Asserts a boolean condition. If false, evaluates the message expression
/// and returns an error with that message. The message is only evaluated
/// on failure — short-circuit via direct `ctx.evaluate` calls.
#[derive(Debug)]
pub struct Assert;

impl NativeFunction for Assert {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("assert", args, 2)?;
        // Condition is plain `Boolean[1]` — force directly. Message is
        // typed `Function<{->String[1]}>`, so the compiler wraps it in a
        // zero-parameter lambda; `force_thunk` unwraps that only on failure
        // so a passing assertion never builds its message.
        let condition = ctx.evaluate(&args[0])?.as_boolean()?;
        if condition {
            Ok(Evaluated::new(Value::Boolean(true)))
        } else {
            let msg_val = crate::native::force_thunk(&args[1], ctx)?.into_value();
            let msg = msg_val.as_string().map_or_else(
                |_| "Assertion failed".to_string(),
                smol_str::SmolStr::to_string,
            );
            Err(PureRuntimeError::AssertionFailed(msg).into())
        }
    }

    fn signature(&self) -> &'static str {
        "assert(Boolean[1], Function<{->String[1]}>[1]): Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// executeTest
// ---------------------------------------------------------------------------

/// Pure `executeTest(testFn:Function<{->Any[*]}>[1]): TestResult[1]`
///
/// Executes a zero-arg test function in a sandbox, catches exceptions,
/// measures elapsed time, and returns a fully-populated `TestResult` heap
/// object.
///
/// Outcome classification:
/// - `Ok(_)` → PASS
/// - `Err(AssertionFailed)` or `Err(ConstraintViolation)` → FAIL
/// - `Err(ExecutionError(FunctionNotFound(_)))` → SKIP
/// - Any other error → ERROR
#[derive(Debug)]
pub struct ExecuteTest;

impl NativeFunction for ExecuteTest {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("executeTest", args, 1)?;
        let test_fn_val = ctx.evaluate(&args[0])?.into_value();

        let fqn = function_fqn(&test_fn_val, ctx);
        let start = Instant::now();
        let result = ctx.call_function(&test_fn_val, &[]);
        let elapsed = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);

        let (status, message) = classify_outcome(result);
        build_test_result(ctx, fqn, status, elapsed, message)
    }

    fn signature(&self) -> &'static str {
        "executeTest(testFn:Function<{->Any[*]}>[1]): TestResult[1]"
    }
}

// ---------------------------------------------------------------------------
// executePCTTest
// ---------------------------------------------------------------------------

/// Pure `executePCTTest(testFn:Function<Any>[1], adapter:Function<Any>[1], exclusions:Map<Function<Any>,String>[1]): TestResult[1]`
///
/// Executes a PCT test function, injecting `adapter` as its sole argument.
///
/// # Exclusion-map semantics
///
/// The Pure-level type is `Map<Function<Any>, String>` — keyed by the
/// failing test function, valued by the *expected* error message. Java
/// Pure keys by Function identity; here we key by rendered FQN ([`function_fqn`])
/// since [`crate::value::ValueKey`] has no `Function` variant. Manifests
/// loaded via [`LoadPCTManifest`] use the same FQN-keyed shape, so the
/// two sides agree.
///
/// - Test fails (FAIL or ERROR) AND its FQN is in `exclusions` AND the
///   actual error message equals the expected one → flip to **PASS**
///   (expected failure tolerated).
/// - Test fails AND its FQN is in `exclusions` but the messages diverge
///   → keep the original FAIL/ERROR but rewrite the message to "PCT
///   exclusion mismatch: expected '<expected>' got '<actual>'" so a
///   stale exclusion shows up as a real failure rather than silently
///   absorbing a different bug.
/// - Test passes AND its FQN is in `exclusions` → flip to **FAIL** with
///   "PCT exclusion needs rebase: test now passes" — the exclusion is
///   stale and should be removed from the manifest.
#[derive(Debug)]
pub struct ExecutePCTTest;

impl NativeFunction for ExecutePCTTest {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("executePCTTest", args, 3)?;
        let test_fn_val = ctx.evaluate(&args[0])?.into_value();
        let adapter = ctx.evaluate(&args[1])?.into_value();
        let exclusions_val = ctx.evaluate(&args[2])?.into_value();

        let fqn = function_fqn(&test_fn_val, ctx);
        let start = Instant::now();
        let result = ctx.call_function(&test_fn_val, &[adapter]);
        let elapsed = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);

        let (status, message) = classify_outcome(result);
        let (final_status, final_message) = apply_exclusion(&fqn, status, message, &exclusions_val);
        build_test_result(ctx, fqn, final_status, elapsed, final_message)
    }

    fn signature(&self) -> &'static str {
        "executePCTTest(testFn:Function<Any>[1], adapter:Function<Any>[1], exclusions:Map<Function<Any>,String>[1]): TestResult[1]"
    }
}

/// Apply the PCT exclusion contract documented on [`ExecutePCTTest`].
///
/// `exclusions` is the Pure-level `Map<Function<Any>, String>` argument as
/// received by `executePCTTest`. Non-Map shapes are treated as "no
/// exclusions" rather than erroring — the surveyor passes whatever the
/// manifest produced, and a missing/empty map should not break dispatch.
fn apply_exclusion(
    fqn: &str,
    status: &'static str,
    message: Option<String>,
    exclusions: &Value,
) -> (&'static str, Option<String>) {
    use crate::value::ValueKey;

    let Value::Map(state) = exclusions else {
        return (status, message);
    };
    let entries = &state.borrow().entries;
    let expected = entries
        .get(&ValueKey::String(SmolStr::new(fqn)))
        .and_then(|v| match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        });
    match (status, expected) {
        // Test passed but the manifest expected it to fail — the exclusion
        // is stale.
        (STATUS_PASS, Some(_)) => (
            STATUS_FAIL,
            Some(format!(
                "PCT exclusion needs rebase: test now passes ({fqn})"
            )),
        ),
        // Test failed and the manifest expected exactly this failure → PASS.
        // Match policy: exact equality OR substring containment, so an
        // exclusion entry can pin just the core PureRuntimeError text
        // without having to reproduce the full multi-line "Execution
        // error\n\"...\"\nFull Stack: ..." Display shape. Substring
        // match is meaningful because the runtime error message comes
        // first in every PureException Display — there's no false-
        // positive risk from an unrelated message also appearing later.
        (STATUS_FAIL | STATUS_ERROR, Some(expected_msg)) => {
            let actual = message.as_deref().unwrap_or("");
            if actual == expected_msg || actual.contains(expected_msg.as_str()) {
                (STATUS_PASS, None)
            } else {
                // Keep the original bucket but make the divergence loud.
                (
                    status,
                    Some(format!(
                        "PCT exclusion mismatch: expected '{expected_msg}' got '{actual}'"
                    )),
                )
            }
        }
        // No exclusion entry, or test passed and not in exclusions — keep
        // the original classification.
        _ => (status, message),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Status enum values in the same `"EnumSimpleName.VALUE"` form produced by
/// [`Evaluator::eval_enum_value`](crate::eval::Evaluator) and the enum-value
/// branch of `eval_element_property`. The surveyor compares with
/// `$r.status == TestStatus.PASS` — the two sides must agree string-for-string.
// Member names in `meta::pure::test::surveyor::TestStatus`. Stored as the
// `member` field of the `Value::EnumValue` this native writes to the
// `TestResult.status` slot so surveyor-side `$r.status == TestStatus.PASS`
// comparisons structurally agree (same `enum_id` + same `member`).
//
// `SKIP` intentionally has no constant here — this native never produces a
// SKIP result. See [`classify_outcome`] for the policy: SKIP is reserved
// for the surveyor's exclusion-map path, not a bucket for runtime gaps.
const STATUS_PASS: &str = "PASS";
const STATUS_FAIL: &str = "FAIL";
const STATUS_ERROR: &str = "ERROR";

/// Extract a human-readable FQN from a function-valued argument.
///
/// - Compiled elements render via [`build_element_path`](crate::model_utils::build_element_path)
///   so surveyor reports carry the same `a::b::funcName` format users see in
///   source, not the mangled FQN stored in `ElementNode::name`.
/// - Element refs (e.g. a function passed via `$t->cast(@Function<…>)`) are
///   also path-rendered.
/// - Lambdas collapse to `"<lambda>"`.
/// - Everything else is `"<unknown>"`.
fn function_fqn(callable: &Value, ctx: &dyn EvalContextTrait) -> String {
    use crate::model_utils::build_element_path;
    match callable {
        Value::Function(fv) => match fv.as_ref() {
            FunctionValue::Compiled(id) => build_element_path(ctx.model(), *id, "::", false),
            FunctionValue::Lambda(_) => "<lambda>".to_string(),
        },
        Value::Element(id) => build_element_path(ctx.model(), *id, "::", false),
        _ => "<unknown>".to_string(),
    }
}

/// Classify a function call result into (`status_name`, `optional_message`).
///
/// Status values are *member* names (`"PASS"`, `"FAIL"`, `"ERROR"`,
/// `"SKIP"`) that [`build_test_result`] pairs with the `TestStatus`
/// enumeration's [`ElementId`] to build the [`Value::EnumValue`] the
/// surveyor stores in `TestResult.status`.
///
/// # Classification policy
///
/// - `AssertionFailed` / `ConstraintViolation` → FAIL. The test's
///   behavioural contract was violated — the assertion evaluated to
///   `false` or a class constraint tripped.
/// - Everything else → ERROR. The runtime did not complete the test's
///   execution, whatever the cause (type mismatch, missing native,
///   cast failure, division by zero, …).
///
/// **SKIP is deliberately not produced by this function.** A SKIP
/// result in Pure's surveyor means "this test is expected to be
/// excluded" — driven by `executePCTTest`'s `exclusions` map or an
/// explicit `test.ExcludePlatform` annotation. Skipping on
/// `FunctionNotFound` (by kind or by message substring) would hide
/// real runtime gaps as "intentional exclusions", breaking the Java-
/// parity signal the ERROR bucket exists to surface.
fn classify_outcome(
    result: Result<Value, crate::error::PureException>,
) -> (&'static str, Option<String>) {
    match result {
        Ok(_) => (STATUS_PASS, None),
        Err(e) => {
            let msg = e.to_string();
            match &e.kind {
                PureExceptionKind::AssertionFailed(_)
                | PureExceptionKind::ConstraintViolation { .. } => (STATUS_FAIL, Some(msg)),
                PureExceptionKind::ExecutionError(_) => (STATUS_ERROR, Some(msg)),
            }
        }
    }
}

/// Allocate a `TestResult` heap object and populate its properties.
///
/// The `status` slot holds a `Value::EnumValue` backed by the Pure
/// `meta::pure::test::surveyor::TestStatus` enumeration. The surveyor
/// filter compares `$r.status == TestStatus.PASS` — structural equality
/// requires both sides carry the same `enum_id` + `member`.
fn build_test_result(
    ctx: &mut dyn EvalContextTrait,
    fqn: String,
    status: &str,
    elapsed: i64,
    message: Option<String>,
) -> Result<Evaluated, PureException> {
    // Resolve the `TestStatus` enumeration once per call — the surveyor
    // filter compares `$r.status == TestStatus.PASS`, which requires both
    // sides to carry the same enum_id.
    let test_status_path: [SmolStr; 5] = [
        SmolStr::new_static("meta"),
        SmolStr::new_static("pure"),
        SmolStr::new_static("test"),
        SmolStr::new_static("surveyor"),
        SmolStr::new_static("TestStatus"),
    ];
    let enum_id = ctx
        .model()
        .resolve_by_path(&test_status_path)
        .ok_or_else(|| {
            PureRuntimeError::EvaluationError(
                "testing: meta::pure::test::surveyor::TestStatus not found".into(),
            )
        })?;
    let status_value = Value::EnumValue {
        enum_id,
        member: SmolStr::new(status),
    };

    let heap = ctx.heap_mut();
    let id = heap.alloc_dynamic(crate::m3_paths::TEST_RESULT);
    heap.mutate_add(&id, "fqn", &[Value::String(SmolStr::new(fqn))])?;
    heap.mutate_add(&id, "status", &[status_value])?;
    heap.mutate_add(&id, "elapsed", &[Value::Integer(elapsed)])?;
    if let Some(msg) = message {
        heap.mutate_add(&id, "message", &[Value::String(SmolStr::new(msg))])?;
    }
    Ok(Evaluated::new(Value::Object(id)))
}

// ---------------------------------------------------------------------------
// assertError
// ---------------------------------------------------------------------------

/// Pure `assertError(f:Function<{->Any[*]}>[1], message:String[1],
///                    line:Integer[0..1], column:Integer[0..1]):Boolean[1]`
///
/// Runs `f` (typically `{|expr}`) and asserts it raises a Pure exception
/// whose message equals the expected `message`. The optional `line` /
/// `column` parameters are accepted for signature parity but not matched —
/// source positions from compiled Pure tests don't always line up with the
/// platform expectations, and matching on the message alone covers every
/// test-suite usage.
#[derive(Debug)]
pub struct AssertError;

impl NativeFunction for AssertError {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        if args.len() < 2 {
            return Err(PureRuntimeError::EvaluationError(format!(
                "assertError: expected at least 2 arguments (function, message), got {}",
                args.len()
            ))
            .into());
        }
        let func_val = ctx.evaluate(&args[0])?.into_value();
        let expected = ctx.evaluate(&args[1])?.into_value();
        let expected_msg = match expected {
            Value::String(s) => s.to_string(),
            other => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "assertError: second argument must be String, got {}",
                    other.type_name()
                ))
                .into());
            }
        };
        match ctx.call_function(&func_val, &[]) {
            Ok(_) => Err(PureRuntimeError::AssertionFailed(format!(
                "assertError: expected error '{expected_msg}' but call succeeded"
            ))
            .into()),
            Err(e) => {
                // Use the kind-specific message payload — `AssertionFailed` is
                // the literal assertion text, `ConstraintViolation` renders in
                // Pure's canonical `"Constraint :[<id>] violated in the Class
                // <name>[, Message: <msg>]"` shape (no source-location prefix,
                // no `(PRE)`/`(POST)` for class-kind constraints), and
                // `ExecutionError` unwraps to the underlying error's Display.
                // `e.to_string()` would include the exception-kind prefix
                // (e.g. "Constraint violation: (resource:… line:… column:…)"),
                // which the Pure platform tests never spell out verbatim.
                let actual = match &e.kind {
                    PureExceptionKind::AssertionFailed(m) => m.clone(),
                    PureExceptionKind::ConstraintViolation {
                        constraint_id,
                        constraint_kind,
                        owner,
                        message,
                    } => {
                        let kind_label = match constraint_kind {
                            crate::error::ConstraintKind::Class => "",
                            crate::error::ConstraintKind::Pre => "(PRE) ",
                            crate::error::ConstraintKind::Post => "(POST) ",
                        };
                        match message {
                            Some(m) => format!(
                                "Constraint {kind_label}:[{constraint_id}] violated in the Class {owner}, Message: {m}"
                            ),
                            None => format!(
                                "Constraint {kind_label}:[{constraint_id}] violated in the Class {owner}"
                            ),
                        }
                    }
                    PureExceptionKind::ExecutionError(err) => err.to_string(),
                };
                if actual == expected_msg {
                    Ok(Evaluated::new(Value::Boolean(true)))
                } else {
                    Err(PureRuntimeError::AssertionFailed(format!(
                        "assertError: expected '{expected_msg}' but got '{actual}'"
                    ))
                    .into())
                }
            }
        }
    }

    fn signature(&self) -> &'static str {
        "assertError(f:Function<{->Any[*]}>[1], message:String[1], line:Integer[0..1], column:Integer[0..1]):Boolean[1]"
    }
}

// ---------------------------------------------------------------------------
// PCT adapter discovery
// ---------------------------------------------------------------------------

/// Find a PCT adapter Function by its `PCT.adapterName` tag.
///
/// An adapter is any Function carrying:
/// 1. The `<<PCT.adapter>>` stereotype — `profile == meta::pure::test::pct::PCT`
///    and `value == "adapter"`.
/// 2. A `PCT.adapterName='<name>'` tagged value — same profile, `tag ==
///    "adapterName"`, `value == name`.
///
/// The shipped in-memory adapter
/// `meta::pure::test::pct::testAdapterForInMemoryExecution` carries
/// `PCT.adapterName='In-Memory'`. New adapters can register themselves
/// purely in Pure code by adding the same stereotype + tag — no Rust
/// changes required.
///
/// Returns `None` if the PCT profile itself doesn't resolve, or if no
/// Function in the model matches both the stereotype and the tag value.
#[must_use]
pub fn find_pct_adapter(
    model: &legend_pure_parser_pure::model::PureModel,
    adapter_name: &str,
) -> Option<legend_pure_parser_pure::ids::ElementId> {
    use legend_pure_parser_pure::ids::ElementId;
    use legend_pure_parser_pure::model::Element;

    let pct_profile = crate::m3_paths::resolve(model, "meta::pure::test::pct::PCT")?;

    for chunk in &model.chunks {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            let has_adapter_stereotype = func
                .stereotypes
                .iter()
                .any(|s| s.profile == pct_profile && s.value == "adapter");
            if !has_adapter_stereotype {
                continue;
            }
            let name_match = func.tagged_values.iter().any(|t| {
                t.profile == pct_profile && t.tag == "adapterName" && t.value == adapter_name
            });
            if name_match {
                return Some(ElementId::InstanceId {
                    chunk_id: chunk.chunk_id,
                    local_idx,
                });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// loadPCTManifest
// ---------------------------------------------------------------------------

/// Pure `loadPCTManifest(manifestPath:String[1]):PCTManifest[1]`
///
/// Reads a PCT manifest JSON file, resolves the adapter FQN to a Function
/// element, and builds a heap `PCTManifest` object carrying the adapter
/// plus the `exclusions` map (test-FQN → expected-error-message).
///
/// # Lookup order
///
/// 1. **Embedded platform manifests** via [`legend_pure_core_platform::repo::find_manifest`]
///    over [`legend_pure_core_platform::repo::Repo::default_embedded`].
///    Both shipped platform manifests
///    (`pct_essential_native.json`, `pct_grammar_native.json`) ship inside
///    the binary and resolve by suffix match against the canonical
///    `/platform/pure/...` resource URL.
/// 2. **Filesystem fallback** for user-supplied paths.
///
/// # Manifest shape
///
/// ```json
/// {
///   "adapter": "meta::pure::test::pct::testAdapterForInMemoryExecution_Function_1__X_o_",
///   "exclusions": [
///     { "test": "meta::pure::functions::math::tests::operation::testFoo<Z|y>_Function_1__Boolean_1_",
///       "message": "expected error string" }
///   ]
/// }
/// ```
///
/// # Map representation
///
/// The Pure-level type is `Map<Function<Any>, String>` but [`ValueKey`] has
/// no `Function` variant — `executePCTTest` looks up exclusions by the
/// rendered FQN of the test function, so the map keys are
/// [`ValueKey::String`]`(test_fqn)` and values are [`Value::String`]`(expected_message)`.
/// Each exclusion's `test` field is also resolved against the model so a
/// typo in the manifest fails fast at load time rather than silently
/// missing every lookup later.
#[derive(Debug)]
pub struct LoadPCTManifest;

impl NativeFunction for LoadPCTManifest {
    fn execute(
        &self,
        args: &[ValueSpec],
        ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        expect_args("loadPCTManifest", args, 1)?;
        let path_val = ctx.evaluate(&args[0])?.into_value();
        let path = match path_val {
            Value::String(s) => s.to_string(),
            other => {
                return Err(PureRuntimeError::EvaluationError(format!(
                    "loadPCTManifest: expected String path, got {}",
                    other.type_name()
                ))
                .into());
            }
        };
        let raw = read_manifest_text(&path)?;
        let parsed: ManifestJson = serde_json::from_str(&raw).map_err(|e| {
            PureRuntimeError::EvaluationError(format!(
                "loadPCTManifest: malformed JSON in '{path}': {e}"
            ))
        })?;
        build_pct_manifest(ctx, &path, &parsed)
    }

    fn signature(&self) -> &'static str {
        "loadPCTManifest(manifestPath:String[1]):PCTManifest[1]"
    }
}

#[derive(serde::Deserialize)]
struct ManifestJson {
    adapter: String,
    #[serde(default)]
    exclusions: Vec<ManifestExclusion>,
}

#[derive(serde::Deserialize)]
struct ManifestExclusion {
    test: String,
    message: String,
}

fn read_manifest_text(path: &str) -> Result<String, PureException> {
    use legend_pure_core_platform::repo::{Repo, find_manifest};
    let repos = Repo::default_embedded();
    if let Some(content) = find_manifest(&repos, path) {
        return Ok(content.to_string());
    }
    std::fs::read_to_string(path).map_err(|e| {
        PureRuntimeError::EvaluationError(format!(
            "loadPCTManifest: cannot read '{path}': {e} (not embedded in platform; \
             supply a real path or use one of the embedded names like 'pct_essential_native.json')"
        ))
        .into()
    })
}

fn resolve_function_fqn(
    ctx: &dyn EvalContextTrait,
    fqn: &str,
    role: &str,
    manifest_path: &str,
) -> Result<legend_pure_parser_pure::ids::ElementId, PureException> {
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    ctx.model().resolve_by_path(&segments).ok_or_else(|| {
        PureRuntimeError::EvaluationError(format!(
            "loadPCTManifest: {role} '{fqn}' from '{manifest_path}' did not resolve in the model"
        ))
        .into()
    })
}

fn build_pct_manifest(
    ctx: &mut dyn EvalContextTrait,
    manifest_path: &str,
    parsed: &ManifestJson,
) -> Result<Evaluated, PureException> {
    use crate::value::{MapState, ValueKey};
    use std::cell::RefCell;
    use std::rc::Rc;

    let adapter_id = resolve_function_fqn(ctx, &parsed.adapter, "adapter", manifest_path)?;
    let adapter_value = Value::Element(adapter_id);

    let mut state = MapState::default();
    for ex in &parsed.exclusions {
        // Validate the test FQN resolves so manifest typos fail loudly at
        // load time, not silently as a never-matching exclusion at lookup.
        let _ = resolve_function_fqn(ctx, &ex.test, "exclusion test", manifest_path)?;
        state.entries.insert(
            ValueKey::String(SmolStr::new(&ex.test)),
            Value::String(SmolStr::new(&ex.message)),
        );
    }
    let exclusions_value = Value::Map(Rc::new(RefCell::new(state)));

    let heap = ctx.heap_mut();
    let id = heap.alloc_dynamic(crate::m3_paths::PCT_MANIFEST);
    heap.mutate_add(&id, "adapter", &[adapter_value])?;
    heap.mutate_add(&id, "exclusions", &[exclusions_value])?;
    Ok(Evaluated::new(Value::Object(id)))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register test-support native functions.
pub fn register(registry: &mut NativeRegistry) {
    registry.register("assert_Boolean_1__Function_1__Boolean_1_", Assert);
    registry.register(
        "assertError_Function_1__String_1__Integer_$0_1$__Integer_$0_1$__Boolean_1_",
        AssertError,
    );
    registry.register("executeTest_Function_1__TestResult_1_", ExecuteTest);
    registry.register(
        "executePCTTest_Function_1__Function_1__Map_1__TestResult_1_",
        ExecutePCTTest,
    );
    registry.register("loadPCTManifest_String_1__PCTManifest_1_", LoadPCTManifest);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{MockCtx, lit_bool, lit_str};

    #[test]
    fn assert_true_passes() {
        let r = Assert
            .execute(&[lit_bool(true), lit_str("unreachable")], &mut MockCtx)
            .unwrap();
        assert_eq!(r.into_value(), Value::Boolean(true));
    }

    #[test]
    fn assert_false_fails_with_message() {
        let err = Assert
            .execute(&[lit_bool(false), lit_str("oops")], &mut MockCtx)
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("oops"), "expected 'oops' in: {msg}");
    }

    #[test]
    fn assert_wrong_arg_count_errors() {
        assert!(Assert.execute(&[lit_bool(true)], &mut MockCtx).is_err());
        assert!(Assert.execute(&[], &mut MockCtx).is_err());
    }

    // ----------------------------------------------------------------------
    // apply_exclusion — Phase 3 contract tests
    // ----------------------------------------------------------------------

    use crate::value::{MapState, ValueKey};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn excl(entries: &[(&str, &str)]) -> Value {
        let mut state = MapState::default();
        for (k, v) in entries {
            state.entries.insert(
                ValueKey::String(SmolStr::new(*k)),
                Value::String(SmolStr::new(*v)),
            );
        }
        Value::Map(Rc::new(RefCell::new(state)))
    }

    #[test]
    fn apply_exclusion_no_entry_keeps_classification() {
        let (s, m) = apply_exclusion("pkg::testFoo", STATUS_FAIL, Some("oops".into()), &excl(&[]));
        assert_eq!(s, STATUS_FAIL);
        assert_eq!(m.as_deref(), Some("oops"));
    }

    #[test]
    fn apply_exclusion_match_flips_fail_to_pass() {
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_FAIL,
            Some("expected".into()),
            &excl(&[("pkg::testFoo", "expected")]),
        );
        assert_eq!(s, STATUS_PASS);
        assert!(m.is_none(), "PASS carries no message: got {m:?}");
    }

    #[test]
    fn apply_exclusion_match_flips_error_to_pass() {
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_ERROR,
            Some("missing native: foo_X_Y_".into()),
            &excl(&[("pkg::testFoo", "missing native: foo_X_Y_")]),
        );
        assert_eq!(s, STATUS_PASS);
        assert!(m.is_none());
    }

    #[test]
    fn apply_exclusion_substring_match_flips_to_pass() {
        // Phase 6 part 2: exclusion messages can be a substring of the
        // actual error so callers don't have to reproduce the full
        // PureException Display shape (which prepends "Execution
        // error\n" and appends "\nFull Stack: …").
        let actual = "Execution error\n\
                      \"Date overflow: parameter 'years' is not in the required range of -19998..=19998\"\n\
                      Full Stack:\n  testAdjustByMonths_… <- adjust.pure line:38";
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_ERROR,
            Some(actual.into()),
            &excl(&[(
                "pkg::testFoo",
                "Date overflow: parameter 'years' is not in the required range of -19998..=19998",
            )]),
        );
        assert_eq!(s, STATUS_PASS);
        assert!(m.is_none());
    }

    #[test]
    fn apply_exclusion_message_mismatch_keeps_bucket_marks_divergence() {
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_FAIL,
            Some("actual".into()),
            &excl(&[("pkg::testFoo", "expected")]),
        );
        assert_eq!(s, STATUS_FAIL);
        let msg = m.expect("mismatch message present");
        assert!(msg.contains("PCT exclusion mismatch"), "msg={msg}");
        assert!(msg.contains("expected"), "msg={msg}");
        assert!(msg.contains("actual"), "msg={msg}");
    }

    #[test]
    fn apply_exclusion_pass_in_exclusions_flips_to_fail_with_rebase_msg() {
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_PASS,
            None,
            &excl(&[("pkg::testFoo", "expected")]),
        );
        assert_eq!(s, STATUS_FAIL);
        let msg = m.expect("rebase message present");
        assert!(msg.contains("PCT exclusion needs rebase"), "msg={msg}");
        assert!(msg.contains("pkg::testFoo"), "msg={msg}");
    }

    #[test]
    fn apply_exclusion_non_map_arg_is_treated_as_empty() {
        let (s, m) = apply_exclusion(
            "pkg::testFoo",
            STATUS_FAIL,
            Some("oops".into()),
            &Value::Unit,
        );
        assert_eq!(s, STATUS_FAIL);
        assert_eq!(m.as_deref(), Some("oops"));
    }
}
