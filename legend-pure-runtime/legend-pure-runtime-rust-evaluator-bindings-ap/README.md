# legend-pure-runtime-rust-evaluator-bindings-ap

Annotation processor that emits typed Java bindings for the Pure
runtime by loading the `libpure_rust_jni` cdylib and calling into the
`legend-pure-java-codegen` Rust crate.

This is the build-time companion to
[`legend-pure-runtime-rust-evaluator`](../legend-pure-runtime-rust-evaluator/).
The runtime module's `mvn compile` references this module through
`<annotationProcessorPaths>`; on the AP module side, `<proc>none</proc>`
disables AP discovery for the AP's own compile so we don't trip a
chicken-and-egg.

## Usage

Add `<annotationProcessorPaths>` to your consuming module's
`maven-compiler-plugin` config:

```xml
<plugin>
  <artifactId>maven-compiler-plugin</artifactId>
  <configuration>
    <annotationProcessorPaths>
      <path>
        <groupId>org.finos.legend.pure</groupId>
        <artifactId>legend-pure-runtime-rust-evaluator-bindings-ap</artifactId>
        <version>${project.version}</version>
      </path>
    </annotationProcessorPaths>
    <compilerArgs>
      <arg>-Apure.cdylib.path=/abs/path/to/libpure_rust_jni.dylib</arg>
    </compilerArgs>
  </configuration>
</plugin>
```

Place a `.jpure` manifest under `src/main/resources/`:

```
# src/main/resources/pure-bindings/my-bindings.jpure
@pkg: com.example.generated

user_test::Account
user_test::Trader
```

Then place a marker class anywhere in the consumer's `src/main/java`
tree:

```java
package com.example.bindings;

import org.finos.legend.pure.rust.bindings.PureBindings;

@PureBindings(bindingsFile = "pure-bindings/my-bindings.jpure")
public final class Bootstrap {}
```

The processor reads the manifest as a classpath resource via
`Filer.getResource(StandardLocation.CLASS_OUTPUT, "", path)`, parses
header directives (`@pkg`, `@functions-class`, `@import`),
recursively resolves any imports, builds a Pure-FQN → Java-FQN
external-bindings map, then routes everything to
`legend_pure_java_codegen::generate` via the cdylib JNI bridge and
emits one Java source per produced file using `Filer.createSourceFile`.

## Manifest format (`.jpure`)

```
# Header directives precede the FQN body.
@pkg: <java.root.package>           # required (or via @PureBindings.javaPackage())
@functions-class: <SimpleName>      # optional, default: PureFunctions
@import: <classpath-path>           # zero or more

# Pure FQNs (one per line, blank/comment lines ignored).
<package>::<element>                # Function / Class / Association
```

Imported manifests must each declare their own `@pkg:` directive so
the importer knows where the bindings live. Imports are loaded from
the same compile classpath the AP itself runs on — typically a
`provided`-scope Maven dependency on the upstream module.

## Compiler options

| Option                         | Purpose                                                                  |
| ------------------------------ | ------------------------------------------------------------------------ |
| `-Apure.cdylib.path=…`         | Absolute path to `libpure_rust_jni.{dylib,so,dll}`. Required.           |

## Architecture

```
javac → PureBindingsProcessor
            ↓
        PureBindingsGenerator.ensureLoaded(absolutePath)   System.load()
            ↓
        nativeGenerateBindings(...)                         JNI
            ↓
        legend_pure_java_codegen::generate(...)             Rust
            ↓                                              (loads platform model
        Vec<JavaFile>                                       via repo::load)
            ↓
        flattened to String[] of (relativePath, contents)
            ↓
        Filer.createSourceFile(fqcn, marker)                back in javac
```

The processor is self-contained on the annotation-processor classpath
— it has no Maven runtime dependencies beyond the JDK. The bundled
`META-INF/services/javax.annotation.processing.Processor` registers
`PureBindingsProcessor` for service-loader discovery.

## Disabling

To skip codegen entirely (e.g. on a build host without a built
cdylib), pass `-Dmaven.compiler.proc=none`. The marker class still
compiles cleanly because `@PureBindings` has `RetentionPolicy.SOURCE`.

## Caveats

- The cdylib must already exist at `-Apure.cdylib.path` when `mvn
  compile` runs. The runtime module's pom defaults to the workspace's
  `legend-pure-rust/target/debug/libpure_rust_jni.<ext>`; build it
  with `cargo build -p legend-pure-parser-jni` first.
- The cdylib's CPU architecture must match the JVM running javac. The
  current OS-detection profile only varies the file extension, not
  the arch — for cross-platform Java consumers the deferred
  CI-matrix phase will bundle per-arch `.dylib` / `.so` / `.dll`
  inside this AP JAR under `META-INF/native/<os>-<arch>/`.

## See also

- [`legend-pure-runtime-rust-evaluator/README.md`](../legend-pure-runtime-rust-evaluator/README.md)
  — runtime module that consumes this AP.
- `legend-pure-rust/crates/java-codegen/` — the codegen logic this AP
  invokes via JNI.
- `legend-pure-rust/crates/jni/src/codegen.rs` — the
  `nativeGenerateBindings` JNI export.
