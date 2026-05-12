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

import com.intellij.execution.configurations.RunProfile
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.openapi.project.Project
import com.intellij.platform.dap.DapLaunchArgumentsProvider
import com.intellij.platform.dap.DapStartRequest
import com.intellij.platform.dap.LaunchRequestArguments

/**
 * Maps a [`PureDebugRunProfile`] to the DAP `launch` request body
 * our server expects.
 *
 * Our server's `LaunchArguments` shape is `{ "program": "<fqn>",
 * "stopOnEntry"?: bool, "noDebug"?: bool }` — see
 * `crates/dap/src/protocol.rs::LaunchArguments`. We construct that
 * map here so the platform's `DapDebugSessionImpl` can ship it
 * across the wire.
 *
 * `isApplicable` gates this provider on (a) the adapter id matching
 * ours and (b) the profile being one of ours — every other
 * registered provider in the EP will be polled too, so a strict
 * check keeps unrelated DAP integrations from getting our payload.
 */
class PureDapLaunchArgumentsProvider : DapLaunchArgumentsProvider {
    override fun isApplicable(executorId: String, profile: RunProfile): Boolean {
        // First arg is the IDE executor id (`Debug` / `Run`), not
        // the adapter id — the platform routes through this
        // provider to decide if a Debug-button click against this
        // profile should kick off our DAP adapter.
        return executorId == DefaultDebugExecutor.EXECUTOR_ID && profile is PureDebugRunProfile
    }

    override fun getLaunchArguments(project: Project, profile: RunProfile): LaunchRequestArguments {
        val fqn = (profile as? PureDebugRunProfile)?.fqn.orEmpty()
        // The platform serializes this map straight onto the DAP
        // `launch` request body — keys here must match what our
        // Rust server expects (`crates/dap/src/protocol.rs`).
        val arguments: Map<String, Any> = mapOf(
            "program" to fqn,
            "stopOnEntry" to false,
            "noDebug" to false,
        )
        return LaunchRequestArguments(
            /* adapterId = */ PureDebugAdapterId,
            /* request = */ DapStartRequest.Launch,
            /* arguments = */ arguments,
        )
    }
}
