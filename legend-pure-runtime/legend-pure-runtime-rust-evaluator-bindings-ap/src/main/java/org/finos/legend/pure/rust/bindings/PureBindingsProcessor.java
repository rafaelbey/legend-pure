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

import javax.annotation.processing.AbstractProcessor;
import javax.annotation.processing.RoundEnvironment;
import javax.annotation.processing.SupportedAnnotationTypes;
import javax.annotation.processing.SupportedOptions;
import javax.annotation.processing.SupportedSourceVersion;
import javax.lang.model.SourceVersion;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;
import javax.tools.Diagnostic;
import javax.tools.JavaFileObject;
import java.io.IOException;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Annotation processor that emits typed Java bindings for Pure
 * elements listed in a manifest file. Triggered by {@link PureBindings}
 * on a marker class.
 *
 * <p>Reads two compiler options:
 * <dl>
 *   <dt>{@code -Apure.bindings.basedir}</dt>
 *   <dd>directory the {@link PureBindings#bindingsFile()} path
 *       resolves against (defaults to the JVM working directory).</dd>
 *   <dt>{@code -Apure.cdylib.path}</dt>
 *   <dd>absolute path to {@code libpure_rust_jni.{dylib,so,dll}}.
 *       Required.</dd>
 * </dl>
 */
@SupportedAnnotationTypes("org.finos.legend.pure.rust.bindings.PureBindings")
@SupportedSourceVersion(SourceVersion.RELEASE_11)
@SupportedOptions({"pure.bindings.basedir", "pure.cdylib.path"})
public final class PureBindingsProcessor extends AbstractProcessor
{
    @Override
    public boolean process(Set<? extends TypeElement> annotations, RoundEnvironment roundEnv)
    {
        if (roundEnv.processingOver())
        {
            return false;
        }
        Set<? extends Element> targets = roundEnv.getElementsAnnotatedWith(PureBindings.class);
        if (targets.isEmpty())
        {
            return false;
        }

        String cdylib = processingEnv.getOptions().get("pure.cdylib.path");
        String baseDir = processingEnv.getOptions()
                .getOrDefault("pure.bindings.basedir", System.getProperty("user.dir"));

        try
        {
            PureBindingsGenerator.ensureLoaded(cdylib);
        }
        catch (RuntimeException loadEx)
        {
            processingEnv.getMessager().printMessage(
                    Diagnostic.Kind.ERROR,
                    "PureBindings: failed to load native cdylib — " + loadEx.getMessage());
            return true;
        }

        // Dedupe across multiple @PureBindings markers in case two
        // markers target overlapping packages.
        Set<String> emittedFqcns = new HashSet<>();
        for (Element marker : targets)
        {
            PureBindings cfg = marker.getAnnotation(PureBindings.class);
            Path manifestPath = Paths.get(baseDir).resolve(cfg.bindingsFile());

            List<String> lines;
            try
            {
                lines = Files.readAllLines(manifestPath, StandardCharsets.UTF_8);
            }
            catch (IOException ioe)
            {
                processingEnv.getMessager().printMessage(
                        Diagnostic.Kind.ERROR,
                        "PureBindings: cannot read " + manifestPath + " — " + ioe.getMessage(),
                        marker);
                continue;
            }

            String[] generated;
            try
            {
                generated = PureBindingsGenerator.nativeGenerateBindings(
                        cfg.javaPackage(),
                        new String[0],
                        new String[0],
                        new String[0],
                        lines.toArray(new String[0]),
                        cfg.functionsClassName());
            }
            catch (RuntimeException nativeEx)
            {
                processingEnv.getMessager().printMessage(
                        Diagnostic.Kind.ERROR,
                        "PureBindings: native codegen failed — " + nativeEx.getMessage(),
                        marker);
                continue;
            }

            if (generated == null || generated.length == 0)
            {
                processingEnv.getMessager().printMessage(
                        Diagnostic.Kind.WARNING,
                        "PureBindings: native codegen returned no sources for "
                                + manifestPath,
                        marker);
                continue;
            }

            // alternating (relativePath, contents) entries
            for (int i = 0; i + 1 < generated.length; i += 2)
            {
                String relPath = generated[i];
                String body = generated[i + 1];
                String fqcn = relPath.replace('/', '.').replaceAll("\\.java$", "");
                if (!emittedFqcns.add(fqcn))
                {
                    continue;
                }
                try
                {
                    JavaFileObject sourceFile =
                            processingEnv.getFiler().createSourceFile(fqcn, marker);
                    try (Writer w = sourceFile.openWriter())
                    {
                        w.write(body);
                    }
                }
                catch (IOException ioe)
                {
                    processingEnv.getMessager().printMessage(
                            Diagnostic.Kind.ERROR,
                            "PureBindings: emit " + fqcn + " — " + ioe.getMessage(),
                            marker);
                }
            }
        }
        return true;
    }
}
