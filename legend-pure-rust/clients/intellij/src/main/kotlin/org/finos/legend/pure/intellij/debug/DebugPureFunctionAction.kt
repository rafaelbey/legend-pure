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

import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.runners.ExecutionEnvironmentBuilder
import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project

/**
 * Gutter Debug action sibling to the existing Run / Run Test
 * actions. Spins up a debug session pointed at [`fqn`] via the
 * platform's DAP infrastructure.
 *
 * Flow:
 *   1. Build a [`PureDebugRunProfile`] carrying the FQN.
 *   2. Construct an [`ExecutionEnvironment`] keyed on the standard
 *      Debug executor.
 *   3. Hand it to `ExecutionEnvironmentBuilder.buildAndExecute()`.
 *   4. The platform's `DapProgramRunner` matches the profile via
 *      [`PureDapLaunchArgumentsProvider.isApplicable`], calls
 *      [`PureDebugAdapterDescriptor.launchDebugAdapter`] to spawn
 *      `legend dap`, and starts the debug session.
 *   5. Breakpoint gutters work; pauses produce stopped events;
 *      step / continue dispatch back through DAP.
 *
 * The action is independent of the existing
 * `PureRunCommandsSupport.runViaLsp` path — no LSP roundtrip.
 */
class DebugPureFunctionAction(
    private val project: Project,
    private val fqn: String,
) : AnAction(
    "Debug $fqn",
    null,
    AllIcons.Actions.StartDebugger,
), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val profile = PureDebugRunProfile(fqn)
        val executor = DefaultDebugExecutor.getDebugExecutorInstance()
        try {
            ExecutionEnvironmentBuilder
                .create(project, executor, profile)
                .buildAndExecute()
        } catch (ex: Exception) {
            LOG.warn("Debug launch failed for $fqn", ex)
        }
    }

    private companion object {
        private val LOG = logger<DebugPureFunctionAction>()
    }
}
