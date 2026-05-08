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

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

/**
 * Marks a Java class as the trigger for typed Pure-bindings generation.
 *
 * <p>The {@link PureBindingsProcessor} annotation processor reads the
 * referenced manifest from the <b>compile classpath</b>, dispatches
 * each FQN through the Rust codegen crate via the
 * {@code libpure_rust_jni} cdylib, and emits one {@code .java} file
 * per resulting Java source under the package declared by the
 * manifest's {@code @pkg:} directive (or the {@link #javaPackage()}
 * fallback).
 *
 * <p>Typical usage:
 * <pre>{@code
 * @PureBindings(bindingsFile = "pure-bindings/m3-bindings.jpure")
 * public final class M3Bootstrap {}
 * }</pre>
 *
 * <p>The manifest format supports directives:
 * <pre>{@code
 * @pkg: org.finos.legend.pure.rust.generated
 * @import: pure-bindings/m3-bindings.jpure
 *
 * meta::pure::metamodel::type::Class
 * }</pre>
 *
 * <p>One compiler-level option drives native loading:
 * <ul>
 *   <li>{@code -Apure.cdylib.path=…} — absolute path to the
 *       {@code libpure_rust_jni.{dylib,so,dll}} that the processor
 *       loads at compile time.</li>
 * </ul>
 *
 * <p>Only the {@code @Retention(SOURCE)} retention is needed — the
 * annotation is consumed entirely by the processor at compile time and
 * has no runtime presence.
 */
@Retention(RetentionPolicy.SOURCE)
@Target(ElementType.TYPE)
public @interface PureBindings
{
    /**
     * Classpath path to the bindings manifest (typically a {@code .jpure}
     * resource under {@code src/main/resources/}). Resolved via
     * {@link javax.annotation.processing.Filer#getResource(JavaFileManager.Location, CharSequence, CharSequence)
     * Filer.getResource} against {@code StandardLocation.CLASS_OUTPUT}.
     * Blank lines and {@code #}-prefixed comments are ignored.
     *
     * @return the classpath-relative manifest path
     */
    String bindingsFile();

    /**
     * Optional fallback Java package for emitted classes. Used only
     * when the manifest does not declare a {@code @pkg:} directive of
     * its own. Empty default — the manifest is the preferred home for
     * this value because it then travels with the bindings JAR.
     *
     * @return the root Java package, or empty to defer to the manifest
     */
    String javaPackage() default "";

    /**
     * Optional fallback for the simple class name of the generated
     * static-functions facade. Used only when the manifest does not
     * declare a {@code @functions-class:} directive. Defaults to
     * {@code PureFunctions}.
     *
     * @return the facade class name (or empty for the default)
     */
    String functionsClassName() default "";
}
