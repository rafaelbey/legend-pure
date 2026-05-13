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
package org.finos.legend.pure.intellij

import com.intellij.openapi.util.Condition
import com.intellij.openapi.vfs.VirtualFile

/**
 * Tells [`WolfTheProblemSolver`] that `.pure` files are highlightable,
 * so problems reported via
 * [`WolfTheProblemSolver.reportProblems`] for them survive its
 * internal `isToBeHighlighted` filter.
 *
 * Why this is needed
 * ------------------
 * `WolfTheProblemSolverImpl.reportProblems` calls
 * `isToBeHighlighted(file)` and silently bails if no extension at
 * `com.intellij.problemFileHighlightFilter` returns `true` for the
 * file. Without that approval the file never enters Wolf's problem
 * map, so it never appears in the project-wide "Problems" tool
 * window, gets no red filename badge in Project view, and
 * `isProblemFile(file)` keeps returning `false` even after a
 * successful `reportProblems` call.
 *
 * The Java / Kotlin / Python plugins all register similar filters
 * for their own file types. We add ours so the LSP-driven cross-file
 * diagnostics from
 * [`WorkspaceAwareLspNotificationsHandler`] actually surface.
 *
 * Scope
 * -----
 * Returns `true` for any file whose type is [`PureFileType`] — same
 * shape as [`PureLspServerDescriptor.isSupportedFile`]. Any future
 * extension to LSP coverage (new Pure-derived file types, etc.)
 * should be reflected here too.
 */
class PureProblemFileHighlightFilter : Condition<VirtualFile> {
    override fun value(file: VirtualFile?): Boolean {
        return file != null && file.fileType is PureFileType
    }
}
