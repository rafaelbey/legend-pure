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
import javax.tools.FileObject;
import javax.tools.JavaFileObject;
import javax.tools.StandardLocation;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.Reader;
import java.io.Writer;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.stream.Collectors;

/**
 * Annotation processor that emits typed Java bindings for Pure
 * elements listed in a manifest file. Triggered by {@link PureBindings}
 * on a marker class.
 *
 * <p>The manifest is loaded as a <b>classpath resource</b>: declare
 * its path relative to the resource root (e.g.
 * {@code "pure-bindings/m3-bindings.jpure"}) and place the file under
 * {@code src/main/resources/}. The processor reads it via
 * {@link javax.annotation.processing.Filer#getResource(JavaFileManager.Location, CharSequence, CharSequence)
 * Filer.getResource} after Maven has copied resources into
 * {@code target/classes/}.
 *
 * <p>Manifest format: directive lines ({@code @<key>: <value>}) precede
 * the FQN body. Recognised keys:
 * <dl>
 *   <dt>{@code @pkg}</dt>
 *   <dd>Java root package every emitted class lives under. Overrides
 *       the {@link PureBindings#javaPackage()} fallback.</dd>
 *   <dt>{@code @functions-class}</dt>
 *   <dd>Simple class name for the static-functions facade
 *       (defaults to {@code PureFunctions}).</dd>
 *   <dt>{@code @import}</dt>
 *   <dd>Classpath path to another {@code .jpure} manifest. The
 *       processor resolves it via the same {@code Filer.getResource}
 *       call, reads its {@code @pkg} + FQN body, and treats every
 *       imported FQN as <b>external</b>: codegen references the
 *       imported Java FQN instead of re-emitting an interface
 *       locally.</dd>
 * </dl>
 *
 * <p>Reads two compiler options:
 * <dl>
 *   <dt>{@code -Apure.cdylib.path}</dt>
 *   <dd>Absolute path to {@code libpure_rust_jni.{dylib,so,dll}}.
 *       Required.</dd>
 * </dl>
 */
@SupportedAnnotationTypes("org.finos.legend.pure.rust.bindings.PureBindings")
@SupportedSourceVersion(SourceVersion.RELEASE_11)
@SupportedOptions({"pure.cdylib.path"})
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
            try
            {
                processOneMarker(marker, cfg, emittedFqcns);
            }
            catch (BindingsException be)
            {
                processingEnv.getMessager().printMessage(
                        Diagnostic.Kind.ERROR, "PureBindings: " + be.getMessage(), marker);
            }
        }
        return true;
    }

    /**
     * Read the root manifest, resolve every {@code @import} chain into
     * an external-bindings flat array, then call the JNI generator and
     * emit each returned source file via Filer.
     */
    private void processOneMarker(Element marker, PureBindings cfg, Set<String> emittedFqcns)
            throws BindingsException
    {
        // Walk the import graph: BFS starting at the root manifest's
        // path. The root manifest's @pkg / FQNs become local seeds; an
        // imported manifest's @pkg + FQNs become external bindings.
        ParsedManifest root = readAndParseManifest(cfg.bindingsFile());
        String effectivePackage = !root.pkg.isEmpty() ? root.pkg : cfg.javaPackage();
        String effectiveFunctionsClass = !root.functionsClass.isEmpty()
                ? root.functionsClass
                : cfg.functionsClassName();

        List<String> externalBindingsFlat = new ArrayList<>();
        Set<String> visited = new HashSet<>();
        visited.add(cfg.bindingsFile());
        Deque<String> queue = new ArrayDeque<>(root.imports);

        while (!queue.isEmpty())
        {
            String path = queue.pop();
            if (!visited.add(path))
            {
                continue;
            }
            ParsedManifest imported = readAndParseManifest(path);
            if (imported.pkg.isEmpty())
            {
                throw new BindingsException(
                        "imported manifest `" + path + "` must declare a `@pkg:` directive "
                                + "so the importing module knows where the bindings live");
            }
            for (String fqn : imported.fqns)
            {
                String javaFqn = imported.pkg + "." + javaSimpleNameOf(fqn);
                externalBindingsFlat.add(fqn);
                externalBindingsFlat.add(javaFqn);
            }
            queue.addAll(imported.imports);
        }

        if (effectivePackage == null || effectivePackage.isEmpty())
        {
            throw new BindingsException(
                    "no Java package declared — set `@pkg:` in `" + cfg.bindingsFile()
                            + "` or pass `javaPackage` on the @PureBindings annotation");
        }

        String[] generated;
        try
        {
            generated = PureBindingsGenerator.nativeGenerateBindings(
                    effectivePackage,
                    new String[0],
                    new String[0],
                    new String[0],
                    root.fqns.toArray(new String[0]),
                    effectiveFunctionsClass,
                    externalBindingsFlat.toArray(new String[0]));
        }
        catch (RuntimeException nativeEx)
        {
            throw new BindingsException("native codegen failed — " + nativeEx.getMessage());
        }

        if (generated == null || generated.length == 0)
        {
            processingEnv.getMessager().printMessage(
                    Diagnostic.Kind.WARNING,
                    "PureBindings: native codegen returned no sources for " + cfg.bindingsFile(),
                    marker);
            return;
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
                        "PureBindings: emit " + fqcn + " — " + ioe.getMessage(), marker);
            }
        }
    }

    private ParsedManifest readAndParseManifest(String classpathPath) throws BindingsException
    {
        List<String> lines;
        try
        {
            FileObject f = processingEnv.getFiler().getResource(
                    StandardLocation.CLASS_OUTPUT, "", classpathPath);
            try (Reader r = f.openReader(true);
                 BufferedReader br = new BufferedReader(r))
            {
                lines = br.lines().collect(Collectors.toList());
            }
        }
        catch (IOException ioe)
        {
            throw new BindingsException(
                    "cannot read manifest `" + classpathPath
                            + "` from classpath — " + ioe.getMessage()
                            + ". Ensure the file lives under `src/main/resources/`.");
        }
        return parseManifestLines(lines, classpathPath);
    }

    /**
     * Parse manifest text into directives + FQN body.
     *
     * <p>Mirror of {@code legend_pure_java_codegen::parse_manifest}. Both
     * implementations must accept the same syntax.
     */
    private static ParsedManifest parseManifestLines(List<String> rawLines, String contextPath)
            throws BindingsException
    {
        ParsedManifest out = new ParsedManifest();
        for (String raw : rawLines)
        {
            String trimmed = raw.trim();
            if (trimmed.isEmpty() || trimmed.startsWith("#"))
            {
                continue;
            }
            if (trimmed.startsWith("@"))
            {
                int colon = trimmed.indexOf(':');
                if (colon < 0)
                {
                    throw new BindingsException(
                            "manifest `" + contextPath + "` line `" + trimmed
                                    + "`: directive must be `@<key>: <value>`");
                }
                String key = trimmed.substring(1, colon).trim();
                String value = trimmed.substring(colon + 1).trim();
                switch (key)
                {
                    case "pkg":
                        out.pkg = value;
                        break;
                    case "functions-class":
                        out.functionsClass = value;
                        break;
                    case "import":
                        out.imports.add(value);
                        break;
                    default:
                        throw new BindingsException(
                                "manifest `" + contextPath + "` line `" + trimmed
                                        + "`: unknown directive `@" + key
                                        + "` — supported: @pkg, @functions-class, @import");
                }
                continue;
            }
            out.fqns.add(trimmed);
        }
        return out;
    }

    /**
     * Map a Pure FQN to the Java simple name the codegen emits for it.
     * Mirrors {@code legend_pure_java_codegen::naming::safe_java_identifier}'s
     * behaviour for the *leaf* segment.
     *
     * <p>For type FQNs (Class / Enumeration), the Java simple name is
     * the leaf segment of the Pure FQN, escaped if it collides with a
     * Java keyword. For mangled function FQNs the situation is
     * different — those bind to a static method on the imported
     * module's facade class — but external-bindings entries from the
     * AP are only consulted by the codegen at <em>type reference
     * sites</em>, which can only reference types. So this helper is
     * called for type FQNs only.
     */
    private static String javaSimpleNameOf(String pureFqn)
    {
        int idx = pureFqn.lastIndexOf("::");
        String leaf = idx >= 0 ? pureFqn.substring(idx + 2) : pureFqn;
        // Match the Rust safe_java_identifier rule: append `_` to the
        // 53 Java reserved words. We don't enumerate them here (only the
        // ones that show up in actual M3 FQNs realistically: `package`,
        // `class`, `interface`, `default`, …) — pragmatic v1.
        switch (leaf)
        {
            case "package":
            case "class":
            case "interface":
            case "default":
            case "enum":
            case "switch":
            case "throw":
            case "throws":
            case "case":
            case "abstract":
            case "boolean":
            case "byte":
            case "char":
            case "double":
            case "float":
            case "int":
            case "long":
            case "short":
            case "void":
            case "final":
            case "static":
            case "synchronized":
            case "volatile":
            case "transient":
            case "extends":
            case "implements":
            case "import":
            case "instanceof":
            case "native":
            case "new":
            case "private":
            case "protected":
            case "public":
            case "return":
            case "super":
            case "this":
            case "try":
            case "catch":
            case "finally":
            case "if":
            case "else":
            case "for":
            case "while":
            case "do":
            case "break":
            case "continue":
            case "goto":
            case "const":
            case "null":
            case "true":
            case "false":
                return leaf + "_";
            default:
                return leaf;
        }
    }

    private static final class ParsedManifest
    {
        String pkg = "";
        String functionsClass = "";
        final List<String> imports = new ArrayList<>();
        final List<String> fqns = new ArrayList<>();
    }

    private static final class BindingsException extends Exception
    {
        BindingsException(String message)
        {
            super(message);
        }
    }
}
