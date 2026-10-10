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

//! JNI boundary: the Rust implementation of the native methods of the Java
//! class `org.finos.legend.pure.rust.PureRustEvaluator`.
//!
//! The context created by `nativeInitContext` holds a bootstrap-only model
//! (`PureModel::bootstrapped`) and the fished native registry; see Step 2
//! of `docs/rust-evaluator-mvp.md`. Every export runs inside
//! `catch_unwind`, so a Rust panic reaches Java as `PureRustException`
//! rather than aborting the JVM.

#![deny(missing_docs)]
// PureException carries a call stack (Vec) so its size is inherently large;
// the evaluator crate allows the same lint for the same reason.
#![allow(clippy::result_large_err)]

mod context;
mod conversion;
mod exception;

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::jlong;

use crate::context::JniContext;
use crate::exception::{ExceptionPayload, pure_exception_to_payload};
use legend_pure_evaluator::error::PureException;
use legend_pure_evaluator::error::PureRuntimeError;
use legend_pure_evaluator::native::NativeRegistry;
use legend_pure_metamodel::model::PureModel;

const PURE_RUST_EVAL_EXC: &str = "org/finos/legend/pure/rust/PureRustEvaluationException";

/// Build the Java `PureRustEvaluationException` from a structured
/// `PureException` and throw it via JNI.
///
/// Best-effort: if any JNI call fails (e.g. the rich constructor isn't
/// found because the Java side is older than this binary), falls back
/// to `env.throw_new(PURE_RUST_EVAL_EXC, message)` which targets the
/// pre-existing message-only constructor. The fallback path is also
/// what end users see if they swap in an older
/// `legend-pure-runtime-rust-evaluator` JAR.
pub(crate) fn throw_pure_exception(env: &mut JNIEnv, exc: &PureException) {
    let payload = pure_exception_to_payload(exc);
    if try_throw_rich(env, &payload).is_err() {
        // Ensure any pending exception from the failed rich-throw
        // attempt is cleared before the fallback throw, otherwise
        // throw_new returns Err and the Java side never sees an
        // exception.
        let _ = env.exception_clear();
        let _ = env.throw_new(PURE_RUST_EVAL_EXC, &payload.message);
    }
}

#[allow(clippy::too_many_lines)] // Linear JNI plumbing — splitting hurts readability
fn try_throw_rich(env: &mut JNIEnv, payload: &ExceptionPayload) -> jni::errors::Result<()> {
    use jni::objects::{JObject, JValue};
    let message = env.new_string(&payload.message)?;
    let kind = env.new_string(payload.kind)?;
    let source_info: JObject<'_> = match &payload.source_info {
        Some(s) => env.new_string(s)?.into(),
        None => JObject::null(),
    };
    let stack_class = env.find_class("java/lang/String")?;
    let empty_string = env.new_string("")?;
    let call_stack_arr =
        env.new_object_array(payload.call_stack.len() as i32, &stack_class, &empty_string)?;
    for (i, frame) in payload.call_stack.iter().enumerate() {
        let frame_str = env.new_string(frame)?;
        env.set_object_array_element(&call_stack_arr, i as i32, &frame_str)?;
    }
    let constraint_id: JObject<'_> = match &payload.constraint_id {
        Some(s) => env.new_string(s)?.into(),
        None => JObject::null(),
    };
    let constraint_kind: JObject<'_> = match payload.constraint_kind {
        Some(s) => env.new_string(s)?.into(),
        None => JObject::null(),
    };
    let owner_fqn: JObject<'_> = match &payload.owner_fqn {
        Some(s) => env.new_string(s)?.into(),
        None => JObject::null(),
    };

    let exc_obj = env.new_object(
        PURE_RUST_EVAL_EXC,
        ExceptionPayload::RICH_CTOR_SIG,
        &[
            JValue::Object(&message.into()),
            JValue::Object(&kind.into()),
            JValue::Object(&source_info),
            JValue::Object(&call_stack_arr.into()),
            JValue::Object(&constraint_id),
            JValue::Object(&constraint_kind),
            JValue::Object(&owner_fqn),
        ],
    )?;
    env.throw(jni::objects::JThrowable::from(exc_obj))?;
    Ok(())
}

/// Initializes a `JniContext` over the bootstrap model and the fished
/// native registry. Nothing is loaded from disk; the model holds chunk 0
/// (primitives, `Any`, `Nil`, the classes of `m3.pure`) and nothing else.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jlong {
    let result = std::panic::catch_unwind(|| {
        let model = PureModel::bootstrapped();
        let registry = NativeRegistry::standard();
        Box::into_raw(Box::new(JniContext::new(model, registry))) as jlong
    });

    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during initialization",
            );
            0
        }
    }
}

/// Evaluates a function by path.
///
/// # Panics
/// Internal Rust panics are caught and rethrown as `PureRustException`
/// across the FFI; the Java side never observes a Rust unwind.
#[allow(clippy::unwrap_used)] // inside catch_unwind — panics translate to Java exceptions
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeEvaluate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
    function_path: JString<'local>,
    args: JObjectArray<'local>,
) -> jni::objects::JObject<'local> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *(context_ptr as *mut JniContext) };
        let path_str: String = env.get_string(&function_path).unwrap().into();
        let rust_args = conversion::java_to_rust_args(&mut env, &args, context_ptr)
            .map_err(|e| bad_arguments("evaluate", &e))?;

        context.evaluate(&path_str, &rust_args)
    }));

    match result {
        Ok(Ok(val)) => conversion::rust_to_java_result(&mut env, &val, context_ptr)
            .unwrap_or_else(|_| JObject::null()),
        Ok(Err(err)) => {
            throw_pure_exception(&mut env, &err);
            JObject::null()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during evaluation",
            );
            JObject::null()
        }
    }
}

/// Evaluates a property on a complex pointer.
///
/// # Panics
/// Internal Rust panics are caught and rethrown as `PureRustException`
/// across the FFI; the Java side never observes a Rust unwind.
#[allow(clippy::unwrap_used)] // inside catch_unwind — panics translate to Java exceptions
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetProperty<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
    property_name: JString<'local>,
    args: JObjectArray<'local>,
) -> JObject<'local> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *(context_ptr as *mut JniContext) };
        let prop_str: String = env.get_string(&property_name).unwrap().into();
        let rust_args = conversion::java_to_rust_args(&mut env, &args, context_ptr)
            .map_err(|e| bad_arguments("getProperty", &e))?;

        context.get_property(complex_ptr, &prop_str, &rust_args)
    }));

    match result {
        Ok(Ok(val)) => conversion::rust_to_java_result(&mut env, &val, context_ptr)
            .unwrap_or_else(|_| JObject::null()),
        Ok(Err(err)) => {
            throw_pure_exception(&mut env, &err);
            JObject::null()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during property evaluation",
            );
            JObject::null()
        }
    }
}

/// Evaluates a property on a complex pointer.
///
/// # Panics
/// Internal Rust panics are caught and rethrown as `PureRustException`
/// across the FFI; the Java side never observes a Rust unwind.
#[allow(clippy::unwrap_used)] // inside catch_unwind — panics translate to Java exceptions
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetClassifier<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
) -> jni::objects::JString<'local> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *(context_ptr as *mut JniContext) };
        context.get_classifier(complex_ptr)
    }));

    match result {
        Ok(Ok(val)) => env
            .new_string(val)
            .unwrap_or_else(|_| env.new_string("").unwrap()),
        Ok(Err(err)) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustEvaluationException",
                err,
            );
            jni::objects::JObject::null().into()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during classifier lookup",
            );
            jni::objects::JObject::null().into()
        }
    }
}

/// Allocate a new heap object from Java-side property values.
///
/// Used by `PureProxyFactory.create(userImpl, iface, eval)` to
/// materialise a hand-written interface implementation as a real
/// runtime instance. `property_names` and `property_values` are
/// parallel arrays — entry `i` of one corresponds to entry `i` of the
/// other.
///
/// # Panics
/// Internal Rust panics are caught and rethrown as `PureRustException`
/// across the FFI; the Java side never observes a Rust unwind.
#[allow(clippy::unwrap_used)] // inside catch_unwind — panics translate to Java exceptions
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeNew<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
    classifier_fqn: JString<'local>,
    property_names: JObjectArray<'local>,
    property_values: JObjectArray<'local>,
) -> jlong {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *(context_ptr as *mut JniContext) };
        let classifier: String = env.get_string(&classifier_fqn).unwrap().into();

        // Parallel-array length check up-front so Java sees a clear
        // error instead of a silent zip().
        let names_len = env.get_array_length(&property_names).unwrap_or(0);
        let values_len = env.get_array_length(&property_values).unwrap_or(0);
        if names_len != values_len {
            return Err(format!(
                "nativeNew: property name/value array length mismatch ({names_len} vs {values_len})"
            ));
        }

        let values = conversion::java_to_rust_args(&mut env, &property_values, context_ptr)
            .map_err(|e| format!("nativeNew: argument could not be marshalled: {e}"))?;
        let mut props: Vec<(String, legend_pure_evaluator::value::Value)> =
            Vec::with_capacity(values.len());
        for (i, value) in values.into_iter().enumerate() {
            let name_obj = env
                .get_object_array_element(&property_names, i as jni::sys::jsize)
                .unwrap();
            let name_jstr = jni::objects::JString::from(name_obj);
            let name: String = env.get_string(&name_jstr).unwrap().into();
            props.push((name, value));
        }

        context.new_object(&classifier, &props)
    }));

    match result {
        Ok(Ok(handle)) => handle,
        Ok(Err(err)) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustEvaluationException",
                err,
            );
            0
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during nativeNew",
            );
            0
        }
    }
}

/// An argument Java handed over could not be marshalled (unknown
/// `PureRustResult.Type`, stale handle, JNI failure). Reported as an
/// execution error instead of being silently dropped.
fn bad_arguments(entry: &str, err: &jni::errors::Error) -> PureException {
    PureException::execution(
        PureRuntimeError::EvaluationError(format!(
            "{entry}: argument could not be marshalled: {err}"
        )),
        legend_pure_metamodel::source_info::SourceInfo::new("<ffi>", 0, 0, 0, 0),
        Vec::new(),
    )
}

/// Number of instance handles the JNI table currently keeps alive.
/// Java asserts this returns to zero after every wrapper is closed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeLiveHandleCount<
    'local,
>(
    mut _env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
) -> jlong {
    if context_ptr == 0 {
        return 0;
    }
    std::panic::catch_unwind(|| {
        let context = unsafe { &*(context_ptr as *const JniContext) };
        i64::try_from(context.live_handle_count()).unwrap_or(i64::MAX)
    })
    .unwrap_or(-1)
}

/// Frees the Evaluator context.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeContext<
    'local,
>(
    mut _env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
) {
    if context_ptr != 0 {
        let _ = std::panic::catch_unwind(|| unsafe {
            drop(Box::from_raw(context_ptr as *mut JniContext));
        });
    }
}

/// Frees a given instance from Evaluator heap.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeInstance<
    'local,
>(
    mut _env: JNIEnv<'local>,
    _class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
) {
    if context_ptr != 0 {
        let _ = std::panic::catch_unwind(|| {
            let context = unsafe { &mut *(context_ptr as *mut JniContext) };
            context.release(complex_ptr);
        });
    }
}
