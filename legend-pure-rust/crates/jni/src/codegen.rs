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

//! Annotation-processor JNI bridge for the Java-bindings code generator.
//!
//! Mirrors what `legend-cli java-bindings` does, minus the I/O: load the
//! platform model via `legend_pure_core_platform::repo`, run
//! [`legend_pure_java_codegen::generate`], and return the produced
//! `Vec<JavaFile>` to the caller as a flat Java `String[]` of
//! alternating `(relativePath, contents)` entries. The Java-side
//! `PureBindingsProcessor` then emits each pair via `Filer`.
//!
//! No `JniContext` is involved — the AP path doesn't touch the runtime
//! heap. Model loading and codegen happen inline and are released when
//! the JNI call returns.

use jni::JNIEnv;
use jni::objects::{JClass, JObjectArray, JString};

use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_java_codegen::{
    FqnInput, JavaFile, Options, dispatch_bindings_by_kind, generate,
};
use smol_str::SmolStr;

/// JNI entry called from
/// `org.finos.legend.pure.rust.bindings.PureBindingsGenerator.nativeGenerateBindings`.
///
/// Parameters mirror the CLI shape: explicit `--functions` /
/// `--classes` / `--associations` seeds plus a kind-agnostic bindings
/// list. `functions_class_name` is the empty string when the caller
/// wants the default (`PureFunctions`).
///
/// Returns a `String[]` of even length, alternating `(relativePath,
/// contents)`. On error throws `org/finos/legend/pure/rust/PureRustException`
/// and returns null.
///
/// # Panics
/// Internal Rust panics are caught and rethrown as `PureRustException`
/// across the FFI; the Java side never observes a Rust unwind.
#[allow(clippy::unwrap_used)] // inside catch_unwind — panics translate to Java exceptions
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_finos_legend_pure_rust_bindings_PureBindingsGenerator_nativeGenerateBindings<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    java_package: JString<'local>,
    functions: JObjectArray<'local>,
    classes: JObjectArray<'local>,
    associations: JObjectArray<'local>,
    bindings_lines: JObjectArray<'local>,
    functions_class_name: JString<'local>,
) -> JObjectArray<'local> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let java_package_str: String = env.get_string(&java_package).unwrap().into();
        let functions_class_name_str: String =
            env.get_string(&functions_class_name).unwrap().into();
        let functions_vec = read_string_array(&mut env, &functions)
            .map_err(|e| format!("converting --functions array: {e:?}"))?;
        let classes_vec = read_string_array(&mut env, &classes)
            .map_err(|e| format!("converting --classes array: {e:?}"))?;
        let associations_vec = read_string_array(&mut env, &associations)
            .map_err(|e| format!("converting --associations array: {e:?}"))?;
        let bindings_lines_vec = read_string_array(&mut env, &bindings_lines)
            .map_err(|e| format!("converting bindings-lines array: {e:?}"))?;

        run_codegen(
            &java_package_str,
            &functions_vec,
            &classes_vec,
            &associations_vec,
            &bindings_lines_vec,
            functions_class_name_str.as_str(),
        )
    }));

    match result {
        Ok(Ok(files)) => match flatten_to_java_string_array(&mut env, &files) {
            Ok(arr) => arr,
            Err(e) => {
                let _ = env.throw_new(
                    "org/finos/legend/pure/rust/PureRustException",
                    format!("Failed to marshal generated files to Java: {e:?}"),
                );
                JObjectArray::default()
            }
        },
        Ok(Err(msg)) => {
            let _ = env.throw_new("org/finos/legend/pure/rust/PureRustException", msg);
            JObjectArray::default()
        }
        Err(_) => {
            let _ = env.throw_new(
                "org/finos/legend/pure/rust/PureRustException",
                "Rust paniced during nativeGenerateBindings",
            );
            JObjectArray::default()
        }
    }
}

/// Pure-Rust core of the JNI export, factored out so it can be unit
/// tested without a JVM.
///
/// Loads the platform model with build-snapshots, resolves the
/// per-kind seed buckets from `bindings_lines`, and calls into the
/// codegen library. Returns the produced `Vec<JavaFile>` on success or
/// a flat error string on failure.
///
/// # Errors
/// Returns `Err(String)` on any bindings-resolution or codegen error;
/// the Java side translates this into a `PureRustException`.
pub(crate) fn run_codegen(
    java_package: &str,
    functions: &[String],
    classes: &[String],
    associations: &[String],
    bindings_lines: &[String],
    functions_class_name: &str,
) -> Result<Vec<JavaFile>, String> {
    let repos = Repo::default_with_build_snapshots();
    let auto_imports: Vec<SmolStr> = legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    // Mirror the CLI: tolerate partial-load (warnings, not failures) so
    // a single platform-source diagnostic doesn't block the AP run.
    let model = match repo::load(&repos, &auto_imports) {
        Ok(m) => m,
        Err(partial) => partial.model,
    };

    let mut requested: Vec<FqnInput> = functions
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();
    let mut extra_classes: Vec<FqnInput> = classes
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();
    let mut extra_associations: Vec<FqnInput> = associations
        .iter()
        .map(|s| FqnInput::new(s.trim()))
        .filter(|f| !f.raw.is_empty())
        .collect();

    let cleaned_lines: Vec<String> = bindings_lines
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    if !cleaned_lines.is_empty() {
        let dispatched =
            dispatch_bindings_by_kind(&model, &cleaned_lines).map_err(|e| e.to_string())?;
        requested.extend(dispatched.functions);
        extra_classes.extend(dispatched.classes);
        extra_associations.extend(dispatched.associations);
    }

    if requested.is_empty() && extra_classes.is_empty() && extra_associations.is_empty() {
        return Err(
            "nothing to generate — pass functions, classes, associations, or a bindings list"
                .to_owned(),
        );
    }

    let mut opts = Options::new(java_package);
    if !functions_class_name.is_empty() {
        opts.functions_class_name = Some(functions_class_name.to_owned());
    }

    generate(&model, &requested, &extra_classes, &extra_associations, &opts)
        .map_err(|e| e.to_string())
}

/// Read a `String[]` argument into a `Vec<String>`. A null array is
/// treated as empty.
fn read_string_array<'local>(
    env: &mut JNIEnv<'local>,
    array: &JObjectArray<'local>,
) -> Result<Vec<String>, jni::errors::Error> {
    if array.is_null() {
        return Ok(Vec::new());
    }
    let len = env.get_array_length(array)?;
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let element = env.get_object_array_element(array, i)?;
        let jstr = JString::from(element);
        let s: String = env.get_string(&jstr)?.into();
        out.push(s);
    }
    Ok(out)
}

/// Flatten `Vec<JavaFile>` into a Java `String[]` of alternating
/// `(relativePath, contents)` entries.
///
/// The relative path is rendered with forward slashes regardless of
/// host OS so the Java caller can convert it to a fully-qualified
/// class name with a single `replace('/', '.')`.
fn flatten_to_java_string_array<'local>(
    env: &mut JNIEnv<'local>,
    files: &[JavaFile],
) -> Result<JObjectArray<'local>, jni::errors::Error> {
    let string_class = env.find_class("java/lang/String")?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let total_len = (files.len() * 2) as jni::sys::jsize;
    let array =
        env.new_object_array(total_len, &string_class, jni::objects::JObject::null())?;

    for (i, file) in files.iter().enumerate() {
        let path_str = relative_path_with_forward_slashes(&file.relative_path);
        let path_jstr = env.new_string(&path_str)?;
        let contents_jstr = env.new_string(&file.contents)?;

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let path_idx = (i * 2) as jni::sys::jsize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let contents_idx = (i * 2 + 1) as jni::sys::jsize;
        env.set_object_array_element(&array, path_idx, &path_jstr)?;
        env.set_object_array_element(&array, contents_idx, &contents_jstr)?;
    }

    Ok(array)
}

fn relative_path_with_forward_slashes(path: &std::path::Path) -> String {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_codegen_with_empty_inputs_errors_cleanly() {
        let err = run_codegen("com.example.gen", &[], &[], &[], &[], "")
            .expect_err("empty seeds must error");
        assert!(
            err.contains("nothing to generate"),
            "wrong error: {err}"
        );
    }

    #[test]
    fn run_codegen_dispatches_bindings_lines() {
        let lines = vec![
            "meta::pure::functions::math::plus_Integer_MANY__Integer_1_".to_owned(),
        ];
        let files = run_codegen(
            "com.example.gen",
            &[],
            &[],
            &[],
            &lines,
            "",
        )
        .expect("plus dispatches and codegen succeeds");
        assert!(
            files.iter().any(|f| f
                .relative_path
                .ends_with("PureFunctions.java")),
            "facade not emitted: {:?}",
            files
                .iter()
                .map(|f| f.relative_path.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn run_codegen_strips_comments_and_blank_lines() {
        let lines = vec![
            "# this is a comment".to_owned(),
            "".to_owned(),
            "   ".to_owned(),
            "meta::pure::functions::math::plus_Integer_MANY__Integer_1_".to_owned(),
        ];
        let files = run_codegen(
            "com.example.gen",
            &[],
            &[],
            &[],
            &lines,
            "",
        )
        .expect("comment + whitespace lines stripped");
        assert!(!files.is_empty());
    }

    #[test]
    fn run_codegen_unresolved_bindings_entry_errors() {
        let lines = vec!["totally::made::up".to_owned()];
        let err = run_codegen("com.example.gen", &[], &[], &[], &lines, "")
            .expect_err("unresolved entry must error");
        assert!(
            err.contains("totally::made::up"),
            "wrong error: {err}"
        );
    }

    #[test]
    fn relative_path_uses_forward_slashes() {
        let p = std::path::PathBuf::from("a").join("b").join("C.java");
        assert_eq!(relative_path_with_forward_slashes(&p), "a/b/C.java");
    }
}
