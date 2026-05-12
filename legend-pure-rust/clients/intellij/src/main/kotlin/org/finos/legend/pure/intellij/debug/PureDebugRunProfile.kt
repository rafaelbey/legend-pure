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

import com.intellij.execution.DefaultExecutionResult
import com.intellij.execution.ExecutionResult
import com.intellij.execution.Executor
import com.intellij.execution.configurations.RunProfile
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.icons.AllIcons
import com.intellij.execution.process.NopProcessHandler
import javax.swing.Icon

/**
 * Lightweight in-memory `RunProfile` for a one-off ▶ Debug click.
 *
 * The platform's [`DapProgramRunner`] uses the profile to identify
 * which DAP support provider applies (via
 * [`PureDapLaunchArgumentsProvider.isApplicable`]) and pulls the
 * launch arguments off the same profile. The function FQN we want
 * to run is the only payload — everything else (classpath
 * resolution, executable lookup) lives in the DAP server side and
 * the descriptor's `launchDebugAdapter`.
 *
 * `getState` returns a stub: real execution is driven by
 * `DapProgramRunner.execute(env, _)` reading the profile back off
 * the environment, not via the state. We satisfy the API contract
 * with a no-op `ProcessHandler`.
 */
class PureDebugRunProfile(val fqn: String) : RunProfile {
    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState =
        RunProfileState { _, _ -> stubExecutionResult(executor) }

    override fun getName(): String = "Debug Legend Pure: $fqn"

    override fun getIcon(): Icon = AllIcons.Actions.StartDebugger

    private fun stubExecutionResult(executor: Executor): ExecutionResult {
        // DapProgramRunner replaces this with the actual debug
        // console wired to the DAP `output` event stream. Until
        // that wiring lands we hand back a NopProcessHandler so
        // the framework's `canRun → execute` round-trip stays
        // well-typed.
        val handler = NopProcessHandler()
        return DefaultExecutionResult(null, handler)
    }
}
