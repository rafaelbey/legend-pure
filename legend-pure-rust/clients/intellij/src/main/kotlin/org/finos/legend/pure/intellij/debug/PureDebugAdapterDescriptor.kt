/*
 * Copyright 2026 Goldman Sachs
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package org.finos.legend.pure.intellij.debug

import com.intellij.execution.ExecutionResult
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.Project
import com.intellij.platform.dap.DapBreakpointsDescription
import com.intellij.platform.dap.DebugAdapterDescriptor
import com.intellij.platform.dap.connection.CommandLineDebugAdapterHandle
import com.intellij.platform.dap.connection.DebugAdapterHandle
import org.finos.legend.pure.intellij.settings.LegendPureSettings
import java.nio.charset.StandardCharsets

/**
 * Spawns `legend -vv dap` and wraps the process in a
 * [`CommandLineDebugAdapterHandle`] for the platform's DAP
 * infrastructure to talk to.
 *
 * `launchDebugAdapter` is a `suspend fun` per the experimental
 * API; the implementation is synchronous (process spawn is cheap)
 * so we don't introduce any actual suspension points.
 *
 * `getBreakpointsDescription` returns the registered
 * [`PureLineBreakpointType`] for source breakpoints. The
 * exception-breakpoint slot uses the same type as a stub — the
 * platform's UI surfaces exception breakpoints as a configurable
 * option but our server never emits Exception events to fire one.
 * Phase 4 swaps the stub for a real `XBreakpointType` once the
 * runtime starts forwarding `PureException` events on top of the
 * step semantics.
 */
class PureDebugAdapterDescriptor(
    private val project: Project,
) : DebugAdapterDescriptor<PureDebugAdapterId>() {
    override val id: PureDebugAdapterId
        get() = PureDebugAdapterId

    override val breakpointsDescription: DapBreakpointsDescription
        get() = DapBreakpointsDescription(
            /* sourceBreakpointType = */ PureLineBreakpointType::class.java,
            /* exceptionBreakpointType = */ PureLineBreakpointType::class.java,
        )

    override suspend fun launchDebugAdapter(
        environment: ExecutionEnvironment,
        executionResult: ExecutionResult?,
        sessionId: String,
    ): DebugAdapterHandle {
        val settings = LegendPureSettings.getInstance(project)
        val exe = settings.resolveExecutableOrThrow()
        val cmd = GeneralCommandLine(exe.toString())
            .withCharset(StandardCharsets.UTF_8)
            .apply {
                // `-vv` raises the global tracing subscriber to
                // `info` level so the DAP request log lines reach
                // idea.log via the platform's stderr capture.
                // Matches the LSP spawn shape.
                addParameter("-vv")
                addParameter("dap")
                val classpath = settings.classpathTomlPath.trim()
                if (classpath.isNotEmpty()) {
                    addParameters("--classpath", classpath)
                }
                project.basePath?.let { withWorkDirectory(it) }
            }
        LOG.info("spawning legend DAP: ${cmd.commandLineString}")
        return CommandLineDebugAdapterHandle(cmd)
    }

    private companion object {
        private val LOG = logger<PureDebugAdapterDescriptor>()
    }
}
