//! JNI Evaluator Bridge
//!
//! Provides the Rust implementation of the JNI endpoints for the Java `PureRustEvaluator`.

#![deny(missing_docs)]

mod context;
mod conversion;

use jni::JNIEnv;
use jni::objects::{JClass, JObjectArray, JString};
use jni::sys::jlong;

use crate::context::JniContext;
use legend_pure_core_platform::platform::parse_and_compile;
use legend_pure_core_platform::sources;

/// Initializes the `JniContext` by loading the platform models.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jlong {
    let result = std::panic::catch_unwind(|| {
        let platform = sources::platform_sources();
        let pairs: Vec<(&str, &str)> = platform.iter().map(|s| (s.content, s.path)).collect();

        let auto_imports: Vec<smol_str::SmolStr> =
            legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
                .iter()
                .map(|s| smol_str::SmolStr::new(*s))
                .collect();
        let model = match parse_and_compile(pairs.into_iter(), &auto_imports) {
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
