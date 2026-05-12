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
package org.finos.legend.pure.intellij.run

import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory

/**
 * Registers the "Legend Pure Run" tool window — the destination
 * for `legend.run` / `legend.runTest` / `legend.runPCT` output.
 *
 * Each invocation from the gutter ▶ creates a new tab in the
 * window's content manager (see
 * [`PureRunToolWindowService.showRun`]), so a developer can keep
 * previous test reports around while running a new one. The tab
 * wraps an IntelliJ `ConsoleView` so stack frames render as
 * native clickable hyperlinks (`OpenFileHyperlinkInfo`) rather
 * than HTML anchors — the same surface a real Run/Debug
 * configuration uses.
 *
 * The factory itself does nothing on `createToolWindowContent` —
 * tabs are added lazily by the run flow. Without this hook
 * IntelliJ would refuse to surface the tool window at all
 * (`ToolWindowManager.getToolWindow` returns `null` for
 * unregistered IDs).
 */
class PureRunToolWindowFactory : ToolWindowFactory, DumbAware {
    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        // No-op. The tool window exists from launch; tabs are
        // appended by `PureRunToolWindowService` on each run.
    }

    override fun shouldBeAvailable(project: Project): Boolean = true

    companion object {
        const val TOOL_WINDOW_ID = "Legend Pure Run"
    }
}
