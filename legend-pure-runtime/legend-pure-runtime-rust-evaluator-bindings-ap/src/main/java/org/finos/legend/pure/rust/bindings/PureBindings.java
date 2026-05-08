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
 * referenced manifest, dispatches each FQN through the Rust codegen
 * crate via the {@code libpure_rust_jni} cdylib, and emits one
 * {@code .java} file per resulting Java source under
 * {@link #javaPackage()}.
 *
 * <p>Typical usage:
 * <pre>{@code
 * @PureBindings(
 *     bindingsFile = "src/main/pure-bindings/m3-bindings.txt",
 *     javaPackage  = "org.finos.legend.pure.rust.generated"
 * )
 * public final class M3Bootstrap {}
 * }</pre>
 *
 * <p>Two compiler-level options drive the path resolution:
 * <ul>
 *   <li>{@code -Apure.bindings.basedir=…} — directory the
 *       {@link #bindingsFile()} path resolves against (defaults to the
 *       JVM's working directory, which is the Maven {@code basedir}).</li>
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
     * Path to the bindings manifest, one Pure FQN per line. Resolved
     * relative to {@code -Apure.bindings.basedir} (which the Maven
     * compiler plugin sets to {@code ${project.basedir}}). Blank lines
     * and {@code #}-prefixed comments are ignored.
     *
     * @return the manifest path
     */
    String bindingsFile();

    /**
     * Java package every emitted source class lives under.
     *
     * @return the root Java package
     */
    String javaPackage();

    /**
     * Optional override for the simple class name of the generated
     * static-functions facade. Defaults to {@code PureFunctions} when
     * empty.
     *
     * @return the facade class name (or empty for the default)
     */
    String functionsClassName() default "";
}
