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

import com.intellij.openapi.project.Project
import com.intellij.platform.dap.DebugAdapterDescriptor
import com.intellij.platform.dap.DebugAdapterSupportProvider

/**
 * Entry point for the experimental
 * `com.intellij.platform.dap.debugAdapterSupportProvider` EP.
 *
 * The platform's [`DapProgramRunner`] (part of
 * `intellij.platform.dap.jar`) walks registered providers,
 * matches on the [`DebugAdapterId`], and routes Debug-button
 * clicks for matching run configurations through the returned
 * descriptor's [`launchDebugAdapter`]. We answer with
 * [`PureDebugAdapterId`] / [`PureDebugAdapterDescriptor`].
 */
class PureDebugAdapterSupportProvider : DebugAdapterSupportProvider<PureDebugAdapterId> {
    override val adapterId: PureDebugAdapterId
        get() = PureDebugAdapterId

    override fun createDebugAdapterDescriptor(
        project: Project,
    ): DebugAdapterDescriptor<PureDebugAdapterId> = PureDebugAdapterDescriptor(project)
}
