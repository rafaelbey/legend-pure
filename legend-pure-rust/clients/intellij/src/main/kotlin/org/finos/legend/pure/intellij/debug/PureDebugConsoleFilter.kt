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

import com.intellij.execution.filters.ConsoleFilterProvider
import com.intellij.execution.filters.Filter
import com.intellij.execution.filters.OpenFileHyperlinkInfo
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.ProjectRootManager
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile

/**
 * Console filter that turns Pure runtime stack-frame text into
 * clickable hyperlinks.
 *
 * The runtime emits exceptions with frames in the
 * `printPureStackTrace`-style format:
 *
 * ```
 *     abc::demo__Any_1_     <-     resource:/myproj/abc.pure line:5 column:3
 *     toOne_T_MANY__T_1_    <-     resource:/myproj/abc.pure line:6 column:5
 * ```
 *
 * IntelliJ's debug `ConsoleView` doesn't natively recognize this
 * pattern, so the user sees plain text — useful but not
 * actionable. This filter scans each line, finds the
 * `resource:PATH line:N column:M` triple, and binds an
 * `OpenFileHyperlinkInfo` over that range so a click jumps
 * directly to the call site. Mirrors the Pure Run tool window's
 * post-failure link rendering — same UX for streaming console
 * output during a debug session.
 *
 * Path resolution: three-stage lookup matching the
 * `FailureLinkListener` used by the notification path.
 */
class PureDebugConsoleFilter(private val project: Project) : Filter {
    override fun applyFilter(line: String, entireLength: Int): Filter.Result? {
        val match = PATTERN.find(line) ?: return null
        val (sourcePath, lineNum, columnNum) = Triple(
            match.groupValues[1],
            match.groupValues[2].toIntOrNull() ?: return null,
            match.groupValues[3].toIntOrNull() ?: return null,
        )
        val file = resolveFile(sourcePath) ?: return null
        // Hyperlink range covers the whole `resource:… line:…
        // column:…` triple so the user has a visible click target.
        // `entireLength - line.length` is the offset of the start
        // of this line within the entire document — the offsets we
        // hand back are global, not line-local.
        val lineStart = entireLength - line.length
        val info = OpenFileHyperlinkInfo(
            project,
            file,
            (lineNum - 1).coerceAtLeast(0),
            (columnNum - 1).coerceAtLeast(0),
        )
        return Filter.Result(
            lineStart + match.range.first,
            lineStart + match.range.last + 1,
            info,
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

    private companion object {
        /**
         * Matches `resource:PATH line:N column:M`. `PATH` allows
         * any non-whitespace character so `/` and `.pure`
         * extensions pass through. Tolerant of multiple spaces
         * between segments since the runtime aligns the columns
         * cosmetically.
         */
        private val PATTERN = Regex(
            """resource:(\S+)\s+line:(\d+)\s+column:(\d+)""",
        )
    }
}

/**
 * Provider that hooks [`PureDebugConsoleFilter`] into every
 * `ConsoleView` in the project. Registered via the
 * `com.intellij.consoleFilterProvider` extension point — the
 * platform invokes `getDefaultFilters` whenever a new console is
 * constructed (Run, Debug, terminal, …).
 *
 * The filter is project-scoped — file resolution needs project
 * roots — so we instantiate per project.
 */
class PureDebugConsoleFilterProvider : ConsoleFilterProvider {
    override fun getDefaultFilters(project: Project): Array<Filter> =
        arrayOf(PureDebugConsoleFilter(project))
}
