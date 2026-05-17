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

//! `mydsl_pure_jni` — worked downstream JNI cdylib.
//!
//! Two responsibilities:
//!
//! 1. **Force-link the downstream extension crate** so its
//!    `#[distributed_slice]` statics reach the final cdylib's link
//!    graph. Without these `use … as _;` lines the linker would
//!    drop the crate (no reachable code → no contribution to
//!    `RUNTIME_EXTENSIONS` / `COMPILER_EXTENSIONS` / …) and the
//!    JNI dispatch path inside the upstream rlib would silently
//!    see an empty extension set.
//!
//! 2. **Re-export the upstream `Java_*` entry points** via thin
//!    forwarder `#[no_mangle] pub extern "system" fn` declarations
//!    that delegate to the rlib's implementation. This is necessary
//!    because rustc's default cdylib build dead-code-eliminates
//!    unreachable rlib code — `pub use upstream::*;` is not enough,
//!    nor are linker-flag tricks. The forwarder pattern is the
//!    canonical Rust-JNI cdylib-on-rlib idiom; sees use in many
//!    other ecosystems (Tantivy's JNI bindings, etc.).
//!
//! After building, verify the symbols ship:
//!
//! ```bash
//! cargo build --release
//! nm -gU target/release/libmydsl_pure_jni.dylib | grep ' _Java_' | wc -l
//! ```
//!
//! should equal the count for the stock `libpure_rust_jni.dylib`.

#![allow(unused_imports)]

// (1) Force-link the downstream extension crate.
use legend_pure_mydsl_extension::prelude::MyDslCompilerExtension as _;
use legend_pure_mydsl_extension::prelude::MyDslIdeExtension as _;
use legend_pure_mydsl_extension::prelude::MyDslIslandParser as _;
use legend_pure_mydsl_extension::prelude::MyDslPopulator as _;
use legend_pure_mydsl_extension::prelude::MyDslRuntimeExtension as _;
use legend_pure_mydsl_extension::prelude::MyDslSectionParser as _;

// (2) Forwarder declarations for each upstream `Java_*` entry
//     point. Each delegates to the upstream rlib's implementation
//     via a regular Rust call (the upstream fns are `pub extern
//     "system" fn` items, which Rust can invoke like any function).
//     The `#[no_mangle]` on the FORWARDER guarantees its symbol
//     ships in the final cdylib's export table — the upstream's
//     `#[no_mangle]` doesn't carry through cdylib-on-rlib DCE.
//
//     When upstream adds a new `Java_*` entry, add a forwarder
//     here; the cargo build will fail loudly if a signature changes.

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::jlong;

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
) -> jlong {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeInitContext(env, class)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeEvaluate<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
    function_path: JString<'local>,
    args: JObjectArray<'local>,
) -> JObject<'local> {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeEvaluate(
        env,
        class,
        context_ptr,
        function_path,
        args,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetProperty<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
    property_name: JString<'local>,
    args: JObjectArray<'local>,
) -> JObject<'local> {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetProperty(
        env,
        class,
        context_ptr,
        complex_ptr,
        property_name,
        args,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetClassifier<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
) -> JString<'local> {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeGetClassifier(
        env,
        class,
        context_ptr,
        complex_ptr,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeNew<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
    classifier_fqn: JString<'local>,
    property_names: JObjectArray<'local>,
    property_values: JObjectArray<'local>,
) -> jlong {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeNew(
        env,
        class,
        context_ptr,
        classifier_fqn,
        property_names,
        property_values,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeContext<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
) {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeContext(
        env,
        class,
        context_ptr,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeInstance<
    'local,
>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    context_ptr: jlong,
    complex_ptr: jlong,
) {
    pure_rust_jni::Java_org_finos_legend_pure_rust_PureRustEvaluator_nativeFreeInstance(
        env,
        class,
        context_ptr,
        complex_ptr,
    )
}
