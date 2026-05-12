# IntelliJ DAP integration

Status: **landed in `org.finos.legend.pure.intellij.debug.*`**.
This doc describes the wiring; the runtime + DAP server lives in
the sibling Rust workspace under `crates/dap/`.

## What ships

Plugin classes under
`src/main/kotlin/org/finos/legend/pure/intellij/debug/`:

| File | Role |
|------|------|
| `PureLineBreakpointType.kt` | `XLineBreakpointType` for `.pure` source — accepts every file/line; runtime filters at pause time. Properties type is a trivial `EmptyProperties` (no condition / hit-count / log-message for the MVP). |
| `PureDebugAdapterId.kt` | Singleton `DebugAdapterId("legend-pure", "Legend Pure")`. |
| `PureDebugAdapterDescriptor.kt` | Spawns `legend -vv dap` via `GeneralCommandLine`, wraps in `CommandLineDebugAdapterHandle`. Reuses `LegendPureSettings.resolveExecutableOrThrow()` so the same binary backs LSP + DAP. Source-breakpoint type points at `PureLineBreakpointType`; exception-breakpoint slot also uses `PureLineBreakpointType` as a stub (the API requires non-null; our server never fires Exception events to trigger it). |
| `PureDebugAdapterSupportProvider.kt` | Implements the experimental EP. Returns the singleton id + a fresh descriptor per project. |
| `PureDebugRunProfile.kt` | Minimal in-memory `RunProfile` carrying the function FQN. `getState` returns a stub `NopProcessHandler`-based result; the platform's `DapProgramRunner` replaces it with the real debug-console state once `launchDebugAdapter` returns. |
| `PureDapLaunchArgumentsProvider.kt` | Maps `PureDebugRunProfile` → DAP `launch` body (`{program, stopOnEntry, noDebug}`). The DAP framework serializes this map straight onto the wire; keys must match the Rust server (`crates/dap/src/protocol.rs::LaunchArguments`). |
| `DebugPureFunctionAction.kt` | Gutter Debug action — builds a `PureDebugRunProfile`, calls `ExecutionEnvironmentBuilder.create(project, DefaultDebugExecutor, profile).buildAndExecute()`. |

## Wiring (`plugin.xml`)

```xml
<!-- Standard IntelliJ namespace -->
<extensions defaultExtensionNs="com.intellij">
    <xdebugger.breakpointType
        implementation="org.finos.legend.pure.intellij.debug.PureLineBreakpointType"/>
</extensions>

<!-- Experimental DAP namespace -->
<extensions defaultExtensionNs="com.intellij.platform.dap">
    <debugAdapterSupportProvider
        implementation="org.finos.legend.pure.intellij.debug.PureDebugAdapterSupportProvider"/>
    <launchArgumentsProvider
        implementation="org.finos.legend.pure.intellij.debug.PureDapLaunchArgumentsProvider"/>
</extensions>
```

`build.gradle.kts` adds the DAP module to the plugin classpath:

```kotlin
intellijPlatform {
    bundledModule("intellij.platform.dap")
}
```

Without this, `com.intellij.platform.dap.*` resolves at neither
compile nor run time — the module ships inside the unified IDE
distribution (`lib/modules/intellij.platform.dap.jar`) but isn't
on the default plugin classpath.

## Gutter integration

`PureRunLineMarkerContributor.kt` returns each runnable function's
`Info` with two actions:

```kotlin
return Info(
    icon, tooltip,
    RunPureFunctionAction(project, fqn, command),
    DebugPureFunctionAction(project, fqn),  // sibling Debug action
)
```

Left-click runs (existing LSP path); right-click → "Debug …" routes
through the DAP server.

## End-to-end flow

1. User clicks the gutter on `pow.pure` line 34 → red breakpoint
   dot (via `PureLineBreakpointType`).
2. Right-click ▶ on `testNumberPow` → "Debug testNumberPow".
3. `DebugPureFunctionAction` constructs a `PureDebugRunProfile` and
   hands it to `ExecutionEnvironmentBuilder` with the Debug
   executor.
4. The platform's `DapProgramRunner` consults
   `PureDapLaunchArgumentsProvider.isApplicable("Debug", profile)`
   → `true`. It calls our descriptor's `launchDebugAdapter` which
   spawns `legend -vv dap` and returns the handle.
5. The DAP framework sends `initialize` → `launch` (body from
   `getLaunchArguments`) → `setBreakpoints` (built from the
   user's red dots).
6. Our Rust server (`crates/dap/`) runs the function. Hitting line
   34 fires `DapHooks::pause`, the server emits a `Stopped` event,
   and the eval thread blocks on its command channel.
7. IDE Variables / Frames panels populate from `scopes` /
   `variables` / `stackTrace` responses. Step Over / Resume
   buttons send `next` / `continue` over the wire.

## Verification

`./gradlew compileKotlin` and `./gradlew buildPlugin` both green
on IDEA 2025.3. Manual:

1. `./gradlew runIde` to launch a sandbox.
2. Open a project with a `.pure` file.
3. Click in the gutter on a line inside a parameterless function
   → red breakpoint dot appears.
4. Right-click ▶ on the function → "Debug …".
5. Execution stops at the breakpoint; Frames + Variables panels
   open; Step Over / Resume dispatch correctly.

## Known limitations (deferred)

- **Locals are empty on pause.** The MVP's `DapHooks::before_eval`
  doesn't have a `&VariableContext` parameter, so it can't
  snapshot live bindings. Phase 4 extends `EvalHooks::before_eval`
  to take the context (zero-cost for `NoOpHooks`) and renders
  bindings to display strings on the eval thread before storing.
- **Step-in / step-out** not supported — the runtime's
  `StepMode` only has `Continue` and `StepOver` today.
- **Evaluate-while-paused** not supported — needs a re-entrancy
  guard plus a lowering entry point that takes a
  `&VariableContext`.
- **Conditional / log breakpoints** not supported — `XBreakpointProperties`
  is the trivial `EmptyProperties` placeholder; the runtime path
  is the eval-while-paused work above.
- **Exception breakpoints** — the slot is wired to
  `PureLineBreakpointType` as a stub. The runtime never emits DAP
  Exception events, so the IDE option is inert.

## Experimental-API risk

The DAP EPs and supporting types (`DebugAdapterDescriptor`,
`DebugAdapterSupportProvider`, `DapLaunchArgumentsProvider`,
`LaunchRequestArguments`, `DapStartRequest`) are all
`@ApiStatus.Experimental` in 2025.3. Signature drift is likely
across IDE versions. Pin `platformVersion` in `gradle.properties`
to a known-good build until the API stabilises.

If the experimental APIs break and we need to ship before they're
fixed, the fallback is LSP4IJ DAP integration — same `legend dap`
binary, different glue. Switch by adding
`<depends>com.redhat.devtools.lsp4ij</depends>` and registering an
`com.redhat.devtools.lsp4ij.debugAdapterServer` extension.
