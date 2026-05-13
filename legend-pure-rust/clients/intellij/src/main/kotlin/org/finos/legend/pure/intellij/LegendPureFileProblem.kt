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

import com.intellij.analysis.problemsView.FileProblem
import com.intellij.analysis.problemsView.ProblemsProvider
import com.intellij.openapi.vfs.VirtualFile

/**
 * A single LSP-published diagnostic, packaged as
 * `com.intellij.analysis.problemsView.FileProblem` so the
 * **Problems tool window** picks it up from
 * `ProblemsListener.TOPIC` events.
 *
 * The Problems tool window queries the project's
 * `ProblemsCollector` (a `ProblemsListener` subscriber) for files
 * and their per-line problems. The stock LSP integration only
 * funnels diagnostics through per-editor `MarkupModel`s, so
 * unopened files never reach the collector. Publishing
 * `FileProblem`s for them via this class closes that gap —
 * see [`WorkspaceAwareLspNotificationsHandler`].
 *
 * `equals` / `hashCode` are identity-based — IntelliJ tracks
 * problem appearance/disappearance via reference equality. The
 * wrapper keeps the constructed `LegendPureFileProblem` instances
 * in its `activeProblems` map so each `problemDisappeared` event
 * carries the same instance that was published in
 * `problemAppeared`.
 */
class LegendPureFileProblem(
    override val provider: ProblemsProvider,
    override val file: VirtualFile,
    override val line: Int,
    override val column: Int,
    override val text: String,
) : FileProblem
