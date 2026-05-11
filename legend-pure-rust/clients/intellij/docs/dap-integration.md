# IntelliJ DAP integration (design doc)

The Pure DAP server (`legend dap`) is shipping in `crates/dap/` and
works end-to-end with any DAP-compatible client (VS Code, neovim
`nvim-dap`, etc.). This doc captures what the IntelliJ-side plugin
integration needs, so the follow-up PR is purely Kotlin work
against IntelliJ APIs.

## What ships today (runtime + DAP server + CLI)

- `crates/runtime/` exposes `VariableContext::iter_bindings()` plus
  the `debug` module (`StepMode`, `Frame`, `DebugBackbone`) —
  stable surface for any embedder driving the evaluator with
  pause/step semantics.
- `crates/dap/` runs a self-contained DAP server over stdio. It
  implements the MVP request set: `initialize`, `launch`,
  `setBreakpoints`, `configurationDone`, `threads`, `stackTrace`,
  `scopes`, `variables`, `continue`, `next`, `disconnect`,
  `terminate`. Captured Pure-level `println` is forwarded as DAP
  `output` events.
- `crates/cli` adds `legend dap` which spawns the server with the
  same classpath cascade `legend lsp` uses.

Smoke-tested via `echo '<init+disconnect JSON>' | legend -vv dap`.

## Plugin-side work (follow-up PR)

Target: IDEA 2025.3+. Uses the experimental EP
`com.intellij.platform.dap.debugAdapterSupportProvider` (class
`com.intellij.platform.dap.DebugAdapterSupportProvider` lives in
`lib/modules/intellij.platform.dap.jar` of the unified IDE
distribution).

### Class layout

Five new Kotlin files under
`src/main/kotlin/org/finos/legend/pure/intellij/debug/`:

1. **`PureLineBreakpointType.kt`** — `XLineBreakpointType<XBreakpointProperties>`
   for `.pure` files. `canPutAt(file, line, project)` returns
   `true` for any `.pure` file (line-level breakpoint, no
   per-expression filtering — the runtime decides at pause time).

2. **`PureDebugAdapterId.kt`** — `DebugAdapterId("legend", "Legend Pure")`.
   Type string flows into telemetry; presentable name shows in
   the run-config UI.

3. **`PureDebugAdapterDescriptor.kt`** — `DebugAdapterDescriptor<PureDebugAdapterId>`.
   Implements:
   - `getId()` — returns the singleton id.
   - `getBreakpointsDescription()` — returns
     `DapBreakpointsDescription(PureLineBreakpointType::class.java, /* no exception type */ null!!)`
     (exception breakpoints are out of MVP; pass a stub
     `XBreakpointType` or wait for the API to allow null).
   - `launchDebugAdapter(env, executionResult, programArgs, continuation)`
     — `suspend fun` returning a
     `com.intellij.platform.dap.connection.CommandLineDebugAdapterHandle(commandLine)`.
     The `commandLine` is a `GeneralCommandLine` running
     `legend -vv dap` — reuse
     `LegendPureSettings.resolveExecutableOrThrow()` so the same
     binary serves LSP + DAP.

4. **`PureDebugAdapterSupportProvider.kt`** —
   `DebugAdapterSupportProvider<PureDebugAdapterId>`. Two methods:
   - `getAdapterId()` returns the singleton id.
   - `createDebugAdapterDescriptor(project)` returns a
     `PureDebugAdapterDescriptor`.

5. **`PureDebugRunConfiguration.kt`** + factory + type — a thin
   `LocatableRunConfigurationBase` that carries the function FQN
   to debug. The Debug action launches it; the DAP infrastructure
   passes the configuration's program arg into
   `launchDebugAdapter`'s `programArgs` parameter.

### plugin.xml additions

```xml
<extensions defaultExtensionNs="com.intellij">
    <platform.dap.debugAdapterSupportProvider
        implementation="org.finos.legend.pure.intellij.debug.PureDebugAdapterSupportProvider"/>
    <xdebugger.breakpointType
        implementation="org.finos.legend.pure.intellij.debug.PureLineBreakpointType"/>
    <!-- run-configuration registrations: configurationType + factory -->
</extensions>
```

### Gutter integration

`PureRunLineMarkerContributor.kt` already returns a primary Run
action per gutter ▶. Extend `getInfo` to also include a Debug
action in the same `Info`:

```kotlin
return Info(
    icon, tooltip,
    RunPureFunctionAction(project, fqn, command),
    DebugPureFunctionAction(project, fqn, command),  // NEW
)
```

`DebugPureFunctionAction` builds a `PureDebugRunConfiguration` for
the FQN and launches it via `ExecutionUtil.runConfiguration(...)`.

### Known sharp edges

- **Suspend coroutines required.** `launchDebugAdapter` is a
  `suspend fun`. The Kotlin module already depends on
  `kotlinx-coroutines-core` transitively via IntelliJ; verify the
  Gradle module exposes the right `kotlin.coroutines` version
  before writing the suspend body.
- **`DapBreakpointsDescription` ctor takes a non-null exception
  breakpoint type Class.** If the experimental API doesn't allow
  `null` here, register a stub `XBreakpointType` that never matches
  any exception. Phase 4 swaps the stub for a real exception
  breakpoint type once the runtime side lands.
- **Experimental API churn.** The EP and supporting classes are
  `@ApiStatus.Experimental`. The 2025.3 surface is what this doc
  describes; expect signature shifts in 2026.x. Pin
  `intellijVersion` in `build.gradle.kts` to a known-good build
  number until the API stabilises.

## Verification (for the follow-up PR)

1. Open `pow.pure` in the sandbox IDE; click line 34 in the
   gutter → red breakpoint dot appears.
2. Right-click ▶ on `testNumberPow` → "Debug" option present.
3. Click Debug → execution stops on line 34. Frames panel shows
   the call chain. Variables panel shows the live locals
   (initially empty — exposing them needs the `EvalHooks::before_eval`
   parameter extension noted in `crates/dap/src/hooks.rs::DapHooks::before_eval`).
4. Step Over → one expression of progress + a new Stopped event.
5. Resume → eval completes; Pure Run tool window shows the failure
   with clickable stack frames (existing Phase 2 surface).

## Path-of-least-resistance fallback

If the native experimental EP proves too unstable, the original
plan's alternative (LSP4IJ DAP support) is still on the table.
Switch by adding `<depends>com.redhat.devtools.lsp4ij</depends>`
and registering an `com.redhat.devtools.lsp4ij.debugAdapterServer`
extension instead. Both routes point at the same `legend dap`
binary — the work above is reusable.
