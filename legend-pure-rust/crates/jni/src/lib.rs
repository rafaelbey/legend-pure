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

//! JNI Evaluator Bridge
//!
//! Provides the Rust implementation of the JNI endpoints for the Java `PureRustEvaluator`.

#![deny(missing_docs)]

mod codegen;
mod context;
mod conversion;

use jni::JNIEnv;
use jni::objects::{JClass, JObjectArray, JString};
use jni::sys::jlong;

use crate::context::JniContext;
use legend_pure_core_platform::repo::{self, Repo};

/// Initializes the `JniContext` by loading the platform models.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jlong {
    let result = std::panic::catch_unwind(|| {
        let repos = Repo::default_embedded();
        let auto_imports: Vec<smol_str::SmolStr> =
            legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
                .iter()
                .map(|s| smol_str::SmolStr::new(*s))
                .collect();
        let model = match repo::load(&repos, &auto_imports) {
            Ok(model) => model,
            Err(partial) => {
                let msg = partial
                    .errors
                    .iter()
                    .map(|e| e.message.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(msg);
            }
        };

        Ok(Box::into_raw(Box::new(JniContext::new(model))) as jlong)
    });

    match result {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(e)) => {
            let _ = env.throw_new("org/finos/legend/pure/rust/PureRustException", e);
            0
        }
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
        let rust_args =
            conversion::java_to_rust_args(&mut env, &args, context_ptr).unwrap_or_default();

        context.evaluate(&path_str, &rust_args)
    }));

    match result {
        Ok(Ok(val)) => conversion::rust_to_java_result(&mut env, &val, context_ptr)
            .unwrap_or_else(|_| jni::objects::JObject::null()),
        Ok(Err(err)) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustEvaluationException",
                err,
            );
            jni::objects::JObject::null()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during evaluation",
            );
            jni::objects::JObject::null()
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
) -> jni::objects::JObject<'local> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = unsafe { &mut *(context_ptr as *mut JniContext) };
        let prop_str: String = env.get_string(&property_name).unwrap().into();
        let rust_args =
            conversion::java_to_rust_args(&mut env, &args, context_ptr).unwrap_or_else(|_| vec![]);

        context.get_property(complex_ptr, &prop_str, &rust_args)
    }));

    match result {
        Ok(Ok(val)) => conversion::rust_to_java_result(&mut env, &val, context_ptr)
            .unwrap_or_else(|_| jni::objects::JObject::null()),
        Ok(Err(err)) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustEvaluationException",
                err,
            );
            jni::objects::JObject::null()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during property evaluation",
            );
            jni::objects::JObject::null()
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
            .unwrap_or_default();
        let mut props: Vec<(String, legend_pure_runtime::value::Value)> =
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
