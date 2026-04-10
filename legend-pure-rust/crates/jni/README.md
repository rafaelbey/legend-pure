# legend-pure-parser-jni

JNI bridge exposing the Rust parser to Java. This is the only crate that uses `unsafe` (required for FFI). Initializes the `tracing` subscriber on `JNI_OnLoad`.

## Entry Point

```java
public class RustPureParser {
    public static native String parseToProtocolJson(String source, String section);
}
```
