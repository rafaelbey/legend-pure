# Legend Pure — IntelliJ Plugin

Thin plugin that wires the
[`legend lsp`](../../crates/lsp) server (Rust, `tower-lsp`) into the
IntelliJ Platform's built-in LSP runtime.

> **Requires IntelliJ IDEA 2025.2.1 or later** (build `252.25557+`).
> The LSP API ships in the unified IntelliJ IDEA distribution from this
> build forward, so the plugin works for both Ultimate subscribers and
> free-tier users — no separate Community-Edition build needed. The
> `com.intellij.modules.lsp` dependency is declared *optional*, so the
> plugin still installs (without LSP features) on older IDEs.

## Features (MVP)

The server already exposes the following; the plugin surfaces them
unmodified:

| Feature | Server method | UI |
|---------|---------------|----|
| Diagnostics — current file | `textDocument/publishDiagnostics` | Gutter + squiggles |
| Diagnostics — workspace-wide | `textDocument/publishDiagnostics` (all files) | Problems tool window → *Project Errors* + Project view red-filename badge |
| Hover | `textDocument/hover` | Tooltip |
| Goto definition | `textDocument/definition` | ⌘B / Ctrl+B |
| Find Usages | `textDocument/references` | ⌥F7 / Alt+F7 — element-level (classes, functions, enums, profiles, associations). Property/qualified-property granularity deferred. |
| Document outline | `textDocument/documentSymbol` | Structure tool window |
| Test code lens | `textDocument/codeLens` (`legend.runTest`) | Inline lens (no-op stub for MVP) |

### Workspace-wide diagnostics

Edit one file in a way that breaks cross-references in others (rename
a class, narrow a multiplicity, drop a property) and **every impacted
file appears in the Problems tool window's *Project Errors* tab** —
not just the file you're editing. Click any row to jump straight to
the offending line; reopening the affected file shows the cached
diagnostics as inline squiggles instantly.

This goes beyond the IntelliJ Platform's stock LSP integration, which
only renders diagnostics into the per-editor markup model and silently
drops them for unopened files. The plugin decorates the LSP
notifications handler (see
`WorkspaceAwareLspNotificationsHandler.kt`) and routes every
`publishDiagnostics` notification through:

1. The platform's per-editor pathway (inline squiggles — unchanged).
2. `ProblemsCollector.getInstance(project)` — drives the Problems
   tool window's *Project Errors* tab.
3. `WolfTheProblemSolver` — drives the red-filename badge in Project
   view (requires the registered `problemFileHighlightFilter`
   extension to clear Wolf's internal `isToBeHighlighted` gate).

## Configuration

Two settings under
*Settings → Languages & Frameworks → Legend Pure*:

| Setting | Default | Behaviour |
|---------|---------|-----------|
| **Legend executable** | *(blank)* | Resolves `legend` (`legend.exe` on Windows) via OS `$PATH`. Set explicitly to override. |
| **Classpath TOML** | *(blank)* | Server runs its built-in cascade: env var → ancestor walk for `legend-pure-classpath.toml` → next-to-binary → embedded platform fallback. Set explicitly to pass `--classpath <path>` to the server. |

Use the **Test Connection** button to run `<legend> --version` and
confirm the resolved executable works.

## Build

```bash
./gradlew buildPlugin              # produces build/distributions/*.zip
./gradlew runIde                   # launches a sandbox Ultimate IDE
./gradlew verifyPlugin             # plugin verifier (since-build floor)
```

The wrapper is bootstrapped to Gradle 8.10.2; the IntelliJ Platform
Gradle Plugin is 2.1.0; target platform is IDEA 2025.2.1
(`platformType=IU`, `since-build=252.25557`). Adjust in
`gradle.properties` if you need a different floor.

## Smoke test

1. `./gradlew runIde` opens a sandbox running IDEA 2025.2.1.
2. Open any `.pure` file (e.g. from
   `legend-pure-core/legend-pure-m3-core/src/main/resources/platform/`).
3. Verify diagnostics appear when you introduce a parse error
   (drop a closing `}`); they clear on fix.
4. Open the **Problems** tool window (bottom panel) and switch to
   the *Project Errors* tab. Make an edit that breaks cross-file
   references (e.g. rename a class referenced from multiple files);
   every impacted `.pure` file should appear with its full path,
   even if it's not open in an editor tab.
5. ⌘B / Ctrl+B on a class reference jumps to its declaration.
6. Hovering an identifier shows its inferred type.
7. The Structure tool window lists the file's classes, functions,
   associations, and so on.

## What's not in the MVP

- **Run/Debug configurations** for `legend test` / `legend compile`
  (lens text shows but click is a stub).
- **Bundled `legend` binary** — users supply their own.
- **Pre-2025.2.1 IDEs** — the LSP module dependency is *optional*, so
  the plugin loads, but LSP-driven features are silently disabled. A
  user on an older IDE sees `.pure` syntax highlighting only. (No
  LSP4IJ fallback for now.)
- **Custom syntax highlighting** beyond what the LSP exposes
  (server doesn't emit `textDocument/semanticTokens` yet).

## Layout

```
clients/intellij/
├── build.gradle.kts                            # IntelliJ Platform 2.x + kotlin-jvm
├── settings.gradle.kts
├── gradle.properties                           # plugin coords + platform target
├── gradle/wrapper/, gradlew, gradlew.bat
└── src/main/
    ├── kotlin/org/finos/legend/pure/intellij/
    │   ├── PureLanguage.kt
    │   ├── PureFileType.kt
    │   ├── PureLspServerSupportProvider.kt     # entry point — fileOpened()
    │   ├── PureLspServerDescriptor.kt          # spawns `legend lsp`, wires diagnostic wrapper
    │   ├── WorkspaceAwareLspNotificationsHandler.kt  # routes publishDiagnostics to Problems tool window
    │   ├── LegendPureFileProblem.kt            # FileProblem impl for ProblemsCollector
    │   ├── PureProblemFileHighlightFilter.kt   # WolfTheProblemSolver gate for .pure files
    │   ├── settings/LegendPureSettings.kt      # @State PersistentStateComponent
    │   ├── settings/LegendPureConfigurable.kt  # Settings UI (two fields + test btn)
    │   └── util/ExecutableResolver.kt          # PATH lookup
    └── resources/
        ├── META-INF/plugin.xml             # core descriptor
        ├── META-INF/lsp.xml                # LSP extension point (loaded only
        │                                   #   when modules.lsp is present)
        ├── META-INF/pluginIcon.svg
        └── icons/pure-file.svg
```
