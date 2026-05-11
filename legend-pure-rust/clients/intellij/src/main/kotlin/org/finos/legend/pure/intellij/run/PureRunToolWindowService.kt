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

import com.intellij.execution.filters.OpenFileHyperlinkInfo
import com.intellij.execution.filters.TextConsoleBuilderFactory
import com.intellij.execution.ui.ConsoleView
import com.intellij.execution.ui.ConsoleViewContentType
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.ProjectRootManager
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.wm.ToolWindowManager
import com.intellij.ui.content.ContentFactory

/**
 * Owns the "Legend Pure Run" tool window's tabs. One tab per
 * gutter-▶ invocation. Each tab wraps an IntelliJ [`ConsoleView`]
 * so:
 *
 *   * Stack frames render as native clickable hyperlinks via
 *     `OpenFileHyperlinkInfo` — same surface as a real Run/Debug
 *     configuration, including hover preview and middle-click
 *     "open in new editor".
 *   * Output supports copy/select/wrap/search — anything the
 *     console tool window normally affords.
 *   * The tab title carries the FQN so multiple runs are
 *     distinguishable when stacked.
 *
 * The balloon notification stays as a 1-line "click to focus"
 * affordance: clicking it brings the tool window forward.
 *
 * Path resolution for `OpenFileHyperlinkInfo` is the same three-
 * step lookup `FailureLinkListener` uses: as-is → relative to
 * project base → suffix match against any source root.
 */
@Service(Service.Level.PROJECT)
class PureRunToolWindowService(private val project: Project) {

    /**
     * Open / activate the tool window and append a new tab with
     * the rendered run result. Safe to call from any thread —
     * UI work is dispatched to the EDT internally.
     */
    fun showRun(title: String, render: RunRender) {
        ApplicationManager.getApplication().invokeLater {
            val manager = ToolWindowManager.getInstance(project)
            val toolWindow = manager.getToolWindow(PureRunToolWindowFactory.TOOL_WINDOW_ID)
                ?: return@invokeLater
            val contentManager = toolWindow.contentManager

            // Sweep unpinned previous tabs before adding the new
            // one. This matches the standard Run/Debug tool window
            // UX: by default only the latest run is kept; pin a
            // tab (right-click → Pin Tab, or via the gear menu)
            // to preserve it across subsequent runs. The user
            // can still close any tab manually via the X.
            val existing = contentManager.contents.toList()
            for (c in existing) {
                if (!c.isPinned) {
                    contentManager.removeContent(c, /*dispose*/ true)
                }
            }

            val console = TextConsoleBuilderFactory.getInstance().createBuilder(project).console
            writeRender(console, render)
            val content = ContentFactory.getInstance().createContent(
                console.component,
                title,
                /*isLockable*/ false,
            )
            content.isCloseable = true
            // `isPinnable` exposes the "Pin Tab" affordance in
            // the tab's context menu — combined with the
            // unpinned-sweep above this gives you Java-style
            // tool-window UX: latest run replaces previous,
            // pinned runs survive.
            content.isPinnable = true
            content.setDisposer(console)
            contentManager.addContent(content)
            contentManager.setSelectedContent(content)
            toolWindow.activate(null, /*autoFocusContents*/ false)
        }
    }

    private fun writeRender(console: ConsoleView, render: RunRender) {
        // Captured `print` / `println` output first, so it reads
        // chronologically: program output, then the result
        // summary. Each emitted chunk is whatever the Pure code
        // wrote — `println` appends a `\n`, `print` does not, so
        // the buffer's existing newline structure is preserved.
        if (render.stdout.isNotEmpty()) {
            console.print(render.stdout, ConsoleViewContentType.NORMAL_OUTPUT)
            if (!render.stdout.endsWith("\n")) {
                console.print("\n", ConsoleViewContentType.NORMAL_OUTPUT)
            }
            console.print("\n", ConsoleViewContentType.NORMAL_OUTPUT)
        }

        // Header line — always plain. Carries the command-level
        // summary the LSP returned (`passed=… failed=…`, `[legend.run]
        // Execution error`, or the rendered return value on success).
        console.print(render.header + "\n", render.headerType.toContentType())

        if (render.successValue != null) {
            console.print("\n", ConsoleViewContentType.NORMAL_OUTPUT)
            console.print("returned: ", ConsoleViewContentType.LOG_INFO_OUTPUT)
            console.print(render.successValue + "\n", ConsoleViewContentType.NORMAL_OUTPUT)
            return
        }

        for ((index, failure) in render.failures.withIndex()) {
            if (index > 0) {
                console.print("\n", ConsoleViewContentType.NORMAL_OUTPUT)
            }
            console.print("✗ ", ConsoleViewContentType.ERROR_OUTPUT)
            console.print(failure.fqn + "\n", ConsoleViewContentType.ERROR_OUTPUT)
            if (failure.message.isNotEmpty()) {
                console.print("    " + failure.message + "\n", ConsoleViewContentType.ERROR_OUTPUT)
            }
            for (frame in failure.stack) {
                console.print("    at ", ConsoleViewContentType.NORMAL_OUTPUT)
                console.print(frame.name, ConsoleViewContentType.NORMAL_OUTPUT)
                console.print(" (", ConsoleViewContentType.NORMAL_OUTPUT)
                val info = openFileInfo(frame.source, frame.line, frame.column)
                val hyperlinkText = "${frame.source}:${frame.line}"
                if (info != null) {
                    console.printHyperlink(hyperlinkText, info)
                } else {
                    // Couldn't resolve to a VirtualFile — show as
                    // plain text so the user at least sees where
                    // the runtime *said* it came from. Reasons
                    // include the file being outside the project
                    // (e.g. embedded platform sources) or a
                    // mangled canonical path.
                    console.print(hyperlinkText, ConsoleViewContentType.NORMAL_OUTPUT)
                }
                console.print(")\n", ConsoleViewContentType.NORMAL_OUTPUT)
            }
        }
    }

    private fun openFileInfo(source: String, line: Int, column: Int): OpenFileHyperlinkInfo? {
        val file = resolveFile(source) ?: return null
        return OpenFileHyperlinkInfo(
            project,
            file,
            (line - 1).coerceAtLeast(0),
            (column - 1).coerceAtLeast(0),
        )
    }

    private fun resolveFile(source: String): VirtualFile? {
        val lfs = LocalFileSystem.getInstance()
        lfs.findFileByPath(source)?.let { return it }
        val base = project.basePath
        if (base != null) {
            lfs.findFileByPath("$base/${source.trimStart('/')}")?.let { return it }
        }
        val suffix = source.trimStart('/')
        for (root in ProjectRootManager.getInstance(project).contentSourceRoots) {
            val candidate = root.findFileByRelativePath(suffix)
            if (candidate != null) return candidate
        }
        return null
    }

    /**
     * Plain-data shape describing a single run/test/pct result.
     * `PureRunCommandsSupport` builds one of these per LSP
     * response and hands it off here; this class doesn't know
     * about the LSP wire format.
     *
     * `headerType` controls the color of the first line — green
     * for a successful run, red for a failure / errored test.
     * `successValue` is set when the run produced a return value
     * (only `legend.run` does); test reports leave it null and
     * provide a populated `failures` list instead.
     */
    data class RunRender(
        val header: String,
        val headerType: HeaderType,
        val successValue: String?,
        val failures: List<FailureRender>,
        /** Captured Pure-level `print` / `println` output from the
         *  run, in emission order. The tool window renders it as
         *  plain console text above the failure / success
         *  section. Empty when the run produced no console
         *  writes. */
        val stdout: String = "",
    )

    enum class HeaderType {
        SUCCESS,
        FAILURE;

        fun toContentType(): ConsoleViewContentType = when (this) {
            SUCCESS -> ConsoleViewContentType.LOG_INFO_OUTPUT
            FAILURE -> ConsoleViewContentType.ERROR_OUTPUT
        }
    }

    /** Mirror of the server-side `FailureDetail` shape, kept
     *  separate from `PureRunCommandsSupport`'s private nested
     *  data class so the tool-window code doesn't reach into that
     *  class's privates. */
    data class FailureRender(
        val fqn: String,
        val message: String,
        val stack: List<FrameRender>,
    )

    data class FrameRender(
        val name: String,
        val source: String,
        val line: Int,
        val column: Int,
    )

    companion object {
        fun getInstance(project: Project): PureRunToolWindowService = project.service()
    }
}
