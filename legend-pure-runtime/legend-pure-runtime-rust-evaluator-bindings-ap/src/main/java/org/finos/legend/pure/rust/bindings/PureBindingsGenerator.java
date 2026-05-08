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

package org.finos.legend.pure.rust.bindings;

/**
 * Native bridge to the Rust java-codegen crate. The
 * {@link PureBindingsProcessor} calls
 * {@link #nativeGenerateBindings(String, String[], String[], String[], String[], String)}
 * which in turn invokes {@code legend_pure_java_codegen::generate} and
 * returns a flat {@code String[]} of alternating
 * {@code (relativePath, contents)} pairs.
 *
 * <p>The cdylib is loaded once per JVM via {@link #ensureLoaded(String)}
 * — the JVM's own library cache makes a second {@code System.load}
 * call by absolute path a no-op, so the runtime
 * {@code PureRustEvaluator} and the compile-time AP can both use the
 * same shared library without conflict.
 */
public final class PureBindingsGenerator
{
    private static volatile boolean loaded;

    private PureBindingsGenerator()
    {
    }

    /**
     * Load {@code libpure_rust_jni.{dylib,so,dll}} from
     * {@code absolutePath}. Idempotent — safe to call once per AP
     * round.
     *
     * @param absolutePath absolute path to the cdylib (typically
     *                     supplied by the consumer module via the
     *                     {@code -Apure.cdylib.path=…} compiler option)
     * @throws IllegalStateException if {@code absolutePath} is null or empty
     */
    public static synchronized void ensureLoaded(String absolutePath)
    {
        if (loaded)
        {
            return;
        }
        if (absolutePath == null || absolutePath.isEmpty())
        {
            throw new IllegalStateException(
                    "Pass -Apure.cdylib.path=… to the Java compiler "
                            + "(absolute path to libpure_rust_jni.{dylib,so,dll}).");
        }
        System.load(absolutePath);
        loaded = true;
    }

    /**
     * Run the Rust codegen and return its emitted Java source set as
     * a flat array of alternating {@code (relativePath, contents)}
     * pairs. The relative path uses forward slashes regardless of host
     * OS so the caller can convert it to a fully-qualified class name
     * with a single {@code replace('/', '.')}.
     *
     * @param javaPackage          Java root package every emitted class lives under
     * @param functions            mangled function FQNs
     * @param classes              additional Pure {@code Class} FQN seeds
     * @param associations         additional Pure {@code Association} FQN seeds
     * @param bindingsLines        lines of a kind-agnostic manifest (blank /
     *                             comment lines tolerated and stripped on the Rust side)
     * @param functionsClassName   simple name override for the facade class;
     *                             empty string requests the default ({@code PureFunctions})
     * @param externalBindings     flat array of alternating
     *                             (Pure FQN, Java FQN) pairs declaring
     *                             types already emitted by another module —
     *                             codegen references those Java FQNs instead
     *                             of re-emitting interfaces. Empty array =
     *                             standalone codegen with no imports.
     * @return alternating (path, contents) entries; never null on success
     * @throws RuntimeException carrying the underlying Rust error message
     *                          if codegen fails (translated from the
     *                          {@code PureRustException} thrown across JNI)
     */
    public static native String[] nativeGenerateBindings(
            String javaPackage,
            String[] functions,
            String[] classes,
            String[] associations,
            String[] bindingsLines,
            String functionsClassName,
            String[] externalBindings);
}
